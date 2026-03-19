# C2: query_by_paths() — glob-based decision lookup

**Track**: C — Decision View (L1 read-side projection)
**Dependencies**: C1 (DecisionView + to_view() must exist), Track A (V10 columns)
**Blocks**: E1 (PreToolUse uses this to find decisions governing a file)

---

## Goal

Add two query methods to `crates/edda-ledger/src/ledger.rs`:
1. `query_active_with_paths()` — returns active decisions that have non-empty `affected_paths`
2. `query_by_paths()` — given file paths, returns decisions whose `affected_paths` globs match

These are the read-side queries that Injection (Track E, Track F) will use.

---

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `crates/edda-ledger/src/sqlite_store.rs` | Modify | Add `active_decisions_with_paths()` SQL query |
| `crates/edda-ledger/src/ledger.rs` | Modify | Add `query_active_with_paths()` and `query_by_paths()` |
| `crates/edda-ledger/Cargo.toml` | Modify | Add `glob` crate dependency |

---

## Step 1: Add `glob` dependency

In `crates/edda-ledger/Cargo.toml`:

```toml
[dependencies]
glob = "0.3"
```

Only the `glob::Pattern` type is needed for matching. No filesystem access — we match string patterns against string paths.

---

## Step 2: Add SQL query in `sqlite_store.rs`

Add a new method to `SqliteStore`:

```rust
/// Return active/experimental decisions where `affected_paths` is non-empty.
/// This pre-filters at the SQL level so glob matching runs on a small set.
pub fn active_decisions_with_paths(
    &self,
    branch: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<Vec<DecisionRow>> {
    let mut sql = String::from(
        "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                d.supersedes_id, d.is_active, e.ts,
                d.scope, d.source_project_id, d.source_event_id,
                d.status, d.authority, d.affected_paths, d.tags,
                d.review_after, d.reversibility
         FROM decisions d JOIN events e ON d.event_id = e.event_id
         WHERE d.is_active = TRUE
           AND d.affected_paths IS NOT NULL
           AND d.affected_paths != '[]'",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1;

    if let Some(b) = branch {
        sql.push_str(&format!(" AND d.branch = ?{idx}"));
        param_values.push(Box::new(b.to_string()));
        idx += 1;
    }

    sql.push_str(" ORDER BY e.ts DESC");

    if let Some(lim) = limit {
        sql.push_str(&format!(" LIMIT ?{idx}"));
        param_values.push(Box::new(lim as i64));
    }

    let conn = self.conn.lock().unwrap();
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |r| {
        // Map row to DecisionRow — same pattern as active_decisions()
        // Include V10 fields in the mapping
        Ok(DecisionRow { /* ... field mapping ... */ })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}
```

**Note**: The exact `DecisionRow` field mapping must match the SELECT column order. Follow the same pattern as the existing `active_decisions()` method (line 1421 of `sqlite_store.rs`), adding the V10 columns.

---

## Step 3: Add query methods in `ledger.rs`

Add these methods to the `impl Ledger` block, in the `// ── Decisions ──` section (after `find_active_decision` at line ~237):

### query_active_with_paths()

```rust
use crate::view::{self, DecisionView};

/// Return active decisions that have non-empty `affected_paths`.
/// Used by Injection to get the candidate set for glob matching.
pub fn query_active_with_paths(
    &self,
    branch: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<Vec<DecisionView>> {
    let rows = self.sqlite.active_decisions_with_paths(branch, limit)?;
    Ok(rows.iter().map(view::to_view).collect())
}
```

### query_by_paths()

```rust
/// Given a list of file paths, return decisions whose `affected_paths` globs
/// match any of them. Uses the `glob` crate for pattern matching.
///
/// This is the primary query for PreToolUse hook (Track E):
/// "which active decisions govern the file I'm about to edit?"
///
/// # Arguments
/// - `paths`: concrete file paths to check (e.g., `["crates/edda-ledger/src/lib.rs"]`)
/// - `branch`: optional branch filter
/// - `limit`: max decisions to return
///
/// # Returns
/// Decisions whose `affected_paths` contain at least one glob that matches
/// at least one of the given `paths`.
pub fn query_by_paths(
    &self,
    paths: &[&str],
    branch: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<Vec<DecisionView>> {
    // 1. Get all active decisions that have affected_paths
    let candidates = self.query_active_with_paths(branch, None)?;

    // 2. Filter by glob match
    let mut matched: Vec<DecisionView> = Vec::new();
    for decision in candidates {
        let dominated = decision.affected_paths.iter().any(|glob_pattern| {
            match glob::Pattern::new(glob_pattern) {
                Ok(pattern) => paths.iter().any(|p| pattern.matches(p)),
                Err(_) => false,  // invalid glob → skip silently
            }
        });
        if dominated {
            matched.push(decision);
        }
        if let Some(lim) = limit {
            if matched.len() >= lim {
                break;
            }
        }
    }

    Ok(matched)
}
```

