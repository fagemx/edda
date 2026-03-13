# Phase 2: Innovate -- Cross-Project Metrics Dashboard (#199)

## Design Options

### Option A: Extend Existing Dashboard Endpoint

Augment `GET /api/dashboard` response with cost/quality/trend sections.

**Pros:** Single endpoint, backward compatible (new fields only), minimal API surface change.
**Cons:** Response grows large; clients that only need cost data must parse everything.

### Option B: Dedicated Metrics Endpoints + Composite Dashboard

Add focused endpoints:
- `GET /api/metrics/overview` -- per-project summary with cost + quality
- `GET /api/metrics/trends?granularity=daily|weekly|monthly` -- timeseries
- `GET /api/metrics/compare?projects=a,b` -- side-by-side comparison
- Keep `/api/dashboard` as the composite that calls these internally

**Pros:** Composable, cacheable per endpoint, cleaner separation.
**Cons:** More endpoints to maintain, multiple round-trips for full dashboard.

### Option C: GraphQL-style Single Endpoint with Field Selection

`GET /api/dashboard?include=cost,quality,trends,risks`

**Pros:** Flexible, single endpoint.
**Cons:** Overengineered for current scale, adds query parsing complexity.

### Recommendation: **Option B** (Dedicated Metrics Endpoints)

Rationale:
1. Follows existing pattern -- the codebase already has `GET /api/metrics/quality` as a separate endpoint
2. Allows incremental delivery -- ship per-project overview first, add trends later
3. Cacheable -- rollup-based trend data changes infrequently
4. The composite `/api/dashboard` can evolve independently to include these

---

## Key Design Decisions

### D1: Per-Project Metrics Shape

```rust
struct ProjectMetrics {
    project_id: String,
    name: String,
    group: Option<String>,
    period: MetricsPeriod,
    activity: ActivityMetrics,
    cost: CostMetrics,
    quality: QualityMetrics,
}

struct ActivityMetrics {
    events: usize,
    commits: usize,
    decisions: usize,
    sessions: usize,
}

struct CostMetrics {
    total_usd: f64,
    daily_avg_usd: f64,
    by_model: Vec<ModelCost>,  // breakdown by model
}

struct QualityMetrics {
    success_rate: f64,
    avg_latency_ms: f64,
    total_steps: u64,
}
```

This reuses existing data: `ActivityMetrics` comes from `ProjectSummary`, `CostMetrics` from `QualityReport`, `QualityMetrics` from `ModelQuality`.

### D2: Trend Data via Rollup Extension

Extend existing `DayStat` with cost and quality:

```rust
struct DayStat {
    date: String,
    events: usize,
    commits: usize,
    sessions: usize,
    file_edits: Vec<FileEditStat>,
    // NEW:
    cost_usd: f64,
    success_rate: f64,  // 0.0..1.0, NaN if no executions
    execution_count: u64,
}
```

This is backward-compatible due to `#[serde(default)]`. Existing rollup caches will deserialize with zeros for new fields.

### D3: Cross-Project Comparison

Rather than a dedicated comparison endpoint, return all project metrics in `GET /api/metrics/overview` and let the client sort/filter. The HTML dashboard can render a comparison table.

### D4: Group Filtering

All new endpoints accept `?group=<name>` to filter projects by group. This leverages the existing `list_groups()` / `list_group_members()` infrastructure from #205.

### D5: Reuse `model_quality_from_events` Per-Project

Currently `model_quality_from_events` takes a flat `&[Event]` slice. For per-project quality, call it once per project rather than creating a new function. This avoids duplication.

---

## Innovation: Attention-Driven Cost Alerts

Beyond raw metrics, add cost anomaly detection to the attention routing system:

```rust
// In compute_attention():
// If any project's daily cost exceeds 2x its 7-day average, add a yellow item
// If any project's daily cost exceeds 5x its 7-day average, add a red item
```

This transforms the dashboard from "show numbers" to "tell me what needs attention" -- consistent with the existing red/yellow/green attention model.

## Innovation: Decision-Cost Correlation

Link decisions to their execution costs. When a decision is made (e.g., `runtime.model=claude-opus`), track the cost impact in subsequent executions. This is already partially supported by the `decision_outcomes` endpoint (#186) but not surfaced in the dashboard.

---

## Risks

| Risk | Mitigation |
|------|------------|
| Full event scan per project is O(n) | Use rollup cache for trend data; only scan recent events for real-time metrics |
| Rollup cache format change | Use `#[serde(default)]` for new fields; existing caches auto-upgrade |
| Large response for many projects | Paginate or add `?limit` parameter |
| Cost data may be sparse (not all projects have execution events) | Return `null` / `0.0` for projects without cost data; don't error |
