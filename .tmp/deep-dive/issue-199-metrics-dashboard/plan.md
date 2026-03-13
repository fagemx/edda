# Phase 3: Implementation Plan -- Cross-Project Metrics Dashboard (#199)

## Summary

Add cross-project metrics (cost, quality, activity trends) to edda's dashboard, enabling unified governance across multiple projects.

## Sub-Issues

### Sub-Issue 1: Extend Rollup with Cost and Quality Fields
**Crate:** `edda-aggregate`
**Files:** `crates/edda-aggregate/src/rollup.rs`, `crates/edda-aggregate/src/aggregate.rs`
**Effort:** S

Steps:
1. Add `cost_usd: f64`, `execution_count: u64`, `success_count: u64` fields to `DayStat`, `WeekStat`, `MonthStat` (all with `#[serde(default)]` for backward compat)
2. Add `cost_by_date()` function in `aggregate.rs` that extracts `usage.cost_usd` from `execution_event` events, grouped by date
3. Add `quality_by_date()` function returning `BTreeMap<String, (u64, u64)>` (success_count, total_count) per date
4. Update `build_daily_stats()` to accept and merge the new maps
5. Update `build_weekly_stats()` and `build_monthly_stats()` to sum cost and execution counts
6. Add tests: backward compat deserialization, cost aggregation, empty projects

**Acceptance:** `cargo test -p edda-aggregate` passes; existing rollup caches deserialize without error.

---

### Sub-Issue 2: Per-Project Metrics Aggregation Function
**Crate:** `edda-aggregate`
**Files:** `crates/edda-aggregate/src/aggregate.rs`
**Effort:** S

Steps:
1. Add `ProjectMetrics` struct with `activity`, `cost`, `quality` sub-structs
2. Add `per_project_metrics(projects: &[ProjectEntry], range: &DateRange) -> Vec<ProjectMetrics>` that:
   - Opens each project's ledger
   - Computes activity counts (reuse existing filtering logic)
   - Filters execution events and calls `model_quality_from_events()` per project
   - Extracts cost totals from quality report
3. Add tests with mock ledgers

**Acceptance:** Function returns correct metrics for test ledgers with execution events.

---

### Sub-Issue 3: `GET /api/metrics/overview` Endpoint
**Crate:** `edda-serve`
**Files:** `crates/edda-serve/src/lib.rs`
**Effort:** S

Steps:
1. Add `MetricsOverviewQuery` with `days: usize` (default 30), `group: Option<String>`
2. Add `MetricsOverviewResponse` containing `Vec<ProjectMetricsResponse>` + `totals`
3. Implement `get_metrics_overview()` handler:
   - List projects, optionally filter by group
   - Call `per_project_metrics()` from sub-issue 2
   - Return JSON response
4. Register route: `.route("/api/metrics/overview", get(get_metrics_overview))`
5. Add integration test

**Acceptance:** `GET /api/metrics/overview` returns per-project cost/quality/activity breakdown. `?group=x` filters correctly.

---

### Sub-Issue 4: `GET /api/metrics/trends` Endpoint
**Crate:** `edda-serve`
**Files:** `crates/edda-serve/src/lib.rs`
**Effort:** S

Steps:
1. Add `TrendsQuery` with `days`, `granularity` (daily/weekly/monthly), `group: Option<String>`
2. Add `TrendsResponse` containing timeseries data (array of `{date, events, commits, cost_usd, success_rate}`)
3. Implement `get_metrics_trends()` handler:
   - Use `compute_rollup()` or `compute_rollup_incremental()` with extended stats
   - Slice by granularity
   - Return JSON
4. Register route: `.route("/api/metrics/trends", get(get_metrics_trends))`
5. Add integration test

**Acceptance:** Returns timeseries with cost and quality data at specified granularity.

---

### Sub-Issue 5: Augment Dashboard API with Metrics Summary
**Crate:** `edda-serve`
**Files:** `crates/edda-serve/src/lib.rs`
**Effort:** S

Steps:
1. Extend `DashboardSummary` with `total_cost_usd: f64`, `overall_success_rate: f64`
2. Extend `DashboardResponse` with `project_metrics: Vec<ProjectMetricsResponse>` (from sub-issue 3's types)
3. Update `get_dashboard()` to compute and include project metrics
4. Add cost anomaly detection to `compute_attention()`:
   - Yellow: project daily cost > 2x its period average
   - Red: project daily cost > 5x its period average
5. Update existing dashboard tests

**Acceptance:** `/api/dashboard` response includes cost/quality summary; cost anomalies appear in attention section.

---

### Sub-Issue 6: Update Dashboard HTML
**Crate:** `edda-serve`
**Files:** `crates/edda-serve/static/dashboard.html`
**Effort:** M

Steps:
1. Add "Cost Overview" section with per-project cost bars
2. Add "Quality" section with success rate per project
3. Add simple trend chart (CSS bar chart or SVG sparkline, no JS framework)
4. Add group filter dropdown (populated from API)
5. Color-code projects by health (green/yellow/red based on attention routing)

**Acceptance:** `/dashboard` HTML page shows cost, quality, and trend data for all registered projects.

---

## Dependency Order

```
Sub-Issue 1 (rollup fields)
    |
    v
Sub-Issue 2 (per-project metrics)
    |
    +---> Sub-Issue 3 (API /metrics/overview)
    |         |
    |         v
    |     Sub-Issue 5 (augment /api/dashboard)
    |         |
    |         v
    |     Sub-Issue 6 (HTML dashboard)
    |
    +---> Sub-Issue 4 (API /metrics/trends)
```

Sub-issues 1 and 2 are foundation. Sub-issues 3 and 4 can be done in parallel. Sub-issue 5 depends on 3. Sub-issue 6 depends on 5.

## Total Effort: ~6 small tasks = Medium overall

## Architecture Decisions to Record

```bash
edda decide "dashboard.metrics_api=dedicated_endpoints" --reason "composable, cacheable, follows existing /api/metrics/quality pattern"
edda decide "dashboard.trend_source=rollup_cache" --reason "avoid full event scan on every request; incremental updates"
edda decide "dashboard.cost_anomaly=attention_routing" --reason "reuse existing red/yellow/green attention model"
```
