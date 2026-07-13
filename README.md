<h1 align="center">Edda</h1>

<p align="center">
  <strong>Your agent's decisions shouldn't reset every session.</strong><br/>
  Edda gives coding agents a local, automatic memory of what was decided — and why.<br/>
  Works with Claude Code, Cursor, Codex, OpenClaw, and any MCP client.
</p>

<p align="center">
  <a href="https://crates.io/crates/edda"><img src="https://img.shields.io/crates/v/edda?style=flat-square" alt="crates.io" /></a>
  <a href="https://github.com/fagemx/edda/releases"><img src="https://img.shields.io/github/v/release/fagemx/edda?style=flat-square&label=release" alt="Release" /></a>
  <a href="https://github.com/fagemx/edda/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/fagemx/edda/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://github.com/fagemx/edda/blob/main/LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=flat-square" alt="License" /></a>
  <a href="https://github.com/fagemx/edda/stargazers"><img src="https://img.shields.io/github/stars/fagemx/edda?style=flat-square" alt="Stars" /></a>
</p>

<p align="center">
  <a href="#why-edda">Why Edda?</a> ·
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#how-it-works">How It Works</a> ·
  <a href="#how-edda-compares">Comparison</a> ·
  <a href="#integration">Integration</a> ·
  <a href="#architecture">Architecture</a>
</p>

<p align="center">
  English · <a href="./docs/README_zh-TW.md">繁體中文</a>
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/03180d1f-5943-4a62-808b-0b8d159a94db" width="700" alt="Edda overview" />
</p>

---

## Why Edda?

Yesterday you and your agent argued through the tradeoffs and settled on SQLite. Today's session opens — and it proposes Postgres. Again. The reasoning died with the transcript, and compaction can't bring it back.

Edda fixes exactly this: hooks watch your sessions, capture each decision with its rationale into a local ledger, and hand it to the next session before it starts. The agent stops forgetting.

```
Without edda                          With edda
────────────                          ─────────
Session 2 opens:                      Session 2 opens:
  "I suggest Postgres for this —        "Continuing with SQLite
   it gives us JSONB and…"               (decided yesterday: single
You: "We settled this. YESTERDAY."       writer, JSONB not needed)…"
```

**You don't need to do anything.** After `edda init`, hooks handle everything:

| When | What Edda does | You do |
|------|---------------|--------|
| Session starts | Digests previous session, injects prior decisions into context | Nothing |
| Agent makes decisions | Hooks detect and extract them from the transcript | Nothing |
| Session ends | Writes session digest to the ledger | Nothing |
| Next session starts | Agent sees relevant decisions from all prior sessions | Nothing |

**Data stays local** — the ledger lives in `.edda/` (SQLite + local files), with no cloud and no accounts. The core loop (record, retrieve, inject) is deterministic and never calls out. **Optional LLM assist** for session digests, decision extraction, and pattern correlation is opt-in via `EDDA_LLM_API_KEY` and budget-capped — leave the key unset and edda runs zero-egress.

## One memory, every agent

More and more developers alternate between agents — Claude Code for one task, Codex for a second opinion on the next. Both models are strong; what breaks is the memory. Each tool keeps its own silo, so every switch means re-explaining the project from zero.

Edda's ledger is tool-neutral and local. Bridges on each side read and write the same `.edda/`, so a decision made in one agent is simply there when the other starts:

```
Claude Code (morning)                Codex (afternoon)
  edda decide "auth=JWT"        →      session opens knowing auth=JWT
          └───────────── one local ledger (.edda/) ─────────────┘
```

The same wiring covers produce-and-verify workflows: one model writes, the other reviews, and both argue from the same decision history instead of two private ones.

<details>
<summary><strong>Do I need this if I only use Claude Code?</strong></summary>

Honest answer: **maybe not.** If you're one person, one tool, running one
session at a time on a light project, Claude Code's built-in memory is enough.

Edda starts paying for itself when any of these is true:

| Situation | What edda adds |
|---|---|
| Decisions need to survive with their *reasoning* | A structured ledger entry beats prose notes — rationale, date, and scope, injected automatically next session |
| More than one session runs at once | Peers/claims coordination: sessions see who is working where and stop trampling each other |
| More than one tool (Claude Code + Codex, …) | One local ledger both sides read and write |
| You switch models *inside* Claude Code (router tools) | Orthogonal, not competing: edda sits at the hook layer and keeps recording whichever model is driving — and the new model is exactly the one that needs the old model's decisions |
| Sessions run in containers | Each container is an island; the shared state you'd mount *is* `.edda/` |

