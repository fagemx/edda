# Canonical Form — How does Governance execute the lifecycle?

> Status: `working draft`
>
> Purpose: Define the execution semantics for every lifecycle transition. The state machine is defined in `../decision-model/canonical-form.md` — this file defines HOW Governance executes each transition, with preconditions, authority rules, and side effects.
>
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Governance is the state machine's runtime. The model defines the graph; Governance enforces the guards, fires the transitions, and writes the audit trail.**

---

## 2. What It's NOT / Common Mistakes

### NOT a second state machine

The state machine lives in `../decision-model/canonical-form.md`. This file does not redefine states or transitions — it defines the *execution semantics* that Governance applies when running them.

### NOT a policy engine

Governance does not decide whether a transition *should* happen based on business rules like "don't change databases during release." It enforces *structural* preconditions (status must be X, authority must be Y). Policy enforcement is a future layer.

### NOT synchronous end-to-end

Governance transitions are synchronous in the mutation contract. But the *request* to transition may be asynchronous — Intake queues a promote request, a human approves later, then Governance executes. The async boundary is outside Governance.

---

## 3. Transition Execution Table

Each transition has: guard (precondition), authority requirement, mutation call, and side effects.

```text
State Machine (from decision-model/canonical-form.md):

              promote()
  proposed ──────────────▶ active
     │                      │  │  │
     │ reject()  supersede()│  │  │ freeze()
     ▼                     ▼  │  ▼
  rejected             super- │ frozen
                       seded  │   │
                              │   │ unfreeze()
                       trial()│   │
                              ▼   │
                         experi-  │
                         mental ◀─┘
                            │
                            │ promote()
                            ▼
                          (active)
```

### T0: Bootstrap — (none) → active (NOT a Governance transition)

| Aspect | Detail |
|--------|--------|
| **Guard** | `authority == "human"` AND no existing active decision for `(branch, key)` |
| **Authority** | Human only (implicit — the human typed `edda decide`). |
| **Mutation** | `create_candidate()` with `status = "active"` directly |
| **Side effects** | Write creation event. No supersede, no conflict check — there is nothing to conflict with. |
| **Governance role** | **None.** This is handled entirely by `create_candidate()` in the Mutation Contract. It is NOT a lifecycle transition — it is state initialization. Governance takes over for all subsequent changes to this key. |

> **Why T0 is not in the state machine graph:** The state machine defines transitions between states. Bootstrap has no source state — the decision does not yet exist. It is a creation, not a transition.

### T1: `promote()` — proposed → active

| Aspect | Detail |
|--------|--------|
| **Guard** | `status == "proposed"` OR `status == "experimental"` |
| **Authority** | Requires human approval. `agent_proposed` cannot self-promote. |
| **Mutation** | `promote(event_id)` |
| **Side effects** | (1) If from `proposed`: authority changes `agent_proposed` → `agent_approved`. (2) Check for existing active with same `(branch, key)` → auto-supersede if found. (3) Write promotion event. |
| **Failure modes** | `illegal_transition` if status is not proposed/experimental. `precondition_failed` if no human approval recorded. |

### T2: `reject()` — proposed → rejected

| Aspect | Detail |
|--------|--------|
| **Guard** | `status == "proposed"` |
| **Authority** | Human decision. Agent cannot reject on behalf of human. |
| **Mutation** | `reject(event_id)` |
| **Side effects** | Write rejection event with optional reason. |
| **Failure modes** | `illegal_transition` if not proposed. |

### T3: `trial()` — proposed → experimental, OR active → experimental

| Aspect | Detail |
|--------|--------|
| **Guard** | `status IN ("proposed", "active")` |
| **Authority** | Human approval required. |
| **Mutation** | `transition(event_id, "experimental")` |
| **Side effects** | `is_active = TRUE` (experimental decisions are visible). If from `proposed`: authority becomes `agent_approved`. Write trial event. |
| **Failure modes** | `illegal_transition` from any other status. |

### T4: `supersede()` — active → superseded

| Aspect | Detail |
|--------|--------|
| **Guard** | `is_active == TRUE` (i.e., `status IN (active, experimental)`) AND new decision exists and is being promoted |
| **Authority** | Triggered by `promote()` — inherits its authority check. OR explicit via `edda decide` re-declaration. |
| **Mutation** | `transition(old_id, "superseded", { superseded_by: new_id })` |
| **Side effects** | (1) Old decision: `is_active = FALSE`, `status = "superseded"`. (2) New decision gets `supersedes_id = old_id`. (3) Provenance link: `{ target: old_id, rel: "supersedes" }`. (4) Write supersede event. |
| **Failure modes** | `missing_superseded_by` if new decision ID not provided. `decision_not_found` if old ID invalid. |

