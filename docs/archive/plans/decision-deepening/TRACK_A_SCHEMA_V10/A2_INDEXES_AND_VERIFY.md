# A2: Indexes and Verification

## Bootstrap Instructions

```bash
git checkout track-a/schema-v10-ddl   # A1 must be complete
cargo build -p edda-ledger             # baseline — must pass
cargo test  -p edda-ledger             # baseline — all tests pass
```

**Prerequisite**: A1 (DDL Migration) must be merged or committed on this branch.
The 6 new columns (`status`, `authority`, `affected_paths`, `tags`,
`review_after`, `reversibility`) must exist.

## Final Result

New indexes on `status` and `status+domain` for query performance.
A test suite verifying V10 migration works on both fresh and V9 databases,
and that existing `is_active` queries remain functional.

## Implementation Steps

### Step 1 — Add indexes to `SCHEMA_V10_SQL`

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 161-166 (`SCHEMA_V9_SQL` with index definitions)
**Key changes**:

Append the following index statements to `SCHEMA_V10_SQL` (after the `UPDATE`
backfill statement added in A1):

```rust
const SCHEMA_V10_SQL: &str = "
ALTER TABLE decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE decisions ADD COLUMN authority TEXT NOT NULL DEFAULT 'human';
ALTER TABLE decisions ADD COLUMN affected_paths TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN review_after TEXT;
ALTER TABLE decisions ADD COLUMN reversibility TEXT NOT NULL DEFAULT 'medium';

-- Backfill: sync status from existing is_active boolean
UPDATE decisions SET status = CASE WHEN is_active = 1 THEN 'active' ELSE 'superseded' END;

-- Indexes for status-based queries
CREATE INDEX IF NOT EXISTS idx_decisions_status
    ON decisions(status);
CREATE INDEX IF NOT EXISTS idx_decisions_status_domain
    ON decisions(status, domain);
CREATE INDEX IF NOT EXISTS idx_decisions_affected_paths
    ON decisions(affected_paths) WHERE affected_paths != '[]';
";
```

**Index rationale**:
| Index | Purpose |
|-------|---------|
| `idx_decisions_status` | Filter by lifecycle status (e.g. all `active` or `proposed`) |
| `idx_decisions_status_domain` | Status + domain compound queries (Track C `query_by_paths`) |
| `idx_decisions_affected_paths` | Partial index for decisions with file guards (Track E PreToolUse) |

### Step 2 — Add V10 migration test

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in the `#[cfg(test)]` module at the bottom)
**Reference**: Search for `#[cfg(test)]` and existing migration tests in the file.
**Key changes**:

Add a test that verifies V10 migration works on a fresh DB:

```rust
#[test]
fn test_schema_v10_fresh_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    // Version should be 10
    assert_eq!(store.schema_version().unwrap(), 10);

    // Verify new columns exist by inserting a test row
    store.conn.execute(
        "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
         VALUES ('evt_test', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
        [],
    ).unwrap();

    store.conn.execute(
        "INSERT INTO decisions
         (event_id, key, value, reason, domain, branch, is_active, scope,
          status, authority, affected_paths, tags, reversibility)
         VALUES ('evt_test', 'test.key', 'val', 'reason', 'test', 'main', TRUE, 'local',
                 'active', 'human', '[\"src/**\"]', '[\"arch\"]', 'medium')",
        [],
    ).unwrap();

    // Read back and verify
    let status: String = store.conn.query_row(
        "SELECT status FROM decisions WHERE key = 'test.key'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(status, "active");

    let paths: String = store.conn.query_row(
        "SELECT affected_paths FROM decisions WHERE key = 'test.key'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(paths, "[\"src/**\"]");
}
```

### Step 3 — Add V9→V10 migration test

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in `#[cfg(test)]` module)
**Key changes**:

Add a test that simulates a V9 database with existing decisions, then migrates:

