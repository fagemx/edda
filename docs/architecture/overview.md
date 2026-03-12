---
title: Architecture
---

# Architecture

Edda is a Rust workspace organized in four layers. Each layer may depend on layers
below it, never above.

## Layer model

| Layer | Purpose | Crates |
|-------|---------|--------|
| **L1 Foundation** | Event model, schema, hashing | `edda-core` |
| **L2 Persistence** | Storage, ledger, blobs | `edda-ledger`, `edda-store` |
| **L3 Processing** | View building, indexing, orchestration, analysis | `edda-derive`, `edda-transcript`, `edda-index`, `edda-conductor`, `edda-aggregate`, `edda-chronicle`, `edda-postmortem` |
| **L3 Bridge** | External system integration | `edda-bridge-claude`, `edda-bridge-openclaw` |
| **L3 Query** | Cross-source queries, context generation | `edda-ask`, `edda-pack`, `edda-search-fts` |
| **L4 Interface** | User-facing CLI, MCP, HTTP, notifications | `edda-cli`, `edda-mcp`, `edda-serve`, `edda-notify` |

### Dependency rules

```
L4 Interface  ──▶  L3 Processing / Bridge / Query  ──▶  L2 Persistence  ──▶  L1 Foundation
```

- **L1** has no workspace dependencies (only `std` and external crates).
- **L2** may depend on L1.
- **L3** may depend on L1 and L2.
- **L4** may depend on any layer.

Same-layer dependencies are allowed (e.g. an L3 crate may depend on another L3 crate).

### Dependency matrix

| Depends on →  | L1 | L2 | L3 | L4 |
|---------------|----|----|----|----|
| **L1**        | —  | no | no | no |
| **L2**        | yes | — | no | no |
| **L3**        | yes | yes | yes | no |
| **L4**        | yes | yes | yes | yes |

> **Note**: `edda-derive` is classified as L3 Processing. Its dependency on
> `edda-ledger` (L2) is valid under the layer rules — derive builds derived
> views *from* the ledger, making it a consumer of persistence, not a
> foundation crate.

## Crate map

```
L4 Interface         L3 Bridge                 L2 Persistence     L1 Foundation
─────────────        ──────────────────        ──────────────     ──────────────
edda-cli             edda-bridge-claude        edda-ledger        edda-core
edda-mcp             edda-bridge-openclaw      edda-store
edda-serve
edda-notify

L3 Query             L3 Processing
──────────────       ──────────────
edda-ask             edda-transcript
edda-pack            edda-derive
edda-search-fts      edda-index
                     edda-conductor
                     edda-aggregate
                     edda-chronicle
                     edda-postmortem
```

| Crate | Layer | What it does |
|-------|-------|-------------|
| `edda-core` | L1 Foundation | Event model, hash chain, schema, provenance |
| `edda-ledger` | L2 Persistence | Append-only ledger (SQLite), blob store, locking |
| `edda-store` | L2 Persistence | Per-user store, atomic writes |
| `edda-bridge-claude` | L3 Bridge | Claude Code hooks, transcript ingest, context injection, peer coordination |
| `edda-bridge-openclaw` | L3 Bridge | OpenClaw hooks and plugin |
| `edda-transcript` | L3 Processing | Transcript delta ingest, classification |
| `edda-derive` | L3 Processing | View rebuilding, tiered history |
| `edda-index` | L3 Processing | Transcript index |
| `edda-conductor` | L3 Processing | Multi-phase plan orchestration |
| `edda-aggregate` | L3 Processing | Cross-session aggregation |
| `edda-chronicle` | L3 Processing | Timeline and chronicle generation |
| `edda-postmortem` | L3 Processing | Post-session analysis and rule lifecycle |
| `edda-ask` | L3 Query | Cross-source decision query engine |
| `edda-pack` | L3 Query | Context generation, budget controls |
| `edda-search-fts` | L3 Query | Full-text search (Tantivy) |
| `edda-cli` | L4 Interface | All commands + TUI (`tui` feature) |
| `edda-mcp` | L4 Interface | MCP server (7 tools, stdio JSON-RPC 2.0) |
| `edda-serve` | L4 Interface | HTTP API server |
| `edda-notify` | L4 Interface | Notification dispatch |

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
