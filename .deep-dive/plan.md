# Implementation Plan: Hook Resilience (#83)

## Overview

Three changes, implemented as three small commits:

1. **`catch_unwind` + timeout at hook boundary** (`cmd_bridge.rs`)
2. **Eliminate redundant peer I/O** (`peers.rs` + `dispatch.rs`)
3. **Tests for panic recovery, timeout, and precomputed render paths**

## Step 1: Panic Recovery + Timeout in `cmd_bridge.rs`

**File:** `crates/edda-cli/src/cmd_bridge.rs`

### Change `hook_claude()` (lines 15-52)

Replace the current synchronous call with a thread + channel pattern:

```rust
pub fn hook_claude() -> anyhow::Result<()> {
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("STDIN READ ERROR: {e}"));
        return Ok(());
    }

    debug_log(&format!(
        "STDIN({} bytes): {}",
        stdin_buf.len(),
        &stdin_buf[..stdin_buf.len().min(200)]
    ));

    let timeout_ms: u64 = std::env::var("EDDA_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);

    let (tx, rx) = std::sync::mpsc::channel();
    let stdin = stdin_buf;
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            edda_bridge_claude::hook_entrypoint_from_stdin(&stdin)
        }));
        let _ = tx.send(result);
    });

    let outcome = rx.recv_timeout(std::time::Duration::from_millis(timeout_ms));

    match outcome {
        Ok(Ok(Ok(result))) => {
            // Normal success path (existing logic)
            if let Some(output) = &result.stdout {
                debug_log(&format!("OK output({} bytes)", output.len()));
                print!("{output}");
            }
            if let Some(warning) = &result.stderr {
                debug_log(&format!("WARNING: {warning}"));
                eprintln!("{warning}");
                std::process::exit(1);
            }
            if result.stdout.is_none() && result.stderr.is_none() {
                debug_log("OK (no output)");
            }
            Ok(())
        }
        Ok(Ok(Err(e))) => {
            debug_log(&format!("ERROR: {e}"));
            Ok(()) // exit 0
        }
        Ok(Err(panic_info)) => {
            let msg = panic_info
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_info.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            debug_log(&format!("PANIC: {msg}"));
            Ok(()) // exit 0 — never block host agent
        }
        Err(_) => {
            debug_log(&format!("TIMEOUT after {timeout_ms}ms — graceful exit"));
            Ok(()) // exit 0 — graceful degradation
        }
    }
}
```

Apply the same pattern to `hook_openclaw()` (lines 501-536).

### Acceptance Criteria
- [x] Hook never panics to non-zero exit
- [x] Hook has configurable timeout (`EDDA_HOOK_TIMEOUT_MS`, default 10s)
- [x] Timeout exits 0 with debug log

---

## Step 2: Eliminate Redundant Peer I/O

### 2a. Add precomputed variants in `peers.rs`

**File:** `crates/edda-bridge-claude/src/peers.rs`

Add new functions that accept pre-computed data:

```rust
/// Render peer updates using pre-computed peers and board state.
/// Avoids redundant I/O when caller already has this data.
pub(crate) fn render_peer_updates_with(
    peers: &[PeerSummary],
    board: &BoardState,
    project_id: &str,
    session_id: &str,
) -> Option<String> {
    // ... same logic as render_peer_updates, but uses provided peers/board
}

/// Render full coordination protocol using pre-computed peers and board state.
pub fn render_coordination_protocol_with(
    peers: &[PeerSummary],
    board: &BoardState,
    session_id: &str,
) -> Option<String> {
    // ... same logic as render_coordination_protocol, but uses provided peers/board
}
```

Keep original functions as thin wrappers:

```rust
pub(crate) fn render_peer_updates(project_id: &str, session_id: &str) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);
    render_peer_updates_with(&peers, &board, project_id, session_id)
}

pub fn render_coordination_protocol(
    project_id: &str,
    session_id: &str,
    _cwd: &str,
) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);
    render_coordination_protocol_with(&peers, &board, session_id)
}
```

**Why wrappers:** Preserves existing API for tests, `render.rs:194`, `edda-bridge-openclaw`, and `dispatch.rs:828` (SessionStart path). Only the hot path (`dispatch_with_workspace_only`) uses the `_with` variants.

### 2b. Update `dispatch_with_workspace_only` in `dispatch.rs`

**File:** `crates/edda-bridge-claude/src/dispatch.rs` (lines 319-378)

