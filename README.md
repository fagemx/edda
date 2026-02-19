# Edda

**Decision memory for coding agents.**

<!-- TODO: replace with asciinema or GIF recording -->
```
$ edda decide "db=SQLite" --reason "single-user, no deployment overhead, FTS5 for search"
Recorded: db=SQLite

$ edda query "database"
[2026-02-15 session-7a] db=SQLite
  reason: single-user, no deployment overhead, FTS5 for search

# Next session — your agent sees this automatically
$ edda context
## Recent Decisions
- db=SQLite (reason: single-user, no deployment overhead, FTS5 for search)
## Persistent Tasks
- Implement caching layer (from session-5c)
```

Your coding agent forgets every decision when the session ends. Edda makes them stick.

## Install

```bash
# From source
cargo install edda

# Or download a prebuilt binary
# → https://github.com/fagemx/edda/releases
```

## Quick Start

```bash
# Initialize in your project
edda init

# Install Claude Code hooks (auto-captures decisions)
edda bridge claude install

# Done. Start a Claude Code session.
# Edda captures decisions automatically.
```

## How Edda Compares

|  | MEMORY.md | RAG / Vector DB | LLM Summary | **Edda** |
|--|-----------|----------------|-------------|----------|
| **Storage** | Markdown file | Vector embeddings | LLM-generated text | Append-only JSONL |
| **Retrieval** | Agent reads whole file | Semantic similarity | LLM re-summarizes | FTS5 keyword + structured query |
| **Needs LLM?** | No | Yes (embeddings) | Yes (every read/write) | **No** |
| **Needs vector DB?** | No | Yes | No | **No** |
| **Tamper-evident?** | No | No | No | **Yes** (hash chain) |
| **Tracks "why"?** | Sometimes | No | Lossy | **Yes** (rationale + rejected alternatives) |
| **Cross-session?** | Manual copy | Yes | Session-scoped | **Yes** (automatic) |
| **Cost per query** | Free | Embedding API call | LLM API call | **Free** (local SQLite) |
| **Examples** | Claude Code built-in, OpenClaw | mem0, Zep, Chroma | ChatGPT Memory, Copilot | — |

Edda is **deterministic, local, and free to query**. No API calls, no embeddings, no LLM in the loop.

## What You Can Do

### Record decisions

```bash
$ edda decide "cache=Redis" --reason "need TTL, pub/sub for invalidation"
Recorded: cache=Redis

$ edda note "Explored DynamoDB, too expensive for our scale"
Recorded note.
```

### Query past decisions

```bash
$ edda query "cache"
[2026-02-18 session-3f] cache=Redis
  reason: need TTL, pub/sub for invalidation

[2026-02-17 session-2a] cache=None
  reason: premature optimization, revisit after benchmarks
  (superseded)
```

### Search across session transcripts

```bash
$ edda search query "authentication"
[session-7a turn 23] "JWT with short-lived tokens, refresh via httpOnly cookie..."
[session-5c turn 8]  "Considered Passport.js but went with custom middleware..."
```

### View what your agent sees at session start

```bash
$ edda context
# CONTEXT SNAPSHOT

## Project (main)
- branch: main
- events: 47

## Recent Decisions (last 10)
- db=SQLite: single-user, no deployment overhead
- cache=Redis: need TTL, pub/sub for invalidation
- auth=JWT: short-lived tokens + refresh cookie

## Persistent Tasks
- Implement caching layer
- Add rate limiting to API endpoints
```

### Filter the event log

```bash
$ edda log --type decision --after 2026-02-15
[evt_01kh...] 2026-02-18 decision: cache=Redis
[evt_01kh...] 2026-02-17 decision: auth=JWT
[evt_01kh...] 2026-02-15 decision: db=SQLite

$ edda log --family governance --json
# outputs JSONL for scripting
```

### Draft & approve high-stakes changes

