# Reference: Task File (TRACK_X/X1_NAME.md)

> This is a real task file from a production project.
> Use this as the quality bar when generating individual task files.
> Note the depth of implementation steps — specific files, specific functions, specific field names.

---

# B1: Commit/Rebuild Event Builders

> **Crate**: `crates/gctx-core/`
> **Layer**: L1
> **Dependencies**: A1（Core Types + Hash）, A2（Ledger Store + Blob Store）, A3（CLI Skeleton）
> **Blocks**: B2（Derive Views Engine）, B3（CLI: status/commit/context/rebuild）

---

## Bootstrap Instructions

```bash
# 1. Read existing code from Track A
cat crates/gctx-core/src/lib.rs
cat crates/gctx-core/src/event.rs
cat crates/gctx-core/src/types.rs

# 2. Verify baseline compiles
cargo check -p gctx-core

# 3. Read specs
cat docs/plan/TRACKS.md          # Track B overview
cat docs/plan/CONTRACT.md        # EVIDENCE-01 rule
```

---

## Final Result

- `crates/gctx-core/src/event.rs` gains `new_commit_event()` and `new_rebuild_event()`
- commit event auto-adds `label=claim` when evidence is empty (CONTRACT EVIDENCE-01)
- rebuild event supports scope="branch"|"all" with reason field
- `cargo check -p gctx-core` zero errors
- `cargo clippy -p gctx-core -- -D warnings` zero warnings

## Implementation Steps

### Step 1: Add `new_commit_event()` builder

- **File**: `crates/gctx-core/src/event.rs`
- **Reference**: spec section 2.5.C (commit payload field definitions)
- **Key changes**:
  1. Add after existing `new_cmd_event()`
  2. Function signature:
     ```rust
     pub fn new_commit_event(
         branch: &str,
         parent_hash: Option<&str>,
         title: &str,
         purpose: Option<&str>,
         prev_summary: &str,
         contribution: &str,
         evidence: Vec<Value>,
         mut labels: Vec<String>,
     ) -> Result<Event>
     ```
  3. payload structure: `title`, `purpose`, `prev_summary`, `contribution`, `evidence`, `labels`
  4. Auto-claim logic: if `evidence.is_empty()` and `labels` doesn't contain "claim", auto `labels.push("claim")`
  5. Uses same pattern as existing builders: build Event → to_value → compute_hash → fill hash

### Step 2: Add `new_rebuild_event()` builder

- **File**: `crates/gctx-core/src/event.rs`
- **Reference**: spec section 2.5.G (rebuild payload)
- **Key changes**:
  1. Add after `new_commit_event()`
  2. Function signature:
     ```rust
     pub fn new_rebuild_event(
         branch: &str,
         parent_hash: Option<&str>,
         scope: &str,              // "branch" | "all"
         target_branch: Option<&str>,
         reason: &str,
     ) -> Result<Event>
     ```
  3. payload: `scope`, `branch` (target), `reason`
  4. type field = `"rebuild"`
  5. refs uses `Refs::default()` (rebuild doesn't reference blobs or events)

### Step 3: Verify compilation and lint

- **File**: no new files
- **Reference**: CONTRACT.md RUST-01 (zero warnings)
- **Key changes**:
  1. Confirm `use serde_json::Value;` is imported at top of event.rs
  2. Confirm all parameters are used (no unused variable warnings)
  3. Run verification commands

## Acceptance Criteria

```bash
# 1. Compiles
cargo check -p gctx-core

# 2. Clippy zero warnings
cargo clippy -p gctx-core -- -D warnings

# 3. Full workspace compiles (doesn't break other crates)
cargo check

# 4. Tests pass
cargo test -p gctx-core

# 5. Verify auto-claim logic exists
grep -n "claim" crates/gctx-core/src/event.rs
# Expected: evidence.is_empty() && !labels.contains("claim") → push "claim"

# 6. Both builder functions are pub exported
grep -n "pub fn new_commit_event" crates/gctx-core/src/event.rs
grep -n "pub fn new_rebuild_event" crates/gctx-core/src/event.rs
```

## Git Commit

```
feat(gctx-core): add commit and rebuild event builders

Add new_commit_event() with auto-claim label logic (EVIDENCE-01)
and new_rebuild_event() with scope/branch/reason payload.
```

---

> **Pattern notes for generation:**
> - Header block: Module path, Layer, Dependencies (task IDs **with names** — e.g. `A1（Core Types + Hash）`), Blocks (task IDs with names)
> - **Bootstrap Instructions**: bash commands the agent runs FIRST to understand context — read existing code, verify baseline
> - **Final Result**: bullet list of what changes — specific files, specific functions, specific behaviors
> - **Implementation Steps**: each step has exactly:
>   - **File**: specific file path
>   - **Reference**: where to look for spec/context
>   - **Key changes**: numbered list with code snippets showing signatures, field names, logic
> - Steps are granular enough that an agent can execute them without guessing
> - **Acceptance Criteria**: runnable bash commands with expected outcomes
> - **Git Commit**: conventional commit message ready to use
> - The task file is self-contained — an agent can complete it without reading other task files
