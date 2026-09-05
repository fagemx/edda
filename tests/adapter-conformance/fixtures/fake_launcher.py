#!/usr/bin/env python3
"""Deterministic launcher/CLI fixture for adapter-contract conformance.

It models the three public launcher profiles without contacting a provider or
reading global configuration.  One JSON request on stdin produces one receipt.
"""
import argparse
import json
import sys


def receipt(agent, tools, mode):
    if mode == "crash":
        return {"outcome": "crash", "model_requested": "fixture-model",
                "model_observed": "unknown", "session_observed": "unknown",
                "tools_requested": tools, "tools_applied": [],
                "heartbeat_owner": "launcher"}
    if mode == "bad-receipt":
        return {"outcome": "done"}
    return {"outcome": "done", "model_requested": "fixture-model",
            "model_observed": "fixture-model-%s" % agent,
            "session_observed": "fixture-session-%s" % agent,
            "tools_requested": tools, "tools_applied": tools,
            "heartbeat_owner": "launcher"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent", choices=("claude", "pi", "codex"), required=True)
    ap.add_argument("--tools", default="")
    ap.add_argument("--mode", choices=("ok", "crash", "bad-receipt"), default="ok")
    args = ap.parse_args()
    tools = [item for item in args.tools.split(",") if item]
    sys.stdout.write(json.dumps(receipt(args.agent, tools, args.mode)) + "\n")


if __name__ == "__main__":
    main()
