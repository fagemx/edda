# Query Performance Thresholds

This document describes the query path performance expectations for edda's
ledger storage layer and consumer crates.

## Indexed Query Paths (SQL push-down)

These paths push filtering to SQLite and leverage existing indexes. They are
the preferred patterns for user-facing endpoints.

| Operation | Index Used | Expected Scaling |
|-----------|-----------|-----------------|
| `iter_branch_events(branch)` | `idx_events_branch` | O(B) where B = branch events |
| `iter_events_filtered(branch, ...)` | `idx_events_branch_ts` | O(log N + K) |
| `iter_events_by_type(type)` | `idx_events_type` | O(T) where T = type events |
| `find_active_decision(branch, key)` | `idx_decisions_branch_key` | O(1) indexed |
| `active_decisions(domain, key)` | `idx_decisions_active` | O(D) where D = active count |
| `find_related_commits(...)` | `idx_events_type` + LIKE | O(C) where C = commits |
| `find_related_notes(...)` | `idx_events_type` + LIKE | O(N_notes) |

## Intentionally Linear Paths

These paths load all events. They are kept for correctness in batch/export
scenarios but should NOT be used in user-facing hot paths.

| Operation | Use Case | Note |
|-----------|----------|------|
| `iter_events()` | Full ledger export, migration | Escape hatch only |
| `resolve_branch_created_at_fallback()` | Rare fallback in snapshot builder | Only when branch has no events |

## Contributor Guidelines

1. **New query paths must not call `iter_events()` for user-facing features.**
   Use `iter_branch_events()`, `iter_events_filtered()`, or
   `iter_events_by_type()` instead.

2. **Decision queries must use the `decisions` table**, not scan events.
   Use `find_active_decision()`, `active_decisions()`, or `decision_timeline()`.

3. **Keyword search on payload uses `LIKE` at the SQL level.** This is adequate
   for moderate ledger sizes. For full-text search, use `edda-search-fts`
   (tantivy-backed).

4. **When adding a new consumer**, check whether you can reuse an existing
   indexed method before writing a new one.

## Latency Expectations (informational)

These are rough targets based on typical SSD-backed SQLite performance:

| Operation | 1K events | 10K events | 100K events |
|-----------|-----------|------------|-------------|
| `iter_events()` (full scan) | <10ms | <100ms | <1s |
| `iter_branch_events()` | <5ms | <50ms | <200ms |
| `iter_events_filtered()` | <5ms | <20ms | <50ms |
| `find_active_decision()` | <1ms | <1ms | <1ms |
| `edda ask` (keyword) | <10ms | <50ms | <200ms |
