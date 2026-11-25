#!/usr/bin/env python3
"""Finds NVIDIA driver versions whose kernel interface equals one gVisor knows.

gVisor's GPU proxy (nvproxy) starts only for a driver version listed in its
table (`pkg/sentry/devices/nvproxy/version.go`); every other version fails at
sandbox start. NVIDIA ships many versions whose kernel interface does not
change at all: only the version strings, build numbers, the product-name table
and code inside `.c` files differ. Such a version has the ABI of its nearest
known lower neighbour in the same branch, and gVisor itself chains those
versions to one constructor (for example 580.126.20 uses 580.126.09's).

This program reads gVisor's table and NVIDIA's open kernel module repository
(github.com/NVIDIA/open-gpu-kernel-modules, one tag per driver release),
compares each unknown release with its nearest known lower neighbour, and
accepts it only when no header changed except the version and product-name
lines. Every struct, constant, class and command of the driver's ioctl
interface is defined in a header, so an unchanged header set means an
unchanged interface. The open and the proprietary module of one release share
these headers (`src/common/sdk`) and one user-space library.

  driver_table.py scan --version-go FILE --nvidia-repo DIR --out drivers.json
  driver_table.py verify --version-go FILE --nvidia-repo DIR --drivers drivers.json --notes NOTES.md
  driver_table.py go --drivers drivers.json --out version_flip_drivers.go
  driver_table.py untested --pci-ids pci.ids --out untested.json

`verify` is the release job's gate: it compares every accepted version again
with its base and fails when an interface header changed; NOTES.md gets one
paragraph per version for the release notes. `untested` lists the PCI device
IDs of each card family whose classes Flip added without a run on a card.

`scan` fetches each tag it compares into DIR (a git repository whose origin is
NVIDIA's). `--checksums` also downloads each accepted release's installer, as
gVisor's own tool does (tools/gpu/drivers/download.go), to record its SHA-256.
"""

import argparse
import hashlib
import json
import re
import subprocess
import sys
import urllib.error
import urllib.request

ENTRY = re.compile(
    r"(?P<var>\w+)\s*:?=\s*add(?P<unsupported>Unsupported)?DriverABI(?:WithChecksums)?\(\s*"
    r"(?P<major>\d+),\s*(?P<minor>\d+),\s*(?P<patch>\d+)"
)

# Headers NVIDIA rewrites in every release without an interface change, with
# the only lines that may change in them.
VERSION_LINE = re.compile(
    r"^#define\s+NV_(BUILD_\w+|LAST_OFFICIAL_CHANGELIST_NUM|VERSION_STRING|COPYRIGHT_YEAR)\b"
)
NAME_ROW = re.compile(r'^\{\s*0x[0-9A-Fa-f]+,\s*0x[0-9A-Fa-f]+,\s*0x[0-9A-Fa-f]+,\s*".*"\s*\},?$')
RELEASE_HEADERS = {
    "src/common/inc/nvBldVer.h": VERSION_LINE,
    "src/common/inc/nvUnixVersion.h": VERSION_LINE,
    "src/common/inc/nvVer.h": VERSION_LINE,
    "src/nvidia/generated/g_nv_name_released.h": NAME_ROW,
}
HEADER_SUFFIXES = (".h", ".hpp", ".hh")

# gVisor's own installer locations (tools/gpu/drivers/download.go).
INSTALLERS = {
    "x86_64": "https://us.download.nvidia.com/tesla/{v}/NVIDIA-Linux-x86_64-{v}.run",
    "aarch64": "https://us.download.nvidia.com/XFree86/aarch64/{v}/NVIDIA-Linux-aarch64-{v}.run",
}
NO_DRIVER = "NO_DRIVER"


def version(text):
    parts = text.split(".")
    if len(parts) != 3 or not all(p.isdigit() for p in parts):
        raise ValueError(f"not a driver version: {text!r}")
    return tuple(int(p) for p in parts)


def show(v):
    # NVIDIA writes the patch with two digits (580.126.09); gVisor parses both.
    return f"{v[0]}.{v[1]}.{v[2]:02d}"


def known_drivers(version_go):
    """{version: (variable, supported)} from gVisor's version.go."""
    table = {}
    for m in ENTRY.finditer(version_go):
        v = (int(m["major"]), int(m["minor"]), int(m["patch"]))
        table[v] = (m["var"], m["unsupported"] is None)
    return table


def neighbour(v, table):
    """The nearest supported known version below `v` in the same branch, or None."""
    lower = [k for k, (_, supported) in table.items() if supported and k[0] == v[0] and k < v]
    return max(lower) if lower else None


def changed_lines(patch):
    """The added and removed lines of a unified diff, without their marker."""
    out = []
    for line in patch.splitlines():
        if line.startswith(("+++", "---")):
            continue
        if line.startswith(("+", "-")):
            out.append(line[1:].strip())
    return [line for line in out if line]


