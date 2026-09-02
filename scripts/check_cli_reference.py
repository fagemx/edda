#!/usr/bin/env python3
"""Fail when the public CLI exposes a command missing from cli.md."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CLI_REFERENCE = ROOT / "docs" / "reference" / "cli.md"


def top_level_commands(help_text: str) -> set[str]:
    match = re.search(r"^Commands:\s*$\n(?P<body>.*?)(?:\n\n|\Z)", help_text, re.MULTILINE | re.DOTALL)
    if match is None:
        raise RuntimeError("could not find the Commands section in `edda --help`")
    return {
        command.group(1)
        for line in match.group("body").splitlines()
        if (command := re.match(r"^  ([a-z][a-z0-9-]*)\s{2,}", line))
        and command.group(1) != "help"
    }


def indexed_commands(markdown: str) -> set[str]:
    match = re.search(
        r"^## Command index\s*$\n(?P<body>.*?)(?=^##\s|\Z)",
        markdown,
        re.MULTILINE | re.DOTALL,
    )
    if match is None:
        raise RuntimeError("could not find the Command index in cli.md")
    return set(re.findall(r"`edda ([a-z][a-z0-9-]*)", match.group("body")))


def command_drift(public: set[str], indexed: set[str]) -> tuple[list[str], list[str]]:
    return sorted(public - indexed), sorted(indexed - public)


def self_test() -> None:
    sample = """# CLI Reference

## Command index

`edda alpha`, `edda beta`

## Details

The body mentions `edda gamma`.
"""
    if indexed_commands(sample) != {"alpha", "beta"}:
        raise AssertionError("command index parser leaked into the document body")
    missing, stale = command_drift({"alpha", "gamma"}, {"alpha", "beta"})
    if missing != ["gamma"] or stale != ["beta"]:
        raise AssertionError("command drift must report both missing and stale entries")


def main() -> int:
    self_test()
    completed = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "edda", "--", "--help"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    public = top_level_commands(completed.stdout)
    indexed = indexed_commands(CLI_REFERENCE.read_text(encoding="utf-8"))
    missing, stale = command_drift(public, indexed)
    if missing:
        print("CLI Command index is missing top-level commands:", file=sys.stderr)
        for command in missing:
            print(f"  - edda {command}", file=sys.stderr)
    if stale:
        print("CLI Command index contains stale commands:", file=sys.stderr)
        for command in stale:
            print(f"  - edda {command}", file=sys.stderr)
    if missing or stale:
        return 1
    print(f"CLI Command index exactly matches all {len(public)} public top-level commands.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
