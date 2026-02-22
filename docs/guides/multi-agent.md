---
title: Multi-Agent Coordination
---

# Multi-Agent Coordination

When multiple Claude Code agents work on the same repo simultaneously, they have no awareness of each other. Edda adds a lightweight coordination layer that runs entirely through local files — no server, no configuration beyond `edda init`.

## The problem

Two agents on the same repo will:

1. **Edit the same file** — one overwrites the other
2. **Do redundant work** — both solve the same problem independently
3. **Make contradictory decisions** — one chooses Postgres, the other adds SQLite migrations

These failures are silent. You only discover them after the damage is done.

## How it works

Edda adds three coordination primitives, all running through Claude Code hooks:

### 1. Peer discovery

Every session writes a heartbeat file with its current activity — what files it's editing, what tasks it's working on, and what git branch it's on.

When a new prompt is submitted, Edda checks for other active heartbeats. If peers are found, it injects their status into the agent's context:

```
## Peers (1 active)
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs
```

The agent now knows it's not alone.

### 2. Scope claims

Agents can claim ownership of file paths:

```bash
edda claim "auth" --paths "src/auth/*"
```

Once claimed, other agents see those paths as off-limits:

```
### Off-limits (other agents active)
- src/auth/* -> Agent auth (30s ago)
```

Claims are advisory — the coordination protocol tells agents not to edit off-limits files, and in practice they follow the instruction.

### 3. Binding decisions

When one agent makes a decision that affects everyone:

```bash
edda decide "db.engine=postgres" --reason "JSONB support needed"
```

All other active sessions see this immediately:

```
### Binding Decisions
- db.engine: postgres (backend)
```

If another agent tries to decide differently on the same key, Edda detects the conflict and warns.

## What the agent sees

At session start, if peers are detected, Edda injects the full coordination protocol:

```markdown
## Coordination Protocol
You are one of 2 agents working simultaneously.
Claim your scope: `edda claim "label" --paths "src/scope/*"`
Message a peer: `edda request "peer-label" "your message"`

### Peers Working On
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs

### Off-limits (other agents active)
- src/billing/* -> Agent billing (30s ago)

### Binding Decisions
- db.engine: postgres (backend)
```

On subsequent prompts, Edda injects lightweight peer updates so the agent stays aware without repeating the full protocol.

## Peer messaging

Agents can send requests to each other:

```bash
edda request "billing" "Please expose the invoice total as a public method"
```

The target agent sees the request at its next prompt.

## Monitoring

```bash
edda watch
```

Opens a terminal UI showing all active sessions, their activity, and coordination state in real time:

```
┌─ Sessions ──────────────────────┐┌─ Events ────────────────────────┐
│ auth (2s ago) [main]            ││ cmd:fail $ cargo test [exit:2]  │
│   editing: src/auth/jwt.rs      ││ note    session checkpoint      │
│   task: Add JWT middleware      ││ decide  db.engine=postgres      │
│                                 ││                                 │
│ billing (5s ago) [feat/billing] ││                                 │
│   editing: src/billing/api.rs   ││                                 │
│   task: Implement invoice API   ││                                 │
└─────────────────────────────────┘└─────────────────────────────────┘
```

## Storage

The coordination layer runs entirely on local files:

- **Heartbeats**: JSON files in the per-user store, one per session
- **Claims and bindings**: append-only `coordination.jsonl` log
- **Peer discovery**: filesystem scan at hook time

No central server, no network communication. Two Claude Code sessions on the same repo coordinate automatically.

## Typical workflow

```bash
# Terminal 1
claude   # "Refactor the auth module"

# Terminal 2
claude   # "Add the billing API"
```

Each agent will:
1. See the other's activity at each prompt
2. Respect claimed scopes
3. Share binding decisions
4. Send requests when coordination is needed

## Limitations

- **Advisory enforcement** — off-limits paths are communicated but not hard-blocked
- **Same machine only** — peer discovery uses local filesystem
- **Bash bypass** — scope claims apply to Edit/Write tools; `sed` and `mv` in Bash are not checked
- **Stale heartbeats** — heartbeats older than 120 seconds are considered inactive
