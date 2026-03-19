# Mutation Contract API — How do other specs interact with Decision Model?

> Status: `working draft`
>
> Purpose: Define the complete set of operations that other specs (Intake, Injection, Governance) call. This is the firewall — nothing outside this contract touches the decisions table.
>
> Shared types: see `./shared-types.md`

---

## 1. One-Liner

> **The mutation contract is a set of 11 functions: 4 write, 4 transition, 3 read. If your operation isn't on this list, you don't do it.**

---

## 2. What It's NOT / Common Mistakes

### NOT an HTTP API

This is a Rust trait / module-level API inside `edda-ledger`. HTTP endpoints in `edda-serve` wrap these functions, but the contract is defined at the library level.

### NOT direct SQL access

No spec, no crate, no hook writes `UPDATE decisions SET ...` directly. All mutations go through this contract, which ensures:
- `is_active` stays in sync with `status`
- Every mutation writes an audit event
- Preconditions are enforced

### NOT a CRUD interface

There is no `update_decision(id, fields)` — each operation has specific semantics and preconditions. You can't just "set any field to any value."

### NOT the query API for Injection

This file defines mutations + basic reads for the mutation callers. The full query/search/pack API that Injection needs is defined in the Injection spec.

---

## 3. Contract Overview

```text
┌─────────────────────────────────────────────────────────┐
│                  Mutation Contract                       │
│                                                         │
│  Write Ops (Intake + CLI)   Transition Ops (Governance) │
│  ────────────────────────   ─────────────────────────── │
│  create_candidate()         promote(id)                 │
│  set_affected_paths(id)     reject(id)                  │
│  set_tags(id)               supersede(old, new)         │
│  set_review_after(id)       transition(id, new_status)  │
│                                                         │
│  Read Operations (for mutation callers)                  │
│  ─────────────────────────────────────                  │
│  find_active_by_key(branch, key)                        │
│  get_decision(event_id)                                 │
│  find_conflicts(key, value)                             │
│                                                         │
│  View Projection (for Injection)                        │
│  ───────────────────────────────                        │
│  to_view(row) → DecisionView                            │
│                                                         │
├─────────────────────────────────────────────────────────┤
│  Ownership Rules                                        │
│                                                         │
│  Intake:     create_candidate, set_*, edit only         │
│  Governance: ALL transitions (promote/reject/freeze/...)│
│  Injection:  read-only via to_view(), never DecisionRow │
│                                                         │
├─────────────────────────────────────────────────────────┤
│  Invariants (enforced by ALL operations)                │
│                                                         │
│  1. Every mutation writes an event to the log           │
│  2. is_active = (status IN active, experimental)        │
│  3. State transitions follow canonical-form.md          │
│  4. supersedes_id is set only by supersede()            │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Write Operations

### 4.1 `create_candidate`

Creates a new decision candidate. The only way a decision enters the system.

```typescript
function create_candidate(params: {
  key: string;                      // required: "db.engine"
  value: string;                    // required: "sqlite"
  reason?: string;
  branch: string;                   // required: current git branch
  authority: DecisionAuthority;     // required: who is creating this
  propagation?: PropagationScope;   // default: "local"
  affected_paths?: string[];        // default: []
  tags?: string[];                  // default: []
  review_after?: string;            // ISO 8601 or null
  reversibility?: Reversibility;    // default: "medium"
  refs?: string[];                  // dependency keys
  session_id?: string;              // for provenance
}): MutationResult;
```

**Behavior:**

1. Check for existing active decision with same `(branch, key)`:
   - If exists AND `authority == "agent_proposed"` → skip (don't create duplicate candidate)
   - If exists AND `authority == "human"` → create with `status = "proposed"`, **do not auto-supersede** — Governance decides later via `promote()` which triggers supersede
2. Set initial `status`:
   - `authority == "agent_proposed"` → `status = "proposed"`
   - `authority == "human"` AND no existing active decision → `status = "active"` (direct human decision, no inbox needed)
   - `authority == "human"` AND existing active decision with same key → `status = "proposed"` (Governance must promote, which triggers supersede)
3. Extract `domain` from key (split on `.`, take first segment)
4. Write event to log
5. Insert into `decisions` table

**Callers:** Intake (from extraction), CLI (`edda decide`), Bridge (`edda hook claude`)

> **Bootstrap Path (deliberate exception to "Governance owns all transitions")**
>
> - **When:** `authority == "human"` AND no existing active decision for `(branch, key)`
> - **What happens:** `status = "active"` directly — no Governance mediation, no `promote()` call
> - **Why:** The first decision for a key is a bootstrap, not a lifecycle transition. There is no prior state to transition FROM, no conflict to detect, and no supersede to orchestrate. Governance's value is in mediating changes to existing decisions — bootstrapping a new key has nothing to mediate.
> - **Constraint:** Once a key has an active decision, human re-decide creates `status = "proposed"` and Governance mediates the supersede via `promote()`. The bootstrap path is one-time per key per branch.
>
> This is NOT a governance bypass — it is a creation path that exists outside the transition graph entirely. The transition graph (proposed → active, etc.) governs state changes; bootstrap is state initialization.

### 4.2 `set_affected_paths`

Updates the affected paths of an existing decision.

```typescript
function set_affected_paths(
  event_id: string,
  paths: string[],                  // glob patterns
): MutationResult;
```

**Precondition:** `status IN ('proposed', 'active', 'experimental')` — cannot modify frozen/superseded/rejected.

**Callers:** Intake (during candidate creation), Governance (during promote enrichment), CLI

### 4.3 `set_tags`

Updates the tags of an existing decision.

```typescript
function set_tags(
  event_id: string,
  tags: string[],
): MutationResult;
```

**Precondition:** Same as `set_affected_paths`.

**Callers:** Intake, CLI

### 4.4 `set_review_after`

Sets or clears the review-after date.

```typescript
function set_review_after(
  event_id: string,
  review_after: string | null,      // ISO 8601 or null to clear
): MutationResult;
```

**Precondition:** `status IN ('active', 'experimental', 'frozen')`.

**Callers:** Governance (scheduling reviews), CLI

---

## 5. Transition Operations

### 5.1 `promote`

Moves a decision from `proposed` or `experimental` to `active`.

```typescript
function promote(event_id: string): MutationResult;
```

**Precondition:** `status IN ('proposed', 'experimental')`.

**Side effects:**
- Sets `is_active = TRUE`
- If promoting from `proposed`: updates `authority` from `agent_proposed` → `agent_approved`
- Writes promotion event

**Note:** `promote()` does NOT check for existing active decisions or trigger auto-supersede. That is Governance's responsibility — `gov_promote()` calls `check_supersede_needed()` BEFORE calling `promote()`. This separation ensures supersede logic lives in exactly one place (Governance), not two.

**Callers:** Governance only

### 5.2 `reject`

Moves a decision from `proposed` to `rejected`.

```typescript
function reject(event_id: string): MutationResult;
```

**Precondition:** `status == 'proposed'`.

**Side effects:** Sets `is_active = FALSE`, writes rejection event.

**Callers:** Governance only

### 5.3 `transition`

General-purpose transition for operations not covered by promote/reject.

```typescript
function transition(
  event_id: string,
  new_status: DecisionStatus,
  params?: {
    reason?: string;                // why this transition
    superseded_by?: string;         // required if new_status == "superseded"
  }
): MutationResult;
```

**Validates against state machine** — illegal transitions return error. See `canonical-form.md` for the full transition table.

**Callers:** Governance (freeze, unfreeze, supersede, trial)

---

## 6. Read Operations (for mutation callers)

These are the minimum reads needed by specs that call mutations. The full query API for Injection is defined separately.

### 6.1 `find_active_by_key`

```typescript
function find_active_by_key(
  branch: string,
  key: string,
): DecisionRow | null;
```

Used by `create_candidate()` for bootstrap path conflict check, and by Governance to check conflicts.

### 6.2 `get_decision`

```typescript
function get_decision(event_id: string): DecisionRow | null;
```

Used to verify preconditions before transitions.

### 6.3 `find_conflicts`

```typescript
function find_conflicts(
  key: string,
  value: string,
): ConflictInfo[];
```

Returns active decisions with the same key but different value. Used by Governance to detect drift. Returns **judgment data only** — human-readable explanation is Injection/UI's job.

```typescript
// Canonical definition: see ./shared-types.md §3.2
```

### 6.4 `to_view`

Converts a storage row to the read-side delivery model.

```typescript
function to_view(row: DecisionRow): DecisionView;
```

Parses `affected_paths` and `tags` from JSON strings to arrays, renames `scope` → `propagation`, strips storage-only fields. **Injection calls this — it never parses `DecisionRow` directly.**

**Callers:** Injection (pack building, context injection)

---

## 7. Return Type

All mutations return `MutationResult` — see `./shared-types.md` §3.1 for the canonical definition (includes optional `conflicts` field for non-blocking conflict detection).

---

## 8. Invariants (enforced by all operations)

| # | Invariant | How Enforced |
|---|-----------|-------------|
| I1 | Every mutation writes an event | All functions call `ledger.append_event()` before updating `decisions` table |
| I2 | `is_active` agrees with `status` | Mutation contract sets both in the same transaction |
| I3 | State transitions follow the state machine | `transition()` validates against allowed transitions map |
| I4 | `supersedes_id` is set only by supersede operations | Only `transition(_, "superseded", { superseded_by })` sets this field. `create_candidate()` never auto-supersedes — Governance handles supersede via `gov_promote()` |
| I5 | `domain` is always derived from `key` | `create_candidate()` computes `domain = key.split('.')[0]` |
| I6 | No orphan supersessions | If old decision is superseded, new decision must exist and be active |

---

## 9. Canonical Examples

### Example 1: Intake calls create_candidate for inbox candidate

```typescript
// bg_extract finds "we use thiserror for errors" in transcript
const result = create_candidate({
  key: "error.pattern",
  value: "thiserror+anyhow",
  reason: "consistent pattern across 5 crates",
  branch: "main",
  authority: "agent_proposed",
  tags: ["architecture", "error-handling"],
  reversibility: "medium",
});
// result: { ok: true, event_id: "evt_01JF...", decision_id: "evt_01JF..." }
// Status is "proposed" because authority is "agent_proposed"
```

### Example 2: Governance calls transition to freeze

```typescript
// Governance detects that "core.no_llm" should not be changed during release freeze
const result = transition("evt_01JE...", "frozen", {
  reason: "release freeze — do not modify until v2.0 ships",
});
// result: { ok: true, event_id: "evt_01JG...", decision_id: "evt_01JE..." }
// is_active is now FALSE, status is "frozen"
```

### Example 3: Illegal transition returns error

```typescript
const result = transition("evt_01JE...", "proposed");
// result: { ok: false, error: "illegal transition: active → proposed" }
```

---

## 10. Boundaries / Out of Scope

### In Scope
- Function signatures, parameter types, return types
- Preconditions and postconditions
- Invariants enforced by the contract
- Which specs call which functions

### Out of Scope
- **Rust trait design** — implementation detail
- **HTTP route mapping** — `edda-serve` wraps these, defined in Injection spec
- **Full query API** (search, packs, relevance ranking) → Injection spec
- **Conflict resolution policy** (what to do when conflict found) → Governance spec
- **Inbox workflow** (list, approve, reject UI) → Intake spec

---

## Closing Line

> **11 functions, 6 invariants, 3 ownership rules, zero direct SQL. Intake writes candidates. Governance transitions states. Injection reads views. Nobody else touches the decisions table.**
