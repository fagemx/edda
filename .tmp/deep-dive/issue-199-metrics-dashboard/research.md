# Phase 1: Research -- Cross-Project Metrics Dashboard (#199)

## 1. Problem Statement

Edda manages multiple projects but lacks a unified governance view. Currently:
- Each project's metrics (decisions, commits, costs, quality) are siloed in its own ledger
- The existing `/api/dashboard` endpoint provides cross-project decision risk/timeline but **no cost, quality, or trend metrics**
- No way to compare project health side-by-side or track trends over time

## 2. Current Architecture Inventory

### 2.1 Data Already Collected (per project)

| Data Type | Source | Storage |
|-----------|--------|---------|
| Events (commits, notes, decisions) | `edda-ledger` SQLite | Per-project `.edda/ledger.db` |
| Decision scope (local/shared/global) | `edda-core::DecisionScope` | `decisions` table |
| Execution events (model, cost, tokens, latency, status) | `edda-bridge-claude` | `events` table (`execution_event` type) |
| Session stats (file edits, tool usage) | `edda-bridge-claude` session digest | Event payload `session_stats` |
| Daily cost tracking | `edda-bridge-claude::bg_detect` | `~/.edda/collabs/<tool>/daily_costs.json` |
| Project groups | `edda-store::registry` | `~/.edda/registry.json` (`group` field) |

### 2.2 Existing Cross-Project Aggregation (`edda-aggregate`)

| Function | What it does | Missing for dashboard |
|----------|-------------|----------------------|
| `aggregate_overview()` | Event/commit/decision/session counts per project | No cost, no quality |
| `aggregate_commits()` | Cross-project commit list with date range | OK for timeline |
| `aggregate_decisions()` | Active decisions across all projects | OK |
| `events_by_date()` / `commits_by_date()` | Daily counts | No per-project breakdown |
| `file_edits_by_date()` | File edit heatmap | Good |
| `model_quality_from_events()` | Model success rate, cost, latency | **Only takes event slice, not cross-project** |
| `compute_decision_risks()` | Risk scoring per decision | OK |
| `build_dependency_graph()` | Decision dependency graph | OK |

### 2.3 Existing Dashboard API (`edda-serve`)

**`GET /api/dashboard?days=N`** returns:
```json
{
  "period": { "from", "to", "days" },
  "summary": { "total_projects", "total_decisions", "total_events", "total_commits" },
  "attention": { "red", "yellow", "green" },
  "timeline": [{ "ts", "event_type", "key", "value", "reason", "project", "risk_level" }],
  "graph": { "nodes", "edges" },
  "risks": [{ "event_id", "key", "value", "project", "risk_score", "risk_level", "factors" }]
}
```

**Missing from current dashboard:**
- Per-project cost breakdown (total cost, cost trend)
- Per-project quality metrics (success rate, latency)
- Cross-project comparison (which project costs most, which has lowest quality)
- Trend data (daily/weekly cost and quality timeseries)
- Rollup integration (the `Rollup` struct already supports daily/weekly/monthly but is not exposed via API)

### 2.4 Rollup System (`edda-aggregate::rollup`)

Already implemented but **not connected to the dashboard API**:
- `DayStat` / `WeekStat` / `MonthStat` with events, commits, file_edits
- Incremental computation with `compute_rollup_incremental()`
- Cache at `~/.edda/collabs/<tool>/rollup.json`
- Missing: cost and quality fields in rollup stats

### 2.5 Project Groups (`edda-store::registry`)

- `ProjectEntry.group: Option<String>` -- groups projects for sync
- `list_groups()` returns `BTreeMap<String, Vec<ProjectEntry>>`
- `list_group_members()` returns peers in same group
- Currently used only for decision sync (#205), not for dashboard filtering

## 3. Gaps Analysis

| Gap | Impact | Difficulty |
|-----|--------|------------|
| No per-project cost aggregation in `edda-aggregate` | Cannot show cost comparison | Medium -- need to aggregate `execution_event` costs per project |
| No per-project quality in aggregate | Cannot compare model quality across projects | Medium -- extend `model_quality_from_events` to work per-project |
| Rollup has no cost/quality fields | Trend data incomplete | Low -- add fields to `DayStat`/`WeekStat`/`MonthStat` |
| Dashboard API returns flat structure | Cannot do per-project drill-down | Medium -- need per-project sections |
| No group-level aggregation | Cannot see group health | Low -- filter `list_projects()` by group |
| Event loading is O(n) full scan | Performance issue with many projects | High -- but existing pattern, defer optimization |

## 4. Related Issues and PRs

- **#205 (merged)**: Cross-project decision sync -- added `DecisionScope`, project groups, sync engine
- **#186 (#173)**: Decision outcomes tracking -- added execution event linking to decisions
- **Archive doc `0215-dashboard-ux.md`**: Earlier UX design (from gctx era) -- different scope but has useful layout ideas

## 5. Constraints

- L1-L4 layering: `edda-aggregate` (L3) can depend on `edda-ledger` (L2) and `edda-core` (L1), but not on `edda-serve` (L4)
- No `unwrap()` in library code
- All SQL must use `params![]`
- `cargo clippy -- -D warnings` must pass
- Dashboard HTML is a single embedded file (`static/dashboard.html`), not a SPA framework
