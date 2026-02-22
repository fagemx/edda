# Edda

**Decision memory for coding agents.**

<!-- TODO: replace with asciinema or GIF recording -->
```
$ edda decide "db=SQLite" --reason "single-user, no deployment overhead, FTS5 for search"
Recorded: db=SQLite

$ edda ask "database"
── Decisions ──────────────────────────
  db = SQLite — single-user, no deployment overhead, FTS5 for search
  branch: main | 2026-02-15 | active

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
# One-line install (Linux / macOS)
curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh

# macOS / Linux (Homebrew)
brew install fagemx/tap/edda

# Or download a prebuilt binary
# → https://github.com/fagemx/edda/releases

# Or build from source
cargo install --git https://github.com/fagemx/edda edda-cli
```

## Quick Start

```bash
# Initialize in your project (auto-detects Claude Code / OpenClaw)
edda init

# Done. Start a Claude Code session.
# Edda captures decisions automatically.
```

If `.claude/` exists, `edda init` auto-installs hooks. Use `--no-hooks` to skip.

## How Edda Compares

|  | MEMORY.md | RAG / Vector DB | LLM Summary | **Edda** |
|--|-----------|----------------|-------------|----------|
| **Storage** | Markdown file | Vector embeddings | LLM-generated text | Append-only SQLite |
| **Retrieval** | Agent reads whole file | Semantic similarity | LLM re-summarizes | Tantivy full-text + structured query |
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
$ edda ask "cache"
── Decisions ──────────────────────────
  cache = Redis — need TTL, pub/sub for invalidation
  branch: main | 2026-02-18 | active

── Timeline ───────────────────────────
  2026-02-17  cache = None  (superseded)
  2026-02-18  cache = Redis  (active)
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

After `edda init`, **you don't need to do anything**. Edda runs automatically in the background:

| When | What Edda does | You do |
|------|---------------|--------|
| Session starts | Digests previous session, injects prior decisions into context | Nothing |
| Agent makes decisions | Hooks detect and extract them from the transcript | Nothing |
| Session ends | Writes session digest to the ledger | Nothing |
| Next session starts | Agent sees relevant decisions from all prior sessions | Nothing |

The `decide`, `note`, and `ask` commands exist for **manual use** — when you want to record something yourself or look something up. Day-to-day, the hooks handle everything.

```
Claude Code session
        │
   Bridge hooks (automatic)
        │
        ▼
   ┌─────────┐     edda ask ←── cross-source query
   │  .edda/  │     edda context ←── auto-injected at session start
   │  ledger  │     edda search ←── full-text across transcripts
   └─────────┘
```

**Everything local** — plain files in `.edda/`, no cloud, no accounts.

## What's Inside `.edda/`

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
edda init          Initialize .edda/ (auto-installs hooks if .claude/ detected)
edda decide        Record a binding decision
edda note          Record a note
edda ask           Query decisions, history, and conversations
edda search        Full-text search across transcripts (Tantivy)
edda log           Query events with filters (type, date, tag, branch)
edda context       Output context snapshot (what the agent sees)
edda status        Show workspace status
edda watch         Real-time TUI: peers, events, decisions
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
edda conduct       Multi-phase plan orchestration
edda plan          Plan scaffolding and templates
edda run           Run a command and record output
edda blob          Manage blob metadata
edda gc            Garbage collect expired content
```

## Integration

**Claude Code** — fully supported via bridge hooks. Auto-captures decisions, digests sessions, injects context. Hooks are auto-installed by `edda init` when `.claude/` is detected.

```bash
edda init    # detects Claude Code, installs hooks automatically
```

**OpenClaw** — supported via bridge plugin.

```bash
edda bridge openclaw install    # installs global plugin
```

**Any MCP client** (Cursor, Windsurf, etc.) — 7 tools via MCP server:

```bash
edda mcp serve    # stdio JSON-RPC 2.0
# Tools: edda_status, edda_note, edda_decide, edda_ask, edda_log, edda_context, edda_draft_inbox
```

## Architecture

14 Rust crates:

| Crate | What it does |
|-------|-------------|
| `edda-core` | Event model, hash chain, schema, provenance |
| `edda-ledger` | Append-only ledger (SQLite), blob store, locking |
| `edda-cli` | All commands + TUI (`tui` feature, default on) |
| `edda-bridge-claude` | Claude Code hooks, transcript ingest, context injection |
| `edda-bridge-openclaw` | OpenClaw hooks and plugin |
| `edda-mcp` | MCP server (7 tools) |
| `edda-ask` | Cross-source decision query engine |
| `edda-derive` | View rebuilding, tiered history |
| `edda-pack` | Context generation, budget controls |
| `edda-transcript` | Transcript delta ingest, classification |
| `edda-store` | Per-user store, atomic writes |
| `edda-search-fts` | Full-text search (Tantivy) |
| `edda-index` | Transcript index |
| `edda-conductor` | Multi-phase plan orchestration |

## Roadmap

- [x] Prebuilt binaries (macOS, Linux, Windows)
- [x] One-line install script (`curl | sh`)
- [ ] npm wrapper (`npx edda init`)
- [ ] Decision recall metrics
- [ ] Cross-project decision search
- [ ] tmux-based multi-pane TUI (L3)

## License

MIT OR Apache-2.0

---

*Your agent's architecture decisions shouldn't reset every session.*
