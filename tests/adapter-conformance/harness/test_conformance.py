#!/usr/bin/env python3
"""Focused portable regression tests for the conformance harness."""
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

HARNESS = Path(__file__).with_name("conformance.py")
SPEC = importlib.util.spec_from_file_location("conformance", HARNESS)
conformance = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(conformance)


class HarnessRegressionTest(unittest.TestCase):
    def test_malformed_and_denial_are_not_permissive(self):
        self.assertFalse(conformance._looks_permissive("not-json-and-denying"))
        self.assertFalse(conformance._looks_permissive('{"permission":"deny"}'))
        self.assertTrue(conformance._looks_permissive('{"continue":true}'))

    def test_fixture_exercises_every_launcher_receipt_field(self):
        for agent in conformance.LAUNCHER_AGENTS:
            proc = conformance._fake_launcher(agent)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            receipt = json.loads(proc.stdout)
            self.assertTrue(all(field in receipt for field in conformance.RECEIPT_FIELDS))
            self.assertEqual(receipt["heartbeat_owner"], "launcher")

    def test_incomplete_launcher_control_lacks_required_fields(self):
        proc = conformance._fake_launcher("claude", mode="bad-receipt")
        receipt = json.loads(proc.stdout)
        self.assertTrue(any(field not in receipt for field in conformance.RECEIPT_FIELDS))


if __name__ == "__main__":
    unittest.main()