def interface_change(path, patch):
    """Why this file's change can alter the driver interface, or None when it cannot."""
    if not path.endswith(HEADER_SUFFIXES):
        return None
    allowed = RELEASE_HEADERS.get(path)
    if allowed is None:
        return f"header {path} changed"
    for line in changed_lines(patch):
        if not allowed.match(line):
            return f"{path} changed beyond its release lines: {line[:120]}"
    return None


def compare(changes):
    """`changes` is [(path, patch)]. Returns (same_interface, reasons)."""
    reasons = [r for r in (interface_change(p, d) for p, d in changes) if r]
    return (not reasons, reasons)


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", repo, *args], check=True, capture_output=True, text=True
    ).stdout


def fetch_tag(repo, tag):
    if (
        subprocess.run(
            ["git", "-C", repo, "rev-parse", "-q", "--verify", f"refs/tags/{tag}"],
            capture_output=True,
        ).returncode
        != 0
    ):
        git(repo, "fetch", "-q", "--depth", "1", "origin", "tag", tag)


def tree_changes(repo, base, tag):
    names = [n for n in git(repo, "diff", "--name-only", base, tag).splitlines() if n]
    return [(n, git(repo, "diff", base, tag, "--", n)) for n in names]


def remote_tags(repo):
    out = git(repo, "ls-remote", "--tags", "origin")
    tags = []
    for line in out.splitlines():
        ref = line.split("\t")[-1]
        name = ref.removeprefix("refs/tags/")
        if name.endswith("^{}"):
            continue
        try:
            version(name)
        except ValueError:
            continue
        tags.append(name)
    return tags


def installer_sha256(url, opener=urllib.request.urlopen):
    """SHA-256 of the file at `url`, or NO_DRIVER when NVIDIA has none there."""
    try:
        with opener(url, timeout=120) as resp:
            digest = hashlib.sha256()
            while chunk := resp.read(1 << 20):
                digest.update(chunk)
            return digest.hexdigest()
    except urllib.error.HTTPError as e:
        if e.code in (403, 404):
            return NO_DRIVER
        raise


def scan(version_go, repo, tags, checksums=False, opener=urllib.request.urlopen):
    table = known_drivers(version_go)
    majors = {v[0] for v, (_, supported) in table.items() if supported}
    rows = []
    for tag in sorted(tags, key=version):
        v = version(tag)
        if v in table or v[0] not in majors:
            continue
        base = neighbour(v, table)
        if base is None:
            rows.append(
                {
                    "version": show(v),
                    "base": None,
                    "accepted": False,
                    "reasons": ["no known lower version in this branch"],
                }
            )
            continue
        base_tag = next((t for t in tags if version(t) == base), None)
        if base_tag is None:
            rows.append(
                {
                    "version": show(v),
                    "base": show(base),
                    "accepted": False,
                    "reasons": [f"NVIDIA has no tag for {show(base)}"],
                }
            )
            continue
        fetch_tag(repo, base_tag)
        fetch_tag(repo, tag)
        same, reasons = compare(tree_changes(repo, base_tag, tag))
        row = {"version": show(v), "base": show(base), "accepted": same, "reasons": reasons}
        if same and checksums:
            row["checksums"] = {
                arch: installer_sha256(url.format(v=tag), opener)
                for arch, url in INSTALLERS.items()
            }
        rows.append(row)
    return rows


GO_HEADER = """// Code generated by libs/flip-runsc/driver_table.py. DO NOT EDIT.

package nvproxy

import "gvisor.dev/gvisor/pkg/sentry/devices/nvproxy/nvconf"

// flipDriverABIs lists NVIDIA driver versions whose kernel interface equals
// that of a version gVisor knows (base): no header changed between the two
// releases except version strings and product names.
var flipDriverABIs = []flipDriverABI{
"""


def go_source(rows):
    lines = [GO_HEADER]
    for row in rows:
        if not row["accepted"]:
            continue
        v, b = version(row["version"]), version(row["base"])
        sums = row.get("checksums", {})
        x86 = sums.get("x86_64", NO_DRIVER)
        arm = sums.get("aarch64", NO_DRIVER)
        lines.append(
            f"\t{{version: nvconf.NewDriverVersion({v[0]}, {v[1]}, {v[2]}), "
            f"base: nvconf.NewDriverVersion({b[0]}, {b[1]}, {b[2]}), "
            f"checksums: NewChecksums({json.dumps(x86)}, {json.dumps(arm)})}},\n"
        )
    lines.append("}\n")
    return "".join(lines)


