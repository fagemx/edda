#!/usr/bin/env python3
"""Focused tests for the read-only crates release planner."""

from __future__ import annotations

import importlib.util
import io
import json
import sys
import tarfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("crates_release_plan.py")
SPEC = importlib.util.spec_from_file_location("crates_release_plan", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
release_plan = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release_plan
SPEC.loader.exec_module(release_plan)


def metadata_fixture() -> dict[str, object]:
    package_id = "demo 1.2.3 (path+file:///demo)"
    return {
        "workspace_members": [package_id],
        "packages": [
            {
                "id": package_id,
                "name": "demo",
                "version": "1.2.3",
                "publish": None,
                "dependencies": [],
            }
        ],
    }


def crate_archive(sha: str, *, dirty: bool = False) -> bytes:
    output = io.BytesIO()
    payload = json.dumps({"git": {"sha1": sha, "dirty": dirty}}).encode()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        member = tarfile.TarInfo("demo-1.2.3/.cargo_vcs_info.json")
        member.size = len(payload)
        archive.addfile(member, io.BytesIO(payload))
    return output.getvalue()


class ReleasePlanTests(unittest.TestCase):
    def test_topological_levels_are_dependency_first(self) -> None:
        packages = {
            "core": release_plan.Package("core", "1.2.3", frozenset()),
            "middle": release_plan.Package("middle", "1.2.3", frozenset({"core"})),
            "cli": release_plan.Package("cli", "1.2.3", frozenset({"middle"})),
        }
        self.assertEqual(
            release_plan.topological_levels(packages),
            [["core"], ["middle"], ["cli"]],
        )

    def test_topological_cycle_fails_closed(self) -> None:
        packages = {
            "a": release_plan.Package("a", "1.2.3", frozenset({"b"})),
            "b": release_plan.Package("b", "1.2.3", frozenset({"a"})),
        }
        with self.assertRaises(release_plan.ReleasePlanError):
            release_plan.topological_levels(packages)

    def test_version_must_be_bare_semver(self) -> None:
        packages = {"demo": release_plan.Package("demo", "1.2.3", frozenset())}
        with self.assertRaises(release_plan.ReleasePlanError):
            release_plan.require_version(packages, "v1.2.3")

    def test_archive_provenance_rejects_dirty_package(self) -> None:
        sha = "a" * 40
        self.assertEqual(
            release_plan.vcs_sha_from_archive(crate_archive(sha), "fixture"), sha
        )
        with self.assertRaises(release_plan.ReleasePlanError):
            release_plan.vcs_sha_from_archive(
                crate_archive(sha, dirty=True), "dirty fixture"
            )

    def test_recovery_inventory_allows_missing_registry_version(self) -> None:
        status = [{"crate": "demo", "version": "1.2.3", "exists": False, "yanked": None}]
        argv = [
            str(SCRIPT),
            "--version",
            "1.2.3",
            "--check-crates-io",
            "--allow-missing",
            "--json",
        ]
        with (
            mock.patch.object(release_plan, "cargo_metadata", return_value=metadata_fixture()),
            mock.patch.object(release_plan, "wait_for_registry", return_value=status),
            mock.patch.object(release_plan.sys, "argv", argv),
            redirect_stdout(io.StringIO()) as stdout,
        ):
            self.assertEqual(release_plan.main(), 0)
        self.assertFalse(json.loads(stdout.getvalue())["registry"][0]["exists"])

    def test_verify_rejects_missing_registry_version(self) -> None:
        status = [{"crate": "demo", "version": "1.2.3", "exists": False, "yanked": None}]
        argv = [str(SCRIPT), "--version", "1.2.3", "--check-crates-io"]
        with (
            mock.patch.object(release_plan, "cargo_metadata", return_value=metadata_fixture()),
            mock.patch.object(release_plan, "wait_for_registry", return_value=status),
            mock.patch.object(release_plan.sys, "argv", argv),
            redirect_stderr(io.StringIO()) as stderr,
        ):
            self.assertEqual(release_plan.main(), 1)
        self.assertIn("missing or yanked", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
