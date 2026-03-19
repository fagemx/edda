# B1: Status Sync in insert_decision

## Bootstrap Instructions

```bash
git checkout main && git pull
git checkout -b track-b/mutation-contract
cargo build -p edda-ledger   # baseline — must pass
cargo test  -p edda-ledger   # baseline — record existing pass count
```

**Prerequisite**: Track A (Schema V10) must be merged to `main`. The 6 new
columns (`status`, `authority`, `affected_paths`, `tags`, `review_after`,
`reversibility`) must exist in the `decisions` table, and `DecisionRow` must
include the corresponding fields.

## Final Result

The decision materialization path in `append_event()` writes the `status`
column on every INSERT. A helper function `status_to_is_active()` ensures
`is_active` and `status` always agree (COMPAT-01 invariant). The deactivation
UPDATE also syncs `status` when superseding a prior decision.

## Implementation Steps

### Step 1 — Add `status_to_is_active()` helper

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Place near the top of the `impl SqliteStore` block or as a
free function before `impl SqliteStore`.
**Key changes**:

```rust
/// Map a decision status string to the legacy is_active boolean.
///
/// `is_active = true` iff status is "active" or "experimental".
/// This enforces CONTRACT COMPAT-01.
fn status_to_is_active(status: &str) -> bool {
    matches!(status, "active" | "experimental")
}
```

This function is the **single source of truth** for the `is_active ↔ status`
mapping. All code paths that write `is_active` or `status` must use it.

Valid status values: `proposed`, `active`, `experimental`, `deprecated`, `superseded`.

### Step 2 — Modify decision materialization in `append_event()`

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 947-986 (decision materialization block inside `append_event`)
**Key changes**:

The current INSERT writes `is_active = TRUE` unconditionally. Modify it to:

1. Determine the status (for new decisions created via `edda decide`, always `"active"`).
2. Use `status_to_is_active()` to derive `is_active`.
3. Write both `status` and `is_active` in the INSERT.

**Before** (lines 972-986):
```rust
tx.execute(
    "INSERT INTO decisions
     (event_id, key, value, reason, domain, branch, supersedes_id, is_active, scope)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, TRUE, ?8)",
    params![
        event.event_id,
        key,
        value,
        reason,
        domain,
        event.branch,
        supersedes_id,
        scope_str
    ],
)?;
```

**After**:
```rust
let status = "active";
let is_active = status_to_is_active(status);
tx.execute(
    "INSERT INTO decisions
     (event_id, key, value, reason, domain, branch, supersedes_id, is_active, scope,
      status, authority, affected_paths, tags, review_after, reversibility)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
             ?10, ?11, ?12, ?13, ?14, ?15)",
    params![
        event.event_id,
        key,
        value,
        reason,
        domain,
        event.branch,
        supersedes_id,
        is_active,
        scope_str,
        status,
        "human",   // authority default
        "[]",      // affected_paths default
        "[]",      // tags default
        None::<String>,  // review_after default
        "medium",  // reversibility default
    ],
)?;
```

### Step 3 — Sync status on deactivation UPDATE

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Lines 961-966 (deactivation of prior decision)
**Key changes**:

When a decision supersedes a prior one, the existing code sets
`is_active = FALSE`. It must also set `status = 'superseded'`.

**Before** (lines 962-965):
```rust
tx.execute(
    "UPDATE decisions SET is_active = FALSE
     WHERE key = ?1 AND branch = ?2 AND is_active = TRUE",
    params![key, event.branch],
)?;
```

**After**:
```rust
tx.execute(
    "UPDATE decisions SET is_active = FALSE, status = 'superseded'
     WHERE key = ?1 AND branch = ?2 AND is_active = TRUE",
    params![key, event.branch],
)?;
```

### Step 4 — Sync status in `insert_imported_decision()`

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: Line 1637 (`insert_imported_decision` method) and the INSERT
at approximately line 1680.
**Key changes**:

The imported decision path also writes to the `decisions` table. It must
include `status` derived from `is_active` via `status_to_is_active()`.