**Design note**: We fetch all candidates from SQL first, then glob-match in Rust. This is correct because:
- The candidate set is small (only decisions with non-empty `affected_paths`)
- Glob matching cannot be done in SQLite
- PERF-01 (< 100ms) is satisfied because the SQL query is indexed on `is_active`

---

## Tests

Add tests in `crates/edda-ledger/src/view.rs` (or a separate test file if preferred):

### Test 1: Glob matches file path

```rust
#[test]
fn query_by_paths_glob_matches() {
    // Setup: decision with affected_paths = ["crates/edda-ledger/**"]
    let (dir, ledger) = setup_ledger_with_decision(
        "db.engine", "sqlite",
        &["crates/edda-ledger/**"],
    );

    let results = ledger.query_by_paths(
        &["crates/edda-ledger/src/lib.rs"],
        Some("main"),
        None,
    ).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "db.engine");
}
```

### Test 2: No match returns empty

```rust
#[test]
fn query_by_paths_no_match() {
    let (dir, ledger) = setup_ledger_with_decision(
        "db.engine", "sqlite",
        &["crates/edda-ledger/**"],
    );

    let results = ledger.query_by_paths(
        &["crates/edda-cli/src/main.rs"],  // doesn't match ledger glob
        Some("main"),
        None,
    ).unwrap();

    assert!(results.is_empty());
}
```

### Test 3: Only active/experimental decisions returned

```rust
#[test]
fn query_by_paths_skips_superseded() {
    // Setup: two decisions for same key, second supersedes first
    let (dir, ledger) = setup_ledger();
    // Insert decision 1: db.engine=postgres, paths=["crates/**"]
    // Insert decision 2: db.engine=sqlite, paths=["crates/**"] (supersedes #1)
    // Only decision 2 should appear (is_active = TRUE for it)

    let results = ledger.query_by_paths(
        &["crates/edda-ledger/src/lib.rs"],
        Some("main"),
        None,
    ).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value, "sqlite");
}
```

### Test 4: Multiple globs in one decision

```rust
#[test]
fn query_by_paths_multiple_globs() {
    let (dir, ledger) = setup_ledger_with_decision(
        "error.pattern", "thiserror",
        &["crates/edda-ledger/**", "crates/edda-core/**"],
    );

    // Match on second glob
    let results = ledger.query_by_paths(
        &["crates/edda-core/src/types.rs"],
        Some("main"),
        None,
    ).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "error.pattern");
}
```

### Test 5: Limit respected

```rust
#[test]
fn query_by_paths_respects_limit() {
    // Setup: 3 decisions all matching "crates/**"
    let (dir, ledger) = setup_ledger();
    // ... insert 3 decisions with affected_paths = ["crates/**"]

    let results = ledger.query_by_paths(
        &["crates/edda-ledger/src/lib.rs"],
        Some("main"),
        Some(2),
    ).unwrap();

    assert!(results.len() <= 2);
}
```

---

## Verification

```bash
cargo build -p edda-ledger
cargo test -p edda-ledger -- query_by_paths
cargo test -p edda-ledger -- query_active_with_paths
cargo clippy -p edda-ledger -- -D warnings
```

---

## Constraints

- **BOUNDARY-01**: Return type is `Vec<DecisionView>`, not `Vec<DecisionRow>`
- **BOUNDARY-02**: Glob matching uses parsed `affected_paths` from `to_view()`, never raw JSON
- **PERF-01**: Must be < 100ms for typical workloads (< 50 decisions with paths)
- **CLIPPY-01**: Zero clippy warnings
- **TEST-01**: All workspace tests pass
