# Research: Hook Resilience — Timeout, Panic Recovery, Peer Cache (#83)

## Problem Statement

`UserPromptSubmit` hook occasionally errors in multi-agent environments. Three root causes identified: no panic recovery, no hook timeout, and expensive uncached peer discovery.

## Architecture Analysis

### Hook Execution Flow

```
Claude Code → stdin JSON → edda hook claude (cmd_bridge.rs:15)
  → hook_entrypoint_from_stdin (dispatch.rs:127)
    → parse stdin, resolve project_id
    → append to session ledger
    → detect active peers (dispatch.rs:167)  ← I/O: read_dir + stat heartbeat files
    → touch heartbeat (dispatch.rs:171)      ← I/O: write JSON file
    → dispatch by hook_event_name:
        UserPromptSubmit → dispatch_user_prompt_submit (dispatch.rs:531)
          → dispatch_with_workspace_only (dispatch.rs:319)
            → discover_active_peers()         ← I/O: read_dir + read JSONs + parse coordination.jsonl
            → render_peer_updates()           ← I/O: discover_active_peers() + compute_board_state() AGAIN
```

### Issue 1: No Panic Recovery

**Location:** `cmd_bridge.rs:28` → `dispatch.rs:127`

The CLI wrapper `hook_claude()` calls `hook_entrypoint_from_stdin()` directly. If any code inside panics (e.g., unwrap on corrupt JSON, index out of bounds), the process crashes with non-zero exit code. Claude Code surfaces this as a "hook error" to the user.

Current error handling only catches `anyhow::Error` via the `Err(e)` arm in `cmd_bridge.rs:46-50`, which exits 0. But a Rust panic is NOT caught by `?` or `match` — it unwinds the stack and exits the process.

**Risk areas for panics:**
- `parse_rfc3339_to_epoch()` — parsing time strings from heartbeat files
- `serde_json::from_str()` — parsing heartbeat JSON from disk (could be partially written)
- `.unwrap_or("")` chains — generally safe, but some paths have bare `unwrap()`
- File I/O on corrupted/locked state files

### Issue 2: No Hook Timeout

**Location:** `cmd_bridge.rs:15-52` — the entire `hook_claude()` function runs synchronously.

If any operation hangs (e.g., `discover_active_peers()` scanning thousands of stale heartbeat files, `compute_board_state()` reading a huge coordination.jsonl, or `detect_git_branch()` calling `git rev-parse` in a broken repo), the hook blocks Claude Code indefinitely.

The workspace lock in `digest.rs:530-549` has a timeout (`EDDA_BRIDGE_LOCK_TIMEOUT_MS`, default 2s), but the hook as a whole has no timeout. If the hang occurs outside the lock retry loop (e.g., in `read_dir`, `read_to_string`, or `git` subprocess), there's no safety net.

### Issue 3: Uncached Peer Discovery

**Location:** `peers.rs` + `dispatch.rs`

In a single `UserPromptSubmit` hook execution, the following I/O operations occur:

| Call site | Function | I/O |
|-----------|----------|-----|
| `dispatch.rs:167` | `has_active_peers()` | `read_dir` + `stat` each heartbeat file |
| `dispatch.rs:335` | `discover_active_peers()` | `read_dir` + read/parse each heartbeat JSON + `compute_board_state()` (read+parse coordination.jsonl) |
| `dispatch.rs:351` | `render_peer_updates()` | calls `discover_active_peers()` AGAIN + `compute_board_state()` AGAIN |

**Total redundant work per UserPromptSubmit:**
- `coordination.jsonl` parsed: **3 times** (once in each `discover_active_peers` + once more in `render_peer_updates`)
- Heartbeat directory scanned: **3 times** (`has_active_peers` + 2× `discover_active_peers`)
- Heartbeat JSON files read+parsed: **2 times** (2× `discover_active_peers`)

For `render_coordination_protocol()` (first_peers path), it's even worse:
- `coordination.jsonl` parsed: **4 times**
- Heartbeat directory scanned: **4 times**

With 8 active peers and a growing coordination.jsonl, this is a significant amount of redundant disk I/O.

### Additional Finding: `detect_git_branch()` Subprocess

`peers.rs:39-48` spawns `git rev-parse --abbrev-ref HEAD` every time `write_heartbeat()` is called. This is a subprocess spawn on every hook invocation. In pathological cases (corrupted `.git`, NFS mount, Windows virus scanner), this can hang.

## Call Graph (UserPromptSubmit hot path)

```
dispatch_user_prompt_submit()
├── dispatch_with_workspace_only()
│   ├── render_workspace_section()           // reads .edda/ ledger
│   ├── discover_active_peers()              // ❌ read_dir + N×read JSON + parse coord.jsonl
│   ├── read_peer_count() / write_peer_count()  // 2× file I/O
│   └── [if first_peers]
│   │   └── render_coordination_protocol()
│   │       ├── discover_active_peers()      // ❌ REDUNDANT
│   │       └── compute_board_state()        // ❌ REDUNDANT
│   └── [else]
│       └── render_peer_updates()
│           ├── discover_active_peers()      // ❌ REDUNDANT
│           └── compute_board_state()        // ❌ REDUNDANT
```

## Existing Safeguards

| Safeguard | Location | Status |
|-----------|----------|--------|
| SQLite WAL mode | `sqlite_store.rs:11` | ✅ |
| SQLite busy_timeout 5s | `sqlite_store.rs:110` | ✅ |
| Workspace lock timeout 2s | `dispatch.rs:688`, `digest.rs:530-549` | ✅ |
| Exit 0 on `Err()` | `cmd_bridge.rs:46-50` | ✅ |
| Exit 0 on stdin read error | `cmd_bridge.rs:17-20` | ✅ |
| Hook panic handling | — | ❌ None |
| Hook-level timeout | — | ❌ None |
| Peer discovery cache | — | ❌ None |

## Impact Assessment

- **Panic recovery**: Low frequency, high impact. A single corrupt heartbeat file can crash the hook and break the user experience.
- **Hook timeout**: Safety net for edge cases. Most hooks complete in <100ms, but when they don't, the user is stuck.
- **Peer cache**: Most impactful for multi-agent scenarios. Redundant I/O adds ~50-200ms per hook with 3+ peers. Fixing this also reduces the window where I/O operations can hang.

## Files to Modify

| File | Change |
|------|--------|
| `crates/edda-cli/src/cmd_bridge.rs` | Add `catch_unwind` + timeout wrapper |
| `crates/edda-bridge-claude/src/dispatch.rs` | Pass pre-computed peers/board to render functions |
| `crates/edda-bridge-claude/src/peers.rs` | Accept peers/board as parameters instead of re-computing |
