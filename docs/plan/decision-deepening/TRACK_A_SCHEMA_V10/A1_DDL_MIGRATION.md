# A1: DDL Migration — Schema V10

## Bootstrap Instructions

```bash
git checkout main && git pull
git checkout -b track-a/schema-v10-ddl
cargo build -p edda-ledger   # baseline — must pass before any changes
cargo test  -p edda-ledger   # baseline — record existing pass count
```

## Final Result

The `decisions` table gains 6 new columns with safe defaults. All existing
`is_active` queries continue working. `schema_version` bumps to 10.

After migration on any existing DB:
```sql
SELECT status, authority, affected_paths, tags, review_after, reversibility
FROM decisions LIMIT 1;
-- → 'active', 'human', '[]', '[]', NULL, 'medium'
```

## Implementation Steps

### Step 1 — Add `SCHEMA_V10_SQL` constant

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 161-166 (`SCHEMA_V9_SQL` constant)
**Key changes**:

Add the following constant after `SCHEMA_V9_SQL`:

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
";
```

**Column semantics**:
| Column | Type | Default | Purpose |
|--------|------|---------|---------|
| `status` | TEXT NOT NULL | `'active'` | Lifecycle: `proposed`, `active`, `experimental`, `deprecated`, `superseded` |
| `authority` | TEXT NOT NULL | `'human'` | Who made the decision: `human`, `agent`, `system` |
| `affected_paths` | TEXT NOT NULL | `'[]'` | JSON array of glob patterns (e.g. `["crates/edda-ledger/**"]`) |
| `tags` | TEXT NOT NULL | `'[]'` | JSON array of tag strings (e.g. `["architecture","storage"]`) |
| `review_after` | TEXT | `NULL` | ISO-8601 date for scheduled re-evaluation |
| `reversibility` | TEXT NOT NULL | `'medium'` | `easy`, `medium`, `hard` |

**Backfill logic**: The `UPDATE` statement sets `status = 'active'` for rows where
`is_active = 1`, and `status = 'superseded'` for rows where `is_active = 0`.
This ensures COMPAT-01 invariant holds immediately after migration.

### Step 2 — Add `migrate_v9_to_v10()` method

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 677-681 (`migrate_v8_to_v9` method)
**Key changes**:

Add a new method after `migrate_v8_to_v9`:

```rust
fn migrate_v9_to_v10(&self) -> anyhow::Result<()> {
    self.conn.execute_batch(SCHEMA_V10_SQL)?;
    self.set_schema_version(10)?;
    Ok(())
}
```

### Step 3 — Wire into `apply_schema()` migration chain

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 401-407 (end of `apply_schema`, v9 migration block)
**Key changes**:

Add the v10 migration block after the v9 block (before `Ok(())`):

```rust
// Migrate to v10 if needed (decision deepening columns)
let current = self.schema_version()?;
if current < 10 {
    self.migrate_v9_to_v10()?;
}
```

### Step 4 — Extend `DecisionRow` struct

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 169-186 (`DecisionRow` struct)
**Key changes**:

Add 6 new fields to `DecisionRow`:

```rust
pub struct DecisionRow {
    // ... existing fields unchanged ...
    pub source_event_id: Option<String>,
    // ↓ new fields (V10)
    /// Lifecycle status: "proposed", "active", "experimental", "deprecated", "superseded"
    pub status: String,
    /// Decision authority: "human", "agent", "system"
    pub authority: String,
    /// JSON array of glob patterns for guarded file paths
    pub affected_paths: String,
    /// JSON array of tag strings
    pub tags: String,
    /// Optional ISO-8601 date for scheduled re-evaluation
    pub review_after: Option<String>,
    /// Reversibility level: "easy", "medium", "hard"
    pub reversibility: String,
}
```

### Step 5 — Update all `DecisionRow` construction sites

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Key changes**:

Every place that constructs a `DecisionRow` from a SQL query must now read the
6 new columns. Search for all `DecisionRow {` constructions in the file.

For each query that `SELECT`s from `decisions`, append the new columns to the
SELECT list and populate the struct fields. Example pattern:

```rust
// Before:
// SELECT event_id, key, value, reason, domain, branch, supersedes_id, is_active,
//        scope, source_project_id, source_event_id
// After:
// SELECT event_id, key, value, reason, domain, branch, supersedes_id, is_active,
//        scope, source_project_id, source_event_id,
//        status, authority, affected_paths, tags, review_after, reversibility

// And in the row mapping:
DecisionRow {
    // ... existing fields ...
    status: row.get("status")?,
    authority: row.get("authority")?,
    affected_paths: row.get("affected_paths")?,
    tags: row.get("tags")?,
    review_after: row.get("review_after")?,
    reversibility: row.get("reversibility")?,
}
```

Use `grep -n "DecisionRow {" crates/edda-ledger/src/sqlite_store.rs` to find
all construction sites. Each one must include the new fields.

For the `ts` field (which may come from a JOIN with `events`), note that it uses
`row.get("ts").ok()` — the new fields use direct `row.get("column_name")?`
since they have NOT NULL defaults.

## Acceptance Criteria

- [ ] `cargo build -p edda-ledger` — zero errors
- [ ] `cargo test -p edda-ledger` — all existing tests pass (no regressions)
- [ ] `cargo clippy -p edda-ledger -- -D warnings` — zero warnings
- [ ] Fresh DB gets `schema_version = 10` (verify via `schema_version()`)
- [ ] Existing V9 DB migrates to V10 without error
- [ ] After migration, `SELECT status FROM decisions WHERE is_active = 1 LIMIT 1` → `'active'`
- [ ] After migration, `SELECT status FROM decisions WHERE is_active = 0 LIMIT 1` → `'superseded'`
- [ ] COMPAT-01 invariant holds:
  ```sql
  SELECT COUNT(*) FROM decisions
  WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
     OR (is_active = 0 AND status IN ('active', 'experimental'));
  -- Expected: 0
  ```
- [ ] Existing `WHERE is_active = TRUE` queries return identical results (COMPAT-02)

## Git Commit

```
feat(ledger): add Schema V10 DDL migration for decision deepening

Add 6 new columns to decisions table: status, authority,
affected_paths, tags, review_after, reversibility. Backfill
status from existing is_active boolean to maintain COMPAT-01
invariant.

Refs: GH-decision-deepening Track A1
```
