# Phase 3: Plan — Issue #272 DecideSnapshot Storage Endpoint

## Implementation Plan

### Step 1: Add `decide_snapshot` event type to L1 (edda-core)

**Files:**
- `crates/edda-core/src/types.rs` — Add `"decide_snapshot"` to `classify_event_type()` with `(GOVERNANCE, MILESTONE)`.
- `crates/edda-core/src/event.rs` — Add `new_snapshot_event()` builder function.

**Details:**
- Builder accepts: `branch`, `parent_hash`, `engine_version`, `schema_version`, `context_hash`, `redaction_level`, `village_id` (Option), `cycle_id` (Option), `context_value` (Value), `result_value` (Value), `blob_refs` (Vec<String>).
- Sets `event_type = "decide_snapshot"`.
- Constructs payload JSON with metadata + inline or blob references.
- Calls `finalize()` for hash chain.

**Tests:** Add `"decide_snapshot"` to the `classify_all_known_event_types` table test.

---

### Step 2: Add blob offload helper to L2 (edda-ledger)

**Files:**
- `crates/edda-ledger/src/blob_store.rs` — Add `blob_put_if_large()` function.

**Details:**
```rust
pub const SNAPSHOT_BLOB_THRESHOLD: usize = 8192;

pub fn blob_put_if_large(
    paths: &EddaPaths,
    data: &[u8],
    class: BlobClass,
    threshold: usize,
) -> anyhow::Result<Option<String>> {
    if data.len() > threshold {
        Ok(Some(blob_put_classified(paths, data, class)?))
    } else {
        Ok(None)
    }
}
```

This returns `Some(blob_ref)` if offloaded, `None` if data should stay inline.

---

### Step 3: Schema v7 migration — `decide_snapshots` materialized view

**Files:**
- `crates/edda-ledger/src/sqlite_store.rs` — Add `SCHEMA_V7_SQL`, `DecideSnapshotRow`, migration function, and CRUD methods.

**Schema:**
```sql
CREATE TABLE IF NOT EXISTS decide_snapshots (
    event_id        TEXT PRIMARY KEY REFERENCES events(event_id),
    context_hash    TEXT NOT NULL,
    engine_version  TEXT NOT NULL,
    schema_version  TEXT NOT NULL DEFAULT 'snapshot.v1',
    redaction_level TEXT NOT NULL DEFAULT 'full',
    village_id      TEXT,
    cycle_id        TEXT,
    has_blobs       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_context_hash ON decide_snapshots(context_hash);
CREATE INDEX IF NOT EXISTS idx_snapshots_village ON decide_snapshots(village_id) WHERE village_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_snapshots_engine ON decide_snapshots(engine_version);
CREATE INDEX IF NOT EXISTS idx_snapshots_village_engine ON decide_snapshots(village_id, engine_version);
```

**Row struct:**
```rust
pub struct DecideSnapshotRow {
    pub event_id: String,
    pub context_hash: String,
    pub engine_version: String,
    pub schema_version: String,
    pub redaction_level: String,
    pub village_id: Option<String>,
    pub cycle_id: Option<String>,
    pub has_blobs: bool,
    pub created_at: String,
}
```

**Methods:**
- `insert_snapshot(row: &DecideSnapshotRow)` — INSERT into materialized view.
- `query_snapshots(village_id: Option<&str>, engine_version: Option<&str>, limit: usize)` — Filtered list query.
- `snapshots_by_context_hash(context_hash: &str)` — All versions for a given context.

**Migration:**
- `migrate_v6_to_v7()` — Create table + backfill from existing `decide_snapshot` events if any.

**Note:** If PR #280 takes v7, this becomes v8. Check before implementation.

---

### Step 4: Expose via Ledger facade

**Files:**
- `crates/edda-ledger/src/ledger.rs` — Add pass-through methods:
  - `insert_snapshot()`
  - `query_snapshots()`
  - `snapshots_by_context_hash()`
- `crates/edda-ledger/src/lib.rs` — Re-export `DecideSnapshotRow`.

---

### Step 5: HTTP endpoints in L4 (edda-serve)

