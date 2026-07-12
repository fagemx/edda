# D1: Add --paths and --tags to `edda decide`

**Track**: D — CLI --paths flag (L2 product-facing)
**Dependencies**: Track B (mutation contract accepts `affected_paths` and `tags` fields)
**Blocks**: None (terminal track)

---

## Goal

Let users specify file scope and tags when recording a decision:

```bash
edda decide "db.engine=sqlite" --reason "embedded" \
  --paths "crates/edda-ledger/**" \
  --paths "crates/edda-core/**" \
  --tags architecture,storage
```

The `--paths` values are glob patterns written to `affected_paths` (JSON array).
The `--tags` value is a comma-separated list written to `tags` (JSON array).

---

## Files to Modify

| File | Line Range | Description |
|------|-----------|-------------|
| `crates/edda-cli/src/main.rs` | 85–100 | Add `--paths` and `--tags` args to `Decide` variant |
| `crates/edda-cli/src/main.rs` | 939–952 | Pass new args to `decide()` call |
| `crates/edda-cli/src/cmd_bridge.rs` | 494–501 | Add `paths` and `tags` params to `decide()` signature |
| `crates/edda-cli/src/cmd_bridge.rs` | 543–548 | Build `DecisionPayload` with new fields |
| `crates/edda-core/src/types.rs` | 119–126 | Add `affected_paths` and `tags` to `DecisionPayload` |

---

## Step 1: Add args to `Decide` command in `main.rs`

Current (line 85–100):
```rust
Decide {
    decision: String,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long = "refs")]
    refs: Vec<String>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long, default_value = "local")]
    scope: String,
},
```

Add after `scope`:
```rust
Decide {
    /// Decision in key=value format (e.g. "db=PostgreSQL")
    decision: String,
    /// Reason for the decision
    #[arg(long)]
    reason: Option<String>,
    /// Decision keys this decision depends on (repeatable)
    #[arg(long = "refs")]
    refs: Vec<String>,
    /// Session ID (auto-inferred from active heartbeats if omitted)
    #[arg(long)]
    session: Option<String>,
    /// Decision scope: local (default), shared, or global
    #[arg(long, default_value = "local")]
    scope: String,
    /// File glob patterns this decision governs (repeatable)
    /// e.g. --paths "crates/edda-ledger/**" --paths "crates/edda-core/**"
    #[arg(long = "paths")]
    paths: Vec<String>,
    /// Comma-separated tags for this decision
    /// e.g. --tags architecture,storage
    #[arg(long, value_delimiter = ',')]
    tags: Vec<String>,
},
```

**Design notes**:
- `--paths` uses `Vec<String>` with repeatable `--paths` (each invocation adds one glob)
- `--tags` uses `value_delimiter = ','` so `--tags a,b,c` becomes `vec!["a", "b", "c"]`
- Both default to empty vec when omitted

---

## Step 2: Pass args in the match arm (line 939–952)

Current:
```rust
Command::Decide {
    decision,
    reason,
    refs,
    session,
    scope,
} => cmd_bridge::decide(
    &repo_root,
    &decision,
    reason.as_deref(),
    &refs,
    session.as_deref(),
    Some(&scope),
),
```

Updated:
```rust
Command::Decide {
    decision,
    reason,
    refs,
    session,
    scope,
    paths,
    tags,
} => cmd_bridge::decide(
    &repo_root,
    &decision,
    reason.as_deref(),
    &refs,
    session.as_deref(),
    Some(&scope),
    &paths,
    &tags,
),
```

---

## Step 3: Update `DecisionPayload` in `crates/edda-core/src/types.rs`

Current (line 119–126):
```rust
pub struct DecisionPayload {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<DecisionScope>,
}
```

Updated:
```rust
pub struct DecisionPayload {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<DecisionScope>,
    /// File glob patterns this decision governs.
    /// Stored as JSON array: `["crates/edda-ledger/**"]`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_paths: Option<Vec<String>>,
    /// Classification tags.
    /// Stored as JSON array: `["architecture", "storage"]`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}
```

**Backward compatibility**: Both fields use `Option` + `#[serde(default)]` so existing callers that construct `DecisionPayload` without these fields pass `None`. Existing serialized events without these fields will deserialize correctly as `None`. This matches B2's `DecisionPayload` definition.

**Check all existing construction sites**: Search for `DecisionPayload {` across the workspace and add the new fields (defaulting to `vec![]`). Key locations:
- `crates/edda-cli/src/cmd_bridge.rs` (line 543) — updated in Step 4
- `crates/edda-chronicle/src/extract.rs` — agent extraction, add `affected_paths: None, tags: None`
- Any test helpers — add defaults

---

## Step 4: Update `decide()` in `crates/edda-cli/src/cmd_bridge.rs`

### Update signature (line 494–501)

Current:
```rust
pub fn decide(
    repo_root: &Path,
    decision: &str,
    reason: Option<&str>,
    refs: &[String],
    cli_session: Option<&str>,
    scope_str: Option<&str>,
) -> anyhow::Result<()> {
```

