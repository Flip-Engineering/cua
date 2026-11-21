"""Inspect upstream job conditions with the fork routing boundary removed."""

import re

import yaml


def condition(value: str) -> str:
    text = " ".join(value.split())
    if text.startswith("${{") and text.endswith("}}"):
        return text[3:-2].strip()
    return text


def upstream_condition(value: str = "") -> str:
    text = condition(value)
    canonical = "github.repository == 'trycua/cua'"
    if text == canonical:
        return ""
    fork_guard = re.search(
        r"\) && (?:github\.repository == 'trycua/cua'$|"
        r"\(github\.repository == 'trycua/cua' \|\| )",
        text,
    )
    if text.startswith("(") and fork_guard:
        return text[1:fork_guard.start()]
    return text


def job_condition(workflow: str, job: str) -> str:
    return upstream_condition(yaml.safe_load(workflow)["jobs"][job].get("if", ""))
