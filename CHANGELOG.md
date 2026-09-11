# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Legacy merge-check compatibility converges on the product gate** — live `edda prs check-merge` forwards to `edda review merge`, while its retained host-agnostic scalar forms are explicitly deprecated advisory diagnostics and cannot merge (GH-1145)

## [0.6.1] - 2026-09-10

### Added

- **Merged-lane cleanup script** — `scripts/fleet/reclaim-merged.sh` reclaims merged lane worktrees and local/remote branches with dry-run-first receipts and fail-closed remote verification (GH-1009, #1091)
- **Explicit fleet machine identity** — sessions carry `label@machine` identity through the fleet label chain and warn when live labels collide on the machine-local board (GH-671, #1120)

### Changed

- **Merge governance follows the rule gate** — loaded policy carriers now describe the current-head LGTM, zero-blocker, CI, union, and base-window conditions instead of assigning merge to a named operator; shared checkouts remain on main and reviewers fetch private refs (GH-1064, #1101)
- **Fleet review rules are complete** — R18–R20 restore Independent Review semantics, review-dispatch pre-claim, and L1 ownership while repairing R21/R22 citations (GH-966, #1095)
- **Release operations are hardened** — Homebrew formulas build the immutable crates.io source package to avoid Linux glibc-runner coupling (#1090), and the release skill records the slower `0.6.x` patch cadence plus v0.6.0 operational lessons (#1106)
- **Review gating has one implementation** — drift detection and merge preconditions now share the Rust verdict reader instead of parallel shell logic (GH-1105, #1121)

### Fixed

- **Review merge safety** — orphan Review Responses now block merge, untrusted public comments cannot create or clear drift, and failed remote verification no longer looks like a clean result (GH-993, #1102)
- **`ratify --by-rule` guards** — supersession is no longer inferred from a key mention; same-key, explicit-marker, domain/date, and bounded-sweep guards preserve binding decisions (GH-1066, #1098)
- **Review diff and shell parsing** — content checks use three-dot PR ranges, and shell-parse evidence reads reviewed blobs without depending on working-tree file presence (GH-1003, GH-992, #1097)
- **doneWhen parsing** accepts the documented spacing variants across issue/spec matchers (GH-1056, #1089)
- **`edda review deliver` read path** has an argv-level regression guard and validates PR-vs-issue identity before reducing comments (GH-1079, #1086)
- **CI test reliability** — verify-contract assertions match Edda's authored success marker instead of random temporary-path text (GH-1033, #1110), and fake-Codex reconciliation tests use event synchronization instead of load-sensitive renewal timing (GH-1078, GH-1031, #1112)
- **Trusted review delivery** — `edda review deliver` refuses §7 verdicts whose authors GitHub does not identify as trusted collaborators (GH-1103, #1119)
- **Stable merge receipts** — `merge-reviewed-pr.sh` pins the squash subject to the PR title and carries the reviewed merge body (GH-1100, #1118)
- **Mirror round trips and recovery** — backticks round-trip without corrupting list boundaries, raw pre-#1017 mirrors retain their backslashes, ambiguous shapes fail closed, and one refused domain file is contained and reported instead of aborting the whole import (GH-1044, #1108)

## [0.6.0] - 2026-09-09

### Added

- **`edda review` in the product** — the review lifecycle moves from shell scripts into the CLI: `edda review` (#872), the union-gate window check (`edda review gate`, GH-769), the writing half with labels and the `Independent Review` status (`edda review deliver`, GH-1030), and the response trigger with the resume layer (`edda review due`, GH-763)
- **Typed ratification** — `--evidence pr#N@sha` and `--by-rule cited-authority` give decisions cited, machine-checkable authority (GH-764, GH-761)
- **`edda fleet order`** — a deterministic health-gated dispatch queue with per-file collision detection and in-product freshness (GH-1015)
- **`edda fleet health`** — product-share/mechanism-rate health gate with GREEN/YELLOW/RED exit codes (GH-1014)
- **ACP dispatch in the CLI** — edda-conductor dispatches through ACP with offline fake evidence for tests (GH-800)
- **Thin TypeScript and Python SDKs** over MCP/HTTP with a shared task-action surface and a complete MCP client contract (GH-611)
- **Cross-machine decision import** — `edda-ledger` imports committed-mirror decisions across machines (GH-671)
- **Skill-less controller loop** — `next-issue.sh`, `next-review.sh`, and the pi controller runbook (GH-899)
- **L0 rule runner** runs every marked REVIEW.md check block as one pass (#915)
- **Brief tooling** — `brief-from-issue` renders the invariant brief skeleton from an issue body (GH-885) and a dry-run validator applies AUTHORED STEPS in a throwaway worktree at the pinned SHA before any lane launch (#945)
- **Issue freshness gate** — `next-issue.sh` re-derives issue applicability at dispatch time (#938)
- **Canary calibration runner** scripts the README recipe (GH-881); shadow-review compare script (GH-887)
- **SessionStart renders the rail task** this session holds (GH-793)
- **Pre-push guard** ships in the git-native installer: reads the remote ref and refuses tag moves (#976)
- **Operator daily digest gains a clock** (GH-765)

### Changed

- SDK packages are made publishable (GH-611, #990)
- Fleet rules R22–R27 codify engine authority, verdict shape, readiness, policy carrier, origin-first work, and transport (#921); the judgement tag is left to R2 with brief v2 and calibration v1 (GH-884)
- Direct review is the default path; the watcher lane is the flash tier and the independence source (#1051)

### Fixed

- The dispatch claim guard gets its own window and one shared rule, bounds bare-CLI claims in time, and reconciles attempts by lease while preparing worktrees outside the workspace lock (GH-1018, GH-1047)
- `merge-reviewed-pr.sh` asks the union gate, not the latest review (GH-1057); the review gate window sees renames with `--no-renames` on both diffs (GH-1062)
- The Claude bridge's SessionStart writeback teaches the rule-based binding path instead of operator-only authority (GH-1063)
- OpenClaw bridge redacts durable bridge event data (#876)
- Verdict publication posts the report only; the watcher rejects transcript dumps, skips non-open PRs, and writes `Independent Review` only when the verdict engine is authoritative for the PR surface (R22, #947, #961)
- `mergeStateStatus` is no longer treated as a readiness signal — a stacked PR with zero verdicts no longer reports CLEAN (GH-946)
- The SDK contract is pinned to `SPEC_PIN.json` and red on spec drift (#973); pre-push guard and capability canaries hardened (#900, #929)
- Review lane terminal lifecycle finished (#867); the release dry-run watches the whole `crates/` tree (#900)

### Docs

- The implementable adapter contract is published (#873); rules and runbooks updated (R22–R27, pi-controller-runbook)

## [0.5.0] - 2026-09-04

### Added

- **Release automation publishes the workspace before the GitHub Release** — pushing a `v*` tag makes CI derive the dependency-first publish order, upload every publishable crate, verify immutable registry provenance against the tag SHA, and only then create and populate the GitHub Release behind a crates.io parity gate (GH-648)
- **Operator daily digest** — `edda recap --digest` prints the deterministic operator digest for the last window, `edda notify send` pushes free text as a digest event, and `scripts/fleet/daily-digest.sh` assembles the fleet-wide daily report (GH-765)
- **`edda verify`** — top-level verb wiring `Ledger::verify_chain` so users can audit their own hash chain (GH-647); `edda status --json` with a golden fixture (GH-730); build identity included in `edda --version` (GH-746)
- **Cross-machine claim guard** — `scripts/fleet-claim-issue.sh` plus `edda dispatch --issue/--machine` coordinate fleet claim handoff across machines (GH-656), with role-aware issue claims before dispatch (GH-782)
- **Merge-precondition gate** — merges are gated on recorded preconditions (GH-580)
- **Ledger event v1 specification** — the ledger's event surface is specified and fixture-backed (GH-608)
- **Conductor phase gates matured** — gate-wait progress signals with a visible deadline (GH-551), honest gate-timeout state with waiver semantics and an `on_gate_timeout` policy (GH-552), phase elapsed time accounting (GH-644), infrastructure contract plans (#856), and phase terminal-state notifications through edda-notify (GH-564)
- **Review system** — REVIEW.md as the one executable review spec (#633), a wiring verdict slot with `scripts/wiring-scan.sh` (#629), the PR review watcher that launches the read-only reviewer and posts SHA-pinned verdicts plus the `Independent Review` status (#641), and one resumable reviewer conversation per PR (GH-708)
- **Dispatch capabilities** — `edda dispatch` passes through launcher capabilities: model, thinking, tool policy, session dir, and model listing (GH-574); pi session transcripts are ingested post-dispatch (GH-577)
- **Fleet lanes** — a tracked lane launcher `scripts/fleet/lane-launch.ps1` / `lane-status.ps1` (GH-606) plus persistent lane helpers (#868); process object claims and merge gate protection (GH-581)
- **CI hardening** — path-aware CI that skips clippy/test for docs-only changes behind an aggregate `CI Gate` job (#643), merge_group support (#635), and an MSRV 1.91.0 check (GH-824)
- **Machine-detectable CLI docs coverage** — `scripts/check-cli-docs.sh` verifies every verb and long flag in `docs/reference/cli.md` against the built binary (GH-650, GH-795)

### Changed

- The toolchain is pinned to 1.98.1 and `rust-version` is declared 1.91 — CI no longer floats ahead of workstations (GH-814)
- `[profile.dev]` uses line-tables-only debuginfo and rust-lld, halving the debug build footprint and cutting Windows link time 12–21% (GH-810)
- `dirs::home_dir` is banned workspace-wide; home resolution goes through `edda_core::paths` (GH-812)
- Opening a ledger refuses newer `schema_version` files instead of guessing (GH-729), and COMPATIBILITY.md documents the upgrade policy plus the enumerated stable `--json` contracts (GH-651)
- The review transport moved from pi to `edda dispatch`-launched Claude with Opus pinned and UUID sessions (GH-708), and the verification ladder's L1 is exact-head CI plus the verifier's focused Windows-gap run (#753)
- Large modules were split for maintainability — `cmd_bridge/`, `cmd_reconcile/`, runner `gate.rs`/`outcome.rs` — and process-spawning tests moved to integration tests (GH-777, GH-778, GH-776, GH-799)
- Pre-commit and commit-msg hooks are git-native via `core.hooksPath`, enforcing the L0 gates locally, with a file/function-length ratchet (GH-634, GH-779)

### Fixed

- Verdict gates fail closed: bad freshness bounds, unparsable `gate_entered_at`, and unreadable ledgers surface errors instead of proceeding (GH-541, GH-744)
- Conductor reconciles in-memory state with disk before phase selection (GH-750), clears stale waivers on retry (GH-747), anchors gate-progress budgets to entry (GH-751), and survives concurrent runner saves and manual skips (GH-556)
- Fleet lane lifecycle: lane-stop kills the whole process tree (GH-672), process snapshot and child PID ordering fixed (GH-706), review lanes supported in lane-stop/lane-status (GH-712), git-config-guard detects and recovers corrupt refs (GH-797), and `.git/config` backups are validated and restored post-kill (GH-715)
- Bridges surface swallowed hook-path write errors (GH-745, GH-692), fix per-turn peer dedup defeated by wall-clock age (GH-678), and stop writing empty session digests with absurd durations (GH-578)
- Claims report standing bare-CLI claims and share one liveness criterion through session inference (GH-705, GH-617)
- `edda ask` and export projections report the governance tier (GH-806) and disambiguate empty vs unregistered stores (GH-701)
- Postmortem command_failure keying, segment command-word matching, and a separate show counter (GH-813)
- Background sharing of the current Haiku model selection (#875); cost estimates return Option and refresh the pricing table (GH-585)
- Release automation enforces distribution channel parity before publishing (GH-655)

## [0.4.0] - 2026-09-02

### Added

- **Multi-agent conductor runtime** — `edda conduct run --agent <claude|pi|codex>` can launch the selected host, while `edda dispatch` provides a hardened single-turn agent command with persistent Codex session-to-thread continuity, honest budget reporting, and cross-process recovery
- **Operator-controlled phase gates** — plans can pause in `AWAITING_VERDICT`, expose the gate during dry runs, and resume through `edda phase approve` or `edda phase reject`; phases can also declare owned write surfaces that become coordination claims
- **Recoverable task execution** — scoped task leases, Codex App Server task clients, unfinished-attempt reconciliation, and validated Windows scheduler launch manifests let interrupted work re-enter safely instead of silently disappearing
- **Coordination inspection and distribution** — `edda claim check` reports path intersections before editing, `edda init` scaffolds coordination skills for Codex as well as Claude, and repository-tracked fleet skills now include sync tooling plus executable CLI doctests
- **Operator control-layer guidance** — the operator runbook and Layer 3/L2 design rulings describe how goals, evidence, verdicts, and non-coding work fit above the ledger and fleet runtime

### Changed

- Coordination reviews now use bounded, SHA-pinned scopes with a verify-once ladder and reusable build lanes, reducing repeated full-workspace runs while keeping Windows-only gaps explicit
- Conductor scheduling now uses runner-owned lane heartbeats, declaration-order tie breaks, time-based retry and timeout deadlines, bounded child reaping, both-stream failure tails, and measured-versus-estimated cost reporting
- Claim and session resolution now disclose replacements and widening, avoid guessing ownership during unclaim, bind process identity consistently across Claude, Codex, and OpenClaw bridges, and expose claim age/staleness through `edda peers --json` for automation consumers
- Existing SQLite stores migrate automatically through the current schema when opened; no manual data migration is required
- Existing installations should run `edda init --force-skills` once to refresh embedded `coord-*` skills; this overwrites local edits to those generated skill files

### Fixed

- Ledger writers now retry bounded SQLite lock contention instead of failing concurrent writes immediately
- Conductor launchers now handle Windows executable lookup, child liveness, dead-child stdin writes, protocol failures, zombie processes, and app-server cleanup consistently
- Windows scheduler manifests now preserve runtime configuration, validate task/query structure, reject unsafe actions, and clean up exact owned tasks
- Coordination output no longer reports releases, claims, widening, or unclaims that did not actually occur
- Markdown-content linting and coordination-skill doctests now distinguish executable commands from nested, indented, or demoted fences without silently skipping assertions
- Release automation now validates tag/workspace/lock consistency before building, safely reuses draft releases, replaces partial assets on retry, verifies the complete non-empty asset set before publishing, and uses the Node 24 checkout runtime

## [0.3.0] - 2026-08-16

### Added

- **Decision provenance** (`edda ratify`) — decisions are now *recorded ≠ ratified*. Every `edda decide` is tagged `authority=agent` (or `system`), never operator; the background decision extractor is tagged `agent` too, and any write that omits authority projects as `unknown` (never the old `human` default). Operator authority is conferred only by a separate, append-only `decision_ratify` event via `edda ratify <key> [--by] [--note]`, so the hash chain can no longer launder machine inference into authoritative fact. The SessionStart decision pack now splits into **Ratified Decisions** (binding) and **Unratified Decisions** (recorded, not binding), with authorship tags; the coordination view and the `coord-sync`/`coord-review` skills were reworded to stop calling broadcasts "binding". Ratified-state is derived from event insertion order (rowid), never stored, and keyed per decision event (a re-decided key must be re-ratified; branch- and import-safe). `--by` is recorded for audit but self-asserted (identity enforcement is a policy-layer concern). Spec: [GH-401](https://github.com/fagemx/edda/issues/401). Existing decisions render as unratified until ratified — a deliberate clean sweep, not a regression. Existing installs: re-run `edda init --force` once to refresh the reworded coordination skills (this overwrites local edits to the `coord-*` skill files)
- **Task rail P1** (`edda task`) — hash-chained `task.*` event family, derived status/readiness projection (never stored), CLI verbs `new/start/done/fail/list/show` (done requires a receipt and reports which successors became ready), Stop-hook nudge for newly-ready assigned tasks, and task-rail verbs taught in the write-back protocol. Spec: `docs/plan/task-rail/TASK_RAIL_V1.md` §3–§7; acceptance drill: `docs/plan/task-rail/P1_DRILL_2026-07-14.md`. Existing installs: re-run `edda init` (or `edda bridge claude install`) once to register the new `Stop` hook
- **Portable reasoning checkpoints** (`edda checkpoint`) — vendor-neutral hypothesis, rejection, open-question, and next-action records are hash-chained, searchable through `edda ask`, and included in deterministic hot packs with section-aware budget degradation
- **Fleet-wide reads** — `edda ask --fleet`, `edda search query --fleet`, `edda log --fleet`, and `edda task list --fleet` fan out across the configured project scope, tag results by project, and report unreachable projects instead of silently treating them as empty; hot packs can also surface sibling-project rulings and queues
- **Incremental full-text indexing** — turn and event cursors avoid full rescans, SessionEnd hooks keep the index current, task receipts are searchable, and the CJK bigram tokenizer supports queries longer than two characters
- **Coordination operations** — the installed `coord-orchestrate` skill and multi-agent discipline guide are joined by hardened peer lifecycle, task-board, notification, TUI, and policy surfaces across supported bridges
- **Richer `edda ask` output** — active task receipts now appear alongside decisions, notes, and related history

### Changed

- Windows Clippy and platform-sensitive tests are now blocking CI; the derived Windows crate set includes conductor and transcript coverage while trimming debug information to keep linking practical

### Fixed

- `edda bundle create` and `edda pair new/revoke/revoke-all` appended chain events without the workspace lock — a concurrent locked writer could interleave and fork the hash chain (two events claiming the same parent). Now serialized like every other writer
- Latent env-var race between `resolve_session_id_tiers` and the `decide()` tests under the parallel test runner (serialized with `ENV_LOCK`, same pattern as edda-bridge-claude)
- Search index safety: an empty ledger no longer erases a populated index, cross-project indexing uses the registered project's own repository, and metadata cursors advance only after the Tantivy commit succeeds
- Coordination requests now acknowledge individual messages, validate and expire targets, avoid treating repo-wide claims as editable scopes, and clean orphaned claims on lifecycle boundaries
- Glob-scoped decision staleness checks now inspect the directory and its matching files, so in-place edits are detected instead of treating the glob as a literal path
- Bridge secret redaction and test-store isolation were expanded to prevent sensitive output leakage and writes into an operator's real registry during tests
- `controls_suggest` fixtures now use a thread-local temporary store without mutating `EDDA_STORE_ROOT`, eliminating Windows test races without touching persistent user data
- `install.sh` now sends latest-release progress to stderr so its default version lookup captures only the tag and builds a valid asset URL

## [0.2.1] - 2026-07-13

### Added

- **Cursor bridge** with native hook installation, context injection, lifecycle tracking, and CLI doctor support

### Fixed

- Workspace formatting and Clippy failures that prevented CI from completing

## [0.2.0] - 2026-07-08

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