Updated:
```rust
pub fn decide(
    repo_root: &Path,
    decision: &str,
    reason: Option<&str>,
    refs: &[String],
    cli_session: Option<&str>,
    scope_str: Option<&str>,
    paths: &[String],
    tags: &[String],
) -> anyhow::Result<()> {
```

### Update DecisionPayload construction (line 543–548)

Current:
```rust
let dp = edda_core::types::DecisionPayload {
    key: key.to_string(),
    value: value.to_string(),
    reason: reason.map(|r| r.to_string()),
    scope,
};
```

Updated:
```rust
let dp = edda_core::types::DecisionPayload {
    key: key.to_string(),
    value: value.to_string(),
    reason: reason.map(|r| r.to_string()),
    scope,
    affected_paths: if paths.is_empty() { None } else { Some(paths.to_vec()) },
    tags: if tags.is_empty() { None } else { Some(tags.to_vec()) },
};
```

### Update output (after line 605)

Add printing for paths and tags:

```rust
if !paths.is_empty() {
    println!("  paths: {}", paths.join(", "));
}
if !tags.is_empty() {
    println!("  tags: {}", tags.join(", "));
}
```

---

## Step 5: Ensure affected_paths and tags reach SQLite

The `affected_paths` and `tags` fields flow through the event payload:
1. `DecisionPayload` is serialized into the event's `payload` JSON by `new_decision_event()`
2. `append_event()` in `sqlite_store.rs` writes the event to the `events` table
3. The `INSERT INTO decisions` SQL (line 973) extracts fields from the payload

After Track A (Schema V10) and Track B (mutation contract), the `INSERT INTO decisions` statement will include `affected_paths` and `tags` columns. The values come from the serialized `DecisionPayload` in the event payload.

**Verification**: After inserting via CLI, check SQLite directly:
```sql
SELECT affected_paths, tags FROM decisions WHERE key = 'test.key';
-- Expected: '["crates/foo/**"]', '["architecture"]'
```

---

## Step 6: Update other callers of `decide()`

Search for all call sites of `cmd_bridge::decide`:

```bash
grep -rn "cmd_bridge::decide\|bridge::decide" crates/ --include="*.rs"
```

The `bridge claude decide` subcommand (around line 646 of `main.rs`) also calls `decide()`. It needs the same `paths` and `tags` args added to its `Decide` variant and passed through.

Check the bridge subcommand's `Decide` variant (line 646) and update it similarly.

---

## Tests

### CLI integration test

Add to `crates/edda-cli/tests/` or inline in `cmd_bridge.rs`:

```rust
#[test]
fn decide_with_paths_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize ledger in temp dir
    // ...

    cmd_bridge::decide(
        dir.path(),
        "db.engine=sqlite",
        Some("embedded"),
        &[],
        None,
        Some("local"),
        &["crates/edda-ledger/**".to_string(), "crates/edda-core/**".to_string()],
        &["architecture".to_string(), "storage".to_string()],
    ).unwrap();

    // Verify in SQLite
    let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();
    let row = ledger.find_active_decision("main", "db.engine").unwrap().unwrap();
    // After V10: check row.affected_paths and row.tags
    // The payload in the event should contain the paths and tags
}
```

### Test: omitted paths/tags default to empty

```rust
#[test]
fn decide_without_paths_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    // ...

    cmd_bridge::decide(
        dir.path(),
        "db.engine=sqlite",
        Some("embedded"),
        &[],
        None,
        Some("local"),
        &[],  // no paths
        &[],  // no tags
    ).unwrap();

    // Verify: affected_paths = "[]", tags = "[]" in SQLite
}
```

### Test: clap parsing

```rust
#[test]
fn clap_parses_paths_repeatable() {
    let args = vec![
        "edda", "decide", "db.engine=sqlite",
        "--reason", "test",
        "--paths", "crates/foo/**",
        "--paths", "crates/bar/**",
        "--tags", "arch,storage",
    ];
    let cli = Cli::try_parse_from(args).unwrap();
    // Verify paths = ["crates/foo/**", "crates/bar/**"]
    // Verify tags = ["arch", "storage"]
}
```

---

## Verification

```bash
# Build
cargo build -p edda-cli

# Unit tests
cargo test -p edda-cli

# Manual smoke test
cargo run -- decide "test.paths=yes" --reason "test" \
  --paths "crates/foo/**" --paths "crates/bar/**" \
  --tags arch,storage

# Verify in SQLite (after Track A+B are done)
sqlite3 .edda/ledger.db "SELECT affected_paths, tags FROM decisions WHERE key='test.paths';"
# Expected: '["crates/foo/**","crates/bar/**"]', '["arch","storage"]'

# Clippy
cargo clippy --workspace -- -D warnings
```

---

## Constraints

- **COMPAT-02**: Existing `edda decide` without `--paths`/`--tags` must work unchanged
- **CLIPPY-01**: Zero clippy warnings
- **TEST-01**: All workspace tests pass — ensure all `DecisionPayload` construction sites compile with new fields
