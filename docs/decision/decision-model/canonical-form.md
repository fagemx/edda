# Canonical Form — What does one decision lifecycle look like?

> Status: `working draft`
>
> Purpose: Define the state machine, legal transitions, and the complete lifecycle from birth to supersession.
>
> Shared types: see `./shared-types.md`

---

## 1. One-Liner

> **A decision's lifecycle is a one-way state machine with exactly 6 states and 9 transitions — no shortcuts, no backdoors.**

---

## 2. What It's NOT / Common Mistakes

### NOT a free-form status field

You cannot set `status = "whatever"`. The state machine defines exactly which transitions are legal from each state. `active → proposed` is illegal. Period.

### NOT the same lifecycle as L3 rules

L3 rules use `Proposed → Active → Dormant → Settled → Dead` with TTL decay. Decisions use a different machine: no automatic decay (decisions don't expire by time), and `frozen` is a terminal hold state that rules don't have.

### NOT reversible by default

Once a decision is `superseded`, it stays superseded. You don't "un-supersede." If the old decision was right after all, you create a new decision that supersedes the superseder.

### NOT just `is_active: bool`

The current boolean collapses 6 states into 2. `proposed`, `experimental`, and `frozen` are all "not quite active" but mean completely different things.

---

## 3. State Machine

```text
                    promote()
    ┌──────────┐ ──────────────▶ ┌──────────┐
    │ proposed │                 │  active   │
    └──────────┘                 └──────────┘
         │                        │    │    │
         │ reject()    supersede()│    │    │ freeze()
         ▼                       ▼    │    ▼
    ┌──────────┐            ┌──────┐  │  ┌──────────┐
    │ rejected │            │super-│  │  │  frozen   │
    └──────────┘            │seded │  │  └──────────┘
                            └──┬───┘  │       │
                               ▲      │       │ unfreeze()
                  supersede()  │      │       │
                               │      │       │
                              trial() │       │
                                      ▼       │
                                 ┌─────────┐  │
                                 │ experi- │◀─┘
                                 │ mental  │   promote()
                                 └─────────┘──────────▶ (active)
```

> **Note:** `experimental` can be superseded directly (T9). An experimental trial that gets replaced by a newer decision should not require promotion to `active` first.

### States

| State | Meaning | `is_active` equiv | Visible in packs? |
|-------|---------|-------------------|-------------------|
| `proposed` | Candidate, awaiting human review | `false` | No (inbox only) |
| `active` | In force, governs behavior | `true` | Yes |
| `experimental` | Trial run, may be promoted or dropped | `true` (with caveat) | Yes (marked trial) |
| `frozen` | Intentionally paused, not superseded | `false` | Yes (marked frozen) |
| `superseded` | Replaced by a newer decision | `false` | No (history only) |
| `rejected` | Reviewed and explicitly declined | `false` | No |

### Transitions

| # | From | To | Operation | Who Can Call | Precondition |
|---|------|----|-----------|-------------|-------------|
| T1 | `proposed` | `active` | `promote()` | **Governance only** | Human approval required |
| T2 | `proposed` | `rejected` | `reject()` | **Governance only** | Human decision |
| T3 | `proposed` | `experimental` | `trial()` | **Governance only** | Human approval |
| T4 | `active` | `superseded` | `supersede(new_id)` | **Governance only** | New decision must exist |
| T5 | `active` | `frozen` | `freeze()` | **Governance only** | Reason required |
| T6 | `active` | `experimental` | `trial()` | **Governance only** | Downgrade for re-evaluation |
| T7 | `frozen` | `active` | `unfreeze()` | **Governance only** | Human approval |
| T8 | `experimental` | `active` | `promote()` | **Governance only** | Human approval |
| T9 | `experimental` | `superseded` | `supersede(new_id)` | **Governance only** | New decision must exist |

> **Hard rule: ALL transitions are owned by Governance.** Intake creates candidates (`proposed`). Injection reads results. Neither calls transition operations.

### Implementation note: `check_supersede_needed`

When auto-supersede runs (e.g., `edda decide "db.engine=postgres"` while `db.engine=sqlite` is active), the check must find all decisions with the same key where `is_active = TRUE` — which covers **both** `active` and `experimental` states. Checking only `status == "active"` would miss experimental decisions with the same key, leaving stale trials in place.

```text
-- CORRECT: finds both active and experimental
SELECT * FROM decisions WHERE key = ? AND is_active = TRUE;

-- WRONG: misses experimental decisions
SELECT * FROM decisions WHERE key = ? AND status = 'active';
```

### Illegal Transitions (examples)

| From | To | Why |
|------|----|-----|
| `superseded` | `active` | Cannot un-supersede. Create a new decision instead. |
| `rejected` | `active` | Cannot un-reject. Re-propose as new decision. |
| `active` | `proposed` | Cannot demote. Use `trial()` for re-evaluation. |
| any | `proposed` | Only `create_candidate()` produces `proposed` state. |

---

## 4. One Complete Cycle (Canonical Flow)

```text
 Agent extracts candidate          Human reviews inbox
 from transcript                   approves with scope
         │                                │
         ▼                                ▼
  ┌─────────────┐  T1: promote()  ┌─────────────┐
  │  proposed   │────────────────▶│   active     │
  └─────────────┘                 └─────────────┘
                                        │
                              (months later, context changes)
                                        │
                              Human types `edda decide "db.engine=postgres"`
                                        │
                                        ▼
                                ┌─────────────────┐
                                │ auto-supersede   │
                                │ old: active→     │
                                │     superseded   │
                                │ new: →active     │
                                └─────────────────┘
```

### Step-by-step:

1. **Create** (Intake): `bg_extract` finds "we chose SQLite because embedded" in transcript → calls `create_candidate(key="db.engine", value="sqlite", authority="agent_proposed")` → status = `proposed`
2. **Inbox** (Intake): Decision appears in `edda inbox list` with status `proposed`
3. **Promote** (Governance): Human runs `edda inbox approve <id> --paths "crates/edda-ledger/**"` → Governance calls `promote(id)` → status becomes `active`, authority becomes `agent_approved`, `affected_paths` set
4. **Live** (Injection): Decision is now returned by `query_by_path("crates/edda-ledger/src/sqlite_store.rs")` as a `DecisionView` → Injection uses this for packs
5. **Supersede** (Governance): Months later, human runs `edda decide "db.engine=postgres"` → `create_candidate()` creates with `proposed` → Governance `promote()` detects existing active for `db.engine` → triggers `supersede(old_id, new_id)` → old becomes `superseded`, new becomes `active`

---

## 5. Backward Compatibility

### `is_active` mapping

The existing `is_active: bool` column must continue to work for queries that haven't migrated:

```text
is_active = TRUE  ←→  status IN ('active', 'experimental')
is_active = FALSE ←→  status IN ('proposed', 'rejected', 'frozen', 'superseded')
```

### Migration path

Schema V10 adds `status TEXT NOT NULL DEFAULT 'active'` column. Existing rows:
- `is_active = TRUE` → `status = 'active'`
- `is_active = FALSE AND supersedes_id IS NOT NULL` → `status = 'superseded'`
- `is_active = FALSE AND supersedes_id IS NULL` → `status = 'superseded'` (best guess)

`is_active` becomes a generated column or is kept in sync by the mutation contract.

---

## 6. Artifacts Produced by Each Transition

| Transition | Event Written | Side Effects |
|-----------|--------------|-------------|
| `create_candidate()` | `note` event with decision payload | Insert into `decisions` table |
| `promote()` | `note` event with `tags: ["decision_promoted"]` | Update `status`, set `is_active=TRUE` |
| `reject()` | `note` event with `tags: ["decision_rejected"]` | Update `status` |
| `supersede()` | New decision event with `supersedes` provenance | Old: `status=superseded, is_active=FALSE`. New: inserted |
| `freeze()` | `note` event with `tags: ["decision_frozen"]` | Update `status`, set `is_active=FALSE` |
| `unfreeze()` | `note` event with `tags: ["decision_unfrozen"]` | Update `status`, set `is_active=TRUE` |
| `trial()` | `note` event with `tags: ["decision_trial"]` | Update `status`, set `is_active=TRUE` |

Every transition writes an event to the append-only log. The `decisions` table is a materialized view that the mutation contract keeps in sync.

---

## 7. Canonical Examples

### Example 1: Promote from inbox

```
Before: { key: "error.pattern", status: "proposed", is_active: false }
Call:    promote("evt_01JF3Q...")
After:  { key: "error.pattern", status: "active", is_active: true }
Event:  { type: "note", tags: ["decision_promoted"], refs: { provenance: [{ target: "evt_01JF3Q...", rel: "reviews" }] } }
```

### Example 2: Auto-supersede on re-decide

```
Existing: { event_id: "evt_A", key: "db.engine", value: "sqlite", status: "active" }
User:     edda decide "db.engine=postgres" --reason "need multi-user"
Created:  { event_id: "evt_B", key: "db.engine", value: "postgres", status: "active",
            supersedes_id: "evt_A", refs.provenance: [{ target: "evt_A", rel: "supersedes" }] }
Updated:  { event_id: "evt_A", status: "superseded", is_active: false }
```

---

## 8. Boundaries / Out of Scope

### In Scope
- State definitions and legal transitions
- `is_active` backward compatibility mapping
- Migration strategy from schema v9 → v10
- Event artifacts produced by transitions

### Out of Scope
- **How candidates enter `proposed`** → Intake spec
- **How `active` decisions get surfaced** → Injection spec
- **How conflicts trigger `supersede` or `freeze`** → Governance spec
- **TTL/decay** — decisions don't auto-expire (rules do, decisions don't)
- **Rendering** — how status is displayed in CLI/TUI/HTTP

---

## Closing Line

> **Six states, nine transitions, one invariant: every mutation writes an event, and the decisions table is always a projection of the event log.**