### T5: `freeze()` — active → frozen

| Aspect | Detail |
|--------|--------|
| **Guard** | `status == "active"` |
| **Authority** | Human decision. Reason required (why freeze). |
| **Mutation** | `transition(event_id, "frozen", { reason })` |
| **Side effects** | `is_active = FALSE`. Decision remains visible in packs (marked frozen) but is not authoritative. Write freeze event with reason. |
| **Failure modes** | `illegal_transition` from non-active status. `precondition_failed` if no reason provided. |

### T6: `unfreeze()` — frozen → active

| Aspect | Detail |
|--------|--------|
| **Guard** | `status == "frozen"` |
| **Authority** | Human approval required. |
| **Mutation** | `transition(event_id, "active", { reason })` |
| **Side effects** | `is_active = TRUE`. Check for conflicts with decisions activated during the freeze period. Write unfreeze event. |
| **Failure modes** | `illegal_transition` from non-frozen status. |

---

## 4. Authority Matrix

Who can trigger which transition:

> **Clarification:** The columns below represent the **caller role** — who is invoking the transition function — NOT the `authority` field stored on the decision object. The decision's `authority` field records who *created* the decision (e.g., `"agent_proposed"`, `"human"`). The matrix below answers: "If this type of actor calls the transition, is it allowed?"

```text
┌──────────────────┬────────┬────────────────┬────────────────┬────────┐
│ Transition       │ human  │ agent_approved │ agent_proposed │ system │
│                  │(caller)│ (caller)       │ (caller)       │(caller)│
├──────────────────┼────────┼────────────────┼────────────────┼────────┤
│ promote()        │  YES   │     YES        │      NO        │  NO    │
│ reject()         │  YES   │     NO         │      NO        │  NO    │
│ trial()          │  YES   │     YES        │      NO        │  NO    │
│ supersede()      │  YES*  │     YES*       │      NO        │  NO    │
│ freeze()         │  YES   │     NO         │      NO        │  NO    │
│ unfreeze()       │  YES   │     NO         │      NO        │  NO    │
└──────────────────┴────────┴────────────────┴────────────────┴────────┘

* supersede() is triggered by promote() or explicit re-declaration, not called directly.
```

> **Bootstrap (T0) is not in this matrix** because it is not a Governance transition. The bootstrap path (`create_candidate()` with `status = "active"` for new keys) is handled by the Mutation Contract, not Governance. See T0 above.

**Authority escalation rules:**
1. `agent_proposed` can never transition its own decisions — human must act.
2. `system` decisions (from sync/import) can be overridden by `human` decisions, but `system` cannot override `human`.
3. When `promote()` changes authority from `agent_proposed` to `agent_approved`, the human approval is the escalation.

---

## 5. Supersede Orchestration

Supersede is the most complex transition because it involves two decisions. Full orchestration:

```text
┌─────────────────────────────────────────────────────────┐
│                Supersede Orchestration                    │
│                                                         │
│  Trigger: promote(new_id) detects existing active       │
│           with same (branch, key)                        │
│                                                         │
│  Step 1: Validate                                       │
│    - old.status == "active"                              │
│    - new.status == "proposed" (about to become active)   │
│    - old.key == new.key                                  │
│    - old.branch == new.branch                            │
│                                                         │
│  Step 2: Transition old                                  │
│    - transition(old_id, "superseded",                    │
│        { superseded_by: new_id })                        │
│    - is_active = FALSE                                   │
│                                                         │
│  Step 3: Promote new                                     │
│    - status = "active", is_active = TRUE                 │
│    - supersedes_id = old_id                              │
│    - authority: agent_proposed → agent_approved           │
│                                                         │
│  Step 4: Provenance                                      │
│    - new.refs.provenance += { target: old_id,            │
│                               rel: "supersedes" }        │
│                                                         │
│  Step 5: Audit                                           │
│    - Write supersede event                               │
│    - Write promotion event                               │
│    - Both events link to each other                      │
│                                                         │
│  Atomicity: Steps 2-5 are one transaction.               │
│  If any step fails, all roll back.                       │
└─────────────────────────────────────────────────────────┘
```

### Supersede approval modes