**Files:**
- `crates/edda-serve/src/lib.rs` — Add three handlers + route registration.

**5a. `POST /api/snapshot`**

Request body:
```rust
#[derive(Deserialize)]
struct SnapshotBody {
    context: serde_json::Value,
    result: serde_json::Value,
    engine_version: String,
    #[serde(default = "default_schema_version")]
    schema_version: String,
    context_hash: String,
    #[serde(default = "default_redaction")]
    redaction_level: String,
    village_id: Option<String>,
    cycle_id: Option<String>,
}
```

Handler logic:
1. Validate `engine_version` non-empty, `context_hash` non-empty.
2. Open ledger, acquire `WorkspaceLock`.
3. Serialize `context` and `result` to bytes.
4. Call `blob_put_if_large()` for each; build payload with inline or blob refs.
5. Create event via `new_snapshot_event()`, `finalize_event()`, `append_event()`.
6. Insert into `decide_snapshots` materialized view.
7. Return `201 Created` with `{ event_id, context_hash }`.

**5b. `GET /api/snapshots`**

Query params: `village_id`, `engine_version`, `limit` (default 20).

Handler logic:
1. Open ledger.
2. Call `query_snapshots()`.
3. For each row, load the event to get full payload.
4. If `has_blobs`, resolve blob refs and reconstruct full snapshot.
5. Return array of snapshot objects.

**5c. `GET /api/snapshots/:context_hash`**

Handler logic:
1. Open ledger.
2. Call `snapshots_by_context_hash()`.
3. Reconstruct full snapshots (same blob resolution as 5b).
4. Return array of versions for this context.

**Route registration** (both `serve()` and `router()`):
```rust
.route("/api/snapshot", post(post_snapshot))
.route("/api/snapshots", get(get_snapshots))
.route("/api/snapshots/{context_hash}", get(get_snapshots_by_hash))
```

---

### Step 6: Tests

**Unit tests:**
- `edda-core`: `classify_event_type("decide_snapshot")` returns correct taxonomy.
- `edda-core`: `new_snapshot_event()` builder produces valid event.
- `edda-ledger`: `blob_put_if_large()` offloads above threshold, returns None below.
- `edda-ledger`: `insert_snapshot()` + `query_snapshots()` + `snapshots_by_context_hash()` round-trip.
- `edda-ledger`: Schema v7 migration on fresh DB and on v6 DB.

**Integration tests (edda-serve):**
- POST snapshot with small payload (inline) -> 201, verify event in ledger.
- POST snapshot with large payload (blob offload) -> 201, verify blobs created.
- POST with missing required fields -> 400.
- GET /api/snapshots with village_id filter.
- GET /api/snapshots/:context_hash returns multiple versions.

---

## Dependency / Risk Analysis

| Risk | Mitigation |
|------|-----------|
| Schema v7 conflict with PR #280 | Check PR #280 status before starting. Use v8 if needed. |
| Large payload performance | Blob offload threshold keeps events table lean. |
| Hash chain breakage | Using existing `finalize()` + `append_event()` guarantees chain integrity. |
| Thyra spec drift | `context` and `result` are opaque `serde_json::Value` — no coupling to Thyra's internal types. |
| Concurrent snapshot writes | `WorkspaceLock` serializes writes, same as all other POST handlers. |

## Estimated Effort

- Step 1 (L1 types + event builder): ~1 hour
- Step 2 (blob helper): ~30 min
- Step 3 (schema v7 + SQLite methods): ~2 hours
- Step 4 (Ledger facade): ~30 min
- Step 5 (HTTP handlers): ~2 hours
- Step 6 (tests): ~2 hours
- **Total: ~8 hours**

## Definition of Done Checklist

- [ ] `POST /api/snapshot` accepts and stores DecideSnapshot
- [ ] `GET /api/snapshots` supports `village_id` + `engine_version` filtering
- [ ] `GET /api/snapshots/:context_hash` returns multiple versions for same context
- [ ] Large payloads (>8KB) offloaded to blob store with `DecisionEvidence` classification
- [ ] Hash chain integrity maintained (parent_hash chaining, finalize())
- [ ] Schema v7 migration with backfill
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes (unit + integration)
