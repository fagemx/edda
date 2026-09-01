---
name: testing
description: Comprehensive testing patterns and anti-patterns for writing and reviewing tests
context: fork
---

# Testing Skill

Use this skill when writing tests, reviewing test code, or investigating test failures.

## Documentation

Read the relevant reference based on context:

| Context | Reference |
|---------|-----------|
| General testing | This document |
| Anti-patterns | [Anti-Patterns](#anti-patterns) section below |
| Patterns | [Patterns](#patterns) section below |
| CLI testing (`edda-cli`) | [CLI Testing](#cli-testing) section below |
| Bridge testing (`edda-bridge-claude`) | [Bridge Testing](#bridge-testing) section below |
| Ledger testing (`edda-ledger`) | [Ledger Testing](#ledger-testing) section below |
| Store testing (`edda-store`) | [Store Testing](#store-testing) section below |

## Key Principles

1. **Integration tests are primary** — test at system entry points (CLI commands, hook dispatch)
2. **Mock at the boundary** — only mock external services, not internal code
3. **Use real infrastructure** — real filesystem (tempdir), real ledger, real coordination log
4. **Test behavior, not implementation** — verify outcomes, not internal function calls

## Test Organization

This project uses **inline tests** exclusively — zero `tests/` directories across all crates.

```
crates/edda-cli/
  src/
    cmd_bridge.rs      # #[cfg(test)] mod tests { ... }
    cmd_gc.rs           # #[cfg(test)] mod tests { ... }

crates/edda-bridge-claude/
  src/
    dispatch.rs         # #[cfg(test)] mod tests { ... } (40+ tests)
    peers.rs            # #[cfg(test)] mod tests { ... } (25+ tests)

crates/edda-ledger/
  src/
    ledger.rs           # #[cfg(test)] mod tests { ... }
```

**Convention:** All tests live inside `#[cfg(test)] mod tests` at the bottom of the source file. Never create separate `tests/` directories.

## Commands

```bash
# Run all tests in workspace
cargo test --workspace

# Run tests for a specific crate
cargo test -p edda-bridge-claude

# Run a specific test by name
cargo test -p edda-bridge-claude -- cross_session_binding

# Run tests matching a pattern
cargo test -- session

# Run tests with output visible
cargo test -- --nocapture

# Run tests with backtrace on failure
RUST_BACKTRACE=1 cargo test -p edda-cli
```

## Setup Pattern

Every test module uses a consistent `setup_workspace()` helper with `AtomicU64` counter for unique temp dirs:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, edda_ledger::Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "edda_test_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = edda_ledger::EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn my_test() {
        let (tmp, ledger) = setup_workspace();
        // ... test logic ...
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

**Key points:**
- `AtomicU64` counter prevents collisions between parallel tests
- `std::process::id()` prevents collisions between test binaries
- Always clean up temp dirs at end of test
- For store-based tests, also clean `edda_store::project_dir(pid)`

## Patterns

### Use `edda_store` for State Tests

```rust
#[test]
fn test_peer_coordination() {
    let pid = "test_unique_name";
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Write coordination events
    crate::peers::write_binding(pid, "s1", "auth", "db.engine", "postgres");

    // Verify via public API
    let conflict = crate::peers::find_binding_conflict(pid, "db.engine", "OTHER");
    assert!(conflict.is_some());

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}
```

### Use `setup_workspace()` for Ledger Tests

```rust
#[test]
fn test_ledger_event() {
    let (tmp, ledger) = setup_workspace();
    let branch = ledger.head_branch().unwrap();
    let parent = ledger.last_event_hash().unwrap();

    let tags = vec!["decision".to_string()];
    let mut event = edda_core::event::new_note_event(
        &branch, parent.as_deref(), "system", "db.engine: postgres", &tags,
    ).unwrap();
    edda_core::event::finalize_event(&mut event);
    ledger.append_event(&event, false).unwrap();

    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);

    let _ = std::fs::remove_dir_all(&tmp);
}
```

### Use Environment Variables for Config Overrides

```rust
#[test]
fn test_with_custom_threshold() {
    // Override config via env var
    std::env::set_var("EDDA_PEER_STALE_SECS", "0");

    // ... test logic ...

    // Always clean up env vars
    std::env::remove_var("EDDA_PEER_STALE_SECS");
}
```

### Test Multi-Session Scenarios

```rust
#[test]
fn test_cross_session() {
    let pid = "test_cross_session";
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Simulate multiple sessions with heartbeats
    let signals = crate::signals::SessionSignals::default();
    crate::peers::write_heartbeat(pid, "s1", &signals, Some("auth"));
    crate::peers::write_heartbeat(pid, "s2", &signals, Some("billing"));

    // Write data from one session
    crate::peers::write_binding(pid, "s1", "auth", "key", "value");

    // Verify other session can see it
    let updates = crate::peers::render_peer_updates(pid, "s2").unwrap();
    assert!(updates.contains("value"));

    crate::peers::remove_heartbeat(pid, "s1");
    crate::peers::remove_heartbeat(pid, "s2");
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}
```

## Anti-Patterns

### AP-1: Don't Create `tests/` Directories

```rust
// BAD: Creating tests/integration_test.rs
// This project uses inline tests exclusively

// GOOD: Add tests inside the source file
#[cfg(test)]
mod tests {
    use super::*;
    // tests here have access to private functions
}
```

### AP-2: Don't Mock Internal Code

```rust
// BAD: Creating trait abstractions just for testing
trait Storage { fn write(&self, key: &str, value: &str); }
struct MockStorage; // Don't do this

// GOOD: Test through the real code path
#[test]
fn test_decide_writes_binding() {
    let (tmp, _ledger) = setup_workspace();
    decide(&tmp, "db.engine=postgres", Some("need JSONB"), None).unwrap();
    // Verify via public API
    let conflict = find_binding_conflict(&pid, "db.engine", "OTHER");
    assert!(conflict.is_some());
}
```

### AP-3: Don't Test Implementation Details

```rust
// BAD: Asserting internal file structure
assert!(fs::read_to_string(path).unwrap().contains("specific_internal_format"));

// GOOD: Test through public API behavior
let result = decide(&tmp, "db.engine=postgres", None, None);
assert!(result.is_ok());
let events = ledger.iter_events().unwrap();
assert_eq!(events.len(), 1);
```

### AP-4: Don't Leak Test State

```rust
// BAD: Using fixed project IDs that collide
let pid = "test"; // Will collide with other tests!

// GOOD: Use unique IDs and clean up
let pid = "test_specific_scenario_name";
let _ = fs::remove_dir_all(edda_store::project_dir(pid)); // Clean first
let _ = edda_store::ensure_dirs(pid);
// ... test ...
let _ = fs::remove_dir_all(edda_store::project_dir(pid)); // Clean after
```

### AP-5: Don't Forget Env Var Cleanup

```rust
// BAD: Setting env var without cleanup
std::env::set_var("EDDA_SESSION_ID", "test");
// test runs... but env var leaks to other tests!

// GOOD: Always remove env vars
std::env::set_var("EDDA_SESSION_ID", "test");
// ... test logic ...
std::env::remove_var("EDDA_SESSION_ID");
```

### AP-6: Don't Use Sleep Without Justification

```rust
// BAD: Arbitrary sleep
std::thread::sleep(Duration::from_secs(5));

// GOOD: Sleep only when testing time-dependent behavior (e.g., stale detection)
// and use the minimum necessary duration
std::thread::sleep(Duration::from_millis(1100)); // Must exceed 1s mtime granularity
std::env::set_var("EDDA_PEER_STALE_SECS", "0");
```

## CLI Testing

Tests for `edda-cli` commands live in each `cmd_*.rs` file.

**Key patterns:**
- Use `setup_workspace()` for commands that need a ledger
- Use `edda_store::ensure_dirs()` for commands that need store state
- Set `EDDA_SESSION_ID` and `EDDA_SESSION_LABEL` env vars for session-aware commands
- Call the command function directly (e.g., `decide()`, `peers()`) — don't spawn a process

**Example crates:**
- `cmd_bridge.rs`: 6 tests covering `decide()`, `find_prior_decision()`, `resolve_session_id()`
- `cmd_gc.rs`: Tests for garbage collection logic
- `cmd_blob.rs`: Tests for blob management
- `cmd_draft.rs`: Tests for draft operations

## Bridge Testing

Tests for `edda-bridge-claude` are the most extensive (~230 tests).

**Key patterns:**
- `dispatch.rs`: Tests hook dispatch flow (`dispatch_session_start`, `dispatch_user_prompt_submit`, `dispatch_session_end`)
- `peers.rs`: Tests coordination protocol (`write_binding`, `write_claim`, `compute_board_state`, `render_peer_updates`)
- Use `crate::peers::write_heartbeat()` to simulate multi-session scenarios
- Use `crate::peers::write_binding()` to set up coordination state

**Access notes:**
- Inline tests can access `pub(crate)` functions (e.g., `compute_board_state`, `write_heartbeat`)
- Cross-crate tests must use `pub` functions only (e.g., `find_binding_conflict`)

## Ledger Testing

Tests for `edda-ledger` cover event creation, appending, querying, and branch operations.

**Key patterns:**
- Always init workspace before testing: `init_workspace()`, `init_head()`, `init_branches_json()`
- Use `Ledger::open()` to get a ledger handle
- Use `ledger.iter_events()` to verify event contents
- Events are immutable once appended — test creation, not modification

## Store Testing

Tests for `edda-store` cover project directory management and atomic writes.

**Key patterns:**
- `project_id()` is deterministic (blake3 hash of normalized path)
- `ensure_dirs()` creates the full directory tree
- `write_atomic()` for safe file writes
- Always clean up `project_dir()` after tests

---

## Checklist for New Tests

Before submitting:
- [ ] Test is inside `#[cfg(test)] mod tests` in the source file
- [ ] Uses unique project ID or temp dir (no collisions)
- [ ] Cleans up temp dirs and store directories
- [ ] Cleans up env vars
- [ ] Tests behavior through public/crate API, not internal details
- [ ] Passes `cargo test --workspace` with zero failures
- [ ] No `#[ignore]` without explanation

## References

- Code quality skill: `.claude/skills/code-quality/SKILL.md`
- Project principles: `.claude/skills/project-principles/SKILL.md`
- CLAUDE.md testing conventions
