---
title: "Running Multiple Claude Code Agents Without Merge Conflicts"
date: "2026-02-23"
author: "fagemx"
category: "Technical"
summary: "How Edda coordinates multiple Claude Code agents working on the same repo — peer discovery, scope claims, and real-time conflict prevention."
---

# Running Multiple Claude Code Agents Without Merge Conflicts

Open two Claude Code sessions on the same repo. Tell one to refactor auth, the other to add billing. Watch them step on each other.

This is the default experience today. Claude Code has no built-in awareness of other sessions working on the same codebase. Each agent operates as if it is the only one.

Edda fixes this with a lightweight coordination layer that runs entirely through Claude Code hooks.

## The Problem

When two agents work on the same repo simultaneously, three things go wrong:

1. **File conflicts** — both agents edit the same file, one overwrites the other
2. **Redundant work** — both agents solve the same problem independently
3. **Contradictory decisions** — one chooses Postgres, the other adds SQLite migrations

These failures are silent. You only discover them after the damage is done.

## How Edda Coordinates Agents

Edda adds three coordination primitives, all running through existing Claude Code hooks with zero configuration beyond `edda init`:

### 1. Peer Discovery

Every session writes a heartbeat file with its current activity — what files it is editing, what tasks it is working on, and what git branch it is on.

When a session starts or a new prompt is submitted, Edda checks for other active heartbeats. If peers are found, it injects a coordination protocol into the agent's context:

```
## Peers (1 active)
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs
```

The agent now knows it is not alone.

### 2. Scope Claims

Agents can claim ownership of file paths:

```bash
edda claim "auth" --paths "src/auth/*"
```

Once claimed, other agents see those paths as off-limits:

```
### Off-limits (other agents active)
- src/auth/* → Agent auth (30s ago)
```

Claims are advisory today. The coordination protocol tells agents not to edit off-limits files, and in practice they follow the instruction. Hard enforcement via PreToolUse deny is planned for a future release.

### 3. Binding Decisions

When one agent makes an architectural decision that affects everyone, it can broadcast it:

```bash
edda decide "db.engine=postgres" --reason "JSONB support needed"
```

All other active sessions see this immediately:

```
### Binding Decisions
- db.engine: postgres (backend)
```

If another agent tries to decide differently on the same key, Edda warns about the conflict.

## What the Agent Actually Sees

At session start, if peers are detected, Edda injects a full coordination protocol:

```markdown
## Coordination Protocol
You are one of 2 agents working simultaneously.
Claim your scope: `edda claim "label" --paths "src/scope/*"`
Message a peer: `edda request "peer-label" "your message"`

### Peers Working On
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs

### Off-limits (other agents active)
- src/billing/* → Agent billing (30s ago)

### Binding Decisions
- db.engine: postgres (backend)
```

On subsequent prompts, Edda injects lightweight peer updates so the agent stays aware without repeating the full protocol.

## No Server, No Config

The coordination layer runs entirely on local files:

- **Heartbeats**: JSON files in the per-user store, one per session
- **Claims and bindings**: append-only `coordination.jsonl` log
- **Peer discovery**: filesystem scan at hook time

There is no central server, no network communication, and no additional configuration. Two Claude Code sessions on the same repo coordinate automatically after `edda init`.

## Real-Time Monitoring

`edda watch` opens a terminal UI that shows all active sessions, their current activity, and coordination state in real time:

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

## Limitations

This is coordination, not orchestration. Edda does not assign work, split tasks, or manage agent lifecycles. Each agent still operates independently — Edda just makes sure they know about each other.

Current limitations:

- **Advisory enforcement** — off-limits paths are communicated to agents but not hard-blocked (yet)
- **Same machine only** — peer discovery uses local filesystem; no cross-machine coordination
- **Bash bypass** — scope claims only apply to Edit/Write tools; `sed` and `mv` in Bash are not checked

## Try It

Open two terminals. Run `edda init` in a repo. Start two Claude Code sessions. Give them different tasks.

Each agent will see the other's activity, respect claimed scopes, and share binding decisions — automatically.

```bash
# Terminal 1
claude   # "Refactor the auth module"

# Terminal 2
claude   # "Add the billing API"
```

No merge conflicts. No duplicated work.

- GitHub: `https://github.com/fagemx/edda`
