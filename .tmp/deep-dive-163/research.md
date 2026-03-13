# Research Phase: Per-File Edit Tracking for Code Heatmap (Issue #163) — Updated

## Issue Summary

**Goal**: Chronicle dashboard needs per-file edit statistics for a code heatmap. Track which files are edited, how many times, by how many agents, with revert detection.

**Target data structure**:
```json
{
  "date": "2026-03-01",
  "repo": "edda",
  "file_edits": {
    "src/auth/middleware.ts": { "edits": 12, "reverts": 3, "agents": 2 }
  }
}
```

## Current Codebase State (main branch)

### Already Implemented

#### 1. Per-file edit counting in session digestion
- **File**: `crates/edda-bridge-claude/src/digest/extract.rs`
- Line 14: `file_edit_map: BTreeMap<String, u64>` tracks edits per file during session
- Line 84: Increments count for Edit/Write tool calls
- Line 137: Stores as `stats.file_edit_counts`
- **File**: `crates/edda-bridge-claude/src/digest/mod.rs`
- Line 107: `pub file_edit_counts: Vec<(String, u64)>` in `SessionStats`

#### 2. Rollup infrastructure
- **File**: `crates/edda-aggregate/src/rollup.rs`
- `DayStat`, `WeekStat`, `MonthStat` structs (lines 13-35)
- `compute_rollup()`, `merge_rollups()`, `build_daily_stats()` functions
- Currently tracks: events, commits, sessions — **no file_edits**

#### 3. Aggregation functions
- **File**: `crates/edda-aggregate/src/aggregate.rs`
- `events_by_date()`, `commits_by_date()` — parallel pattern for new `file_edits_by_date()`
- **No `file_edits_by_date()` exists on main**

#### 4. CLI rollup display
- **File**: `crates/edda-cli/src/cmd_user.rs`
- `execute_rollup()` (line 245) — shows monthly summary, **no file edit display**

#### 5. Noise filtering
- `signals::is_noise_file()` filters .git/, node_modules/ etc. — already applied in digest

### Missing Components

1. **`FileEditStat` struct** — needed in rollup.rs
2. **`file_edits` field on DayStat/WeekStat/MonthStat** — extends existing structs
3. **`file_edits_by_date()` function** — new aggregation query in aggregate.rs
4. **Revert detection** — not implemented anywhere
5. **Agent deduplication** — need unique session counting per file per date
6. **CLI display** — show top edited files in rollup output

## PR #181 Analysis

### What PR #181 does
- Adds `FileEditStat` struct to `rollup.rs` (edits, reverts, agents fields)
- Extends `DayStat`, `WeekStat`, `MonthStat` with `file_edits: BTreeMap<String, FileEditStat>`
- Adds `file_edits_by_date()` to `aggregate.rs`
- Adds `merge_file_edits()` helper for rollup merging
- Updates `build_daily_stats()`, `build_weekly_stats()`, `build_monthly_stats()` signatures
- Adds CLI display of top 10 edited files
- Adds env mutex guard in `render.rs` tests (test race fix)
- Includes one integration test

### PR #181 Problems

#### 1. CI Failures (all platforms)
- **Format check fails** — formatting issues not addressed
- **Clippy fails** — `edda-serve` has dead fields (`store_root`, `since`) causing errors with `-Dwarnings`
- These are **pre-existing issues in the branch base**, not introduced by PR #181 changes themselves

#### 2. Agent Counting Bug
In `file_edits_by_date()`, the PR increments `stat.agents += 1` for every `file_edit_counts` entry within each event. This means if a single session edits 3 files, each file gets `agents = 1`, but if two events from the **same session** exist, agents will be double-counted. The code does not deduplicate by session_id.

**Current PR approach** (line in diff):
```rust
// Each session_stats event represents one agent session
stat.agents += 1;
```

This is acceptable **only if** each session produces exactly one event with `session_stats`. If sessions can emit multiple summary events, agents will be overcounted.

#### 3. No Revert Detection
The `reverts` field exists in `FileEditStat` but is always 0. No detection logic is implemented. The issue spec shows reverts as a desired metric.

#### 4. Missing Session Deduplication
The function doesn't track which sessions contributed to each file's edit count. If the same session appears in multiple events (e.g., due to incremental rollup re-scanning), edits and agents will be double-counted.

### PR #181 Strengths
- Clean structural changes to rollup types
- Correct tuple parsing (`Vec<(String, u64)>` serializes as `[["path", count]]`)
- Good test covering multi-session aggregation
- Proper `merge_file_edits()` helper

## Data Flow

```
Hook (PostToolUse) -> dispatch -> session ledger (.edda/ledger/sessions/)
                                    |
                   digest/extract.rs -> SessionStats.file_edit_counts
                                    |
                   session summary event -> project ledger (events.jsonl)
                                    | [MISSING]
                   aggregate.rs -> file_edits_by_date()
                                    | [MISSING]
                   rollup.rs -> DayStat.file_edits
                                    | [MISSING]
                   cmd_user.rs -> CLI display
```

## Key Design Questions

1. **Session deduplication**: Should we track unique sessions per file per date to avoid double-counting agents? (Yes, for correctness)
2. **Revert detection scope**: MVP without reverts, or include basic `git revert` command detection?
3. **Rollup size**: Large repos could have hundreds of files per day. Need top-N filtering?
4. **Backward compatibility**: Adding fields to DayStat/WeekStat/MonthStat — existing rollup.json files must deserialize with `#[serde(default)]`
