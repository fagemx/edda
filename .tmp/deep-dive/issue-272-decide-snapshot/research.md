# Phase 1: Research — Issue #272 DecideSnapshot Storage Endpoint

## 1. Issue Summary

Thyra's `DecisionEngine.decide()` produces a `DecideSnapshot` (full input context + output result). This issue requests Edda endpoints to receive, store, and query these snapshots for replay and A/B comparison. It is a prerequisite for Thyra Phase 1.5 (#83).

**Endpoints requested:**
- `POST /api/snapshot` — ingest a DecideSnapshot
- `GET /api/snapshots?village_id=xxx&engine_version=phase1&limit=10` — list/filter
- `GET /api/snapshots/:context_hash` — find versions for the same context

## 2. Existing Architecture Analysis

### 2.1 Event Model (edda-core L1)

- **`Event` struct** (`crates/edda-core/src/types.rs`): All ledger entries are `Event` objects with `event_id`, `ts`, `event_type`, `branch`, `parent_hash`, `hash`, `payload` (JSON), `refs` (blobs, events, provenance), `digests`, `event_family`, `event_level`.
- **Hash chain**: Each event's `hash` is computed from its canonical JSON (excluding `hash`, `digests`, `schema_version`). `parent_hash` points to the previous event, forming an append-only chain.
- **`classify_event_type()`** maps `event_type` strings to `(family, level)` taxonomy. Currently handles 15 types. A new `decide_snapshot` type would need to be added here.

### 2.2 Event Construction (edda-core L1)

- `crates/edda-core/src/event.rs` provides builder functions: `new_note_event()`, `new_decision_event()`, `new_execution_event()`, etc.
- Pattern: create `Event` struct with fields, call `finalize()` which sets taxonomy and computes hash/digests.
- A new `new_snapshot_event()` builder would follow the same pattern.

### 2.3 Persistence Layer (edda-ledger L2)

- **SQLite store** (`crates/edda-ledger/src/sqlite_store.rs`): Single `ledger.db` with WAL mode. Schema currently at v6.
- **Events table**: Stores all events. `payload` is stored as JSON text.
- **Materialized views**: `decisions`, `review_bundles`, `task_briefs`, `decision_deps` tables serve as indexed projections for specific event types.
- **Schema migration**: `apply_schema()` runs versioned migrations (`SCHEMA_V2_SQL` through `SCHEMA_V6_SQL`). A new `SCHEMA_V7_SQL` would add a `decide_snapshots` table.
- **Blob store** (`crates/edda-ledger/src/blob_store.rs`): Content-addressable file store. `blob_put()` writes bytes, returns `blob:sha256:<hex>`. Atomic writes via tmp+rename. `blob_put_classified()` also writes metadata.

### 2.4 HTTP API Layer (edda-serve L4)

- `crates/edda-serve/src/lib.rs` — single-file axum server.
- Routes registered in `serve()` and `router()` functions (dual registration pattern).
- Handler pattern: `async fn handler(State(state), body) -> Result<impl IntoResponse, AppError>`.
- `AppError` enum with `Validation`, `NotFound`, `Conflict`, `Internal` variants.
- `WorkspaceLock` used for write operations (see `post_note`, `post_decide`).

### 2.5 Blob Store Integration

- `BlobClass` enum: `Artifact`, `DecisionEvidence`, `TraceNoise`.
- No existing size threshold logic — the issue asks for large payloads to use blob store, but this is a new pattern.
- Blobs are referenced via `refs.blobs` array on events (format: `blob:sha256:<hex>`).

### 2.6 Existing Snapshot Concept (edda-derive)

- `crates/edda-derive/src/snapshot.rs` is a **branch snapshot** (rebuild_branch), not related to DecideSnapshot. This is about deriving the current state of a branch from events. No naming conflict but worth noting.

## 3. Key Constraints

1. **Hash-chain integrity**: New events must properly chain `parent_hash`. Using `finalize()` handles this.
2. **No upward dependencies**: L1/L2 cannot depend on L4.
3. **No unwrap in library code**: Use `thiserror` enums.
4. **SQL must use `params![]`**: No string concatenation.
5. **`cargo clippy -- -D warnings`** must pass.

## 4. Schema Version Status

- Current: v6 (task_briefs)
- Note: PR #280 may introduce v7 — need to coordinate. If v7 is already taken, this would be v8.

## 5. Missing Information

- `docs/DECISION_ENGINE_V02.md` referenced in the issue does **not exist** in this repo. The spec is external (Thyra side).
- `village_id` and `cycle_id` are Thyra concepts not currently present in Edda's domain model.
- No existing blob-offloading threshold pattern exists — this would be a new mechanism.

## 6. Similar Patterns to Follow

- **Karvi event ingestion** (`post_karvi_event`): Best analogy — external system posting structured events, stored as typed events with full payload.
- **Decisions materialized view**: Pattern for creating SQL projection tables from event payloads.
- **Task briefs**: Latest materialized view addition (schema v6), good template for migration code.