| Mode | Trigger | Human approval? |
|------|---------|----------------|
| **Explicit re-declaration** | `edda decide "db.engine=postgres"` when `db.engine=sqlite` is active | Implicit — human typed the command |
| **Inbox promote** | `edda inbox approve <id>` for a proposed decision that conflicts | Explicit — human ran approve |
| **Never auto** | Agent extraction finds conflicting candidate | NO — stays `proposed` until human promotes |

---

## 6. Conflict Detection Logic

Governance detects conflicts deterministically. No LLM involved.

```text
┌─────────────────────────────────────────────────────────┐
│              Conflict Detection Pipeline                 │
│                                                         │
│  Input: (key, value, branch, affected_paths)            │
│                                                         │
│  Check 1: Key Match (value_divergence)                  │
│    SELECT * FROM decisions                               │
│    WHERE key = :key AND status = 'active'                │
│      AND branch = :branch AND value != :value            │
│    → ConflictType = "value_divergence"                   │
│                                                         │
│  Check 2: Path Overlap (scope_overlap)                   │
│    For each active decision with affected_paths:         │
│      If glob_overlap(existing.paths, new.paths)          │
│      AND existing.domain == new.domain                   │
│      AND existing.value != new.value                     │
│    → ConflictType = "scope_overlap"                      │
│                                                         │
│  Check 3: Cross-Branch (cross_branch)                    │
│    SELECT * FROM decisions                               │
│    WHERE key = :key AND status = 'active'                │
│      AND branch != :branch AND value != :value           │
│    → ConflictType = "cross_branch"                       │
│                                                         │
│  Output: ConflictInfo[]                                  │
│  (structural facts only — no explanation, no suggestion) │
└─────────────────────────────────────────────────────────┘
```

**`glob_overlap` semantics:**
- `crates/edda-ledger/**` overlaps with `crates/edda-ledger/src/lib.rs`
- `crates/edda-*/src/**` overlaps with `crates/edda-serve/src/main.rs`
- `crates/edda-ledger/**` does NOT overlap with `crates/edda-serve/**`

---

## 7. Canonical Examples

### Example 1: Full promote-with-supersede sequence

```json
// Before
{
  "decisions": [
    { "event_id": "evt_A", "key": "db.engine", "value": "sqlite",
      "status": "active", "authority": "human" },
    { "event_id": "evt_B", "key": "db.engine", "value": "postgres",
      "status": "proposed", "authority": "agent_proposed" }
  ]
}

// Governance.promote("evt_B")

// After
{
  "decisions": [
    { "event_id": "evt_A", "key": "db.engine", "value": "sqlite",
      "status": "superseded", "is_active": false },
    { "event_id": "evt_B", "key": "db.engine", "value": "postgres",
      "status": "active", "is_active": true,
      "authority": "agent_approved", "supersedes_id": "evt_A" }
  ],
  "events_written": [
    { "type": "note", "tags": ["decision_superseded"],
      "refs": { "provenance": [{ "target": "evt_A", "rel": "supersedes" }] } },
    { "type": "note", "tags": ["decision_promoted"] }
  ]
}
```

### Example 2: Freeze during release

```json
// Governance.freeze("evt_B", { reason: "release freeze until v2.0" })

// Before
{ "event_id": "evt_B", "status": "active", "is_active": true }

// After
{ "event_id": "evt_B", "status": "frozen", "is_active": false }

// Event
{ "type": "note", "tags": ["decision_frozen"],
  "payload": { "reason": "release freeze until v2.0" } }
```

### Example 3: Authority rejection

```json
// agent_proposed decision tries to self-promote
// Governance.promote("evt_C") where evt_C.authority == "agent_proposed"
// and no human approval is recorded

// Result
{ "ok": false, "error": "precondition_failed: human approval required for promotion" }
```

---

## 8. Boundaries / Out of Scope

### In Scope

- Execution semantics for all 8 transitions (T1-T8 from decision-model)
- Authority matrix and escalation rules
- Supersede orchestration (multi-decision atomic operation)
- Conflict detection pipeline (3 checks: key, path, branch)
- Guard validation and error reporting

### Out of Scope

- **State machine definition** (states, transitions graph) -- `../decision-model/canonical-form.md`
- **Candidate creation** -- Intake spec
- **Decision surfacing/packs** -- Injection spec
- **Human-readable conflict explanation** -- Injection/UI
- **Policy rules** (e.g., "no infra changes during release") -- future policy layer
- **LLM-assisted conflict analysis** -- future enhancement on top of structural data

---

## Closing Line

> **Governance is the canonical lifecycle source: it defines HOW each transition executes, WHO can trigger it, and WHAT happens atomically. The state machine is the graph; Governance is the runtime.**