</details>

## Install

```bash
# One-line install (Linux / macOS)
curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh

# macOS / Linux (Homebrew)
brew install fagemx/tap/edda

# crates.io
cargo install edda

# Or download a prebuilt binary
# → https://github.com/fagemx/edda/releases
```

## Quick Start

```bash
edda init    # auto-detects Claude Code, installs hooks
# Done. Start coding. Edda works in the background.
```

`edda init` does three things:

1. Creates `.edda/` with an empty ledger
2. Installs lifecycle hooks into `.claude/settings.local.json`
3. Adds decision-tracking instructions to `.claude/CLAUDE.md`

The CLAUDE.md section teaches your agent when and how to record decisions:

```markdown
## Decision Tracking (edda)

When you make an architectural decision, record it:
  edda decide "domain.aspect=value" --reason "why"

Before ending a session, summarize what you did:
  edda note "completed X; decided Y; next: Z" --tag session
```

This is the key to Edda's automation — the agent learns to call `edda decide` naturally during conversation, and hooks capture everything else.

## How It Works

```
Claude Code session
        │
   Bridge hooks (deterministic, always on)
        │  ├── record decisions / notes / peer signals
        │  ├── inject prior context on session start
        │  └── optional: doctrine pack from havamal
        ▼
   ┌─────────┐
   │  .edda/  │  ← append-only SQLite ledger
   │  ledger  │  ← hash-chained events
   └─────────┘
        │
   Session end
        │  ├── deterministic digest (always)
        │  └── LLM digest + pattern detection (opt-in, budget-capped)
        ▼
   Next session sees everything
```

Edda stores every event as a hash-chained JSON record in a local SQLite database. Events include decisions, notes, session digests, and command outputs. The hash chain makes the history tamper-evident and the retrieval deterministic — same query, same answer, no LLM in the loop.