```rust
fn dispatch_with_workspace_only(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    event_name: &str,
) -> anyhow::Result<HookResult> {
    let workspace_budget: usize = /* unchanged */;
    let mut ws = render_workspace_section(cwd, workspace_budget);

    // Compute peers + board ONCE for the entire dispatch
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    let board = crate::peers::compute_board_state(project_id);

    let prev_count = read_peer_count(project_id, session_id);
    let first_peers = prev_count == 0 && !peers.is_empty();
    write_peer_count(project_id, session_id, peers.len());

    if first_peers {
        if let Some(coord) = crate::peers::render_coordination_protocol_with(&peers, &board, session_id) {
            ws = Some(match ws {
                Some(w) => format!("{w}\n\n{coord}"),
                None => coord,
            });
        }
    } else {
        if let Some(updates) = crate::peers::render_peer_updates_with(&peers, &board, project_id, session_id) {
            ws = Some(match ws {
                Some(w) => format!("{w}\n{updates}"),
                None => updates,
            });
        }
    }

    // ... rest unchanged
}
```

### 2c. Fold `has_active_peers` into peers result

**File:** `crates/edda-bridge-claude/src/dispatch.rs` (line 167)

Currently `has_active_peers()` is called in the main `hook_entrypoint_from_stdin()` before dispatch. Move the peer discovery earlier and reuse:

```rust
// Before: separate has_active_peers call
let peers_active = !session_id.is_empty() && has_active_peers(&project_id, &session_id);

// After: pass down to dispatch functions, or compute later
// has_active_peers is only used for peers_active flag, which is only
// consumed by dispatch_session_end. For non-SessionEnd hooks, skip entirely.
```

Wait — `peers_active` is only used in `SessionEnd` → `cleanup_session_state`. For all other hooks, it's wasted I/O. The simplest fix: move `has_active_peers()` inside `dispatch_session_end()` only.

### I/O reduction summary

| Operation | Before | After |
|-----------|--------|-------|
| `coordination.jsonl` parsed | 3× (UserPromptSubmit) | 1× |
| Heartbeat dir scanned | 3× | 1× |
| Heartbeat JSONs read | 2× | 1× |
| `has_active_peers` dir scan (non-SessionEnd) | 1× | 0× |

### Acceptance Criteria
- [x] `compute_board_state` called at most once per UserPromptSubmit
- [x] `discover_active_peers` called at most once per UserPromptSubmit
- [x] `has_active_peers` not called for non-SessionEnd hooks
- [x] All existing tests pass unchanged (wrapper functions preserve API)

---

## Step 3: Tests

**File:** `crates/edda-cli/src/cmd_bridge.rs` (test module)

### Test: panic recovery

```rust
#[test]
fn hook_panic_exits_gracefully() {
    // Use a subprocess approach: call edda with crafted stdin that triggers
    // a known panic path, verify exit code is 0.
    // Alternative: test catch_unwind directly with a panicking closure.
}
```

**File:** `crates/edda-bridge-claude/src/peers.rs` (test module)

### Test: precomputed render variants

```rust
#[test]
fn render_peer_updates_with_matches_original() {
    // Setup peers + board state
    // Call render_peer_updates(pid, sid)
    // Call render_peer_updates_with(&peers, &board, pid, sid)
    // Assert outputs are identical
}

#[test]
fn render_coordination_protocol_with_matches_original() {
    // Same pattern
}
```

---

## Commit Plan

| # | Commit | Files | Scope |
|---|--------|-------|-------|
| 1 | `fix(hook): add catch_unwind + timeout at hook boundary` | `cmd_bridge.rs` | Panic recovery + configurable timeout |
| 2 | `perf(hook): eliminate redundant peer I/O in UserPromptSubmit` | `peers.rs`, `dispatch.rs` | Pass pre-computed data, move `has_active_peers` |
| 3 | `test(hook): add tests for panic recovery and precomputed renders` | `cmd_bridge.rs`, `peers.rs` | Verify new behavior |

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Thread abandoned on timeout leaks resources | Process exits immediately after — OS reclaims all |
| `catch_unwind` doesn't catch all panics (e.g., `abort` on double panic) | Can't help these — they're extremely rare and indicate deeper bugs |
| Wrapper functions add thin overhead | Negligible — one extra function call per render |
| OpenClaw bridge callers unaffected | Using wrappers preserves existing API |
