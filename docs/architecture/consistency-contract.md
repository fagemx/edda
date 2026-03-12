---
title: State Consistency Contract
---

# State Consistency Contract

This document defines the consistency model between Edda's state layers. It is the
authoritative reference for which layer owns which data, durability guarantees,
staleness expectations, and recovery behavior.

## State Layers

Edda maintains three distinct state layers, each with different storage mechanisms,
scoping, and durability guarantees.

### Workspace Ledger (`.edda/ledger.db`)

- **Crate**: `edda-ledger` (L2 Persistence)
- **Storage**: SQLite with WAL mode, `busy_timeout` 5 s
- **Model**: Append-only, hash-chained events. Each event's `hash` = SHA-256 of
  canonical JSON content + `parent_hash`. Tamper-evident chain.
- **Scope**: Per-workspace (`.edda/` in repo root)
- **Concurrency**: WAL mode allows concurrent readers + single writer. CLI operations
  acquire an exclusive `WorkspaceLock` via `.edda/LOCK` (non-blocking
  `fs2::try_lock_exclusive`). Bridge hooks retry with
  `EDDA_BRIDGE_LOCK_TIMEOUT_MS` (default 2 s).
- **Entity types**: `note`, `cmd`, `commit`, `merge`, `branch_create`,
  `branch_switch`, `rebuild`, `task_intake`, `agent_phase_change`, `approval`,
  `approval_request`, `approval_policy_match`, `review_bundle`, `pr`,
  `execution_event`

### Coordination Store (`~/.edda/projects/{pid}/state/`)

- **Crates**: `edda-store` (L2) for path/atomic-write helpers;
  `edda-bridge-claude::peers` (L3 Bridge) for logic
- **Storage**: Flat files — per-session JSON heartbeats + append-only JSONL
  coordination log
- **Scope**: Per-user, cross-workspace (keyed by `project_id` hash)
- **Concurrency**: Heartbeat writes use atomic rename (`edda_store::write_atomic`).
  Coordination log appends use `O_APPEND` — POSIX guarantees atomicity for small
  writes (<= `PIPE_BUF`); Windows does not guarantee this.
- **Files**:
  - `session.{session_id}.json` — heartbeat
  - `coordination.jsonl` — claims, bindings, requests, acks
  - `autoclaim.{session_id}.json` — auto-claim dedup
  - `peer_count.{session_id}` — late-peer detection
  - `inject_hash.{session_id}` — injection dedup
  - `nudge_ts.{session_id}` — nudge cooldown
  - `compact_pending` — GC recovery flag

### Conductor State (`.edda/conductor/{plan}/state.json`)

- **Crate**: `edda-conductor` (L3 Processing)
- **Storage**: JSON state files written via `edda_store::write_atomic()`
- **Scope**: Per-workspace, per-plan
- **Purpose**: Multi-phase plan execution state machine. Independent of ledger and
  coordination layers.

## Source of Truth by Entity

| Entity | Owner Layer | Durability | Staleness | Recovery |
|--------|------------|------------|-----------|----------|
| Decision (`domain.key=value`) | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Note | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Commit event | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Session digest | Ledger | Durable, hash-chained | Immediate | Re-run digest |
| Approval / Review bundle | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| PR outcome | Ledger | Durable, hash-chained | Immediate | Re-ingest |
| Execution event | Ledger | Durable, hash-chained | Immediate | Re-ingest from transcript |
| Branch metadata | Ledger (refs) | Durable | Immediate | — |
| Heartbeat | Coordination | Ephemeral (session-scoped) | Up to 120 s stale | Stale heartbeats pruned; recreated on next hook |
| Scope claim | Coordination | Ephemeral (session-scoped) | Re-derived on read | Compacted by GC; unclaimed on `SessionEnd` |
| Binding (real-time) | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Cross-agent request | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Sub-agent completion | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Plan execution state | Conductor | Workspace-local, durable | Immediate | Reload from JSON; re-run phase |
| Peer count | Coordination | Ephemeral counter file | Per-hook | Recreated from live peers |
| Inject hash (dedup) | Coordination | Ephemeral | Per-hook | Recreated on next inject |
| Nudge cooldown | Coordination | Ephemeral timestamp | Per-hook | Reset to "allow" on missing |

## Consistency Semantics

### Write Ordering

- **Ledger writes** are serialized by SQLite WAL (single-writer, concurrent readers).
- **Coordination writes** have no global lock. Heartbeat writes use atomic rename.
  Coordination log appends use `O_APPEND` — atomic for small writes on POSIX, not
  guaranteed on Windows.
- **Decision dual-write**: When `edda decide` is called, both a ledger event AND a
  coordination binding are written. These are **not** transactional — a crash between
  the two writes may leave them inconsistent. The ledger event is the source of
  truth; the coordination binding is a best-effort real-time notification.

### Read Consistency

- **Ledger**: Immediately consistent. WAL ensures readers see the latest committed
  writes.
- **Coordination**: Eventually consistent per hook cycle. `compute_board_state()`
  fully re-parses `coordination.jsonl` on each call — no caching, no stale reads
  within a single invocation.
