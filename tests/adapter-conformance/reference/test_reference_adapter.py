#!/usr/bin/env python3
"""Own meaningful tests for the source-blind reference adapter.

Runs the adapter as a subprocess over the normalized protocol against a
throwaway temp workspace + store, using the public `edda` CLI data plane
(path taken from EDDA_BIN, default <repo>/tools/edda.exe). Never touches
the operator's real store, never calls `edda hook`.

Run:  python -m unittest reference.test_reference_adapter -v
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ADAPTER = Path(__file__).resolve().parent / "edda_reference_adapter.py"
EDDA = os.environ.get("EDDA_BIN") or str(REPO / "tools" / "edda.exe")
SENTINEL = "AKIAIOSFODNN7EXAMPLE"


class AdapterTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix="edda-ref-adapter-test-")
        cls.project = Path(cls.tmp) / "project"
        cls.store = Path(cls.tmp) / "store"
        cls.project.mkdir(parents=True)
        cls.store.mkdir(parents=True)
        subprocess.run(
            [EDDA, "init", "--no-hooks"], cwd=str(cls.project),
            capture_output=True, text=True,
            env={**os.environ, "EDDA_STORE_ROOT": str(cls.store)},
            timeout=60,
        )

    def envelope(self, event, session_id, payload=None):
        env = {
            "event": event,
            "conformance": {
                "contract_version": "adapter-contract/0.1",
                "protocol_version": "adapter-normalized-protocol/0.1",
                "session_id": session_id,
                "workspace": str(self.project),
                "store_root": str(self.store),
                "edda_bin": EDDA,
            },
        }
        if payload is not None:
            env["payload"] = payload
        return env

    def run_adapter(self, stdin_text, extra_env=None):
        env = dict(os.environ)
        env["EDDA_STORE_ROOT"] = str(self.store)
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            [sys.executable, str(ADAPTER)], input=stdin_text,
            capture_output=True, text=True, cwd=str(self.project),
            env=env, timeout=60,
        )

    def edda_log_notes(self):
        proc = subprocess.run(
            [EDDA, "log", "--type", "note", "--json", "--limit", "0"],
            capture_output=True, text=True, cwd=str(self.project),
            env={**os.environ, "EDDA_STORE_ROOT": str(self.store)},
            timeout=60,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return proc.stdout

    # 1. Fail-open on malformed stdin.
    def test_fail_open_on_garbage(self):
        proc = self.run_adapter("this is not json {{{")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertIsInstance(out, dict)
        self.assertEqual(str(out.get("permission", "allow")).lower(), "allow")

    # 2. Unknown future event exits 0 permissively.
    def test_unknown_event_permissive(self):
        proc = self.run_adapter(json.dumps(self.envelope("event.from.the.future.v9", "t-unk")))
        self.assertEqual(proc.returncode, 0)
        self.assertIsInstance(json.loads(proc.stdout), dict)

    # 3. session.start injects bounded context with boundaries + write-back.
    def test_session_start_injection(self):
        proc = self.run_adapter(json.dumps(self.envelope("session.start", "t-start")))
        self.assertEqual(proc.returncode, 0)
        ctx = json.loads(proc.stdout)["context"]
        self.assertIn("<!-- edda:start -->", ctx)
        self.assertIn("<!-- edda:end -->", ctx)
        self.assertIn("edda decide", ctx)

    # 4. Tiny budget truncates the body but preserves the write-back tail.
    def test_budget_truncation_preserves_tail(self):
        big = self.run_adapter(json.dumps(self.envelope("session.start", "t-big")),
                               {"EDDA_MAX_CONTEXT_CHARS": "100000"})
        tiny = self.run_adapter(json.dumps(self.envelope("session.start", "t-tiny")),
                                {"EDDA_MAX_CONTEXT_CHARS": "300",
                                 "EDDA_WORKSPACE_BUDGET_CHARS": "1200"})
        big_ctx = json.loads(big.stdout)["context"]
        tiny_ctx = json.loads(tiny.stdout)["context"]
        self.assertIn("edda decide", tiny_ctx)
        self.assertLess(len(tiny_ctx), len(big_ctx))
        self.assertLessEqual(len(tiny_ctx), 320)

    # 5. Identical consecutive prompt injections are deduped.
    def test_prompt_dedup(self):
        ev = self.envelope("prompt.submit", "t-dedup", {"prompt": "probe"})
        first = json.loads(self.run_adapter(json.dumps(ev)).stdout).get("context")
        second = json.loads(self.run_adapter(json.dumps(ev)).stdout).get("context")
        self.assertIsNotNone(first)
        self.assertIsNone(second)

    # 6. Lifecycle notes: heartbeat start/end + activity + exactly one digest
    #    across repeated session.end delivery.
    def test_lifecycle_and_digest_idempotency(self):
        sid = "t-lifecycle"
        self.run_adapter(json.dumps(self.envelope("session.start", sid)))
        self.run_adapter(json.dumps(self.envelope("tool.post", sid, {
            "tool_name": "Bash",
            "tool_input": {"command": "git status --short"},
            "tool_output": {"stdout": " M f", "exit_code": 0}})))
        self.run_adapter(json.dumps(self.envelope("session.end", sid)))
        self.run_adapter(json.dumps(self.envelope("session.end", sid)))  # repeat
        log = self.edda_log_notes()
        self.assertIn("session=%s action=heartbeat.start" % sid, log)
        self.assertIn("session=%s action=activity.append" % sid, log)
        self.assertIn("session=%s action=heartbeat.end" % sid, log)
        digest_notes = sum(1 for ln in log.splitlines()
                           if "session=%s action=digest.complete" % sid in ln)
        self.assertEqual(digest_notes, 1)

    # 7. Sentinel secret never reaches the durable public ledger.
    def test_secret_redaction(self):
        sid = "t-redact"
        self.run_adapter(json.dumps(self.envelope("session.start", sid)))
        self.run_adapter(json.dumps(self.envelope("tool.post.secret", sid, {
            "tool_name": "Bash",
            "tool_input": {"command": "export AWS_ACCESS_KEY_ID=" + SENTINEL},
            "tool_output": {"stdout": "", "exit_code": 0}})))
        self.assertNotIn(SENTINEL, self.edda_log_notes())

    # 8. Decision nudges are rate-limited to <= 2 per session.
    def test_nudge_rate_limit(self):
        sid = "t-nudge"
        self.run_adapter(json.dumps(self.envelope("session.start", sid)))
        nudges = 0
        for i in range(6):
            ev = self.envelope("tool.post.signal", sid, {
                "tool_name": "Bash",
                "tool_input": {"command": "git commit -m 'probe %d'" % i},
                "tool_output": {"stdout": "[main] probe", "exit_code": 0}})
            out = json.loads(self.run_adapter(json.dumps(ev)).stdout)
            if out.get("context"):
                nudges += 1
        self.assertLessEqual(nudges, 2)

    # 9. tool.pre stays advisory-allow.
    def test_tool_pre_allow(self):
        proc = self.run_adapter(json.dumps(self.envelope("tool.pre", "t-pre", {
            "tool_name": "Bash", "tool_input": {"command": "whoami"}})))
        out = json.loads(proc.stdout)
        self.assertEqual(out["hookSpecificOutput"]["permissionDecision"], "allow")
        self.assertNotIn("updatedInput", json.dumps(out))


if __name__ == "__main__":
    unittest.main()
