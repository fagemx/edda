#!/usr/bin/env python3
"""Edda source-blind reference adapter (non-CLI agent runtime).

Implements the adapter-contract/0.1 obligations described in
docs/guides/writing-a-bridge.md over the normalized protocol
adapter-normalized-protocol/0.1, using the public `edda` CLI as its
data plane (note/log/context) -- it never invokes `edda hook` and
never imports any bridge implementation.

Design (fail-open adapter):
  * stdin: one normalized envelope
        {"event": "...", "payload": {...}, "conformance": {...}}
  * stdout: exactly one permissive JSON object (or `{"continue": true}`
    when input is unusable). It never exits non-zero and never blocks
    the host agent.
  * Durable lifecycle evidence is written as the exact public CLI notes
    required by the guide's source-blind reference profile, e.g.
        adapter-contract/0.1 session=<id> action=heartbeat.start
    so an independent observer can verify via `edda log`.
  * Small per-session state (injection dedupe hashes, nudge counters,
    digest watermark) lives under <workspace>/.edda-reference-adapter/.
    It is private adapter bookkeeping, not a store engine; the durable
    public record is the CLI ledger. The digest watermark survives
    session end so a repeated session.end cannot produce a second
    digest (idempotent digest per completed session).
  * Redaction: tool payloads are NEVER persisted; lifecycle notes are
    fixed-format and contain only the session id and action word.

Python 3.8+ stdlib only.
"""

import hashlib
import json
import os
import subprocess
import sys

CONTRACT_VERSION = "adapter-contract/0.1"
BOUNDARY_START = "<!-- edda:start -->"
BOUNDARY_END = "<!-- edda:end -->"
STATE_DIRNAME = ".edda-reference-adapter"
DEFAULT_MAX_CONTEXT_CHARS = 20000

WRITEBACK_TAIL = (
    "Write-back protocol: when you reach a durable decision, record it with "
    "`edda decide <KEY>=<VALUE>` and record completed work with `edda note` "
    "so it persists in the workspace ledger."
)

PROMPT_REMINDER = (
    "edda reminder: durable outcomes belong in the ledger; "
    "record decisions with `edda decide <KEY>=<VALUE>`."
)

NUDGE_TEXT = (
    "edda nudge: this looked like a commit. If it settled a decision, "
    "record it with `edda decide <KEY>=<VALUE>`."
)

# Fixed-format lifecycle actions (public reference profile notes).
ACTION_HEARTBEAT_START = "heartbeat.start"
ACTION_HEARTBEAT_END = "heartbeat.end"
ACTION_ACTIVITY_APPEND = "activity.append"
ACTION_DIGEST_COMPLETE = "digest.complete"


# ── helpers ──────────────────────────────────────────────────────────────────


def permissive(obj=None):
    """Emit exactly one permissive JSON object; never fail the host."""
    if obj is None:
        obj = {"continue": True}
    try:
        sys.stdout.write(json.dumps(obj))
        sys.stdout.flush()
    except Exception:  # noqa: BLE001
        pass
    return 0


def state_path(workspace, session_id):
    """Injectively map arbitrary session bytes to a private filename."""
    digest = hashlib.sha256(session_id.encode("utf-8")).hexdigest()
    return os.path.join(workspace, STATE_DIRNAME, digest + ".json")


def load_state(workspace, session_id):
    path = state_path(workspace, session_id)
    try:
        with open(path, "r", encoding="utf-8") as fh:
            state = json.load(fh)
        return state if isinstance(state, dict) else None
    except FileNotFoundError:
        return {}
    except (OSError, json.JSONDecodeError):
        # A failed state read must not be treated as an empty, successful
        # state: callers leave the event retryable rather than overwriting it.
        return None


def save_state(workspace, session_id, state):
    path = state_path(workspace, session_id)
    tmp = path + ".tmp"
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(state, fh, sort_keys=True)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
        return True
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        return False


def remove_state(workspace, session_id):
    try:
        os.unlink(state_path(workspace, session_id))
    except FileNotFoundError:
        return True
    except OSError:
        return False
    return True

def edda_run(edda_bin, args, workspace, store_root, timeout=60):
    env = dict(os.environ)
    if store_root:
        env["EDDA_STORE_ROOT"] = store_root
    return subprocess.run(
        [edda_bin] + args,
        capture_output=True,
        text=True,
        cwd=workspace,
        env=env,
        timeout=timeout,
    )


def note(edda_bin, workspace, store_root, session_id, action):
    """Write one public lifecycle note and report durable success."""
    text = "%s session=%s action=%s" % (CONTRACT_VERSION, session_id, action)
    try:
        proc = edda_run(edda_bin, ["note", text, "--role", "system"], workspace, store_root)
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def note_exists(edda_bin, workspace, store_root, session_id, action):
    try:
        proc = edda_run(edda_bin, ["log", "--type", "note", "--json", "--limit", "0"], workspace, store_root)
        return proc.returncode == 0 and ("session=%s action=%s" % (session_id, action)) in proc.stdout
    except (OSError, subprocess.SubprocessError):
        return False


def durable_note(edda_bin, workspace, store_root, session_id, action):
    """Retry failed CLI writes; do not advance local state on failure."""
    return note_exists(edda_bin, workspace, store_root, session_id, action) or note(edda_bin, workspace, store_root, session_id, action)

