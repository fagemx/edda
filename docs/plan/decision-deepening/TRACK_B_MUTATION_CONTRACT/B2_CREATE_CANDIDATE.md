# B2: Extend DecisionPayload with New Fields

## Bootstrap Instructions

```bash
git checkout track-b/mutation-contract  # B1 must be complete
cargo build --workspace                 # baseline — must pass
cargo test  --workspace                 # baseline — record existing pass count
```

**Prerequisite**: B1 (Status Sync) must be complete. `status_to_is_active()`
must exist, and `append_event()` must already write `status`, `authority`,
`affected_paths`, `tags`, `review_after`, `reversibility` (with hardcoded
defaults). This task makes those values dynamic via `DecisionPayload`.

## Final Result

`DecisionPayload` in `edda-core` gains 5 optional fields. The decision
materialization in `append_event()` reads these fields from the payload
instead of using hardcoded defaults. All existing callers that construct
`DecisionPayload` continue to compile unchanged (fields are `Option` with
`#[serde(default)]`).

## Implementation Steps

### Step 1 — Extend `DecisionPayload` struct

**File**: `crates/edda-core/src/types.rs`
**Reference**: Lines 118-126 (`DecisionPayload` struct)
**Key changes**:

Add 5 new optional fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionPayload {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<DecisionScope>,
    // ↓ new fields (Decision Deepening)
    /// Decision authority: "human", "agent", "system". Default: "human".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// Glob patterns for guarded file paths. Default: [].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_paths: Option<Vec<String>>,
    /// Categorization tags. Default: [].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// ISO-8601 date for scheduled re-evaluation. Default: None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_after: Option<String>,
    /// Reversibility level: "easy", "medium", "hard". Default: "medium".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversibility: Option<String>,
}
```

**Important**: All new fields use `Option` + `#[serde(default)]` so that:
1. Existing `DecisionPayload { key, value, reason, scope }` constructions
   compile (Rust struct update syntax or explicit `None`).
2. Existing JSON payloads without these fields deserialize correctly.

### Step 2 — Update all `DecisionPayload` construction sites

**File**: Multiple files across the workspace
**Key changes**:

Search for all places that construct `DecisionPayload`:

```bash
grep -rn "DecisionPayload {" crates/ --include="*.rs"
```

Each construction site must include the 5 new fields. Since all are `Option`,
add `..Default::default()` or explicit `None` values. Example:

**In `crates/edda-cli/src/cmd_bridge.rs`** (lines 543-548):

```rust
// Before:
let dp = edda_core::types::DecisionPayload {
    key: key.to_string(),
    value: value.to_string(),
    reason: reason.map(|r| r.to_string()),
    scope,
};

// After (Option A — explicit None):
let dp = edda_core::types::DecisionPayload {
    key: key.to_string(),
    value: value.to_string(),
    reason: reason.map(|r| r.to_string()),
    scope,
    authority: None,
    affected_paths: None,
    tags: None,
    review_after: None,
    reversibility: None,
};
```

**Note**: Do NOT change the values being passed — all new fields should be
`None` at existing call sites. Track D will later wire `--paths` and `--tags`
CLI args into these fields.

### Step 3 — Read payload fields in decision materialization

**File**: `crates/edda-ledger/src/sqlite_store.rs`
**Reference**: The decision materialization block in `append_event()` (modified in B1)
**Key changes**:

Replace the hardcoded defaults with values from `DecisionPayload`:

```rust
// In the decision materialization block, after extracting dp:
let authority = dp.authority.as_deref().unwrap_or("human");
let affected_paths = dp.affected_paths.as_ref()
    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
    .unwrap_or_else(|| "[]".to_string());
let tags = dp.tags.as_ref()
    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
    .unwrap_or_else(|| "[]".to_string());
let review_after = dp.review_after.as_deref();
let reversibility = dp.reversibility.as_deref().unwrap_or("medium");

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
        authority,
        affected_paths,
        tags,
        review_after,
        reversibility,
    ],
)?;
```

**Note**: `affected_paths` and `tags` are `Vec<String>` in `DecisionPayload`
but stored as JSON strings in SQLite. Use `serde_json::to_string()` for
serialization.

Ensure `serde_json` is in `edda-ledger`'s `Cargo.toml` dependencies (it likely
already is — verify with `grep serde_json crates/edda-ledger/Cargo.toml`).

### Step 4 — Add test for new fields round-trip

**File**: `crates/edda-ledger/src/sqlite_store.rs` (in `#[cfg(test)]` module)
**Key changes**:

