# Decision Governance — Who controls the decision lifecycle?

> Status: `working draft`
>
> Purpose: Define the bounded context that owns ALL decision lifecycle transitions, conflict detection, supersede orchestration, coverage analysis, and authority enforcement.
>
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Governance is the judge and executor of decision state. It owns every transition, detects every conflict, and enforces every authority rule. If you want a decision to change state, you ask Governance.**

---

## 2. What It's NOT / Common Mistakes

### NOT an explanation engine

Governance classifies conflicts structurally (`ConflictType`). It does NOT produce human-readable explanations like "this contradicts your earlier choice because..." — that's Injection/UI's job. Governance emits `ConflictInfo`, not prose.

### NOT a candidate creator

Governance never calls `create_candidate()`. When a human types `edda decide`, Intake creates the candidate. Governance is only invoked when state needs to change (promote, reject, freeze, supersede).

### NOT a recommendation system

Governance does not suggest what decisions to make. It reports coverage gaps ("domain X has high change but no decisions") and conflict facts ("key Y has two values"). What to *do* about those facts is the human's problem.

### NOT automatic

Governance never auto-promotes, auto-rejects, or auto-supersedes without human trigger. The one exception: when `promote()` detects an existing active decision with the same key, it auto-supersedes the old one — but promote itself requires human approval.

### NOT responsible for bootstrap

Governance owns all lifecycle transitions AFTER a key's first decision. The initial human `edda decide` for a new key goes directly to `active` via `create_candidate()` — this is a bootstrap, not a governance bypass. There is no prior decision to transition from, no conflict to detect, and no supersede to orchestrate. Once a key has an active decision, all subsequent changes — including human re-declarations — go through Governance.

---

## 3. Core Definitions

### Governance Domain

The bounded context that answers: "Is this transition legal? Is there a conflict? Is the decision landscape healthy?"

### Lifecycle Executor

Governance is the sole caller of all transition operations in the mutation contract: `promote()`, `reject()`, `transition()` (for freeze, unfreeze, trial, supersede). No other spec calls these.

### Conflict Detector

Governance owns the judgment: "Does this proposed value contradict an existing active decision?" It returns `ConflictInfo[]` — structural facts, not explanations.

### Coverage Analyst

Governance reports on the health of the decision landscape: domains with high code churn but no decisions, stale decisions (active but old), and high-churn decisions (frequently superseded).

### Authority Enforcer

Governance validates that the caller has sufficient authority for the requested transition. `agent_proposed` decisions cannot self-promote. `system` decisions cannot override `human` decisions.

---

## 4. Position in the Overall System

```text
                         ┌─────────────────────────┐
                         │    Decision Model        │
                         │  (schema, state machine, │
                         │   mutation contract)     │
                         └────────────┬────────────┘
                                      │
          ┌───────────────────────────┼───────────────────────────┐
          │                           │                           │
          ▼                           ▼                           ▼
   ┌──────────────┐         ┌──────────────────┐         ┌──────────────┐
   │   Intake     │         │   GOVERNANCE     │         │  Injection   │
   │              │         │                  │         │              │
   │ create only  │────────▶│ ALL transitions  │         │ read only    │
   │              │ request │ conflict detect  │         │              │
   └──────────────┘         │ coverage analysis│         └──────────────┘
                            │ authority enforce│
                            └──────────────────┘
                                      │
                              calls mutation
                              contract only
```

**Dependency flow:**
- Intake creates candidates → requests Governance to promote/reject
- Governance calls mutation contract (`promote()`, `reject()`, `transition()`, `find_conflicts()`)
- Injection reads `DecisionView` — never interacts with Governance directly

**Governance never calls Intake or Injection.** The dependency is one-directional.

---

## 5. What Governance Owns

| Capability | Description | Spec File |
|-----------|-------------|-----------|
| Lifecycle execution | All 8 transitions from `canonical-form.md` | `canonical-form.md` |
| Conflict detection | Structural classification of contradictions | `schema-v0.md`, `api.md` |
| Supersede orchestration | old → superseded, new → active, provenance link | `canonical-form.md` |
| Coverage analysis | Stale decisions, uncovered domains, churn rate | `schema-v0.md`, `api.md` |
| Authority enforcement | Who can call which transition | `canonical-form.md` |
| Drift detection | New proposals/PRs contradicting active decisions | `api.md` |

---

## 6. What Governance Does NOT Own

| Capability | Owner | Why |
|-----------|-------|-----|
| Candidate creation | **Intake** | Governance judges, it does not ingest |
| Decision retrieval/surfacing | **Injection** | Governance judges, it does not display |
| Human-readable explanation | **Injection/UI** | Governance emits `ConflictInfo`, not prose |
| Schema definition | **Decision Model** | Governance consumes types, does not define them |
| L3 rules lifecycle | **edda-postmortem** | Separate immune-system model, not decision lifecycle |
| Session-scoped bindings | **L2 coordination** | Bindings are ephemeral, decisions are persistent |

---

## 7. Canonical Examples

### Example 1: Promote with auto-supersede

```text
State before:
  evt_A: { key: "db.engine", value: "sqlite", status: "active" }
  evt_B: { key: "db.engine", value: "postgres", status: "proposed" }

Governance.promote("evt_B")
  1. Check precondition: evt_B.status == "proposed" ✓
  2. Check authority: human approval present ✓
  3. Detect existing active for "db.engine" → evt_A found
  4. Call transition(evt_A, "superseded", { superseded_by: "evt_B" })
  5. Call promote(evt_B) → status = "active", authority = "agent_approved"
  6. Write provenance: evt_B.refs → { target: "evt_A", rel: "supersedes" }

State after:
  evt_A: { status: "superseded", is_active: false }
  evt_B: { status: "active", is_active: true, supersedes_id: "evt_A" }
```

### Example 2: Conflict detection on drift

```text
Active decision:
  { key: "error.pattern", value: "thiserror+anyhow", status: "active" }

New PR proposes:
  { key: "error.pattern", value: "custom_enum" }

Governance.detect_conflicts("error.pattern", "custom_enum")
  → [{ existing_event_id: "evt_...", existing_value: "thiserror+anyhow",
       conflict_type: "value_divergence" }]

Governance returns the ConflictInfo. Injection/UI decides how to warn the human.
```

---

## 8. Boundaries / Out of Scope

### In Scope

- All lifecycle transition execution (promote, reject, freeze, unfreeze, supersede, trial)
- Conflict detection and structural classification
- Supersede orchestration (including provenance linking)
- Coverage/debt analysis (stale, uncovered, high-churn)
- Authority enforcement (who can do what)
- Drift detection (new proposals contradicting active decisions)

### Out of Scope

- **How candidates enter the system** -- Intake spec
- **How decisions surface to agents/humans** -- Injection spec
- **Human-readable conflict explanations** -- Injection/UI layer
- **LLM-assisted analysis** -- future layer on top of Governance's structural data
- **L3 rules lifecycle** -- edda-postmortem, separate immune-system model
- **L2 session-scoped bindings** -- coordination layer

---

## Closing Line

> **Governance is the sole executor of decision state transitions, the sole judge of conflicts, and the sole auditor of decision coverage. Intake asks; Governance decides; Injection displays.**
