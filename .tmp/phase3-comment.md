## Phase 3: Implementation Plan

### Overview

One new document + one small edit to an existing doc. Pure documentation change — no code modifications.

### Step 1: Create `docs/architecture/consistency-contract.md`

**Content outline with specific details from codebase research:**

#### Title: State Consistency Contract

This document defines the consistency model between Edda's state layers. It is the authoritative reference for which layer owns which data, durability guarantees, staleness expectations, and recovery behavior.

#### State Layers

| Layer | Storage | Location | Crate(s) |
|-------|---------|----------|----------|
| **Workspace Ledger** | SQLite (WAL mode) | `.edda/ledger.db` | `edda-ledger` |
| **Coordination Store** | Flat files (JSON, JSONL) | `~/.edda/projects/{pid}/state/` | `edda-store`, `edda-bridge-claude::peers` |
| **Conductor State** | JSON files | `.edda/conductor/{plan}/state.json` | `edda-conductor::state` |

#### Source of Truth by Entity

| Entity | Owner Layer | Durability | Staleness | Recovery |
|--------|------------|------------|-----------|----------|
| Decision (domain.key=value) | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Note | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Commit event | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| Session digest | Ledger | Durable, hash-chained | Immediate | Re-run digest |
| Approval / Review bundle | Ledger | Durable, hash-chained | Immediate | Rebuild from event log |
| PR outcome | Ledger | Durable, hash-chained | Immediate | Re-ingest |
| Execution event | Ledger | Durable, hash-chained | Immediate | Re-ingest from transcript |
| Branch metadata | Ledger (refs table) | Durable | Immediate | — |
| Heartbeat | Coordination | Ephemeral (session-scoped) | Up to 120s stale | Stale -> pruned; recreated on next hook |
| Scope claim | Coordination | Ephemeral (session-scoped) | Re-derived on read | Compacted by GC; unclaimed on SessionEnd |
| Binding (real-time) | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Cross-agent request | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Sub-agent completion | Coordination | Ephemeral | Re-derived on read | Compacted by GC |
| Plan execution state | Conductor | Workspace-local, durable | Immediate | Reload from JSON; re-run phase |
| Peer count | Coordination | Ephemeral counter file | Per-hook | Recreated from live peers |
| Inject hash (dedup) | Coordination | Ephemeral | Per-hook | Recreated on next inject |
| Nudge cooldown | Coordination | Ephemeral timestamp | Per-hook | Reset to "allow" on missing |

#### Consistency Semantics

**Write Ordering:**
- Ledger writes are serialized by SQLite WAL (single-writer, concurrent readers).
- Coordination writes have no global lock. Heartbeat writes use atomic rename (`write_atomic`). Coordination log appends use `O_APPEND` — POSIX guarantees atomicity for small writes (<= PIPE_BUF); Windows does not guarantee this.
- **Decision dual-write**: When `edda decide` is called, both a ledger event AND a coordination binding are written. These are NOT transactional — a crash between the two writes may leave them inconsistent. The ledger event is the source of truth; the binding is a best-effort real-time notification.

**Read Consistency:**
- Ledger: immediately consistent (WAL ensures readers see committed writes).
- Coordination: eventually consistent per hook cycle. `compute_board_state()` fully re-parses `coordination.jsonl` on each call — no caching, no stale reads within a single invocation.
- Cross-layer: coordination state may reference sessions or decisions that have not yet been written to the ledger (or vice versa). Consumers must tolerate missing references.

**Staleness Windows:**

| Signal | Default | Env Override | Notes |
|--------|---------|-------------|-------|
| Heartbeat liveness | 120s | `EDDA_PEER_STALE_SECS` | Peers older than this are excluded from discovery |
| Sub-agent heartbeat | 1800s (15x) | — | Sub-agents cannot fire hooks -> extended threshold |
| Nudge cooldown | 180s | `EDDA_NUDGE_COOLDOWN_SECS` | Minimum time between nudge injections |
| Hook timeout | 10000ms | `EDDA_HOOK_TIMEOUT_MS` | Max hook execution time before graceful exit |
| Workspace lock timeout | 2000ms | `EDDA_BRIDGE_LOCK_TIMEOUT_MS` | Max wait for ledger lock in bridge hooks |

#### Recovery Behavior

**Crash During Hook Execution:**
- Hook exits 0 on any error/panic/timeout (never blocks host agent).
- Partial heartbeat write: atomic rename ensures either old or new version exists.
- Partial coordination append: may leave a truncated last line; `compute_board_state()` skips unparseable lines.
- Partial ledger write: SQLite WAL rollback ensures no partial events.

