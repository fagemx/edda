# Edda

**Decision memory for coding agents.**

<p>
  <a href="https://github.com/fagemx/edda/releases"><img src="https://img.shields.io/github/v/release/fagemx/edda?style=flat-square&label=release" alt="Release" /></a>
  <a href="https://github.com/fagemx/edda/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/fagemx/edda/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://github.com/fagemx/edda/blob/main/LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=flat-square" alt="License" /></a>
  <a href="https://github.com/fagemx/edda/stargazers"><img src="https://img.shields.io/github/stars/fagemx/edda?style=flat-square" alt="Stars" /></a>
</p>

[What is Edda?](#what-is-edda) · [Install](#install) · [How It Works](#how-it-works) · [Comparison](#how-edda-compares) · [Integration](#integration) · [Architecture](#architecture)

---

## What is Edda?

Coding agents (Claude Code, Cursor, Windsurf) lose all context when a session ends. The next session starts from scratch — no memory of what was decided, what was tried, or why.

Edda is an **automatic** decision memory that runs in the background. It captures decisions from your agent's conversations, stores them in a local append-only ledger, and injects relevant context when the next session starts.

**You don't need to do anything.** After `edda init`, hooks handle everything:

| When | What Edda does | You do |
|------|---------------|--------|
| Session starts | Digests previous session, injects prior decisions into context | Nothing |
| Agent makes decisions | Hooks detect and extract them from the transcript | Nothing |
| Session ends | Writes session digest to the ledger | Nothing |
| Next session starts | Agent sees relevant decisions from all prior sessions | Nothing |

```
Session 1                          Session 2
  Agent decides "db=SQLite"          Agent starts
  Agent decides "cache=Redis"   →    Edda injects context automatically
  Session ends                       Agent sees: db=SQLite, cache=Redis
  Edda digests transcript            Agent continues where it left off
```

**Everything local** — plain files in `.edda/`, no cloud, no accounts, no API calls.

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
edda init    # auto-detects Claude Code / OpenClaw, installs hooks
# Done. Start coding. Edda works in the background.
```

## How It Works

```
Claude Code session
        │
   Bridge hooks (automatic)
        │
        ▼
   ┌─────────┐
   │  .edda/  │  ← append-only SQLite ledger
   │  ledger  │  ← hash-chained events
   └─────────┘
        │
   Context injection (next session)
        │
        ▼
   Agent sees all prior decisions
```

Edda stores every event as a hash-chained JSON record in a local SQLite database. Events include decisions, notes, session digests, and command outputs. The hash chain makes the history tamper-evident.

At the start of each session, Edda assembles a context snapshot from the ledger and injects it — the agent sees recent decisions, active tasks, and relevant history without reading through old transcripts.

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

## Integration

**Claude Code** — fully supported via bridge hooks. Auto-captures decisions, digests sessions, injects context.

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

## Manual Tools

Most of the time, hooks handle everything automatically. These commands are available when you want to record something manually or look something up:

```bash
edda ask "cache"           # query past decisions
edda search query "auth"   # full-text search across transcripts
edda context               # see what the agent sees at session start
edda log --type decision   # filter the event log
edda watch                 # real-time TUI: peers, events, decisions
```

<details>
<summary>All commands</summary>

| Command | Description |
|---------|-------------|
| `edda init` | Initialize `.edda/` (auto-installs hooks if `.claude/` detected) |
| `edda decide` | Record a binding decision |
| `edda note` | Record a note |
| `edda ask` | Query decisions, history, and conversations |
| `edda search` | Full-text search across transcripts (Tantivy) |
| `edda log` | Query events with filters (type, date, tag, branch) |
| `edda context` | Output context snapshot (what the agent sees) |
| `edda status` | Show workspace status |
| `edda watch` | Real-time TUI: peers, events, decisions |
| `edda commit` | Create a commit event |
| `edda branch` | Branch operations |
| `edda switch` | Switch branch |
| `edda merge` | Merge branches |
| `edda draft` | Propose / list / approve / reject drafts |
| `edda bridge` | Install/uninstall tool hooks |
| `edda doctor` | Health check |
| `edda config` | Read/write workspace config |
| `edda pattern` | Manage classification patterns |
| `edda mcp` | Start MCP server (stdio JSON-RPC 2.0) |
| `edda conduct` | Multi-phase plan orchestration |
| `edda plan` | Plan scaffolding and templates |
| `edda run` | Run a command and record output |
| `edda blob` | Manage blob metadata |
| `edda gc` | Garbage collect expired content |

</details>

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

<details>
<summary>What's inside .edda/</summary>

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

</details>

## Roadmap

- [x] Prebuilt binaries (macOS, Linux, Windows)
- [x] One-line install script (`curl | sh`)
- [x] Homebrew tap (`brew install fagemx/tap/edda`)
- [ ] Decision recall metrics
- [ ] Cross-project decision search
- [ ] tmux-based multi-pane TUI (L3)

## Contributing

Contributions welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

## Community

- [GitHub Issues](https://github.com/fagemx/edda/issues) — bugs & feature requests
- [Releases](https://github.com/fagemx/edda/releases) — changelog & binaries

## License

MIT OR Apache-2.0

---

*Your agent's architecture decisions shouldn't reset every session.*
