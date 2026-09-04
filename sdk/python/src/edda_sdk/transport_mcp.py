"""MCP transport: JSON-RPC 2.0 over stdio against ``edda mcp serve``
(newline-delimited JSON, rmcp stdio framing).

Safety: the child process is spawned from a FIXED argv list supplied by the
caller (binary path + fixed args). This module never interpolates user input
into a shell command and never uses a shell.
"""

from __future__ import annotations

import itertools
import json
import os
import subprocess
import threading
from dataclasses import dataclass, field

from .errors import (
    CancelledError_,
    ProtocolError,
    RpcError,
    TimeoutError_,
    TransportError,
)


@dataclass
class McpSpawnSpec:
    """Fixed spawn spec — never a shell string."""

    command: str
    args: list[str]
    cwd: str | None = None
    env: dict[str, str] = field(default_factory=dict)


@dataclass
class CallOptions:
    """Deadline/cancellation for one operation (contract §6)."""

    timeout_s: float | None = None
    cancel: threading.Event | None = None


class _Pending:
    __slots__ = ("event", "result", "error")

    def __init__(self) -> None:
        self.event = threading.Event()
        self.result: object = None
        self.error: Exception | None = None


class McpTransport:
    def __init__(self, spec: McpSpawnSpec) -> None:
        self._spec = spec
        self._proc: subprocess.Popen[str] | None = None
        self._ids = itertools.count(1)
        self._pending: dict[int, _Pending] = {}
        self._lock = threading.Lock()
        self._tools_cache: list[dict] | None = None
        self._stderr_tail: list[str] = []
        self._reader: threading.Thread | None = None

    # ── child lifecycle ──

    def _ensure_proc(self) -> subprocess.Popen[str]:
        if self._proc is not None and self._proc.poll() is None:
            return self._proc
        env = dict(os.environ)
        env.update(self._spec.env)
        proc = subprocess.Popen(
            [self._spec.command, *self._spec.args],  # fixed argv — never a shell
            cwd=self._spec.cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self._proc = proc
        self._stderr_tail = []
        self._reader = threading.Thread(target=self._read_loop, args=(proc,), daemon=True)
        self._reader.start()
        return proc

    def _read_loop(self, proc: subprocess.Popen[str]) -> None:
        assert proc.stdout is not None
        assert proc.stderr is not None
        stderr_thread = threading.Thread(target=self._drain_stderr, args=(proc,), daemon=True)
        stderr_thread.start()
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue  # non-JSON lines are ignored
            if not isinstance(msg, dict):
                continue
            msg_id = msg.get("id")
            if msg_id is None:
                continue  # notification
            pending = self._pending.pop(msg_id, None)
            if pending is None:
                continue
            if "error" in msg and msg["error"] is not None:
                err = msg["error"]
                pending.error = RpcError(err.get("code", -32603), str(err.get("message", "rpc error")), err.get("data"))
            else:
                pending.result = msg.get("result")
            pending.event.set()
        # process exited
        with self._lock:
            pendings = list(self._pending.values())
            self._pending.clear()
        tail = " ".join(self._stderr_tail)[-2000:].strip()
        for p in pendings:
            p.error = TransportError(f"edda mcp exited: {tail}" if tail else "edda mcp exited")
            p.event.set()

    def _drain_stderr(self, proc: subprocess.Popen[str]) -> None:
        assert proc.stderr is not None
        for line in proc.stderr:
            self._stderr_tail.append(line)
            del self._stderr_tail[:-100]

    # ── json-rpc ──

    def _request(self, method: str, params: object, opts: CallOptions) -> object:
        proc = self._ensure_proc()
        if opts.cancel is not None and opts.cancel.is_set():
            raise CancelledError_()
        msg_id = next(self._ids)
        pending = _Pending()
        self._pending[msg_id] = pending
        req = {"jsonrpc": "2.0", "id": msg_id, "method": method, "params": params}
        assert proc.stdin is not None
        try:
            proc.stdin.write(json.dumps(req) + "\n")
            proc.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            self._pending.pop(msg_id, None)
            raise TransportError("write to edda mcp stdin failed", exc) from exc

        waited = 0.0
        step = 0.01
        limit = opts.timeout_s
        cancel = opts.cancel
        while not pending.event.wait(step):
            waited += step
            if cancel is not None and cancel.is_set():
                self._pending.pop(msg_id, None)
                raise CancelledError_()
            if limit is not None and waited >= limit:
                self._pending.pop(msg_id, None)
                raise TimeoutError_(f"{method} exceeded {limit}s")
        if pending.error is not None:
            raise pending.error
        return pending.result

    def initialize(self, opts: CallOptions | None = None) -> None:
        opts = opts or CallOptions()
        self._request(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "edda-sdk-python", "version": "0.1.0"},
            },
            opts,
        )
        # notifications/initialized is a JSON-RPC NOTIFICATION — no id, no
        # response; sending it as a request aborts the rmcp handshake.
        proc = self._ensure_proc()
        assert proc.stdin is not None
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}) + "\n")
        proc.stdin.flush()

    def list_tools(self, opts: CallOptions | None = None) -> list[dict]:
        opts = opts or CallOptions()
        if self._tools_cache is None:
            self.initialize(opts)
            result = self._request("tools/list", {}, opts)
            self._tools_cache = list((result or {}).get("tools", []))  # type: ignore[union-attr]
        return self._tools_cache

    def call_tool(self, name: str, args: dict, opts: CallOptions | None = None) -> object:
        opts = opts or CallOptions()
        self.initialize(opts)
        result = self._request("tools/call", {"name": name, "arguments": args}, opts)
        if not isinstance(result, dict):  # pragma: no cover - defensive
            raise ProtocolError("unexpected tools/call result shape")
        if result.get("isError"):
            text = "tool error"
            for c in result.get("content", []):
                if c.get("type") == "text":
                    text = str(c.get("text", text))
                    break
            raise RpcError(-32000, text)
        text_content = None
        for c in result.get("content", []):
            if c.get("type") == "text":
                text_content = c.get("text")
                break
        if text_content is None:
            return result
        try:
            return json.loads(text_content)
        except json.JSONDecodeError:
            return text_content

    def close(self) -> None:
        proc = self._proc
        self._proc = None
        if proc is not None and proc.poll() is None:
            try:
                if proc.stdin is not None:
                    proc.stdin.close()
            except OSError:
                pass
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()