def context_body(edda_bin, workspace, store_root):
    """Bounded context snapshot from the public CLI data plane."""
    try:
        proc = edda_run(
            edda_bin, ["context", "--depth", "5"], workspace, store_root
        )
        if proc.returncode == 0 and proc.stdout.strip():
            return proc.stdout.strip()
    except Exception:  # noqa: BLE001
        pass
    return "(no edda context available)"


def budget_chars():
    """EDDA_MAX_CONTEXT_CHARS / EDDA_WORKSPACE_BUDGET_CHARS, both optional."""
    limit = DEFAULT_MAX_CONTEXT_CHARS
    for var in ("EDDA_MAX_CONTEXT_CHARS", "EDDA_WORKSPACE_BUDGET_CHARS"):
        try:
            limit = min(limit, int(os.environ.get(var, str(limit))))
        except (TypeError, ValueError):
            pass
    return max(1, limit)


def pack_context(edda_bin, workspace, store_root):
    """Boundary-wrapped context; truncates the body, keeps the write-back tail."""
    tail = WRITEBACK_TAIL
    limit = budget_chars()
    body = context_body(edda_bin, workspace, store_root)
    fixed = len(BOUNDARY_START) + len(BOUNDARY_END) + 4
    body_budget = limit - fixed - len(tail)
    if len(body) > body_budget:
        body = "[edda: context body elided]"[:max(body_budget, 0)]
    pack = "\n".join([BOUNDARY_START, body, tail, BOUNDARY_END])
    return pack


# ── event handlers ───────────────────────────────────────────────────────────


def on_session_start(env):
    edda, ws, store, sid = env
    state = load_state(ws, sid)
    if state is None:
        return {}
    if not state.get("heartbeat_started"):
        if not durable_note(edda, ws, store, sid, ACTION_HEARTBEAT_START):
            return {}
        state["heartbeat_started"] = True
        if not save_state(ws, sid, state):
            return {}  # later delivery verifies CLI and retries state write
    return {"context": pack_context(edda, ws, store)}

def on_prompt_submit(env, payload):
    edda, ws, store, sid = env
    state = load_state(ws, sid)
    if state is None:
        return {}
    hashes = state.setdefault("injected_hashes", [])
    digest = hashlib.sha256(PROMPT_REMINDER.encode("utf-8")).hexdigest()
    if digest in hashes:
        return {}  # identical consecutive injection deduped (SHOULD)
    hashes.append(digest)
    state["injected_hashes"] = hashes[-32:]
    save_state(ws, sid, state)
    return {"context": "\n".join([BOUNDARY_START, PROMPT_REMINDER, BOUNDARY_END])}


def on_tool_pre(_env, _payload):
    # Rules are advisory: stay allow, never rewrite host input.
    return {"hookSpecificOutput": {"permissionDecision": "allow"}}


def on_tool_post(env, payload):
    edda, ws, store, sid = env
    # Durable activity before consumption; payloads are never persisted
    # (redaction happens before any durable write).
    if not durable_note(edda, ws, store, sid, ACTION_ACTIVITY_APPEND):
        return {}
    # Rate-limited decision-signal nudge (SHOULD).
    command = ""
    try:
        command = str((payload or {}).get("tool_input", {}).get("command", ""))
    except Exception:  # noqa: BLE001
        command = ""
    if "commit" in command:
        state = load_state(ws, sid)
        if state is None:
            return {}
        nudges = state.get("nudges", 0)
        if nudges < 2:
            state["nudges"] = nudges + 1
            if save_state(ws, sid, state):
                return {"context": NUDGE_TEXT}
            return {}
    return {}


def on_session_end(env):
    edda, ws, store, sid = env
    # Public CLI evidence is the durable idempotency source.  Private state is
    # only a retry cache and is removed after every completed session.
    if not durable_note(edda, ws, store, sid, ACTION_HEARTBEAT_END):
        return {}
    if not durable_note(edda, ws, store, sid, ACTION_DIGEST_COMPLETE):
        return {}
    if not remove_state(ws, sid):
        return {}
    return {}

def on_compact_pre(_env, _payload):
    # Vendor schemas forbid injection here; hot-pack rebuild is a no-op.
    return {}


def handle(envelope):
    event = envelope.get("event")
    payload = envelope.get("payload") or {}
    conf = envelope.get("conformance") or {}
    sid = conf.get("session_id") or envelope.get("session_id") or "unknown"
    workspace = conf.get("workspace") or os.getcwd()
    store_root = conf.get("store_root") or os.environ.get("EDDA_STORE_ROOT")
    edda_bin = conf.get("edda_bin") or os.environ.get("EDDA_BIN") or "edda"
    env = (edda_bin, workspace, store_root, sid)

    if event == "session.start":
        return on_session_start(env)
    if event == "prompt.submit":
        return on_prompt_submit(env, payload)
    if event == "tool.pre":
        return on_tool_pre(env, payload)
    if event in ("tool.post", "tool.post.signal", "tool.post.secret"):
        return on_tool_post(env, payload)
    if event == "compact.pre":
        return on_compact_pre(env, payload)
    if event == "session.end":
        return on_session_end(env)
    # Unknown / future events: permissive no-op (forward compatibility).
    return {}


def main():
    try:
        raw = sys.stdin.read()
    except Exception:  # noqa: BLE001
        return permissive()
    try:
        envelope = json.loads(raw) if raw.strip() else {}
        if not isinstance(envelope, dict):
            return permissive()
        return permissive(handle(envelope))
    except Exception:  # noqa: BLE001
        # Fail-open: malformed input must never block the host agent.
        return permissive()


if __name__ == "__main__":
    sys.exit(main())
