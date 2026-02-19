# Edda

**Decision memory for coding agents.**

Your coding agent mass of decisions every session — architecture choices, trade-offs, rejected alternatives. Then the session ends and it all vanishes. Next session, the agent re-litigates the same choices, asks the same questions, rediscovers the same constraints.

Edda is an append-only decision ledger that gives coding agents persistent memory across sessions and tools. Not chat history. Not RAG. A structured, queryable, auditable record of *why* — not just *what*.

```
$ edda query "database"

[2026-02-15 session-7a] db=SQLite
  reason: single-user, no deployment overhead, FTS5 for search
  (rejected: Postgres — overkill for V1, JSON files — no query capability)
```

## The Problem

Coding agents are stateless. Each session starts fresh.

- **Repeated decisions** — the agent re-evaluates choices it already made
- **Lost rationale** — "why not Postgres?" disappears with the session
- **No continuity** — every new session starts from zero context
- **No audit trail** — you can't review what the agent decided or why

## How Edda Fixes This

```
Session 1                          Session 2
┌──────────────┐                   ┌──────────────┐
│  Agent works  │                   │  Agent wakes  │
│  Makes 3      │  ──  digest ──▶  │  Sees prior   │
│  decisions    │      store       │  decisions in  │
│  Moves on     │                   │  context       │
└──────────────┘                   └──────────────┘
        │                                  ▲
        ▼                                  │
   ┌─────────┐                        inject
   │  Edda   │  ─────────────────────────┘
   │ Ledger  │
   └─────────┘
```

1. **Capture** — hooks extract decisions from agent sessions automatically
2. **Digest** — each session gets a compact summary written to the ledger
3. **Inject** — next session starts with relevant prior decisions in context
4. **Query** — humans and agents search the ledger anytime

## Quick Start

```bash
# Install
cargo install edda

# Initialize in your project
cd your-project
edda init

# Install Claude Code hooks
edda bridge claude install

# Start a Claude Code session — Edda captures decisions automatically.
# When the next session starts, run:
edda context
# → see prior decisions injected into agent context
```

**60 seconds to value**: install → init → hook → one session → `edda context` shows decisions from that session. `edda query` finds them.

## Core Commands

```bash
# Record a decision (key=value format, with rationale)
edda decide "db=SQLite" --reason "single-user, no deployment overhead"

# Record a note
edda note "Explored Redis caching, decided against it for now"

# Query decisions (substring match, ranked by time + relevance)
edda query "database"

# Full-text search across session transcripts (FTS5)
edda search query "authentication flow"

# Structured event log with filters
edda log --type decision --after 2026-02-01

# View the context snapshot (what the agent sees at session start)
edda context

# Health check
edda doctor claude
```

**`query` vs `search`**: `query` searches the decision ledger (decisions, notes, drafts — the curated record). `search` does full-text search across raw session transcripts (the complete history). Different granularity, different use cases.

## What Gets Stored

### File Layout

```
.edda/
├── ledger/
│   ├── events.jsonl      # append-only event ledger (hash-chained)
│   └── blobs/            # large payloads (stdout, stderr, artifacts)
├── cache/                # derived views, hot.md, digests
├── branches/             # branch metadata
├── drafts/               # pending draft proposals
├── patterns/             # pattern definitions for classification
├── refs/
│   ├── HEAD              # current branch pointer
│   └── branches.json     # branch refs
├── actors.yaml           # role definitions (lead, reviewer, etc.)
└── policy.yaml           # approval policy gates
```

### Event Schema

Every event is a single JSON line in `events.jsonl`:

```json
{
  "event_id": "evt_01khj03c1bteqm3ffrv57adtmt",
  "ts": "2026-02-16T01:12:38.187Z",
  "type": "note",
  "branch": "main",
  "parent_hash": "217456ef18f6...c456717ed0cc",
  "hash": "2dfe06e73a2c...e5bda232",
  "payload": {
    "role": "user",
    "tags": [],
    "text": "Phase 0 complete: gctx in PATH, orchestrator installed"
  },
  "refs": {}
}
```

Key properties:
- **Hash-chained**: each event's `hash` covers its content + `parent_hash`, forming a tamper-evident chain
- **Typed**: `note`, `decision`, `cmd`, `commit`, `merge`, `draft`, `signal`
- **Branched**: events belong to a branch (like git)
- **Referenceable**: `refs` link to other events, blobs, or external artifacts

## How Capture Works

When Claude Code hooks are installed, Edda automatically:

