# E2: Hook Integration — PreToolUse Decision Warning

**Track**: E (PreToolUse File Warning)
**Layer**: L2 — Product-facing
**Depends on**: E1 (`decision_file_warning()`)

---

## Goal

Wire `decision_file_warning()` (from E1) into the existing `dispatch_pre_tool_use`
function so that editing a file governed by an active decision triggers an
inline warning via `additionalContext`.

## Target File

**Modify**: `crates/edda-bridge-claude/src/dispatch/tools.rs`

## Integration Point

`dispatch_pre_tool_use()` currently combines three optional context sources
(lines 106-113 in `tools.rs`):

```rust
// Combine pattern context, request nudge, and rules warning
let combined_ctx = [pattern_ctx, request_nudge, rules_warning]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
```

The decision warning becomes a **fourth** source in this array.

## Implementation

### Step 1: Add import

At the top of `tools.rs`, add:

```rust
use crate::decision_warning::decision_file_warning;
```

### Step 2: Extract file_path and call decision_file_warning

Insert **after** the `rules_warning` line (line 103) and **before** the
`combined_ctx` construction (line 106):

```rust
// Decision file warning: check if edited file is governed by active decisions
let decision_warning = {
    let tool_name_dw = get_str(raw, "tool_name");
    if tool_name_dw == "Edit" {
        let file_path = raw
            .pointer("/tool_input/file_path")
            .or_else(|| raw.pointer("/input/file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if file_path.is_empty() {
            None
        } else {
            // Resolve ledger path from cwd
            let cwd_path = std::path::Path::new(cwd);
            match edda_ledger::EddaPaths::find_root(cwd_path) {
                Some(root) => {
                    let ledger_path = root.join(".edda").join("ledger.db");
                    let branch = edda_ledger::Ledger::open(&root)
                        .and_then(|l| l.head_branch())
                        .unwrap_or_default();
                    decision_file_warning(&ledger_path, file_path, &branch)
                }
                None => None,
            }
        }
    } else {
        None // Only trigger on Edit tool — not Bash, Read, Write, etc.
    }
};
```

### Step 3: Add to combined context

Update the `combined_ctx` array to include `decision_warning`:

```rust
let combined_ctx = [pattern_ctx, request_nudge, rules_warning, decision_warning]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
```

## Design Decisions

### Why only `Edit`, not `Write`?

- `Edit` is the primary tool for modifying existing files — the use case
  where decision warnings matter most ("you're changing code governed by X")
- `Write` is typically used for new files, where existing decisions are less
  relevant
- Adding `Write` later is trivial (extend the `tool_name_dw` check)

### Why `additionalContext`, not `block`?

Decision warnings are **informational**, not blocking. The agent should know
about governing decisions but can still proceed. This follows the same pattern
as `evaluate_learned_rules()` (L3 rules) and `match_tool_patterns()` — both
use `additionalContext` to inject context without blocking the tool call.

### file_path extraction pattern

The `tool_input/file_path` → `input/file_path` fallback pattern is already
used in the off-limits enforcement (lines 67-71 of `tools.rs`). This task
reuses the same extraction pattern for consistency.

### Ledger/branch resolution

Reuses the same pattern as `session.rs` line 76-82 (`EddaPaths::find_root` →
`Ledger::open` → `head_branch`). The branch is needed to scope decisions to
the current git branch.

## Existing PreToolUse Logic — What NOT to Touch

The following existing logic must remain untouched:

| Logic | Lines | Purpose |
|-------|-------|---------|
| Branch guard | 22-57 | Block git commit on wrong branch |
| Off-limits enforcement | 59-92 | Block Edit/Write on peer-claimed files |
| Auto-approve | 94, 116-127 | Allow tool calls without user confirmation |
| Pattern matching | 97 | Match tool input against Pattern Store |
| Request nudge | 100 | Surface pending coordination requests |
| L3 rules warning | 103 | Evaluate learned rules |

The decision warning slots in **after** these (as another `additionalContext`
source) and **before** the combined context assembly.

## Performance Budget

The decision warning is part of the PreToolUse hot path. Per PERF-01:

```rust
// Add timing instrumentation for debugging:
let dw_start = std::time::Instant::now();
let decision_warning = { /* ... */ };
tracing::debug!("decision_warning_ms={}", dw_start.elapsed().as_millis());
```

Expected: < 5ms with cache hit (E1's `DECISION_CACHE`), < 50ms on cache miss.
Combined with other PreToolUse logic, total must stay < 100ms.

## Tests

Location: `crates/edda-bridge-claude/src/dispatch/tests.rs` (add new test functions)

### Test cases

1. **`test_pretooluse_edit_with_decision_warning`**
   - Set up a ledger with a decision having `affected_paths: ["src/**"]`
   - Send a PreToolUse event for `Edit` tool with `file_path: "src/main.rs"`
   - Assert: `additionalContext` contains `"Active decisions governing this file"`

2. **`test_pretooluse_edit_no_match`**
   - Decision exists but `affected_paths` don't match the edited file
   - Assert: no decision warning in output (other context may still appear)

3. **`test_pretooluse_bash_no_decision_check`**
   - Send a PreToolUse event for `Bash` tool
   - Assert: decision warning logic is not triggered (no ledger query)

4. **`test_pretooluse_edit_no_ledger`**
   - No `.edda/` directory in cwd
   - Assert: `decision_file_warning` returns `None`, no error

## DoD

- [ ] `dispatch_pre_tool_use` calls `decision_file_warning()` on `Edit` tool calls
- [ ] Warning appears in `additionalContext` when decisions match
- [ ] No warning for non-Edit tools (Bash, Read, Write, Glob, Grep)
- [ ] No warning when no decisions match the file path
- [ ] Existing PreToolUse logic (branch guard, off-limits, L3 rules) unchanged
- [ ] Timing instrumented via `tracing::debug!` for PERF-01 verification
- [ ] `cargo test -p edda-bridge-claude` all pass (TEST-01)
- [ ] `cargo clippy -p edda-bridge-claude --all-targets` zero warnings (CLIPPY-01)

## Verification

```bash
cargo build -p edda-bridge-claude
cargo test -p edda-bridge-claude
cargo clippy -p edda-bridge-claude --all-targets

# Manual smoke test:
edda decide "test.guard=on" --reason "test" --paths "crates/edda-ledger/**"
# Start Claude Code session, edit crates/edda-ledger/src/lib.rs
# Expect: hook output contains "[edda] Active decisions governing this file"
```
