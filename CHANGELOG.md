# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **edda watch** — real-time TUI with peers, events, and decisions panels; now built into `edda` binary behind `tui` feature flag (default on), with plain-text fallback when disabled (#34, #44)
- **edda ask** — cross-source decision aggregator combining ledger, coordination, and transcript data (#54)
- **edda init --no-hooks** — auto-detect `.claude/` and install bridge hooks; `--no-hooks` to skip (#50)
- **MCP server** expanded from 3 to 7 tools: `edda_status`, `edda_note`, `edda_decide`, `edda_query`, `edda_log`, `edda_context`, `edda_draft_inbox` (#37)
- **SQLite ledger** — migrate from append-only JSONL to SQLite with hash-chain integrity (#27)
- **Decisions table** with auto-extraction from notes and supersede tracking (#28)
- **Tantivy full-text search** replacing FTS5, with fuzzy and regex support (#36)
- **OpenClaw bridge** — full 7-event hook support matching Claude bridge (#16, #19)
- **Multi-agent coordination** — auto-claim scope from edited files, decision conflict detection, `edda claim` / `edda request` commands (#24, #121)
- **Late peer detection** — inject coordination protocol when new peers join mid-session (#11)
- **Context budget** — reserved tail slots for critical protocol sections (#9)
- **CLI commands** — `edda bridge claude render-*`, `edda bridge claude heartbeat-*` exposed as subcommands (#20)
- `--json` flag for `edda draft list`, `edda draft inbox`, `edda conduct status`
- TUI: focus files, current task in peers panel; type-aware event display with color coding
- Auto-init `.edda/` when `edda watch` runs in uninitialized workspace (#45)

### Changed

- Ledger storage switched from JSONL files to SQLite (breaking: old `.edda/ledger/events.jsonl` no longer used)
- Search engine switched from SQLite FTS5 to Tantivy
- OpenClaw integration consolidated into `integrations/openclaw/`
- License changed to MIT OR Apache-2.0 dual license

### Fixed

- Session identity resolution via heartbeat inference (#145)
- L2 bindings visible in solo mode (#147)
- Git worktrees resolve to common root for consistent `project_id` (#21)
- Claims sorted by label for stable display order
- Ledger auto-creates schema on open to prevent missing table errors
- `edda init` repairs partial workspace (missing schema/HEAD)
- Event dedup and `project_id` indexing in search

### Removed

- **edda-tui** standalone crate — TUI consolidated into `edda-cli` behind `tui` feature flag (#44)
- JSONL dual-mode code, `refs/` directory, and `edda migrate` command (#40)
- TypeScript orchestrator prototype (replaced by Rust `edda-conductor`)

## [0.1.0] - 2026-02-21

Initial release.

- 15 Rust crates: core, ledger, cli, tui, bridge-claude, bridge-openclaw, mcp, ask, derive, pack, transcript, store, search-fts, index, conductor
- Append-only hash-chained event ledger
- Claude Code bridge with 7 hook events
- `edda decide`, `edda note`, `edda ask`, `edda context`, `edda log`, `edda search`
- Draft proposal workflow (`edda draft propose/approve/reject`)
- Branch operations (`edda branch`, `edda switch`, `edda merge`)
- Multi-phase plan orchestration (`edda conduct`)
