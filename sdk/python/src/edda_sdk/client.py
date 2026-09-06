"""EddaClient: the contracted operation surface
(docs/reference/client-contract.md §2). Thin by design — transport + types
only. No state rules.
"""

from __future__ import annotations

import threading

from .errors import CapabilityNotAvailable
from .transport_http import HttpTransport
from .transport_mcp import CallOptions, McpSpawnSpec, McpTransport

# Contracted operations in order; capability probe decides availability at
# runtime (contract §5) — tools that do not exist on today's server
# (task/claim/receipt/verify) are still modeled here.
OPERATIONS = (
    "ask",
    "note",
    "decide",
    "task.new",
    "task.start",
    "task.done",
    "claim",
    "receipt",
    "verify",
    "status",
    "log",
    "context",
)

_OP_TOOLS = {
    "ask": "edda_ask",
    "note": "edda_note",
    "decide": "edda_decide",
    "task.new": "edda_task_new",
    "task.start": "edda_task_start",
    "task.done": "edda_task_done",
    "claim": "edda_claim",
    "receipt": "edda_receipt",
    "verify": "edda_verify",
    "status": "edda_status",
    "log": "edda_log",
    "context": "edda_context",
}


def _op_args(op: str, inp: dict) -> dict:
    if op == "ask":
        out = {"query": inp.get("query")}
        if "domain" in inp:
            out["domain"] = inp["domain"]
        return out
    if op == "note":
        out = {"text": inp.get("note")}
        for k in ("tags", "role"):
            if k in inp:
                out[k] = inp[k]
        return out
    if op == "decide":
        out = {"decision": f"{inp.get('key')}={inp.get('value')}"}
        if "reason" in inp:
            out["reason"] = inp["reason"]
        return out
    if op in ("status", "context"):
        return {}
    if op == "log":
        out = {}
        for k in ("event_type", "keyword", "after", "before", "limit"):
            if k in inp:
                out[k] = inp[k]
        return out
    return dict(inp)


class EddaClient:
    def __init__(self, mcp: McpSpawnSpec | None = None, http: str | None = None) -> None:
        if mcp is None and http is None:
            raise ValueError("EddaClient requires an MCP spawn spec and/or an HTTP base URL")
        self._mcp = McpTransport(mcp) if mcp else None
        self._http = HttpTransport(http) if http else None

    def capabilities(self, opts: CallOptions | None = None) -> dict[str, bool]:
        out = {op: False for op in OPERATIONS}
        if self._mcp is not None:
            names = {t.get("name") for t in self._mcp.list_tools(opts)}
            for op in OPERATIONS:
                out[op] = _OP_TOOLS[op] in names
        return out

    def call(self, op: str, inp: dict | None = None, opts: CallOptions | None = None) -> object:
        if self._mcp is None:
            raise CapabilityNotAvailable(op, "client (no MCP transport configured)")
        inp = inp or {}
        tool = _OP_TOOLS[op]
        names = {t.get("name") for t in self._mcp.list_tools(opts)}
        if tool not in names:
            raise CapabilityNotAvailable(op, "server tool list")
        return self._mcp.call_tool(tool, _op_args(op, inp), opts)

    @property
    def http_transport(self) -> HttpTransport:
        if self._http is None:
            raise ValueError("no HTTP transport configured")
        return self._http

    def close(self) -> None:
        if self._mcp is not None:
            self._mcp.close()


__all__ = [
    "EddaClient",
    "HttpTransport",
    "McpSpawnSpec",
    "McpTransport",
    "CallOptions",
    "OPERATIONS",
]
