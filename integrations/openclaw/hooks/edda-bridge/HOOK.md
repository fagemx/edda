---
name: edda-bridge
description: "Bridge between OpenClaw and Edda — inject decision context + write-back protocol + coordination on bootstrap, capture decisions from messages, digest + checkpoint on session boundaries"
metadata:
  {
    "openclaw":
      {
        "emoji": "📜",
        "events":
          [
            "agent:bootstrap",
            "message:sent",
            "message:received",
            "command:new",
            "command:reset",
            "command:stop",
            "gateway:startup",
          ],
        "requires": { "anyBins": ["edda", "edda.exe"] },
      },
  }
---

# Edda Bridge Hook

Integrates [Edda](https://github.com/fagemx/edda) with OpenClaw's agent lifecycle.

## What It Does

### On `agent:bootstrap`

Injects a layered context snapshot into the session's bootstrap:
- **Body** (truncatable): `edda context` output — prior decisions and session history
- **Tail** (reserved, never truncated):
  - Write-back protocol — teaches the agent to use `edda decide` and `edda note`
  - Coordination section — peer discovery via `edda bridge claude peers` + L2 instructions

### On `message:sent` (agent response)

Scans outgoing agent messages for decision patterns (e.g., "decided to", "chose X over Y",
"going with", "rejected"). When detected, automatically records them to the Edda ledger
via `edda note`.

### On `message:received` (user message)

Stub — logs event for verification. Will implement per-turn context refresh once
event availability is confirmed.

### On `command:new` (session reset)

Runs `edda commit` to checkpoint the workspace state before the session resets.
Preserves a clean session boundary in the append-only ledger.

### On `command:reset` / `command:stop` (session boundary)

Runs `edda bridge claude digest --all` to summarize the session, then `edda commit`
to checkpoint. Ensures session context is preserved across boundaries.

### On `gateway:startup`

Logs session start to the edda ledger via `edda note --tag session`.

## Requirements

- `edda` or `edda.exe` must be on PATH
- The workspace must have been initialized with `edda init`

## Configuration

Optional configuration in OpenClaw config:

```json
{
  "hooks": {
    "internal": {
      "entries": {
        "edda-bridge": {
          "enabled": true,
          "contextBudget": 6000,
          "workdir": "C:\\path\\to\\project",
          "autoCapture": true
        }
      }
    }
  }
}
```

- **`contextBudget`**: Max characters for context injection (default: 8000)
- **`workdir`**: Override working directory for edda commands (default: agent workspace dir)
- **`autoCapture`**: Enable automatic decision capture from messages (default: true)
