# Phase 2: Innovate — Issue #272 DecideSnapshot Storage Endpoint

## Design Options

### Option A: Pure Event + Inline Payload

Store the entire DecideSnapshot as an event with `event_type = "decide_snapshot"`, keeping the full `context` and `result` JSON inside `payload`.

**Pros:** Simple, no schema change needed, hash chain works automatically.
**Cons:** Large payloads (context can be 50KB+) bloat the events table and slow `iter_events()`. No efficient filtering by `village_id`, `engine_version`, or `context_hash`.

### Option B: Event + Blob Offload + Materialized View (Recommended)

1. **Event**: `event_type = "decide_snapshot"` with a compact payload (metadata only: `engine_version`, `schema_version`, `context_hash`, `redaction_level`, `village_id`, `cycle_id`).
2. **Blob offload**: If `context` + `result` exceed a threshold (e.g., 8KB), serialize them to blob store and reference via `refs.blobs`. Otherwise inline.
3. **Materialized view**: New `decide_snapshots` SQL table (schema v7) with indexed columns for query endpoints.

**Pros:** Keeps events table lean, enables efficient SQL queries, follows existing patterns (decisions, bundles, task_briefs). Blob store handles large payloads with content-addressable dedup.
**Cons:** More code, one more migration. But this is the established pattern.

### Option C: Separate Storage Outside Event Chain

Store snapshots in a standalone table not linked to the event chain. Query-only, no hash chain participation.

**Pros:** Zero impact on existing events.
**Cons:** Breaks the "everything is an event" contract. No provenance tracking. No replay via event stream. Against the project's design philosophy.

## Recommendation: Option B

This follows established patterns (Karvi events, decisions materialized view, task_briefs) and satisfies all requirements:
- Hash chain participation via event creation
- Efficient queries via materialized view
- Large payload handling via blob offload
- `context_hash` indexing for version comparison

## Key Design Decisions

### 1. Blob Offload Threshold

**Decision: 8KB threshold**

Rationale: The `events.payload` column stores JSON text. Events are scanned in `iter_events()`. Typical event payloads are 200-2000 bytes. A 50KB DecideSnapshot would be ~25x larger. 8KB balances between avoiding unnecessary blob indirection for small snapshots and keeping the events table performant.

Implementation: If `serde_json::to_vec(&context)?.len() + serde_json::to_vec(&result)?.len() > 8192`, offload both to separate blobs (`context_blob`, `result_blob`). Otherwise, embed inline.

### 2. Blob Classification

Snapshots are `BlobClass::DecisionEvidence` — they support decision analysis but aren't final artifacts. They can be GC'd after `keep_days` but aren't ephemeral noise.

### 3. Event Type Taxonomy

```
"decide_snapshot" => (event_family::GOVERNANCE, event_level::MILESTONE)
```

Governance family because it tracks decision engine behavior. Milestone level because each snapshot represents a discrete decision outcome.

### 4. Schema Version

The materialized view requires schema v7 (or v8 if #280 takes v7). This needs coordination with pending PRs.

### 5. context_hash as Primary Query Key

The `context_hash` field (SHA-256 of input context) enables finding all decisions made for the same situation across different engine versions. This is the core A/B comparison mechanism.

### 6. Payload Structure

**Event payload** (compact, stored in events table):
```json
{
  "engine_version": "phase1",
  "schema_version": "snapshot.v1",
  "context_hash": "abc123...",
  "redaction_level": "full",
  "village_id": "blog-village",
  "cycle_id": "cycle-xxx",
  "context_inline": { ... },
  "result_inline": { ... },
  "context_blob": "blob:sha256:...",
  "result_blob": "blob:sha256:..."
}
```

### 7. Query Response Assembly

GET endpoints need to reconstruct the full snapshot:
1. Read from `decide_snapshots` materialized view for filtering
2. Load event for metadata
3. If blob refs present, read from blob store; otherwise, use inline payload

## Edge Cases

- **Duplicate context_hash + engine_version**: Not an error. Multiple snapshots for the same context are expected (that's the replay/comparison use case). Each gets a unique event_id.
- **Missing village_id/cycle_id**: Should be optional. Not all Thyra contexts may have these.
- **Blob store unavailable**: Fallback to inline storage with a warning log. Don't fail the request.
- **Concurrent writes**: `WorkspaceLock` handles this, same as existing POST handlers.
