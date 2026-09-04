"""Capture actual Claude hook stdout; run with Python 3 and an edda binary."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--edda", default="edda")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = Path(shutil.which(args.edda) or args.edda).resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    runtime = Path(tempfile.mkdtemp(prefix="edda-gh710-"))
    repo = runtime / "repo"
    repo.mkdir()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("EDDA_", "GIT_"))}
    env.update(EDDA_STORE_ROOT=str(runtime / "store"),
               EDDA_BRIDGE_AUTO_DIGEST="0", EDDA_PLANS_DIR=str(runtime / "plans"))
    records = []

    def call(name, command, payload=None):
        stdin = b"" if payload is None else json.dumps(payload).encode("utf-8")
        (output / (name + ".stdin.json")).write_bytes(stdin)
        started = time.time()
        result = subprocess.run(command, input=stdin, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, cwd=repo, env=env,
                                check=False, timeout=60)
        (output / (name + ".stdout")).write_bytes(result.stdout)
        (output / (name + ".stderr")).write_bytes(result.stderr)
        records.append({"name": name, "command": command, "started_unix": started,
                        "exit_code": result.returncode,
                        "stdout_bytes": len(result.stdout),
                        "stdout_sha256": hashlib.sha256(result.stdout).hexdigest()})
        if result.returncode:
            raise RuntimeError(f"{name} failed: {result.stderr!r}")
        return result.stdout

    def hook(name, event, session):
        return call(name, [str(binary), "hook", "claude"], {
            "hook_event_name": event, "session_id": session,
            "cwd": str(repo), "transcript_path": str(repo / (session + ".jsonl")),
            "permission_mode": "default",
        })

    try:
        call("00-git-init", ["git", "init", "--initial-branch=main"])
        call("01-edda-init", [str(binary), "init", "--no-hooks"])
        for session in ("gh710-a", "gh710-b"):
            (repo / (session + ".jsonl")).write_bytes(b"")
        hook("02-session-a", "SessionStart", "gh710-a")
        hook("03-session-b", "SessionStart", "gh710-b")
        hook("04-first-contact", "UserPromptSubmit", "gh710-a")
        baseline = hook("05-baseline", "UserPromptSubmit", "gh710-a")
        assert "## Peers (1 active)" in json.loads(baseline)["hookSpecificOutput"]["additionalContext"]
        time.sleep(2)
        assert hook("06-dedup", "UserPromptSubmit", "gh710-a") == b""
        time.sleep(2)
        assert hook("07-dedup", "UserPromptSubmit", "gh710-a") == b""
        call("08-change-peer", [str(binary), "bridge", "claude", "heartbeat-write",
                                "--label", "changed-peer", "--session", "gh710-b"])
        changed = hook("09-changed", "UserPromptSubmit", "gh710-a")
        assert "changed-peer" in json.loads(changed)["hookSpecificOutput"]["additionalContext"]
        assert changed != baseline
        call("10-peers", [str(binary), "bridge", "claude", "peers", "--json"])
        call("11-version", [str(binary), "--version"])
    finally:
        manifest = {"binary": str(binary),
                    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                    "runtime": str(runtime), "cwd": str(repo),
                    "environment_overrides": {k: v for k, v in env.items() if k.startswith("EDDA_")},
                    "records": records}
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"Captured {len(records)} calls in {output}; runtime retained at {runtime}")


if __name__ == "__main__":
    main()
