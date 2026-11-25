import hashlib
import io
import os
import subprocess
import sys
import urllib.error

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import driver_table as dt  # noqa: E402

VERSION_GO = """
		v580_65_06 := addDriverABI(580, 65, 06, "a", "b", func() *driverABI {
		v580_126_09 := addDriverABI(580, 126, 9, "c", "d", v580_105_08)
		v580_126_20 := addDriverABI(580, 126, 20, "e", ChecksumNoDriver, v580_126_09)
		v580_173_10 := addDriverABIWithChecksums(580, 173, 10, Checksums{
		v535_104_05 := addUnsupportedDriverABI(535, 104, 05, func() *driverABI {
		_ = addDriverABI(580, 196, 07, "f", "g", func() *driverABI {
"""


def test_reads_every_entry_form_of_the_table():
    table = dt.known_drivers(VERSION_GO)
    assert table[(580, 126, 9)] == ("v580_126_09", True)
    assert table[(580, 173, 10)] == ("v580_173_10", True)
    assert table[(535, 104, 5)] == ("v535_104_05", False)
    assert table[(580, 196, 7)] == ("_", True)


def test_neighbour_is_the_nearest_supported_lower_version_of_the_branch():
    table = dt.known_drivers(VERSION_GO)
    assert dt.neighbour((580, 126, 16), table) == (580, 126, 9)
    assert dt.neighbour((580, 200, 1), table) == (580, 196, 7)
    assert dt.neighbour((580, 60, 1), table) is None
    # An unsupported entry is never a base.
    assert dt.neighbour((535, 200, 1), table) is None


VERSION_PATCH = """--- a/src/common/inc/nvBldVer.h
+++ b/src/common/inc/nvBldVer.h
-#define NV_BUILD_CHANGELIST_NUM         (37137176)
+#define NV_BUILD_CHANGELIST_NUM         (37265831)
-#define NV_BUILD_NAME                   "582.16"
+#define NV_BUILD_NAME                   "582.28"
"""
NAME_PATCH = """--- a/src/nvidia/generated/g_nv_name_released.h
+++ b/src/nvidia/generated/g_nv_name_released.h
+    { 0x25B2, 0x8f5b, 0x103c, "NVIDIA RTX A400" },
"""


def test_release_lines_and_code_files_keep_the_interface():
    same, reasons = dt.compare(
        [
            ("src/common/inc/nvBldVer.h", VERSION_PATCH),
            ("src/nvidia/generated/g_nv_name_released.h", NAME_PATCH),
            ("kernel-open/nvidia/nv-mmap.c", "+    memcpy_toio(a, b, n);\n"),
            ("version.mk", "+NVIDIA_VERSION = 580.126.16\n"),
        ]
    )
    assert same, reasons


def test_any_other_header_change_is_an_interface_change():
    same, reasons = dt.compare(
        [
            ("src/common/sdk/nvidia/inc/ctrl/ctrl2080/ctrl2080gpu.h", "+    NvU32 newField;\n"),
        ]
    )
    assert not same
    assert "ctrl2080gpu.h" in reasons[0]


def test_a_release_header_with_a_non_release_line_is_an_interface_change():
    patch = "+#define NV_SOMETHING_NEW 1\n"
    same, reasons = dt.compare([("src/common/inc/nvVer.h", patch)])
    assert not same
    assert "NV_SOMETHING_NEW" in reasons[0]


def git(repo, *args):
    subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True)


@pytest.fixture
def nvidia(tmp_path):
    """A repository with three driver tags: .09 (known), .16 (release lines only), .30 (a header change)."""
    origin = tmp_path / "origin"
    origin.mkdir()
    git(str(origin), "init", "-q")
    git(str(origin), "config", "user.email", "t@t")
    git(str(origin), "config", "user.name", "t")
    inc = origin / "src/common/inc"
    sdk = origin / "src/common/sdk/nvidia/inc"
    inc.mkdir(parents=True)
    sdk.mkdir(parents=True)

    def release(tag, version, field):
        (inc / "nvUnixVersion.h").write_text(f'#define NV_VERSION_STRING "{version}"\n')
        (sdk / "nvos.h").write_text(f"typedef struct {{ int a; {field} }} NVOS;\n")
        (origin / "nv.c").write_text(f"/* {version} */\n")
        git(str(origin), "add", "-A")
        git(str(origin), "commit", "-q", "-m", tag)
        git(str(origin), "tag", tag)

    release("580.126.09", "580.126.09", "")
    release("580.126.16", "580.126.16", "")
    release("580.126.30", "580.126.30", "int b;")
    clone = tmp_path / "clone"
    git(str(tmp_path), "init", "-q", str(clone))
    git(str(clone), "remote", "add", "origin", str(origin))
    return str(clone)


def test_scan_accepts_a_release_with_the_same_headers_and_refuses_a_changed_one(nvidia):
    table = 'v580_126_09 := addDriverABI(580, 126, 9, "c", "d", v)\n'
    rows = dt.scan(table, nvidia, dt.remote_tags(nvidia))
    by = {r["version"]: r for r in rows}
    assert by["580.126.16"]["accepted"] and by["580.126.16"]["base"] == "580.126.09"
    assert not by["580.126.30"]["accepted"]
    assert "nvos.h" in by["580.126.30"]["reasons"][0]
    assert "580.126.09" not in by


def test_go_source_lists_only_accepted_versions_with_their_checksums():
    rows = [
        {
            "version": "580.126.16",
            "base": "580.126.09",
            "accepted": True,
            "checksums": {"x86_64": "ab", "aarch64": "NO_DRIVER"},
        },
        {"version": "580.126.30", "base": "580.126.09", "accepted": False, "reasons": ["x"]},
    ]
    src = dt.go_source(rows)
    assert (
        "nvconf.NewDriverVersion(580, 126, 16), base: nvconf.NewDriverVersion(580, 126, 9)" in src
    )
    assert 'NewChecksums("ab", "NO_DRIVER")' in src
    assert "580, 126, 30" not in src


def test_installer_checksum_is_no_driver_when_nvidia_has_no_installer():
    def missing(url, timeout):
        raise urllib.error.HTTPError(url, 404, "Not Found", {}, None)

    def present(url, timeout):
        return io.BytesIO(b"installer")

    assert dt.installer_sha256("u", missing) == dt.NO_DRIVER
    assert dt.installer_sha256("u", present) == hashlib.sha256(b"installer").hexdigest()
