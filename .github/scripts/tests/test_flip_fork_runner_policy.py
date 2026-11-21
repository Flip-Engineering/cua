"""Every executable workflow job must select an admitted runner on the Flip fork."""

from pathlib import Path
import re

import yaml

from tests.workflow_conditions import condition, upstream_condition

WORKFLOWS = Path(__file__).resolve().parents[2] / "workflows"
CANONICAL = "github.repository == 'trycua/cua'"
FORK = "github.repository == 'Flip-Engineering/cua'"
LABELS_BY_OS = {
    "Linux": {"baton2-native-linux-x64", "docker-linux"},
    "macOS": {"flip-macos-lab"},
    "Windows": {"flip-windows-lab"},
}


def approved(labels):
    return (
        isinstance(labels, list)
        and len(labels) == 4
        and labels[0] == "self-hosted"
        and labels[2] == "X64"
        and labels[3] in LABELS_BY_OS.get(labels[1], ())
    )


def canonical_only(raw):
    value = condition(raw)
    original = upstream_condition(raw)
    unwrapped = re.sub(r"'(?:''|[^'])*'", "''", value)
    while re.search(r"\([^()]*\)", unwrapped):
        unwrapped = re.sub(r"\([^()]*\)", "", unwrapped)
    return (
        value == CANONICAL
        or value == f"({original}) && {CANONICAL}"
        or (value.startswith(f"{CANONICAL} && ") and "||" not in unwrapped)
    )


def test_every_fork_workflow_dispatches_only_to_approved_runners():
    for path in WORKFLOWS.glob("*.y*ml"):
        data = yaml.safe_load(path.read_text())
        for name, job in data["jobs"].items():
            if canonical_only(job.get("if", "")):
                continue
            if "uses" in job:
                uses = job["uses"]
                assert uses.startswith("./.github/workflows/"), (path.name, name)
                assert (WORKFLOWS / Path(uses).name).is_file(), (path.name, name)
                continue

            route = job["runs-on"]
            labels = route.get("labels") if isinstance(route, dict) else route
            if approved(labels):
                if isinstance(route, dict) and "group" in route:
                    assert labels[1] == "Linux" and route["group"] == "baton2-native-homelab"
                continue
            if isinstance(labels, list) and "${{ matrix.runner.os }}" in labels:
                assert labels == ["self-hosted", "X64", "${{ matrix.runner.os }}", "${{ matrix.runner.label }}"]
                assert "group" not in route
                for row in job["strategy"]["matrix"]["include"]:
                    runner = row["runner"]
                    assert runner["label"] in LABELS_BY_OS[runner["os"]], (path.name, name, runner)
                continue
            assert isinstance(route, str), (path.name, name, route)
            fork_selector = re.fullmatch(
                re.escape(FORK) + r" && fromJSON\('(\[[^\]]+\])'\) \|\| 'ubuntu-[^']+'",
                condition(route),
            )
            assert fork_selector and approved(yaml.safe_load(fork_selector[1])), (path.name, name, route)


def test_fork_desktop_lane_uses_native_harness_for_trusted_pull_requests():
    workflow = yaml.safe_load((WORKFLOWS / "e2e-rust-macos.yml").read_text())
    jobs = workflow["jobs"]
    lab = jobs["lab"]
    assert lab["runs-on"] == ["self-hosted", "macOS", "X64", "flip-macos-lab"]
    assert FORK in lab["if"]
    assert "github.event.pull_request.head.repo.full_name == github.repository" in lab["if"]
    assert any(step.get("run") == "bash scripts/ci/macos/run-lab-rust-e2e.sh" for step in lab["steps"])
    for name in ("probe", "matrix", "certify", "register-lume"):
        assert canonical_only(jobs[name]["if"]), name