def verify(version_go, repo, rows, tags=None):
    """Compares every accepted row again. Returns (failures, notes in Markdown)."""
    table = known_drivers(version_go)
    failures, notes = [], []
    for row in rows:
        if not row["accepted"]:
            continue
        v, base = version(row["version"]), version(row["base"])
        want = neighbour(v, table)
        if want != base:
            failures.append(
                f"{row['version']}: base is {row['base']}, gVisor's nearest lower version is "
                f"{show(want) if want else 'none'}"
            )
            continue
        v_tag = next(t for t in tags if version(t) == v) if tags else row["version"]
        b_tag = next(t for t in tags if version(t) == base) if tags else row["base"]
        fetch_tag(repo, b_tag)
        fetch_tag(repo, v_tag)
        changes = tree_changes(repo, b_tag, v_tag)
        same, reasons = compare(changes)
        if not same:
            failures.extend(f"{row['version']}: {r}" for r in reasons)
            continue
        headers = sorted(p for p, _ in changes if p.endswith(HEADER_SUFFIXES))
        code = sorted(p for p, _ in changes if not p.endswith(HEADER_SUFFIXES))
        notes.append(
            f"- **{row['version']}** uses the ABI of {row['base']}. Between the two NVIDIA tags "
            f"{len(changes)} files changed; no interface header changed. Headers with release lines "
            f"only: {', '.join(headers) or 'none'}. Other files: {', '.join(code) or 'none'}."
        )
    return failures, "\n".join(notes) + ("\n" if notes else "")


# Card families whose classes Flip added to gVisor's table without a run on a
# card (version_flip_volta.go). NVIDIA's chip names begin with the
# architecture's letters (GV100 is Volta), and pci.ids names the chip of each
# device ID.
UNTESTED_FAMILIES = [
    {
        "family": "Volta",
        "chip_prefix": "GV",
        "classes": [
            "VOLTA_CHANNEL_GPFIFO_A",
            "VOLTA_COMPUTE_A",
            "VOLTA_DMA_COPY_A",
            "VOLTA_A",
            "VOLTA_USERMODE_A",
        ],
    }
]
NVIDIA_VENDOR = "10de"
PCI_DEVICE = re.compile(r"^\t(?P<id>[0-9a-f]{4})\s+(?P<name>.*)$")


def untested(pci_ids, families=UNTESTED_FAMILIES):
    """The families with the NVIDIA device IDs whose chip name starts with their prefix."""
    devices, inside = [], False
    for line in pci_ids.splitlines():
        if line and not line.startswith(("\t", "#")):
            inside = line.startswith(NVIDIA_VENDOR + " ")
            continue
        m = PCI_DEVICE.match(line) if inside else None
        if m:
            devices.append((m["id"], m["name"]))
    out = []
    for fam in families:
        chip = re.compile(re.escape(fam["chip_prefix"]) + r"\d")
        ids = sorted("0x" + i for i, name in devices if chip.match(name))
        out.append({**fam, "vendor_id": "0x" + NVIDIA_VENDOR, "device_ids": ids})
    return out


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scan")
    s.add_argument("--version-go", required=True)
    s.add_argument("--nvidia-repo", required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--checksums", action="store_true")
    v = sub.add_parser("verify")
    v.add_argument("--version-go", required=True)
    v.add_argument("--nvidia-repo", required=True)
    v.add_argument("--drivers", required=True)
    v.add_argument("--notes", required=True)
    u = sub.add_parser("untested")
    u.add_argument("--pci-ids", required=True)
    u.add_argument("--out", required=True)
    g = sub.add_parser("go")
    g.add_argument("--drivers", required=True)
    g.add_argument("--out", required=True)
    a = p.parse_args(argv)
    if a.cmd == "scan":
        with open(a.version_go) as f:
            source = f.read()
        rows = scan(source, a.nvidia_repo, remote_tags(a.nvidia_repo), a.checksums)
        with open(a.out, "w") as f:
            json.dump(rows, f, indent=1)
            f.write("\n")
        accepted = sum(r["accepted"] for r in rows)
        print(f"{accepted} of {len(rows)} unknown releases have a known interface", file=sys.stderr)
    elif a.cmd == "verify":
        with open(a.version_go) as f:
            source = f.read()
        with open(a.drivers) as f:
            rows = json.load(f)
        failures, notes = verify(source, a.nvidia_repo, rows, remote_tags(a.nvidia_repo))
        with open(a.notes, "w") as f:
            f.write(notes)
        for line in failures:
            print(line, file=sys.stderr)
        return 1 if failures else 0
    elif a.cmd == "untested":
        with open(a.pci_ids, encoding="utf-8", errors="replace") as f:
            rows = untested(f.read())
        with open(a.out, "w") as f:
            json.dump(rows, f, indent=1)
            f.write("\n")
    else:
        with open(a.drivers) as f:
            rows = json.load(f)
        with open(a.out, "w") as f:
            f.write(go_source(rows))
    return 0


if __name__ == "__main__":
    sys.exit(main())