Find the INSERT statement in `insert_imported_decision` and add `status`:

```rust
let status = if p.is_active { "active" } else { "superseded" };
// ... then include status in the INSERT column list and params
```

Also update the deactivation UPDATE in `insert_imported_decision` (if present)
to set `status = 'superseded'` alongside `is_active = FALSE`.

### Step 5 — Add invariant test

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in `#[cfg(test)]` module)
**Key changes**:

```rust
#[test]
fn test_status_is_active_sync_on_insert() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    // Create a decision event through the normal append path
    let dp = edda_core::types::DecisionPayload {
        key: "sync.test".to_string(),
        value: "v1".to_string(),
        reason: Some("testing sync".to_string()),
        scope: None,
    };
    let event = edda_core::event::new_decision_event(
        "main", None, "system", &dp,
    ).unwrap();
    store.append_event(&event).unwrap();

    // Verify status and is_active agree
    let (status, is_active): (String, bool) = store.conn.query_row(
        "SELECT status, is_active FROM decisions WHERE key = 'sync.test'",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(status, "active");
    assert!(is_active);

    // Supersede with a new value
    let dp2 = edda_core::types::DecisionPayload {
        key: "sync.test".to_string(),
        value: "v2".to_string(),
        reason: Some("supersede".to_string()),
        scope: None,
    };
    let mut event2 = edda_core::event::new_decision_event(
        "main", Some(&event.hash), "system", &dp2,
    ).unwrap();
    event2.refs.provenance.push(edda_core::types::Provenance {
        target: event.event_id.clone(),
        rel: "supersedes".to_string(),
        note: None,
    });
    store.append_event(&event2).unwrap();

    // Old decision: is_active=false, status=superseded
    let (status, is_active): (String, bool) = store.conn.query_row(
        "SELECT status, is_active FROM decisions WHERE key = 'sync.test' AND value = 'v1'",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(status, "superseded");
    assert!(!is_active);

    // New decision: is_active=true, status=active
    let (status, is_active): (String, bool) = store.conn.query_row(
        "SELECT status, is_active FROM decisions WHERE key = 'sync.test' AND value = 'v2'",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(status, "active");
    assert!(is_active);

    // COMPAT-01 full table check
    let violations: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM decisions
         WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
            OR (is_active = 0 AND status IN ('active', 'experimental'))",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(violations, 0, "COMPAT-01 violated");
}
```

### Step 6 — Add unit test for `status_to_is_active()`

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in `#[cfg(test)]` module)
**Key changes**:

```rust
#[test]
fn test_status_to_is_active() {
    assert!(status_to_is_active("active"));
    assert!(status_to_is_active("experimental"));
    assert!(!status_to_is_active("proposed"));
    assert!(!status_to_is_active("deprecated"));
    assert!(!status_to_is_active("superseded"));
}
```

## Acceptance Criteria

- [ ] `cargo build -p edda-ledger` — zero errors
- [ ] `cargo test -p edda-ledger` — all tests pass including the 2 new tests
- [ ] `cargo clippy -p edda-ledger -- -D warnings` — zero warnings
- [ ] New decisions get `status = 'active'` and `is_active = TRUE`
- [ ] Superseded decisions get `status = 'superseded'` and `is_active = FALSE`
- [ ] `status_to_is_active()` is the single mapping function (no hardcoded booleans)
- [ ] COMPAT-01 invariant holds after insert + supersede cycle
- [ ] Imported decisions also write correct `status`
- [ ] MUTATION-01: `grep -rn "UPDATE decisions" crates/ --include="*.rs" | grep -v sqlite_store.rs` → 0 results

## Git Commit

```
feat(ledger): sync status ↔ is_active on every decision write (B1)

Add status_to_is_active() helper as single source of truth for the
is_active/status mapping. Modify decision materialization in
append_event() and insert_imported_decision() to always write both
columns in sync. Deactivation UPDATE now also sets status='superseded'.

Contract: COMPAT-01, MUTATION-01
Refs: GH-decision-deepening Track B1
```
