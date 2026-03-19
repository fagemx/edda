# Demo Path — Incremental implementation plan

> Status: `working draft`
> Purpose: Define the minimum viable implementation steps to get Decision Injection working end-to-end.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Three milestones: path-match query, session start pack, PreToolUse warning. Each is independently shippable.**

---

## 2. What It's NOT

### NOT a full project plan

This is a demo path — the minimal sequence to prove the architecture works. Production hardening (caching, performance tuning, telemetry) comes after.

### NOT a timeline

No dates. Each milestone has a "done when" definition. Ship when it passes.

---

## 3. Prerequisites

Before starting, these must exist:

| Prerequisite | Status | Location |
|-------------|--------|----------|
| **M0: Schema V10 migration** | **NOT YET IMPLEMENTED** | `edda-ledger` — see `../decision-model/schema-v0.md` §5 |
| `DecisionView` type in Rust | Exists conceptually in spec; needs Rust struct | `edda-ledger` or `edda-core` |
| `to_view()` function | Exists conceptually; needs implementation | `edda-ledger` |
| `affected_paths` stored as JSON array | **Requires M0** — column does not exist yet | `edda-ledger/sqlite_store` |
| `edda-ask` query engine | Exists | `crates/edda-ask/src/lib.rs` |
| Hook dispatch infrastructure | Exists | `crates/edda-bridge-claude/src/dispatch/` |

> **Blocking dependency: M0 (Schema V10 migration) must be completed before any milestone below can proceed.** The `affected_paths`, `status`, `authority`, `tags`, `review_after`, and `reversibility` columns referenced throughout M1-M3 do not exist in the current schema (v9). See `../decision-model/schema-v0.md` §5 for the migration SQL and §5a for the phased rollout plan.

---

## 4. Milestone 1: Path-Match Query

**Goal:** Given a file path, return matching decisions.

### What to build

1. **`query_by_paths()` function** in `edda-ask` (or new `edda-injection` crate)
   - Input: `Vec<String>` file paths, branch, limit
   - Load all active decisions from ledger
   - Parse `affected_paths` JSON strings into glob patterns
   - Match each glob against each input path using `glob::Pattern`
   - Return matched decisions sorted by match specificity

2. **`to_view()` implementation** in `edda-ledger`
   - Convert `DecisionRow` -> `DecisionView`
   - Parse `affected_paths` and `tags` from JSON strings to `Vec<String>`
   - Rename `scope` -> `propagation`
   - Strip storage-only fields

### Done when

```bash
# Unit test: query_by_paths finds decisions matching file path
cargo test -p edda-ask test_query_by_paths

# Manual verification:
edda decide "db.engine=sqlite" --reason "embedded" --paths "crates/edda-ledger/**"
# Then query:
# query_by_paths(["crates/edda-ledger/src/lib.rs"], "main", 5) -> returns db.engine
```

### Files touched
- `crates/edda-ask/src/lib.rs` — add `query_by_paths()`, `to_view()` usage
- `crates/edda-ledger/src/sqlite_store.rs` — add `to_view()` if not already present
- `crates/edda-ask/Cargo.toml` — add `glob` dependency if needed

### Estimated scope: ~150 lines of new code + tests

---

## 5. Milestone 2: Session Start Decision Pack

**Goal:** At session start, inject a markdown block of relevant decisions into `additionalContext`.

### What to build

1. **`build_pack()` function**
   - Takes scored decisions + stage + branch
   - Applies stage-specific filtering (status, max items)
   - Returns `DecisionPack` struct

2. **`render_decision_pack_md()` function**
   - Converts `DecisionPack` -> markdown string
   - Budget-aware: truncates if over ~2000 chars
   - Format: `## Active Decisions (N)` + bullet list

3. **Integration in `dispatch_session_start()`**
   - After hot pack, before workspace section
   - Build `RetrievalContext` from cwd + branch
   - Call `query_for_context()` (simplified: domain match + path match from cwd)
   - Render and append to content

### Done when

```bash
# Integration test: session start includes decisions
cargo test -p edda-bridge-claude test_session_start_includes_decisions

# Manual verification:
edda decide "db.engine=sqlite" --reason "embedded"
edda hook claude --event SessionStart --cwd /path/to/project
# Output includes: "## Active Decisions (1)\n- db.engine=sqlite..."
```

### Files touched
- `crates/edda-ask/src/lib.rs` — add `build_pack()`, `render_decision_pack_md()`
- `crates/edda-bridge-claude/src/dispatch/session.rs` — integrate in `dispatch_session_start()`
- `crates/edda-bridge-claude/src/dispatch/mod.rs` — expose new functions if needed