```bash
$ edda draft propose --message "Migrate from REST to GraphQL"
Draft created: draft_01kh...

$ edda draft list --pending
[draft_01kh...] Migrate from REST to GraphQL (pending)

$ edda draft approve draft_01kh...
Approved.
```

## How It Works

After `edda init` and `edda bridge claude install`, **you don't need to do anything**. Edda runs automatically in the background:

| When | What Edda does | You do |
|------|---------------|--------|
| Session starts | Digests previous session, injects prior decisions into context | Nothing |
| Agent makes decisions | Hooks detect and extract them from the transcript | Nothing |
| Session ends | Writes session digest to the ledger | Nothing |
| Next session starts | Agent sees relevant decisions from all prior sessions | Nothing |

The `decide`, `note`, and `query` commands exist for **manual use** — when you want to record something yourself or look something up. Day-to-day, the hooks handle everything.

```
Claude Code session
        │
   Bridge hooks (automatic)
        │
        ▼
   ┌─────────┐     edda query ←── manual lookup
   │  .edda/  │     edda context ←── auto-injected at session start
   │  ledger  │     edda search ←── full-text across transcripts
   └─────────┘
```

**Everything local** — plain files in `.edda/`, no cloud, no accounts.

## What's Inside `.edda/`

```
.edda/
├── ledger/
│   ├── events.jsonl      # append-only, hash-chained
│   └── blobs/            # large payloads
├── cache/                # derived views, digests
├── branches/             # branch metadata
├── drafts/               # pending proposals
├── refs/
│   ├── HEAD              # current branch
│   └── branches.json
├── actors.yaml           # roles (lead, reviewer)
└── policy.yaml           # approval rules
```

Every event is a single JSON line, hash-chained:

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

## All Commands

```
edda init          Initialize .edda/ in your project
edda decide        Record a binding decision
edda note          Record a note
edda query         Search decisions by keyword
edda search        Full-text search across transcripts (FTS5)
edda log           Query events with filters (type, date, tag, branch)
edda context       Output context snapshot (what the agent sees)
edda status        Show workspace status
edda commit        Create a commit event
edda branch        Branch operations
edda switch        Switch branch
edda merge         Merge branches
edda draft         Propose / list / approve / reject drafts
edda bridge        Install/uninstall tool hooks
edda doctor        Health check
edda config        Read/write workspace config
edda pattern       Manage classification patterns
edda mcp           Start MCP server (stdio JSON-RPC 2.0)
edda run           Run a command and record output
edda blob          Manage blob metadata
edda gc            Garbage collect expired content
```

## Integration

**Claude Code** — fully supported via bridge hooks. Auto-captures decisions, digests sessions, injects context.

```bash
edda bridge claude install    # one command, done
```

**Other tools** — MCP server available (experimental):

```bash
edda mcp serve    # stdio JSON-RPC 2.0, works with any MCP client
```

## Architecture

12 Rust crates:

| Crate | What it does |
|-------|-------------|
| `edda-core` | Event model, hash chain, schema, provenance |
| `edda-ledger` | Append-only ledger, blob store, locking |
| `edda-cli` | All commands |
| `edda-bridge-claude` | Claude Code hooks, transcript ingest, context injection |
| `edda-mcp` | MCP server |
| `edda-derive` | View rebuilding, tiered history |
| `edda-pack` | Context generation, budget controls |
| `edda-transcript` | Transcript delta ingest, classification |
| `edda-store` | Per-user store, atomic writes |
| `edda-search-fts` | FTS5 SQLite search |
| `edda-index` | Transcript index |
| `edda-conductor` | Multi-phase plan orchestration *(experimental)* |

## Status

539 tests · 0 clippy warnings · MIT license

## Roadmap

- [ ] Prebuilt binaries (macOS, Linux, Windows)
- [ ] Second bridge (Cursor / generic MCP client)
- [ ] npm wrapper (`npx edda init`)
- [ ] Decision recall metrics
- [ ] Multi-session coordination
- [ ] Cross-project decision search

## License

MIT

---

*Your agent's architecture decisions shouldn't reset every session.*
