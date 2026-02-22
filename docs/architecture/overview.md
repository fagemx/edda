---
title: Architecture
---

# Architecture

Edda is a Rust workspace with 14 crates, organized in three layers: core storage, bridge integrations, and user-facing interfaces.

## Crate map

```
User-facing          Bridge                    Core
─────────────        ──────────────────        ──────────────
edda-cli             edda-bridge-claude        edda-core
edda-mcp             edda-bridge-openclaw      edda-ledger
                                               edda-store

Query & context      Processing
──────────────       ──────────────
edda-ask             edda-transcript
edda-pack            edda-derive
edda-search-fts      edda-index
                     edda-conductor
```

| Crate | Layer | What it does |
|-------|-------|-------------|
| `edda-core` | Core | Event model, hash chain, schema, provenance |
| `edda-ledger` | Core | Append-only ledger (SQLite), blob store, locking |
| `edda-store` | Core | Per-user store, atomic writes |
| `edda-bridge-claude` | Bridge | Claude Code hooks, transcript ingest, context injection, peer coordination |
| `edda-bridge-openclaw` | Bridge | OpenClaw hooks and plugin |
| `edda-transcript` | Processing | Transcript delta ingest, classification |
| `edda-derive` | Processing | View rebuilding, tiered history |
| `edda-index` | Processing | Transcript index |
| `edda-conductor` | Processing | Multi-phase plan orchestration |
| `edda-ask` | Query | Cross-source decision query engine |
| `edda-pack` | Query | Context generation, budget controls |
| `edda-search-fts` | Query | Full-text search (Tantivy) |
| `edda-cli` | Interface | All commands + TUI (`tui` feature) |
| `edda-mcp` | Interface | MCP server (7 tools, stdio JSON-RPC 2.0) |

## Data flow

```
Claude Code session
        │
   Hook events (stdin JSON)
        │
        ▼
   edda-bridge-claude
   ├── dispatch.rs        → route by hook event type
   ├── ingest.rs          → write to session ledger
   ├── peers.rs           → heartbeat, peer discovery, coordination
   ├── pack_builder.rs    → build context snapshot
   └── digest.rs          → transcript analysis at session end
        │
        ▼
   edda-ledger (SQLite)
   ├── events table       → hash-chained event log
   ├── head table         → branch HEAD pointers
   └── blobs/             → large payloads
        │
        ▼
   edda-pack → context injection at next session start
```

## Workspace layout

```
.edda/
├── ledger.db             # SQLite: events, HEAD, branches (append-only, hash-chained)
├── ledger/
│   └── blobs/            # large payloads
├── branches/             # branch metadata
├── drafts/               # pending proposals
├── patterns/             # classification patterns
├── actors.yaml           # roles (lead, reviewer)
├── policy.yaml           # approval rules
└── config.json           # workspace config
```

## Event schema

Every event is a hash-chained JSON record in the SQLite ledger:

```json
{
  "event_id": "evt_01khj03c1bteqm3ffrv57adtmt",
  "ts": "2026-02-16T01:12:38.187Z",
  "type": "note",
  "branch": "main",
  "parent_hash": "217456ef...",
  "hash": "2dfe06e7...",
  "payload": {
    "role": "user",
    "tags": [],
    "text": "Phase 0 complete: edda in PATH, hooks installed"
  },
  "refs": {}
}
```

Each event's `hash` is computed from its content plus the `parent_hash`, forming a chain. This makes the history tamper-evident — changing any event invalidates all subsequent hashes.

## Coordination layer

Multi-agent coordination uses a separate storage path for real-time performance:

```
Per-user store (~/.config/edda/ or %APPDATA%/edda/)
├── projects/{project_hash}/
│   ├── heartbeats/{session_id}.json    # per-session heartbeat
│   └── coordination.jsonl              # claims, bindings, requests
```

Heartbeats are written at every prompt. The coordination log is append-only. Both are scanned at hook time by `peers.rs` to build the board state injected into agent context.