### Estimated scope: ~200 lines of new code + tests

### ASCII: Session Start Flow (with Injection)

```text
dispatch_session_start()
  │
  ├── read_hot_pack()                    // EXISTING
  ├── render_skill_guide_directive()     // EXISTING
  ├── render_active_plan()               // EXISTING
  │
  ├── ┌─────────────────────────────┐    // NEW (Milestone 2)
  │   │ Build RetrievalContext      │
  │   │   cwd -> file_paths        │
  │   │   branch -> branch         │
  │   │   task_brief -> stage      │
  │   │                            │
  │   │ query_for_context(ctx)     │
  │   │   query_by_paths()         │
  │   │   query_by_domain()        │
  │   │   rank + filter            │
  │   │                            │
  │   │ render_decision_pack_md()  │
  │   └────────────┬────────────────┘
  │                │
  │                ▼
  ├── append decision_md to content
  │
  ├── compose_narrative()                // EXISTING
  ├── render_workspace_section()         // EXISTING
  ├── render_write_back_protocol()       // EXISTING
  └── apply_context_budget()             // EXISTING
```

---

## 6. Milestone 3: PreToolUse File Warning

**Goal:** When an agent edits a file governed by a decision, inject a warning.

### What to build

1. **File path extraction from PreToolUse payload**
   - Edit/Write: `input.file_path` (direct)
   - Bash: parse `command` for file path references (best-effort, regex)

2. **Warning rendering**
   - If `query_by_paths()` returns a match with score > 0.5
   - Render: `[Decision] db.engine=sqlite governs this file. Ensure changes align.`

3. **Integration in PreToolUse handler**
   - Extract file path
   - Call `query_by_paths([path], branch, 1)`
   - If match, render and return as `additionalContext`
   - If no match, return empty (no overhead)

### Done when

```bash
# Unit test: PreToolUse returns warning for governed file
cargo test -p edda-bridge-claude test_pre_tool_use_decision_warning

# Manual verification:
edda decide "db.engine=sqlite" --paths "crates/edda-ledger/**"
edda hook claude --event PreToolUse --tool Edit \
  --file crates/edda-ledger/src/lib.rs
# Output includes: "[Decision] db.engine=sqlite governs this file"
```

### Files touched
- `crates/edda-bridge-claude/src/dispatch/` — PreToolUse handler
- `crates/edda-ask/src/lib.rs` — reuse `query_by_paths()` from Milestone 1

### Estimated scope: ~100 lines of new code + tests

### Performance constraint

PreToolUse MUST complete in < 100ms. This means:
- `query_by_paths()` must be fast: iterate active decisions (typically < 100), glob match is O(1) per pattern
- No Tantivy index lookup needed — pure glob matching
- If performance is a concern, cache active decisions with `affected_paths` in memory at session start

---

## 7. Future Milestones (Post-Demo)

These are not part of the demo path but documented for planning:

| Milestone | Description | Depends On |
|-----------|-------------|-----------|
| M4: UserPromptSubmit injection | Lightweight decision hints on prompt submit | M1, M2 |
| M5: Stage inference | Auto-detect plan/implement/review from context | M2 |
| M6: Conflict explanation | Format `ConflictInfo` from Governance | Governance spec |
| M7: HTTP endpoints | `/api/decisions/pack`, `/api/decisions/file-match` | M1, M2 |
| M8: Ranking weights tuning | A/B test weight configurations | M2 operational |
| M9: Task-aware retrieval | Extract keywords from issue/brief for domain matching | M2, karvi integration |

---

## 8. Risk Register

| Risk | Impact | Mitigation |
|------|--------|-----------|
| PreToolUse latency > 100ms | Blocks agent tool use, degrades UX | Cache decisions at session start; glob matching is inherently fast |
| Too many decisions injected | Context bloat reduces agent effectiveness | Hard cap per stage (3-7); budget-aware rendering |
| `affected_paths` rarely populated | Path matching returns nothing useful | Fallback to domain match; encourage `--paths` in CLI |
| Stale decisions in pack | Agent follows outdated guidance | Always query live from ledger; no cross-session caching |

---

## Closing Line

> **Three milestones, each independently shippable: path-match query (M1), session start pack (M2), file edit warning (M3). Total estimated new code: ~450 lines + tests. Build on existing infrastructure, don't replace it.**
