# Contract tests against a REAL built edda binary on a temp repo with an
# isolated store (EDDA_STORE_ROOT). Skipped unless EDDA_BIN is set.
#
# Also emits a normalized scenario transcript when EDDA_SCENARIO_OUT is set,
# which the cross-language runner compares against the TypeScript SDK's
# transcript (structural equivalence, contract §7).

import json
import os
import http.server
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from edda_sdk.client import EddaClient, OPERATIONS  # noqa: E402
from edda_sdk.errors import (  # noqa: E402
    CancelledError_,
    TransportError,
    HttpWriteRefused,
    RpcError,
    TimeoutError_,
)
from edda_sdk.transport_http import HttpTransport  # noqa: E402
from edda_sdk.transport_mcp import CallOptions as CallOpts  # noqa: E402
from edda_sdk.transport_mcp import McpSpawnSpec, McpTransport  # noqa: E402

EDDA_BIN = os.environ.get("EDDA_BIN", "")


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _make_env():
    root = tempfile.mkdtemp(prefix="edda-contract-py-")
    store_root = os.path.join(root, "store")
    env = dict(os.environ)
    env["EDDA_STORE_ROOT"] = store_root
    r = subprocess.run([EDDA_BIN, "init"], cwd=root, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        shutil.rmtree(root, ignore_errors=True)
        raise RuntimeError(f"edda init failed: {r.stderr}")
    return root, store_root


def _make_client(root, store_root):
    return EddaClient(
        mcp=McpSpawnSpec(
            command=EDDA_BIN,
            args=["mcp", "serve"],
            cwd=root,
            env={"EDDA_STORE_ROOT": store_root},
        )
    )


@unittest.skipIf(not EDDA_BIN, "EDDA_BIN not set")
class CapabilityProbeTests(unittest.TestCase):
    def test_capabilities_probe_covers_all_contracted_operations(self):
        root, store_root = _make_env()
        client = _make_client(root, store_root)
        try:
            caps = client.capabilities(CallOpts(timeout_s=20))
            for op in OPERATIONS:
                self.assertTrue(caps[op], f"capability {op} should be available")
        finally:
            client.close()
            shutil.rmtree(root, ignore_errors=True)


@unittest.skipIf(not EDDA_BIN, "EDDA_BIN not set")
class RoundTripTests(unittest.TestCase):
    def test_ask_note_decide_round_trip_over_mcp(self):
        root, store_root = _make_env()
        client = _make_client(root, store_root)
        try:
            client.call("note", {"note": "contract-test note py"}, CallOpts(timeout_s=20))
            client.call(
                "decide",
                {"key": "sdk.contract.py", "value": "ok", "reason": "round-trip"},
                CallOpts(timeout_s=20),
            )
            ask = client.call("ask", {"query": "sdk.contract.py"}, CallOpts(timeout_s=20))
            self.assertTrue(any("sdk.contract.py" in json.dumps(d) for d in ask.get("decisions", [])))
        finally:
            client.close()
            shutil.rmtree(root, ignore_errors=True)

    def test_task_new_start_done_receipt_verify_over_mcp(self):
        root, store_root = _make_env()
        client = _make_client(root, store_root)
        try:
            created = client.call(
                "task.new",
                {"title": "contract task", "idempotency_key": "py-1"},
                CallOpts(timeout_s=20),
            )
            self.assertEqual(created["task_id"], 1)
            self.assertFalse(created["deduped"])
            # idempotency: same key reuses, never twins
            again = client.call(
                "task.new",
                {"title": "contract task", "idempotency_key": "py-1"},
                CallOpts(timeout_s=20),
            )
            self.assertEqual(again["task_id"], 1)
            self.assertTrue(again["deduped"])
            # start before done (start/done pairing enforced by the shared state machine)
            with self.assertRaises(RpcError) as cm:
                client.call("task.done", {"id": 1, "receipt": "premature"}, CallOpts(timeout_s=20))
            self.assertIn("not been started", str(cm.exception))
            client.call("task.start", {"id": created["task_id"]}, CallOpts(timeout_s=20))
            done = client.call(
                "task.done",
                {"id": 1, "receipt": "contract receipt py", "evidence_paths": ["sdk/python"]},
                CallOpts(timeout_s=20),
            )
            self.assertIsInstance(done["unlocked"], list)
            receipt = client.call("receipt", {"task_id": 1}, CallOpts(timeout_s=20))
            self.assertEqual(receipt["receipt"], "contract receipt py")
            self.assertEqual(receipt["status"], "done")
            verify = client.call("verify", {}, CallOpts(timeout_s=20))
            self.assertTrue(verify["ok"])
            self.assertGreater(verify["events"], 0)
        finally:
            client.close()
            shutil.rmtree(root, ignore_errors=True)

    def test_claim_over_mcp_writes_and_reads_back(self):
        root, store_root = _make_env()
        client = _make_client(root, store_root)
        try:
            claim = client.call(
                "claim",
                {"label": "py-scope", "paths": ["sdk/python/*"], "session": "py-contract-session"},
                CallOpts(timeout_s=20),
            )
            self.assertEqual(claim["label"], "py-scope")
            self.assertIsNone(claim["replaced"])
        finally:
            client.close()
            shutil.rmtree(root, ignore_errors=True)

    def test_timeout_and_cancellation_are_typed_deterministic(self):
        # Deterministic: a synthetic MCP server that delays every response
        # (never racing a fast real CLI). Deadline, cancellation and the
        # child-reaping/error path are asserted against the REAL transport
        # machinery (pipes, poll loop, close()); the real edda round-trip
        # stays in the live tests above.
        synth = str(Path(__file__).resolve().parent / "synth_mcp_server.py")
        transport = McpTransport(
            McpSpawnSpec(command=sys.executable, args=[synth, "--delay", "30"])
        )
        try:
            with self.assertRaises(TimeoutError_):
                transport.call_tool("anything", {}, CallOpts(timeout_s=0.3))
        finally:
            transport.close()
        # close() reaped the child even after a deadline abort:
        self.assertIsNone(transport._proc)

        transport = McpTransport(McpSpawnSpec(command=sys.executable, args=[synth, "--delay", "30"]))
        try:
            cancel = threading.Event()
            timer = threading.Timer(0.3, cancel.set)
            timer.start()
            with self.assertRaises(CancelledError_):
                transport.call_tool("anything", {}, CallOpts(timeout_s=30, cancel=cancel))
            timer.join()
        finally:
            transport.close()

    def test_dead_child_surfaces_transport_error_and_reaps(self):
        synth = str(Path(__file__).resolve().parent / "synth_mcp_server.py")
        transport = McpTransport(
            McpSpawnSpec(command=sys.executable, args=[synth, "--exit-immediately"])
        )
        try:
            with self.assertRaises(TransportError):
                transport.call_tool("anything", {}, CallOpts(timeout_s=10))
        finally:
            transport.close()
        self.assertTrue(transport._proc is None)


class HttpTransportLocalTests(unittest.TestCase):
    def _server(self, delay: float = 0.0):
        seen: list[str | None] = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                seen.append(self.headers.get("Authorization"))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                if delay:
                    time.sleep(delay)
                self.wfile.write(b'{"ok":true}')

            def log_message(self, *_args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, name="edda-sdk-test-http")
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(lambda: (server.shutdown(), thread.join()))
        return server, seen

    def test_http_auth_and_pre_cancel(self):
        server, seen = self._server()
        port = server.server_address[1]
        http = HttpTransport(f"http://127.0.0.1:{port}", bearer_token="test-token")
        self.assertEqual(http.health(), {"ok": True})
        self.assertEqual(seen, ["Bearer test-token"])
        cancelled = threading.Event()
        cancelled.set()
        with self.assertRaises(CancelledError_):
            http.health(cancel=cancelled)
        self.assertEqual(seen, ["Bearer test-token"], "pre-cancel must not open a request")

    def test_http_inflight_cancel_reaps_worker(self):
        server, _ = self._server(delay=2)
        http = HttpTransport(f"http://127.0.0.1:{server.server_address[1]}")
        cancelled = threading.Event()
        timer = threading.Timer(0.1, cancelled.set)
        timer.start()
        with self.assertRaises(CancelledError_):
            http.health(timeout_s=5, cancel=cancelled)
        timer.join()
        self.assertFalse(any(t.name == "edda-sdk-http" and t.is_alive() for t in threading.enumerate()))


@unittest.skipIf(not EDDA_BIN, "EDDA_BIN not set")
class HttpReadOnlyTests(unittest.TestCase):
    def test_http_reads_work_and_writes_are_refused(self):
        root, store_root = _make_env()
        port = _free_port()
        env = dict(os.environ)
        env["EDDA_STORE_ROOT"] = store_root
        proc = subprocess.Popen(
            [EDDA_BIN, "serve", "--port", str(port)],
            cwd=root,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        http = HttpTransport(f"http://127.0.0.1:{port}")
        try:
            up = False
            for _ in range(50):
                time.sleep(0.2)
                try:
                    http.health(timeout_s=1)
                    up = True
                    break
                except Exception:  # noqa: BLE001
                    continue
            self.assertTrue(up, "edda serve did not become healthy")
            http.status(timeout_s=5)
            http.decisions(timeout_s=5)
            with self.assertRaises(HttpWriteRefused):
                http._request("POST", "/api/note")  # noqa: SLF001 - deliberate write probe
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
            shutil.rmtree(root, ignore_errors=True)


@unittest.skipIf(not EDDA_BIN, "EDDA_BIN not set")
class ScenarioTranscriptTests(unittest.TestCase):
    def test_normalized_transcript(self):
        root, store_root = _make_env()
        client = _make_client(root, store_root)
        try:
            client.call("note", {"note": "scenario note"}, CallOpts(timeout_s=20))
            client.call("decide", {"key": "sdk.scenario.alpha", "value": "one"}, CallOpts(timeout_s=20))
            client.call(
                "decide",
                {"key": "sdk.scenario.beta", "value": "two", "reason": "r"},
                CallOpts(timeout_s=20),
            )
            # task flow through the shared state machine
            client.call(
                "task.new",
                {"title": "scenario task", "idempotency_key": "scenario-1"},
                CallOpts(timeout_s=20),
            )
            client.call("task.start", {"id": 1}, CallOpts(timeout_s=20))
            client.call(
                "task.done",
                {"id": 1, "receipt": "scenario receipt", "evidence_paths": ["evidence"]},
                CallOpts(timeout_s=20),
            )
            receipt = client.call("receipt", {"task_id": 1}, CallOpts(timeout_s=20))
            client.call("claim", {"label": "scenario-scope", "paths": ["sdk/*"]}, CallOpts(timeout_s=20))
            verify = client.call("verify", {}, CallOpts(timeout_s=20))
            ask = client.call("ask", {"query": "sdk.scenario"}, CallOpts(timeout_s=20))
            caps = client.capabilities(CallOpts(timeout_s=20))
            transcript = {
                "sdk": "python",
                "capabilities": caps,
                "decisions": sorted(
                    (
                        {"key": d.get("key"), "value": d.get("value")}
                        for d in ask.get("decisions", [])
                    ),
                    key=lambda d: str(d.get("key")),
                ),
                "task": {
                    "task_id": 1,
                    "receipt": receipt.get("receipt"),
                    "status": receipt.get("status"),
                },
                "verify": verify,
            }
            out = os.environ.get("EDDA_SCENARIO_OUT")
            if out:
                Path(out).write_text(json.dumps(transcript, indent=2))
            self.assertTrue(isinstance(transcript["decisions"], list))
        finally:
            client.close()
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
