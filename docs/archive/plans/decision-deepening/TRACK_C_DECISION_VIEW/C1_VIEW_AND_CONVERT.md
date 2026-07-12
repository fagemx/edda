# C1: DecisionView struct + to_view() function

**Track**: C — Decision View (L1 read-side projection)
**Dependencies**: Track A (Schema V10 columns must exist)
**Blocks**: E1 (glob match), F1 (decision pack)

---

## Goal

Create `crates/edda-ledger/src/view.rs` with:
1. `DecisionView` struct — the read-side projection consumed by Injection
2. `to_view()` function — converts `DecisionRow` → `DecisionView`

**Key contract (BOUNDARY-01)**: After this task, all read-side consumers (hooks, pack builder) use `DecisionView`. They never import `DecisionRow`.

---

## Files to Create / Modify

| File | Action | Description |
|------|--------|-------------|
| `crates/edda-ledger/src/view.rs` | **Create** | `DecisionView` struct + `to_view()` |
| `crates/edda-ledger/src/lib.rs` | Modify | Add `pub mod view;` |

---

## Step 1: Create `crates/edda-ledger/src/view.rs`

### DecisionView struct

Reference: `docs/decision/decision-model/shared-types.md` §2.3

```rust
/// Read-side projection of a decision. Injection consumers use this type
/// instead of `DecisionRow` (BOUNDARY-01).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionView {
    // Identity
    pub event_id: String,
    pub branch: String,
    pub ts: Option<String>,

    // What
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,

    // Governance state
    pub status: String,           // "active" | "experimental" | ...
    pub authority: String,        // "human" | "agent_approved" | ...
    pub reversibility: String,    // "low" | "medium" | "high"

    // Scope — parsed arrays, not JSON strings
    pub affected_paths: Vec<String>,
    pub tags: Vec<String>,
    pub propagation: String,      // renamed from `scope` column

    // Graph (optional)
    pub supersedes_id: Option<String>,
}
```

**Key differences from `DecisionRow`** (per spec):
- `affected_paths` and `tags` are `Vec<String>`, not JSON strings
- `scope` column renamed to `propagation`
- No `is_active` field — consumers filter by `status` directly
- No `source_project_id` / `source_event_id` — storage-only concerns

### to_view() function

```rust
use crate::sqlite_store::DecisionRow;

/// Convert a storage row into a delivery view.
///
/// - Parses `affected_paths` and `tags` from JSON string → `Vec<String>`
/// - Renames `scope` → `propagation`
/// - Drops `is_active`, `source_project_id`, `source_event_id`
///
/// If `affected_paths` or `tags` JSON is invalid or missing, defaults to `vec![]`.
pub fn to_view(row: &DecisionRow) -> DecisionView {
    let affected_paths: Vec<String> =
        serde_json::from_str(&row.affected_paths).unwrap_or_default();

    let tags: Vec<String> =
        serde_json::from_str(&row.tags).unwrap_or_default();

    DecisionView {
        event_id: row.event_id.clone(),
        branch: row.branch.clone(),
        ts: row.ts.clone(),
        key: row.key.clone(),
        value: row.value.clone(),
        reason: row.reason.clone(),
        domain: row.domain.clone(),
        status: row.status.clone(),
        authority: row.authority.clone(),
        reversibility: row.reversibility.clone(),
        affected_paths,
        tags,
        propagation: row.scope.clone(),
        supersedes_id: row.supersedes_id.clone(),
    }
}
```

**Note on `DecisionRow` field types**: After Track A (Schema V10), `DecisionRow` will have these new fields. Until then, use `Option<String>` with defaults. The exact field names and types depend on A1 — check the `DecisionRow` struct at implementation time.

Current `DecisionRow` (pre-V10, line 170 of `sqlite_store.rs`):
```rust
pub struct DecisionRow {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,
    pub branch: String,
    pub supersedes_id: Option<String>,
    pub is_active: bool,
    pub ts: Option<String>,
    pub scope: String,
    pub source_project_id: Option<String>,
    pub source_event_id: Option<String>,
}
```

