# Innovation: Hook Resilience (#83)

## Approach Overview

Three improvements, ordered by ROI:

| # | Improvement | Complexity | Impact | Risk |
|---|------------|------------|--------|------|
| 1 | `catch_unwind` at hook entrypoint | Low | High — prevents all panics from blocking Claude Code | Near zero — strictly defensive |
| 2 | Eliminate redundant peer I/O | Medium | High — 3-4× fewer disk reads per hook in multi-agent | Low — internal refactor, same external behavior |
| 3 | Hook-level timeout | Medium | Medium — safety net for edge cases | Low — only kills own process |

---

## 1. Panic Recovery: `catch_unwind`

### Option A: Wrap in `cmd_bridge.rs` (CLI layer) ✅ Recommended

```rust
pub fn hook_claude() -> anyhow::Result<()> {
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("STDIN READ ERROR: {e}"));
        return Ok(());
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        edda_bridge_claude::hook_entrypoint_from_stdin(&stdin_buf)
    }));

    match result {
        Ok(Ok(hook_result)) => { /* existing dispatch logic */ }
        Ok(Err(e)) => {
            debug_log(&format!("ERROR: {e}"));
            Ok(()) // exit 0
        }
        Err(panic_info) => {
            let msg = panic_info
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_info.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            debug_log(&format!("PANIC: {msg}"));
            Ok(()) // exit 0 — never block host agent
        }
    }
}
```

**Why CLI layer, not library layer:**
- The library (`edda-bridge-claude`) can propagate errors normally via `Result`. Panics are exceptional.
- `catch_unwind` at the outermost boundary catches everything, including panics in dependencies.
- The `debug_log` function is already in `cmd_bridge.rs`, making logging natural.

### Option B: Wrap in `dispatch.rs` (library layer)

Could add `catch_unwind` inside `hook_entrypoint_from_stdin`. But this would hide panics from non-hook callers (e.g., tests), and the library already uses `Result` for normal error handling. **Not recommended** — defense belongs at the boundary.

**Decision: Option A** — wrap at the CLI boundary in `cmd_bridge.rs`.

---

## 2. Eliminate Redundant Peer I/O

### Current Problem (from research)

A single `UserPromptSubmit` calls:
1. `has_active_peers()` → scan heartbeat dir
2. `discover_active_peers()` → scan dir + read JSONs + parse `coordination.jsonl`
3. `render_peer_updates()` → calls BOTH `discover_active_peers()` and `compute_board_state()` internally

That's 3× dir scan, 2× JSON reads, 3× coordination.jsonl parse.

### Option A: Pass pre-computed data down ✅ Recommended

Refactor `render_peer_updates()` and `render_coordination_protocol()` to accept pre-computed `peers: &[PeerSummary]` and `board: &BoardState` as parameters instead of computing them internally.

```rust
// Before
pub(crate) fn render_peer_updates(project_id: &str, session_id: &str) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id); // ❌ redundant
    let board = compute_board_state(project_id);                // ❌ redundant
    // ...
}

// After
pub(crate) fn render_peer_updates(
    peers: &[PeerSummary],
    board: &BoardState,
) -> Option<String> {
    // ... uses provided data directly
}
```

Caller (`dispatch_with_workspace_only`) already has `peers` from line 335. Just compute `board` once and pass both down.

**Pros:**
- Zero allocation overhead, no caching mechanism to maintain
- Caller controls data lifecycle — easy to reason about
- Breaking change is internal only (pub(crate) functions)

**Cons:**
- Signature changes to several functions

### Option B: Thread-local / process-scoped cache

Use `std::cell::RefCell<Option<(Instant, BoardState)>>` as a thread-local cache with a TTL. `compute_board_state()` checks if cache is fresh before re-reading.

**Pros:** No API changes.
**Cons:** Hidden state, harder to reason about, cache invalidation bugs. The hook is a short-lived process so the cache would only help within a single invocation — at which point passing data down is simpler.

### Option C: File mtime check

Before parsing `coordination.jsonl`, check its mtime. If unchanged since last read, return cached result.

**Pros:** Works across invocations.
**Cons:** Hook is short-lived (single invocation, ~50ms), so cross-invocation cache has no benefit. Same-invocation redundancy is the real issue.

**Decision: Option A** — pass pre-computed data down. Simplest, most explicit, zero hidden state.

### Also: fold `has_active_peers()` into `discover_active_peers()`

`has_active_peers()` (dispatch.rs:86) scans the same heartbeat directory that `discover_active_peers()` will scan moments later. Instead, compute peers once and derive `has_active_peers` from `!peers.is_empty()`.

This eliminates one full directory scan.

---

## 3. Hook-Level Timeout

### Option A: Thread + channel with timeout ✅ Recommended

```rust
pub fn hook_claude() -> anyhow::Result<()> {
    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf)?;

    let timeout_ms: u64 = std::env::var("EDDA_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);

    let (tx, rx) = std::sync::mpsc::channel();
    let stdin = stdin_buf.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            edda_bridge_claude::hook_entrypoint_from_stdin(&stdin)
        }));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
        Ok(Ok(Ok(hook_result))) => { /* normal dispatch */ }
        Ok(Ok(Err(e))) => { debug_log(&format!("ERROR: {e}")); }
        Ok(Err(panic_info)) => { debug_log("PANIC"); }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            debug_log(&format!("TIMEOUT after {timeout_ms}ms"));
            // Exit 0 — graceful degradation
        }
        Err(_) => { debug_log("CHANNEL ERROR"); }
    }
    Ok(())
}
```

**Key insight:** This naturally combines with `catch_unwind` from improvement #1. The spawned thread catches panics AND the main thread enforces a timeout. Two birds, one stone.

**Pros:**
- Clean separation: worker thread does the work, main thread enforces the deadline
- No external dependencies (just std::thread + mpsc)
- Configurable via `EDDA_HOOK_TIMEOUT_MS`
- Combines panic recovery and timeout in one pattern

**Cons:**
- Worker thread is abandoned on timeout (resources leak until process exits — acceptable since we exit immediately)
- Slightly more complex than no-timeout

### Option B: `tokio::time::timeout` (async)

Use async runtime with timeout. **Not recommended** — adding a runtime dependency for a 50ms synchronous hook is overkill.

### Option C: SIGALRM / signal-based timeout (Unix only)

Use `alarm()` to set a process-level timeout. **Not recommended** — not portable to Windows, and signal handlers are error-prone.

**Decision: Option A** — thread + channel. Combines naturally with panic recovery.

---

## Combined Architecture

The three improvements compose into a single clean pattern in `cmd_bridge.rs`:

```
hook_claude()
├── Read stdin
├── Spawn worker thread {
│   ├── catch_unwind {
│   │   └── hook_entrypoint_from_stdin()  ← existing logic
│   │       └── dispatch_with_workspace_only()
│   │           ├── compute board + peers once
│   │           └── pass to render_* functions
│   }
│   └── Send result via channel
├── recv_timeout(EDDA_HOOK_TIMEOUT_MS)
└── Match: Ok(result) | Panic | Timeout | Error → always exit 0
```

## Test Strategy

| Test | What it verifies |
|------|-----------------|
| `hook_panic_recovery_exits_zero` | Inject a panicking hook, verify process exits 0 |
| `hook_timeout_exits_zero` | Inject a sleeping hook, verify timeout triggers |
| `render_peer_updates_with_precomputed_data` | Verify render functions work with passed-in data |
| `no_redundant_board_computation` | Verify `compute_board_state` called only once per dispatch (harder to test directly — rely on integration tests + timing) |
