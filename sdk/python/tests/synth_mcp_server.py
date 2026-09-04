#!/usr/bin/env python3
"""Deterministic synthetic MCP server for deadline/cancellation/error tests.

Speaks newline-delimited JSON-RPC 2.0 over stdio (rmcp stdio framing):
answers `initialize` immediately, then delays every other request by
--delay seconds (default: never answers) before replying with a generic
result. --exit-immediately exits at startup instead — the error-path probe.

This is a TEST fixture: it deterministically controls timing so deadline and
cancellation assertions cannot race a fast real server. Real edda
round-trip/equivalence coverage stays in the live contract tests.
"""

import argparse
import json
import sys
import time


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--delay", type=float, default=None)
    ap.add_argument("--exit-immediately", action="store_true")
    args = ap.parse_args()
    if args.exit_immediately:
        sys.exit(3)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(msg, dict):
            continue
        if msg.get("method") == "initialize":
            reply = {
                "jsonrpc": "2.0",
                "id": msg.get("id"),
                "result": {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}, "serverInfo": {"name": "synth", "version": "0"}},
            }
            sys.stdout.write(json.dumps(reply) + "\n")
            sys.stdout.flush()
            continue
        if "id" not in msg or msg.get("id") is None:
            continue  # notification
        if args.delay is not None:
            time.sleep(args.delay)
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg.get("id"), "result": {"content": [{"type": "text", "text": "synth"}]}}) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
