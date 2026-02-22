# Demo Project

A pre-populated edda workspace that demonstrates decisions, supersede chains, evidence-linked commits, and cross-source queries.

## Setup

```bash
# Build edda first (if not already installed)
cargo build --bin edda

# Run the setup script
cd examples/demo-project
bash setup.sh
```

The script creates a `.edda/` workspace with 4 simulated sessions:

| Session | What happens |
|---------|-------------|
| 1 | Project kickoff: choose Rust, SQLite, REST |
| 2 | Auth decisions: JWT, 15min expiry, httpOnly refresh |
| 3 | Caching deferred: "premature optimization" |
| 4 | Cache upgrade: None → Redis (supersede chain) |

## Try It

```bash
# Overview: all active decisions
edda ask

# Domain browse: all auth decisions
edda ask auth

# Supersede chain: cache.strategy None → Redis
edda ask cache

# Exact key lookup with timeline
edda ask db.engine

# Keyword search across decisions + commits + notes
edda ask "Redis"

# JSON output (what an LLM agent sees via MCP)
edda ask --json

# Full event log
edda log

# Only decisions
edda log --type decision

# Context snapshot (injected into agent sessions)
edda context

# Real-time TUI
edda watch
```

## What's Inside `.edda/`

After running `setup.sh`:

```
.edda/
├── ledger.db        # SQLite: all events, decisions table, refs
├── ledger/
│   └── blobs/       # large payloads (empty in this demo)
├── branches/        # branch metadata
├── drafts/          # pending proposals (empty)
├── patterns/        # classification patterns
├── actors.yaml      # roles
├── policy.yaml      # approval rules
└── config.json      # workspace config
```

The `ledger.db` contains:
- **8 decisions** (7 active + 1 superseded)
- **5 commit events** with auto-collected evidence links
- **8 notes** (session markers + context notes)
- Hash-chained event integrity
