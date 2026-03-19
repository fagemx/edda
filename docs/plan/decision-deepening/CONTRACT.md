# Decision Deepening — Architecture Constraints

> These rules cannot be violated during development.
> Any task that violates these rules is considered incomplete.

## Rules

| Rule ID | Description | Verification | Affected Tracks |
|---------|------------|--------------|-----------------|
| COMPAT-01 | `is_active` must always agree with `status` | `SELECT * FROM decisions WHERE (is_active=1 AND status NOT IN ('active','experimental')) OR (is_active=0 AND status IN ('active','experimental'));` → 0 rows | A, B |
| COMPAT-02 | Existing `WHERE is_active = TRUE` queries must continue working | `cargo test --workspace` passes without modifying any existing query | A |
| MUTATION-01 | No direct `UPDATE decisions SET status = ...` outside mutation contract | `grep -rn "UPDATE decisions" crates/ --include="*.rs"` → only in sqlite_store.rs mutation functions | B |
| BOUNDARY-01 | Injection (edda-pack, edda-bridge-claude hooks) never imports `DecisionRow` | `grep -rn "DecisionRow" crates/edda-pack/ crates/edda-bridge-claude/` → 0 results after Track C | C, E, F |
| BOUNDARY-02 | Injection reads through `to_view()`, never parses `affected_paths` JSON directly | Code review of E1, E2, F1, F2 | C, E, F |
| PERF-01 | PreToolUse hook total time < 100ms | Measure with `std::time::Instant` in dispatch_pre_tool_use | E |
| CLIPPY-01 | `cargo clippy --workspace --all-targets` zero warnings | CI check | All |
| TEST-01 | `cargo test --workspace` zero failures | CI check | All |

---

## Detailed Rules

### COMPAT-01: is_active ↔ status Invariant

**Description**: The `is_active` boolean and `status` text columns must always agree. `is_active = TRUE` iff `status IN ('active', 'experimental')`.

**Rationale**: Existing code uses `WHERE is_active = TRUE` extensively. If `status` and `is_active` diverge, queries return wrong results silently.

**Verification**:
```sql
-- Run against any ledger.db after mutation
SELECT COUNT(*) FROM decisions
WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
   OR (is_active = 0 AND status IN ('active', 'experimental'));
-- Expected: 0
```

**Consequence of violation**: Active decisions invisible to existing queries, or superseded decisions appearing as active.

---

### MUTATION-01: Mutation Contract is the Only Write Path

**Description**: All decision status changes go through the mutation contract functions in `sqlite_store.rs`. No crate writes `UPDATE decisions SET status = ...` inline.

**Rationale**: The `is_active ↔ status` invariant (COMPAT-01) is enforced by the mutation contract. Bypassing it breaks the invariant.

**Verification**:
```bash
grep -rn "UPDATE decisions" crates/ --include="*.rs" | grep -v sqlite_store.rs
# Expected: 0 results
```

**Consequence of violation**: `is_active` and `status` silently diverge.

---

### BOUNDARY-01: Injection Never Touches DecisionRow

**Description**: After Track C introduces `DecisionView` and `to_view()`, all read-side consumers (hooks, pack builder) must use `DecisionView`. They never import `DecisionRow` directly.

**Rationale**: `DecisionRow` is a storage type. If read-side code depends on it, schema changes break the delivery layer.

**Verification**:
```bash
grep -rn "DecisionRow" crates/edda-pack/ crates/edda-bridge-claude/
# Expected: 0 results (after Track C is complete)
```

**Consequence of violation**: Schema changes in `sqlite_store.rs` cascade into hook code and pack generation.

---

### PERF-01: PreToolUse < 100ms

**Description**: The decision file warning in `dispatch_pre_tool_use` must complete within 100ms. This includes querying active decisions, parsing `affected_paths`, and glob matching.

**Rationale**: PreToolUse hooks run on every tool call. > 100ms adds perceptible lag to every edit operation.

**Verification**:
```rust
// In dispatch_pre_tool_use:
let start = std::time::Instant::now();
// ... decision warning logic ...
debug!("decision_warning_ms={}", start.elapsed().as_millis());
```

**Consequence of violation**: User-perceptible lag on every file edit.