At the start of each session, edda assembles a context snapshot from the ledger and injects it — the agent sees recent decisions, active tasks, peer coordination, and (if configured) a doctrine pack from [havamal](https://github.com/fagemx/havamal), without reading through old transcripts.

**Where LLM shows up (opt-in only):** long-transcript decision extraction, richer session-end digests, and cross-session pattern correlation live in `bg_extract` / `bg_digest` / `bg_detect`. All three are gated on `EDDA_LLM_API_KEY` plus a daily budget; without the key, edda falls back to deterministic heuristics.

## How Edda Compares

|  | MEMORY.md | RAG / Vector DB | LLM Summary | **Edda** |
|--|-----------|----------------|-------------|----------|
| **Storage** | Markdown file | Vector embeddings | LLM-generated text | Append-only SQLite |
| **Retrieval** | Agent reads whole file | Semantic similarity | LLM re-summarizes | Tantivy full-text + structured query |
| **Needs LLM?** | No | Yes (embeddings) | Yes (every read/write) | **No for core; opt-in for digests** ¹ |
| **Needs vector DB?** | No | Yes | No | **No** |
| **Tamper-evident?** | No | No | No | **Yes** (hash chain) |
| **Tracks "why"?** | Sometimes | No | Lossy | **Yes** (rationale + rejected alternatives) |
| **Cross-session?** | Manual copy | Yes | Session-scoped | **Yes** (automatic) |
| **Cross-agent?** | No — one tool's file | Per-app integration | No — vendor silo | **Yes** (Claude Code, Codex, OpenClaw, MCP) |
| **Cost per query** | Free | Embedding API call | LLM API call | **Free** (local SQLite); optional digests budget-capped |

| **Examples** | Claude Code built-in, OpenClaw | mem0, Zep, Chroma | ChatGPT Memory, Copilot | — |

Every ledger query runs locally against SQLite — same answer every time, in milliseconds, at zero cost.

¹ *LLM assist is off by default. Set `EDDA_LLM_API_KEY` to enable session-end digests, decision extraction from long transcripts, and cross-session pattern correlation; each caller has a daily budget cap. The core loop — recording decisions, hash chaining, retrieval, hook-based injection — never calls an LLM.*

## Integration

**Claude Code** — fully supported via bridge hooks. Auto-captures decisions, digests sessions, injects context.

```bash
edda init    # detects Claude Code, installs hooks automatically
```

**Cursor** — supported via native Cursor hooks. Session start pushes the existing hot pack, doctrine, and workspace context into the Agent model.

```bash
edda bridge cursor install      # installs ~/.cursor/hooks.json entries
edda doctor cursor              # verifies PATH, hooks, and writable store
```

Cursor v1 uses the same read path as the Codex bridge. Cursor can send `transcript_path: null` at `sessionStart`, so the bridge reads the existing hot pack and does not claim to rebuild the Cursor transcript at that point.

**Codex** — supported via native hooks with the same shared Edda context machinery.

```bash
edda bridge codex install
```

**OpenClaw** — supported via bridge plugin.

```bash
edda bridge openclaw install    # installs global plugin
```

**Havamal** (judgment layer) — drop a `.havamal-pack.md` in your repo and edda auto-injects it as the doctrine section at session start. See [havamal](https://github.com/fagemx/havamal) — facts flow through edda, judgment enters curated.

<details>
<summary><strong>Do I need havamal too?</strong></summary>

Short answer: **no — edda is useful on its own**. They compose when both are present, but neither depends on the other.

| Your pain | Use |
|---|---|
| "Decisions I made last session vanish when a new session starts." | **edda only** |
| "The agent doesn't know what my project values, refuses, or has already tried." | **havamal only** (write doctrine, reference it from `CLAUDE.md` / `AGENTS.md`) |
| Both of the above, especially on a long project with many sessions | **both** — edda auto-injects the havamal pack, so you skip the manual "read the doctrine first" step |

Havamal works standalone with any harness (Claude Code, Codex, Cursor, Gemini CLI) because its contract is a plain markdown file. Edda works standalone because decisions and injection don't need doctrine to function.
</details>

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

16 Rust crates:

| Crate | What it does |
|-------|-------------|
| `edda-core` | Event model, hash chain, schema, provenance |
| `edda-ledger` | Append-only ledger (SQLite), blob store, locking |
| `edda-cli` | All commands + TUI (`tui` feature, default on) |
| `edda-bridge-claude` | Claude Code hooks, transcript ingest, context injection |
| `edda-bridge-cursor` | Cursor native hooks, context injection, lifecycle tracking |
| `edda-bridge-codex` | Codex hooks and context injection |
| `edda-bridge-openclaw` | OpenClaw hooks and plugin |
| `edda-mcp` | MCP server (7 tools) |
| `edda-ask` | Cross-source decision query engine |
| `edda-derive` | View rebuilding, tiered history |
| `edda-pack` | Context generation, budget controls |
| `edda-transcript` | Transcript delta ingest, classification |
| `edda-store` | Per-user store, atomic writes |
| `edda-search-fts` | Full-text search (Tantivy) |
| `edda-index` | Transcript index |
| `edda-conductor` | Multi-phase plan orchestration — self-domain phase pipelines only; mission dispatch belongs to [bryti](https://github.com/fagemx/bryti), and conductor never touches external work queues |

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

Every event follows a hash-chained JSON schema (stored in the local SQLite ledger):

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

Shipped:

- [x] Distribution — prebuilt binaries (macOS, Linux, Windows), one-line installer, Homebrew tap
- [x] v0.2.0 — `edda watch` TUI, `edda ask`, peers/coordination commands, sub-agent visibility, model/token/cost capture in session hooks, user-level store (`~/.edda/`), post-mortem learned rules
- [x] Decision deepening — `--paths`-scoped decisions, PreToolUse guard warnings, session-start decision pack, decision status lifecycle

Next:

- [ ] Cross-repo decision query surface — the user-level store already aggregates across projects; a first-class search/ask across repos is the remaining gap
- [ ] Decision recall metrics — measure how often injected decisions actually change behavior

## Contributing

Contributions welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

## Community

- [GitHub Issues](https://github.com/fagemx/edda/issues) — bugs & feature requests
- [Releases](https://github.com/fagemx/edda/releases) — changelog & binaries

## License

MIT OR Apache-2.0

---

*Stop re-teaching your agent what you already decided.*