**Stale Session Cleanup:**
- Heartbeats exceeding `stale_secs()` are ignored by `discover_active_peers()`.
- On SessionEnd: heartbeat file is deleted; `write_unclaim()` removes claims; sub-agent heartbeats are cleaned up via `cleanup_subagent_heartbeats()`.
- Orphaned heartbeats (no SessionEnd): naturally expire via staleness threshold.

**Coordination Log Compaction:**
- `compute_board_state_for_compaction()` derives current state and rewrites `coordination.jsonl` as minimal JSONL.
- `compact_pending` flag file guards against interrupted compaction — if flag exists on next hook, compaction is retried.

**Conductor State Recovery:**
- Plan state is a JSON file written atomically. On corruption, `load_state()` returns Err — the plan must be re-initialized.
- Conductor state is independent of ledger and coordination layers.

#### Invariants

| ID | Statement | Rationale |
|----|-----------|-----------|
| INV-01 | Coordination store MUST NOT mutate ledger history directly | Separation of concerns; ledger integrity depends on hash chain |
| INV-02 | Ledger events are append-only; no UPDATE or DELETE | Hash chain would break; tamper-evidence guarantee |
| INV-03 | Heartbeat files are session-scoped; removed on SessionEnd | Prevents unbounded state growth; stale threshold as safety net |
| INV-04 | `compute_board_state()` MUST tolerate malformed JSONL lines | Crash resilience — partial writes must not break all coordination |
| INV-05 | Decision source of truth is the ledger, not coordination binding | Coordination binding is a real-time notification, not authoritative |
| INV-06 | Coordination state may reference entities not yet in the ledger | Eventual consistency — consumers must handle missing refs gracefully |
| INV-07 | Conductor state is workspace-local and independent of coordination | Plan execution does not depend on peer state |

#### Contributor Guide: Adding New State

When adding a new stateful feature, use this decision tree:

1. **Is this a permanent project record?** (decision, note, commit, approval)
   -> Write to **Ledger** via `edda-ledger::Ledger::append_event()`
   -> Must be hash-chained (use `edda_core::event::new_*_event()` constructors)

2. **Is this real-time coordination between concurrent sessions?** (claim, binding, request, heartbeat)
   -> Write to **Coordination Store** via `peers::append_coord_event()` or `write_heartbeat()`
   -> Must tolerate staleness and missing data
   -> Must handle concurrent appends without corruption

3. **Is this session-scoped ephemeral state?** (counters, dedup hashes, cooldowns)
   -> Write to **Coordination Store** as a per-session file in `state/`
   -> Must have a cleanup path (SessionEnd or staleness expiry)

4. **Is this plan execution state?** (phase progress, check results)
   -> Write to **Conductor State** in `.edda/conductor/`
   -> Use `edda_store::write_atomic()` for crash safety

5. **Does this need real-time visibility AND permanent record?**
   -> Write to BOTH layers (like decisions: coordination binding + ledger event)
   -> Document which is authoritative (always the ledger)
   -> Accept that the two may be temporarily inconsistent

### Step 2: Add Cross-Reference in `docs/architecture/overview.md`

At the end of the "Coordination layer" section (after line 165), add:

> For detailed consistency semantics, staleness windows, recovery behavior, and contributor guidance on where to write new state, see [State Consistency Contract](consistency-contract.md).

### Acceptance Criteria Mapping

| Criteria | Addressed By |
|----------|-------------|
| A dedicated doc exists (architecture/reference section) | `docs/architecture/consistency-contract.md` |
| It defines source-of-truth + durability + staleness semantics for major state types | "Source of Truth by Entity" table + "Staleness Windows" table |
| It includes contributor guidance for adding new stateful features | "Contributor Guide: Adding New State" section |
| Relevant docs link to it | Cross-reference added to `docs/architecture/overview.md` |

### Commit Plan

| # | Commit Message | Files |
|---|---------------|-------|
| 1 | `docs(architecture): add state consistency contract (GH-93)` | `docs/architecture/consistency-contract.md`, `docs/architecture/overview.md` |

### Risks

| Risk | Mitigation |
|------|-----------|
| Document drifts from code over time | Invariants are numbered and can be referenced in code comments; contributor guide is decision-tree format (stable) |
| Missing entities | Table is comprehensive based on current codebase scan; new entities should be added as they are created |
| Over-specification constrains future design | Document describes current behavior, not prescriptive rules; invariants focus on safety properties, not implementation details |
