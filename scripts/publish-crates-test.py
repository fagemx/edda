#!/usr/bin/env python3
"""Offline regressions for GH648. No token and no registry writes."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import unittest
from unittest.mock import patch
import urllib.error

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).with_name("publish-crates.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
SHA = "a" * 40


def package(name, dependencies=()):
    return {"id": name, "name": name, "version": "1.2.3", "publish": None,
            "manifest_path": f"/repo/crates/{name}/Cargo.toml",
            "dependencies": [{"name": d, "kind": None, "path": f"/repo/crates/{d}", "req": "^1.2.3"}
                             for d in dependencies]}


def metadata(*packages):
    return {"workspace_members": [p["id"] for p in packages], "packages": list(packages)}


def archive(sha=SHA, dirty=False, path="crates/edda", has_vcs=True):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
        if has_vcs:
            data = json.dumps({"git": {"sha1": sha, "dirty": dirty}, "path_in_vcs": path}).encode()
            entry = tarfile.TarInfo("edda-1.2.3/.cargo_vcs_info.json")
            entry.size = len(data)
            tar.addfile(entry, io.BytesIO(data))
    return buffer.getvalue()


class PublisherTests(unittest.TestCase):
    def test_order_derived_after_dependency_changes_and_cli_last(self):
        edda, a, z = package("edda", ["a"]), package("a", ["z"]), package("z")
        self.assertEqual([p["name"] for p in publisher.publish_order(metadata(edda, a, z), "1.2.3")],
                         ["z", "a", "edda"])
        a["dependencies"] = []
        z["dependencies"] = package("z", ["a"])["dependencies"]
        self.assertEqual([p["name"] for p in publisher.publish_order(metadata(edda, z, a), "1.2.3")],
                         ["a", "z", "edda"])

    def test_rejects_cycle_private_dependency_and_version_mismatch(self):
        cases = [metadata(package("edda"), package("a", ["b"]), package("b", ["a"])),
                 metadata(package("edda", ["private"]), dict(package("private"), publish=[])),
                 metadata(dict(package("edda"), version="9.9.9"))]
        for data in cases:
            with self.subTest(data=data), self.assertRaises(publisher.ReleaseError):
                publisher.publish_order(data, "1.2.3")

    def test_versioned_dev_and_optional_target_dependencies_affect_order(self):
        edda, z = package("edda", ["z"]), package("z")
        edda["dependencies"][0].update(kind="dev", optional=True, target="cfg(windows)")
        self.assertEqual([p["name"] for p in publisher.publish_order(metadata(edda, z), "1.2.3")],
                         ["z", "edda"])

    def registry(self, data, **fields):
        record = {"crate": "edda", "num": "1.2.3", "yanked": False,
                  "checksum": hashlib.sha256(data).hexdigest(), **fields}
        registry = publisher.Registry(SHA, "/repo")
        registry.fetch = lambda url, **kwargs: json.dumps({"version": record}).encode() if "/api/" in url else data
        return registry

    def test_archive_source_identity_not_presence_is_success(self):
        self.assertTrue(self.registry(archive()).verified(package("edda")))
        for data, fields in [(archive("b" * 40), {}), (archive(dirty=True), {}),
                             (archive(path="crates/not-edda"), {}), (archive(has_vcs=False), {}),
                             (archive(), {"yanked": True}), (archive(), {"checksum": "0" * 64}),
                             (archive(), {"num": "9.9.9"})]:
            with self.subTest(fields=fields), self.assertRaises(publisher.ReleaseError):
                self.registry(data, **fields).verified(package("edda"))

    def test_404_is_missing_other_http_failures_do_not_authorize_upload(self):
        registry = publisher.Registry(SHA, "/repo")
        with patch.object(publisher.urllib.request, "urlopen", side_effect=urllib.error.HTTPError(
                "url", 404, "missing", {}, None)):
            self.assertIsNone(registry.fetch("https://crates.io/example", missing_ok=True))
        with patch.object(publisher.urllib.request, "urlopen", side_effect=urllib.error.HTTPError(
                "url", 403, "denied", {}, None)), self.assertRaises(publisher.ReleaseError):
            registry.fetch("https://crates.io/example", missing_ok=True)

    def test_rerun_skips_existing_and_accepts_upload_timeout_then_continues(self):
        seen, uploaded = [], {"core"}

        class Registry:
            def verified(self, p):
                return p["name"] in uploaded

        def run(args, check):
            self.assertFalse(check)
            seen.append(args[-1])
            uploaded.add(args[-1])
            return subprocess.CompletedProcess(args, 101)

        packages = [package("core"), package("other"), package("edda")]
        publisher.publish(packages, Registry(), run=run, sleep=lambda _: None)
        self.assertEqual(seen, ["other", "edda"])
        publisher.publish(packages, Registry(), run=run, sleep=lambda _: None)
        self.assertEqual(seen, ["other", "edda"])

    def test_missing_final_version_is_red_even_when_cargo_exits_zero(self):
        class Registry:
            def verified(self, p):
                return False

        with self.assertRaisesRegex(publisher.ReleaseError, "crates.io incomplete: edda"):
            publisher.publish([package("edda")], Registry(),
                              run=lambda args, check: subprocess.CompletedProcess(args, 0), sleep=lambda _: None)

    def test_workflow_token_skip_and_release_dependency_are_wired(self):
        workflow = Path(__file__).resolve().parents[1] / ".github/workflows/release.yml"
        text = workflow.read_text()
        prepare = text.split("  prepare-crates:", 1)[1].split("  publish-crates:", 1)[0]
        publish = text.split("  publish-crates:", 1)[1].split("  create-release:", 1)[0]
        create = text.split("  create-release:", 1)[1].split("  build-release:", 1)[0]
        self.assertIn('echo "enabled=false"', prepare)
        self.assertIn("::notice::CARGO_REGISTRY_TOKEN is absent", prepare)
        self.assertIn("needs.prepare-crates.outputs.enabled == 'true'", publish)
        self.assertIn("github.event_name == 'push'", publish)
        self.assertIn("needs: publish-crates", create)
        self.assertIn("publish-crates.py verify", create)


if __name__ == "__main__":
    unittest.main()