1. **Ingests session transcripts** — extracts structured events from raw conversation
2. **Generates session digests** — compact decision summaries written to the ledger
3. **Injects context** — generates `hot.md` with relevant prior decisions for the next session
4. **Tracks persistent tasks** — cross-session TODOs that survive session boundaries

### On "Deterministic"

The write path (event creation, hashing, chaining, storage) is fully deterministic — no LLM involved. The extraction pipeline uses heuristics and pattern matching by default. When LLM-assisted extraction is used, it produces **draft candidates** that go through the same draft/approval flow as any other proposal. LLM can propose; it cannot write directly to the ledger.

### Security & Trust

- **Local-only**: everything stays in `.edda/`, nothing leaves your machine
- **Configurable capture level**: full transcript or decision-only
- **Pattern-based classification**: control what gets extracted and stored
- **No cloud, no accounts, no telemetry**

## Integration

Currently supports **Claude Code** via bridge hooks. MCP server available for other tools (experimental).

```
┌─────────────┐
│ Claude Code  │
└──────┬──────┘
       │
  Bridge Hooks (auto-capture)
       │
       ▼
┌─────────────┐
│    Edda     │
│   Ledger    │
│  (.edda/)   │
└─────────────┘
```

- **Bridge hooks** (Claude Code): auto-capture via session lifecycle hooks — the primary integration
- **MCP server** (stdio JSON-RPC 2.0): 3 tools + 2 resources, available for other MCP-compatible tools *(experimental)*
- **CLI**: direct access for humans and scripts

## What Edda Is Not

| Tool | Tracks | Edda tracks |
|------|--------|-------------|
| **Git** | what changed (code) | **why we decided** (rationale + rejected alternatives) |
| **Chat history** | what was said | **what was concluded** (structured, queryable) |
| **RAG / KB** | reference docs | **project-specific decisions** (temporal, hash-chained) |
| **Issue tracker** | what to do | **what was decided and why** (including "why not X") |

## Draft & Approval

For high-stakes decisions, Edda supports policy-gated governance:

```bash
# Propose a draft
edda draft propose --message "Migrate from REST to GraphQL"

# Review pending drafts
edda draft list --pending

# Approve or reject
edda draft approve <id>
edda draft reject <id>
```

Policies are defined in `.edda/policy.yaml`:

```yaml
rules:
  - id: require
    when:
      labels_any: ["risk", "security", "prod"]
    stages:
      - role: lead
        min_approvals: 1
```

## Architecture

Rust monorepo, 12 crates:

| Crate | Purpose |
|-------|---------|
| `edda-core` | Event model, hash chain, schema, refs, provenance |
| `edda-ledger` | Append-only ledger, blob store, workspace lock |
| `edda-cli` | 25+ commands |
| `edda-bridge-claude` | Claude Code hooks, transcript ingest, context injection |
| `edda-mcp` | MCP server (stdio JSON-RPC 2.0) |
| `edda-derive` | Deterministic view rebuild, tiered history rendering |
| `edda-pack` | Turn alignment, hot.md generation, budget controls |
| `edda-transcript` | Session transcript delta ingest, classification |
| `edda-store` | Per-user store with atomic writes |
| `edda-search-fts` | FTS5 SQLite full-text search |
| `edda-index` | Transcript index (uuid chain) |
| `edda-conductor` | Multi-phase plan orchestration *(experimental)* |

### Design Principles

- **Append-only** — events are never modified. GC may reclaim blob content; metadata and hashes persist.
- **Deterministic write path** — no LLM in the event pipeline. Extraction is rule-based; LLM can only draft.
- **Tool-agnostic** — bridge hooks for Claude Code, MCP for everything else.
- **Local-first** — `.edda/` directory, no cloud, no accounts.
- **Git-like mental model** — branches, commits, merges, refs.

## Status

- **151 commits** · **539 tests** · **0 clippy warnings**
- V1.0 (Decision Memory) — ✅ Done
- V1.1 (Storage Hygiene) — ✅ Done
- Active work: improving decision recall rates via passive capture

## Roadmap

- [ ] Second bridge (Cursor / generic MCP client)
- [ ] npm/pip distribution (beyond cargo)
- [ ] Decision recall metrics (`edda doctor` capture quality reports)
- [ ] Drift detection (schema/config change tracking)
- [ ] Cross-project decision search
- [ ] Multi-session coordination (parallel session awareness, claim/binding)
- [ ] Conductor (multi-phase AI plan orchestration)

## License

MIT

---

*Stop re-litigating architecture every session.*