- **Cross-layer**: Coordination state may reference sessions or decisions that have
  not yet been written to the ledger (or vice versa). Consumers must tolerate missing
  references.

### Staleness Windows

| Signal | Default | Env Override | Notes |
|--------|---------|-------------|-------|
| Heartbeat liveness | 120 s | `EDDA_PEER_STALE_SECS` | Peers older than this are excluded from discovery |
| Sub-agent heartbeat | 1800 s (15×) | — | Sub-agents cannot fire hooks; extended threshold |
| Nudge cooldown | 180 s | `EDDA_NUDGE_COOLDOWN_SECS` | Minimum interval between nudge injections |
| Hook timeout | 10 000 ms | `EDDA_HOOK_TIMEOUT_MS` | Max hook execution time before graceful exit |
| Workspace lock retry | 2000 ms | `EDDA_BRIDGE_LOCK_TIMEOUT_MS` | Max wait for ledger lock in bridge hooks |

## Recovery and Rebuild

### Crash During Hook Execution

- Hooks exit 0 on any error/panic/timeout (never block the host agent).
- **Partial heartbeat write**: atomic rename ensures either the old or new version
  exists — never a corrupt intermediate.
- **Partial coordination append**: may leave a truncated last line.
  `compute_board_state()` skips unparseable lines.
- **Partial ledger write**: SQLite WAL rollback ensures no partial events are
  visible.

### Stale Session Cleanup

- Heartbeats exceeding `stale_secs()` are ignored by `discover_active_peers()`.
- On `SessionEnd`: heartbeat file is deleted, `write_unclaim()` removes claims,
  sub-agent heartbeats are cleaned up via `cleanup_subagent_heartbeats()`.
- Orphaned heartbeats (no `SessionEnd`): naturally expire via the staleness threshold.

### Coordination Log Compaction

- `compute_board_state_for_compaction()` derives current state and rewrites
  `coordination.jsonl` as minimal JSONL.
- A `compact_pending` flag file guards against interrupted compaction — if the flag
  exists on the next hook fire, compaction is retried.

### Conductor State Recovery

- Plan state is a JSON file written atomically. On corruption, `load_state()` returns
  `Err` — the plan must be re-initialized.
- Conductor state is independent of ledger and coordination layers.

## Invariants

These invariants are numbered so they can be referenced in code comments, PR reviews,
and future issues.

| ID | Statement | Rationale |
|----|-----------|-----------|
| INV-01 | Coordination store MUST NOT mutate ledger history directly. | Separation of concerns; ledger integrity depends on hash chain. |
| INV-02 | Ledger events are append-only; no `UPDATE` or `DELETE`. | Hash chain would break; tamper-evidence guarantee. |
| INV-03 | Heartbeat files are session-scoped; removed on `SessionEnd`. | Prevents unbounded state growth; stale threshold as safety net. |
| INV-04 | `compute_board_state()` MUST tolerate malformed JSONL lines. | Crash resilience — partial writes must not break all coordination. |
| INV-05 | Decision source of truth is the ledger, not the coordination binding. | Coordination binding is a real-time notification, not authoritative. |
| INV-06 | Coordination state may reference entities not yet in the ledger. | Eventual consistency — consumers must handle missing refs gracefully. |
| INV-07 | Conductor state is workspace-local and independent of coordination. | Plan execution does not depend on peer state. |

## Contributor Guide: Adding New State

When adding a new stateful feature, use this decision tree:

1. **Is this a permanent project record?** (decision, note, commit, approval)
   - Write to the **Ledger** via `edda_ledger::Ledger::append_event()`.
   - Must be hash-chained (use `edda_core::event::new_*_event()` constructors).

2. **Is this real-time coordination between concurrent sessions?** (claim, binding,
   request, heartbeat)
   - Write to the **Coordination Store** via `peers::append_coord_event()` or
     `write_heartbeat()`.
   - Must tolerate staleness and missing data.
   - Must handle concurrent appends without corruption.

3. **Is this session-scoped ephemeral state?** (counters, dedup hashes, cooldowns)
   - Write to the **Coordination Store** as a per-session file in `state/`.
   - Must have a cleanup path (`SessionEnd` handler or staleness expiry).

4. **Is this plan execution state?** (phase progress, check results)
   - Write to **Conductor State** in `.edda/conductor/`.
   - Use `edda_store::write_atomic()` for crash safety.

5. **Does this need real-time visibility AND a permanent record?**
   - Write to **both** layers (like decisions: coordination binding + ledger event).
   - Document which layer is authoritative (always the ledger).
   - Accept that the two may be temporarily inconsistent.

## References

- [Architecture Overview](overview.md) — layer model, crate map, data flow
- `crates/edda-ledger/src/lib.rs` — ledger implementation
- `crates/edda-store/src/lib.rs` — atomic write helpers, per-user store paths
- `crates/edda-bridge-claude/src/peers/mod.rs` — coordination logic, heartbeats,
  board state
- `crates/edda-conductor/src/state.rs` — conductor state machine
