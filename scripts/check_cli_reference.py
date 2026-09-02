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


def documented_commands(markdown: str) -> set[str]:
    return set(re.findall(r"`edda ([a-z][a-z0-9-]*)", markdown))


def main() -> int:
    completed = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "edda", "--", "--help"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    public = top_level_commands(completed.stdout)
    documented = documented_commands(CLI_REFERENCE.read_text(encoding="utf-8"))
    missing = sorted(public - documented)
    if missing:
        print("docs/reference/cli.md is missing top-level commands:", file=sys.stderr)
        for command in missing:
            print(f"  - edda {command}", file=sys.stderr)
        return 1
    print(f"CLI reference covers all {len(public)} public top-level commands.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