After Track A adds V10 columns, expect these additional fields (DB has `NOT NULL DEFAULT`):
- `status: String` (default `"active"`)
- `authority: String` (default `"human"`)
- `affected_paths: String` (default `"[]"`, JSON string)
- `tags: String` (default `"[]"`, JSON string)
- `review_after: Option<String>` (default `None` — the only nullable V10 field)
- `reversibility: String` (default `"medium"`)

---

## Step 2: Register module in `crates/edda-ledger/src/lib.rs`

Add after the existing `pub mod` declarations (currently: `blob_meta`, `blob_store`, `device_token`, `ledger`, `lock`, `paths`, `sqlite_store`, `sync`, `tombstone`):

```rust
pub mod view;
```

---

## Tests

Add `#[cfg(test)] mod tests` block inside `view.rs`:

### Test 1: Parse valid JSON arrays

```rust
#[test]
fn to_view_parses_json_arrays() {
    let row = make_row_with_paths(
        r#"["crates/edda-ledger/**", "crates/edda-core/**"]"#,
        r#"["architecture", "storage"]"#,
    );
    let view = to_view(&row);
    assert_eq!(view.affected_paths, vec!["crates/edda-ledger/**", "crates/edda-core/**"]);
    assert_eq!(view.tags, vec!["architecture", "storage"]);
}
```

### Test 2: Empty/null JSON defaults to empty vec

```rust
#[test]
fn to_view_defaults_empty_on_empty_array() {
    let row = make_row_with_paths("[]", "[]");  // affected_paths and tags are empty JSON arrays
    let view = to_view(&row);
    assert!(view.affected_paths.is_empty());
    assert!(view.tags.is_empty());
}

#[test]
fn to_view_defaults_empty_on_invalid_json() {
    let row = make_row_with_paths("not json", "{bad}");
    let view = to_view(&row);
    assert!(view.affected_paths.is_empty());
    assert!(view.tags.is_empty());
}
```

### Test 3: Rename scope → propagation

```rust
#[test]
fn to_view_renames_scope_to_propagation() {
    let mut row = make_default_row();
    row.scope = "shared".to_string();
    let view = to_view(&row);
    assert_eq!(view.propagation, "shared");
}
```

### Test 4: No is_active field

```rust
#[test]
fn decision_view_has_no_is_active() {
    // Compile-time check: DecisionView must not have is_active field.
    // This test is structural — if someone adds `is_active`, it will
    // fail to compile because the struct literal below won't match.
    let view = to_view(&make_default_row());
    let _ = view.status;  // exists
    // view.is_active;  // must not compile
}
```

### Test helper

```rust
fn make_default_row() -> DecisionRow {
    DecisionRow {
        event_id: "evt_test".to_string(),
        key: "db.engine".to_string(),
        value: "sqlite".to_string(),
        reason: "embedded".to_string(),
        domain: "db".to_string(),
        branch: "main".to_string(),
        supersedes_id: None,
        is_active: true,
        ts: Some("2026-03-20T00:00:00Z".to_string()),
        scope: "local".to_string(),
        source_project_id: None,
        source_event_id: None,
        // V10 fields (after Track A — NOT NULL DEFAULT in DB):
        status: "active".to_string(),
        authority: "human".to_string(),
        affected_paths: "[]".to_string(),
        tags: "[]".to_string(),
        review_after: None,  // only nullable V10 field
        reversibility: "medium".to_string(),
    }
}
```

---

## Verification

```bash
cargo build -p edda-ledger
cargo test -p edda-ledger -- view
cargo clippy -p edda-ledger -- -D warnings
```

---

## Constraints

- **BOUNDARY-01**: After this task, `edda-bridge-claude` and `edda-pack` must use `DecisionView`, never `DecisionRow`
- **BOUNDARY-02**: Only `to_view()` parses `affected_paths` JSON — no other module does it
- **CLIPPY-01**: Zero clippy warnings
- **TEST-01**: All workspace tests pass
