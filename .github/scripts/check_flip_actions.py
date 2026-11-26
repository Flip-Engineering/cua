#!/usr/bin/env python3
"""Fail when this fork could start a run on GitHub's own machines.

Flip-Engineering/cua runs its workflows on Flip's self-hosted runners only.
The fork carries upstream's workflow files, and almost all of them ask for
GitHub-hosted machines, so they are switched off on GitHub and only the files
named in .github/flip-active-workflows.txt may be active. A merge from
upstream can add a new workflow file, and GitHub makes a new file active.
This check stops that from going unnoticed.

  static [root]   every job of every listed workflow names `self-hosted` in
                  its runs-on, and no label is an expression or a GitHub image.
  live REPO       no workflow other than the listed ones is active on GitHub
                  (reads GITHUB_TOKEN, needs `actions: read`).

The release workflow runs both as its first job, so no release is built while
either fails. No third-party module is needed.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.request
from pathlib import Path

ALLOWED_FILE = Path(".github/flip-active-workflows.txt")
HOSTED = re.compile(r"^(ubuntu|windows|macos)-", re.IGNORECASE)
JOB = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
RUNS_ON = re.compile(r"^    runs-on:\s*(.*?)\s*$")


def allowed(root: Path) -> list[str]:
    lines = (root / ALLOWED_FILE).read_text().splitlines()
    return [line.split("#", 1)[0].strip() for line in lines if line.split("#", 1)[0].strip()]


def labels_of(text: str) -> list[str]:
    text = text.strip()
    if text.startswith("["):
        text = text.strip("[]")
    return [part.strip().strip("'\"") for part in text.split(",") if part.strip()]


def runs_on(lines: list[str]) -> dict[str, str]:
    """Each job's runs-on as one string: a scalar, a flow list, or the
    `labels:` of a block mapping (with `group:` noted)."""
    jobs: dict[str, str] = {}
    in_jobs = False
    job = None
    for i, line in enumerate(lines):
        if line.rstrip() == "jobs:":
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if line and not line.startswith(" ") and not line.startswith("#"):
            in_jobs = False
            continue
        m = JOB.match(line)
        if m:
            job = m.group(1)
            jobs.setdefault(job, "")
            continue
        m = RUNS_ON.match(line)
        if m and job:
            value = m.group(1)
            if value:
                jobs[job] = value
                continue
            block = []
            for nxt in lines[i + 1 :]:
                if not nxt.startswith("      "):
                    break
                block.append(nxt.strip())
            label_line = next((b for b in block if b.startswith("labels:")), "")
            jobs[job] = label_line.removeprefix("labels:").strip() or " ".join(block)
    return jobs


def static(root: Path) -> list[str]:
    problems = []
    for name in allowed(root):
        path = root / name
        if not path.is_file():
            problems.append(f"{name}: listed in {ALLOWED_FILE} but missing")
            continue
        jobs = runs_on(path.read_text().splitlines())
        if not jobs:
            problems.append(f"{name}: no jobs found")
        for job, value in jobs.items():
            if not value:
                problems.append(f"{name}: job {job} has no runs-on")
                continue
            if "${{" in value:
                problems.append(f"{name}: job {job} computes runs-on ({value}); write the labels out")
                continue
            labels = labels_of(value)
            hosted = [label for label in labels if HOSTED.match(label)]
            if hosted:
                problems.append(f"{name}: job {job} names GitHub's machine {hosted[0]}")
            elif "self-hosted" not in labels:
                problems.append(f"{name}: job {job} runs-on {value} does not name self-hosted")
    return problems


def active_workflows(repo: str, token: str) -> list[str]:
    paths, page = [], 1
    while True:
        req = urllib.request.Request(
            f"https://api.github.com/repos/{repo}/actions/workflows?per_page=100&page={page}",
            headers={"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json"},
        )
        with urllib.request.urlopen(req, timeout=60) as resp:
            body = json.load(resp)
        rows = body.get("workflows", [])
        paths += [w["path"] for w in rows if w.get("state") == "active"]
        if len(rows) < 100:
            return paths
        page += 1


def live(root: Path, active: list[str]) -> list[str]:
    keep = set(allowed(root))
    return [
        f"{path} is active on GitHub; switch it off: "
        f"gh workflow disable {Path(path).name} -R <repo>, or list it in {ALLOWED_FILE}"
        for path in sorted(active)
        if path not in keep
    ]


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[1] not in ("static", "live"):
        print(__doc__)
        return 2
    if argv[1] == "static":
        problems = static(Path(argv[2]) if len(argv) > 2 else Path("."))
    else:
        if len(argv) < 3:
            print("usage: check_flip_actions.py live OWNER/REPO")
            return 2
        token = os.environ.get("GITHUB_TOKEN", "")
        if not token:
            print("GITHUB_TOKEN is not set")
            return 2
        problems = live(Path("."), active_workflows(argv[2], token))
    for problem in problems:
        print(f"::error::{problem}")
    print("ok" if not problems else f"{len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