```rust
#[test]
fn test_schema_v9_to_v10_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");

    // Phase 1: Create a V9 database manually
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '1')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V2_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '2')", [],
        ).unwrap();
        // Apply V3-V5 (decisions table must exist with scope column)
        conn.execute_batch(SCHEMA_V3_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '3')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V4_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '4')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V5_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '5')", [],
        ).unwrap();
        // Skip V6-V8 table creation for brevity — just bump version
        conn.execute_batch(SCHEMA_V6_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '6')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V7_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '7')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V8_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '8')", [],
        ).unwrap();
        conn.execute_batch(SCHEMA_V9_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '9')", [],
        ).unwrap();

        // Insert test events + decisions (V9 schema — no status/authority columns)
        conn.execute(
            "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
             VALUES ('evt_a', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
             VALUES ('evt_b', '2026-01-02T00:00:00Z', 'note', 'main', 'h2', '{}')",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO decisions (event_id, key, value, reason, domain, branch, is_active, scope)
             VALUES ('evt_a', 'db.engine', 'sqlite', 'embedded', 'db', 'main', TRUE, 'local')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO decisions (event_id, key, value, reason, domain, branch, is_active, scope)
             VALUES ('evt_b', 'old.key', 'old_val', 'deprecated', 'old', 'main', FALSE, 'local')",
            [],
        ).unwrap();
    }

    // Phase 2: Reopen — should auto-migrate to V10
    let store = SqliteStore::open_or_create(&db_path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);

    // Active decision should have status='active'
    let status: String = store.conn.query_row(
        "SELECT status FROM decisions WHERE key = 'db.engine'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(status, "active");

    // Inactive decision should have status='superseded'
    let status: String = store.conn.query_row(
        "SELECT status FROM decisions WHERE key = 'old.key'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(status, "superseded");

    // COMPAT-01 invariant check
    let violations: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM decisions
         WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
            OR (is_active = 0 AND status IN ('active', 'experimental'))",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(violations, 0, "COMPAT-01 violated after migration");
}
```

### Step 4 — Add backward compatibility test

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in `#[cfg(test)]` module)
**Key changes**:

Add a test confirming existing `WHERE is_active = TRUE` queries work unchanged:

```rust
#[test]
fn test_v10_backward_compat_is_active_queries() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    // Insert events + decisions via the normal append path
    // (or directly for unit test isolation)
    store.conn.execute(
        "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
         VALUES ('evt_c1', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
        [],
    ).unwrap();
    store.conn.execute(
        "INSERT INTO decisions
         (event_id, key, value, reason, domain, branch, is_active, scope, status)
         VALUES ('evt_c1', 'compat.test', 'yes', 'test', 'compat', 'main', TRUE, 'local', 'active')",
        [],
    ).unwrap();

    // Existing query pattern: WHERE is_active = TRUE
    let count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM decisions WHERE is_active = TRUE AND domain = 'compat'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1);

    // Existing partial index query pattern
    let count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM decisions WHERE is_active = TRUE AND domain = 'compat' AND branch = 'main'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1);
}
```

## Acceptance Criteria

- [ ] `cargo build -p edda-ledger` — zero errors
- [ ] `cargo test -p edda-ledger` — all tests pass including the 3 new tests
- [ ] `cargo clippy -p edda-ledger -- -D warnings` — zero warnings
- [ ] `test_schema_v10_fresh_db` passes — new columns exist on fresh DB
- [ ] `test_schema_v9_to_v10_migration` passes — backfill sets correct status
- [ ] `test_v10_backward_compat_is_active_queries` passes — existing query patterns work
- [ ] COMPAT-01: zero rows where `is_active` and `status` disagree
- [ ] COMPAT-02: no existing `WHERE is_active = TRUE` queries modified

## Git Commit

```
test(ledger): add V10 schema indexes and migration verification tests

Add indexes on status, status+domain, and affected_paths for query
performance. Add 3 tests: fresh V10 DB, V9→V10 migration with backfill
verification, and backward compatibility for is_active queries.

Refs: GH-decision-deepening Track A2
```