```rust
#[test]
fn test_decision_payload_new_fields_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    let dp = edda_core::types::DecisionPayload {
        key: "db.engine".to_string(),
        value: "sqlite".to_string(),
        reason: Some("embedded".to_string()),
        scope: None,
        authority: Some("human".to_string()),
        affected_paths: Some(vec![
            "crates/edda-ledger/**".to_string(),
            "crates/edda-store/**".to_string(),
        ]),
        tags: Some(vec!["architecture".to_string(), "storage".to_string()]),
        review_after: Some("2026-06-01".to_string()),
        reversibility: Some("hard".to_string()),
    };
    let event = edda_core::event::new_decision_event(
        "main", None, "system", &dp,
    ).unwrap();
    store.append_event(&event).unwrap();

    // Verify all fields stored correctly
    let row = store.conn.query_row(
        "SELECT authority, affected_paths, tags, review_after, reversibility
         FROM decisions WHERE key = 'db.engine'",
        [],
        |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
        )),
    ).unwrap();

    assert_eq!(row.0, "human");
    assert_eq!(row.1, r#"["crates/edda-ledger/**","crates/edda-store/**"]"#);
    assert_eq!(row.2, r#"["architecture","storage"]"#);
    assert_eq!(row.3.as_deref(), Some("2026-06-01"));
    assert_eq!(row.4, "hard");
}

#[test]
fn test_decision_payload_defaults_when_none() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    // All new fields are None — should use defaults
    let dp = edda_core::types::DecisionPayload {
        key: "default.test".to_string(),
        value: "val".to_string(),
        reason: None,
        scope: None,
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
    };
    let event = edda_core::event::new_decision_event(
        "main", None, "system", &dp,
    ).unwrap();
    store.append_event(&event).unwrap();

    let row = store.conn.query_row(
        "SELECT authority, affected_paths, tags, review_after, reversibility
         FROM decisions WHERE key = 'default.test'",
        [],
        |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
        )),
    ).unwrap();

    assert_eq!(row.0, "human");       // authority default
    assert_eq!(row.1, "[]");           // affected_paths default
    assert_eq!(row.2, "[]");           // tags default
    assert_eq!(row.3, None);           // review_after default
    assert_eq!(row.4, "medium");       // reversibility default
}
```

### Step 5 — Add serde round-trip test for DecisionPayload

**File**: `crates/edda-core/src/types.rs` (in `#[cfg(test)]` module, or create one)
**Key changes**:

```rust
#[test]
fn test_decision_payload_serde_backward_compat() {
    // Old JSON without new fields — should deserialize with defaults
    let json = r#"{"key":"db.engine","value":"sqlite","reason":"embedded"}"#;
    let dp: DecisionPayload = serde_json::from_str(json).unwrap();
    assert_eq!(dp.key, "db.engine");
    assert_eq!(dp.authority, None);
    assert_eq!(dp.affected_paths, None);
    assert_eq!(dp.tags, None);
    assert_eq!(dp.review_after, None);
    assert_eq!(dp.reversibility, None);
}

#[test]
fn test_decision_payload_serde_with_new_fields() {
    let json = r#"{
        "key": "db.engine",
        "value": "sqlite",
        "reason": "embedded",
        "authority": "agent",
        "affected_paths": ["crates/edda-ledger/**"],
        "tags": ["arch"],
        "review_after": "2026-06-01",
        "reversibility": "hard"
    }"#;
    let dp: DecisionPayload = serde_json::from_str(json).unwrap();
    assert_eq!(dp.authority.as_deref(), Some("agent"));
    assert_eq!(dp.affected_paths, Some(vec!["crates/edda-ledger/**".to_string()]));
    assert_eq!(dp.tags, Some(vec!["arch".to_string()]));
    assert_eq!(dp.review_after.as_deref(), Some("2026-06-01"));
    assert_eq!(dp.reversibility.as_deref(), Some("hard"));

    // Round-trip: serialize and deserialize
    let serialized = serde_json::to_string(&dp).unwrap();
    let dp2: DecisionPayload = serde_json::from_str(&serialized).unwrap();
    assert_eq!(dp, dp2);
}
```

## Acceptance Criteria

- [ ] `cargo build --workspace` — zero errors
- [ ] `cargo test --workspace` — all tests pass (no regressions in any crate)
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
- [ ] `DecisionPayload` has 5 new `Option` fields
- [ ] All existing `DecisionPayload` construction sites compile without changes
  to their passed values (just add explicit `None` or struct update syntax)
- [ ] `append_event()` reads `authority`, `affected_paths`, `tags`,
  `review_after`, `reversibility` from `DecisionPayload` and writes to DB
- [ ] When all new fields are `None`, defaults are applied:
  `authority="human"`, `affected_paths="[]"`, `tags="[]"`,
  `review_after=NULL`, `reversibility="medium"`
- [ ] When new fields are `Some(...)`, the provided values are written
- [ ] Old JSON payloads (without new fields) deserialize correctly
- [ ] COMPAT-01 invariant holds after insert with new fields

## Git Commit

```
feat(core,ledger): extend DecisionPayload with authority/paths/tags (B2)

Add 5 optional fields to DecisionPayload: authority, affected_paths,
tags, review_after, reversibility. Decision materialization in
append_event() now reads these from the payload instead of hardcoded
defaults. All existing callers pass None and get safe defaults.

Contract: COMPAT-01
Refs: GH-decision-deepening Track B2
```
