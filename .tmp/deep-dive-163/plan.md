# Implementation Plan: Per-File Edit Tracking (Issue #163) — Updated

## Overview

Implement per-file edit tracking for Chronicle's code heatmap by building on PR #181's approach, fixing its bugs, and ensuring CI passes.

## Strategy

Start fresh from `main` (PR #181 is OPEN with all CI failing). Cherry-pick the structural approach but fix:
1. Session deduplication for agent counting
2. Backward compatibility with `#[serde(default)]`
3. Formatting and CI compliance

## Prerequisites

- `main` branch is the base (current HEAD: `2f59b65`)
- `SessionStats.file_edit_counts` already exists in `digest/mod.rs` and `digest/extract.rs`
- `edda-serve` dead code warnings may need separate fix if they block CI

## Implementation Steps

### Step 1: Add `FileEditStat` struct and extend rollup types

**File**: `crates/edda-aggregate/src/rollup.rs`

1. Add `FileEditStat` struct with `edits: u64`, `reverts: u64`, `agents: usize`
2. Add `#[serde(default)]` `file_edits: BTreeMap<String, FileEditStat>` to DayStat, WeekStat, MonthStat

**Lines changed**: ~15

### Step 2: Add `file_edits_by_date()` with session deduplication

**File**: `crates/edda-aggregate/src/aggregate.rs`

Key implementation details:
- Parse `session_stats.file_edit_counts` from events (format: `[["path", count], ...]`)
- Track unique sessions per (date, file) using `HashSet<String>`
- Set `agents` from unique session count after iteration
- Use `session_id` from event payload for deduplication

**Lines changed**: ~60

### Step 3: Update `build_daily_stats()` signature and logic

**File**: `crates/edda-aggregate/src/rollup.rs`

- Add `file_edits_map` parameter to `build_daily_stats()`
- Include `file_edits` in `DayStat` construction
- Update `compute_rollup()` to call `file_edits_by_date()` and pass result

**Lines changed**: ~15

### Step 4: Add `merge_file_edits()` and update weekly/monthly builders

**File**: `crates/edda-aggregate/src/rollup.rs`

- Add `merge_file_edits()` helper function
- Update `build_weekly_stats()` to accumulate file_edits per week
- Update `build_monthly_stats()` to accumulate file_edits per month

**Lines changed**: ~40

### Step 5: Update CLI display

**File**: `crates/edda-cli/src/cmd_user.rs`

- Show top 10 edited files from the latest daily entry
- Format: `  path (N edits, M agents)`

**Lines changed**: ~12

### Step 6: Fix existing test fixtures

**File**: `crates/edda-aggregate/src/rollup.rs` (tests)

Update existing test fixtures to include `file_edits: BTreeMap::new()` in DayStat literals and update `build_daily_stats` call signature.

**Lines changed**: ~10

### Step 7: Add new tests

**File**: `crates/edda-aggregate/src/aggregate.rs` (tests)

1. `file_edits_by_date_parses_tuples_correctly` — Two events, verify edit sums and agent counts
2. `file_edits_by_date_deduplicates_sessions` — Same session_id in two events, verify agents=1
3. `file_edits_by_date_empty_projects` — Empty input returns empty result

**File**: `crates/edda-aggregate/src/rollup.rs` (tests)

4. Backward compat test — Deserialize DayStat JSON without file_edits field

**Lines changed**: ~120

### Step 8: Verify CI compliance

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## File Changes Summary

| File | Lines | Description |
|------|-------|-------------|
| `crates/edda-aggregate/src/rollup.rs` | ~80 | FileEditStat struct, extend stat types, merge helper, update builders |
| `crates/edda-aggregate/src/aggregate.rs` | ~180 | file_edits_by_date() + 3 tests |
| `crates/edda-cli/src/cmd_user.rs` | ~12 | CLI display of top edited files |

**Total**: ~270 lines

## Deferred Items

- **Revert detection**: `reverts` field stays at 0. Implement in follow-up issue.
- **Top-N filtering**: Store all files for now. Add size limits if rollup.json grows large.

## Success Criteria

1. DayStat/WeekStat/MonthStat include `file_edits: BTreeMap<String, FileEditStat>`
2. `file_edits_by_date()` correctly parses `file_edit_counts` from session events
3. Agent counting uses unique session_id deduplication
4. Old `rollup.json` files without `file_edits` deserialize without errors (`#[serde(default)]`)
5. `edda user rollup` CLI shows top edited files
6. All existing tests pass, new tests cover core logic
7. `cargo fmt`, `cargo clippy -Dwarnings`, `cargo test` all pass
8. Output structure matches issue spec

## Relationship to PR #181

This plan supersedes PR #181. Same structural approach but fixes:
- Agent counting with session deduplication
- Backward compatibility with `#[serde(default)]`
- CI compliance

PR #181 should be closed when this implementation is merged.
