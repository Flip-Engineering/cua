#!/usr/bin/env python3
"""Reads NVIDIA's security bulletins for its Linux GPU driver into bulletins.json.

NVIDIA publishes each bulletin in github.com/NVIDIA/product-security as
<year>/<id>/<id>.md (a table of CVE, product, platform, affected versions and
updated version) beside a CSAF file. For every bulletin of the GPU display driver
this keeps, per Linux driver branch, the first driver version with its fixes and
the CVEs it fixes. Flip's updater compares a computer's driver with this file and
tells the owner when a newer driver of the same branch has security fixes; it never
installs a driver.

  bulletins.py refresh --out bulletins.json [--text BULLETINS.txt] [--years 2026 ...]

Without --years it reads every year NVIDIA publishes. BULLETINS.txt has one line
per bulletin and branch for the updater, which runs in plain sh:
"<branch> <fixed_in> <bulletin> <number of CVEs> <url>".
"""

import argparse
import json
import os
import re
import sys
import urllib.request

REPO = "NVIDIA/product-security"
RAW = "https://raw.githubusercontent.com/" + REPO + "/main/{path}"
API = "https://api.github.com/repos/" + REPO + "/contents/{path}"
ROW = re.compile(
    r"^\|\s*(?P<cve>CVE-\d{4}-\d+)\s*\|[^|]*\|\s*Linux\(R(?P<branch>\d+)\)\s*\|[^|]*\|\s*"
    r"(?P<fixed>\d+\.\d+\.\d+)\s*\|"
)


def get(url, opener=urllib.request.urlopen):
    headers = {"User-Agent": "flip-runsc-bulletins"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    with opener(urllib.request.Request(url, headers=headers), timeout=60) as r:
        return r.read().decode()


def parse(markdown):
    """{branch: {"fixed_in": version, "cves": [...]}} from a bulletin's table."""
    out = {}
    for line in markdown.splitlines():
        m = ROW.match(line.strip())
        if not m:
            continue
        b = out.setdefault(m["branch"], {"fixed_in": m["fixed"], "cves": []})
        if tuple(map(int, m["fixed"].split("."))) > tuple(map(int, b["fixed_in"].split("."))):
            b["fixed_in"] = m["fixed"]
        if m["cve"] not in b["cves"]:
            b["cves"].append(m["cve"])
    return out


def title_and_date(csaf):
    doc = json.loads(csaf).get("document", {})
    return doc.get("title", ""), doc.get("tracking", {}).get("current_release_date", "")


def refresh(years=None, opener=urllib.request.urlopen):
    if years is None:
        years = [
            e["name"]
            for e in json.loads(get(API.format(path=""), opener))
            if e.get("type") == "dir" and e["name"].isdigit()
        ]
    rows = []
    for year in years:
        for entry in json.loads(get(API.format(path=str(year)), opener)):
            bid = entry["name"]
            base = f"{year}/{bid}/{bid}"
            try:
                linux = parse(get(RAW.format(path=base + ".md"), opener))
            except Exception:
                continue
            if not linux:
                continue
            title, date = title_and_date(get(RAW.format(path=base + ".json"), opener))
            if "GPU Display Driver" not in title:
                continue
            rows.append(
                {
                    "bulletin": bid,
                    "title": title,
                    "date": date,
                    "url": f"https://github.com/{REPO}/blob/main/{base}.md",
                    "linux": linux,
                }
            )
    rows.sort(key=lambda r: r["date"], reverse=True)
    return rows


def text(rows):
    lines = []
    for r in rows:
        for branch, fix in sorted(r["linux"].items()):
            lines.append(
                f"{branch} {fix['fixed_in']} {r['bulletin']} {len(fix['cves'])} {r['url']}\n"
            )
    return "".join(lines)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("refresh")
    r.add_argument("--out", required=True)
    r.add_argument("--text")
    r.add_argument("--years", nargs="+")
    a = p.parse_args(argv)
    rows = refresh(a.years)
    with open(a.out, "w") as f:
        json.dump(rows, f, indent=1)
        f.write("\n")
    if a.text:
        with open(a.text, "w") as f:
            f.write(text(rows))
    print(f"{len(rows)} driver bulletins", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
