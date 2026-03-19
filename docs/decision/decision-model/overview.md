# Decision Model — What is a decision in Edda?

> Status: `working draft`
>
> Purpose: Define the decision object's identity, lifecycle, and mutation boundary — the foundation that Intake, Injection, and Governance specs build on.
>
> Shared types: see `./shared-types.md`

---

## 1. One-Liner

> **A decision is a governed, lifecycle-aware object — not a note with a tag.**

Edda's decision is not "text someone typed with `edda decide`." It is a structured object with identity, scope, status, authority, and a state machine that controls who can mutate it and when.

---

## 2. What It's NOT / Common Mistakes

### NOT a note with `tags: ["decision"]`

Current implementation stores decisions as note events with a decision payload. This conflates the event (immutable log entry) with the decision (mutable lifecycle object). The decision model separates them: the event is evidence; the decision is a living entity that progresses through states.

### NOT a key-value config store

`db.engine=sqlite` looks like config. But decisions carry reason, authority, scope, and can be superseded — config cannot. If you're using decisions as config, you're missing the governance layer.

### NOT the same as a "binding" (L2 coordination)

Bindings in `coordination.jsonl` are ephemeral session-scoped claims. Decisions are project-scoped, persist across sessions, and have formal lifecycle. A binding may *trigger* a decision, but they are different objects.

### NOT the same as a "rule" (L3 postmortem)

Rules (`edda-postmortem`) are learned behavioral patterns with TTL decay. Decisions are explicit human/agent choices with formal authority. A rule might *reference* a decision, but rules have their own lifecycle (`Proposed → Active → Dormant → Dead`).

---

## 3. Core Concepts

### Decision Object

The first-class citizen. Has identity (`event_id`), content (`key`, `value`, `reason`), metadata (`status`, `authority`, `tags`, `affected_paths`, `propagation`), and graph edges (`supersedes`, `depends_on`, `evidence_refs`).

### Decision Status (Lifecycle State)

A decision is always in exactly one state. The state machine controls what operations are legal. See `canonical-form.md` for the full state machine.

### Mutation Contract

The only legal way to change a decision's state. Other specs (Intake, Injection, Governance) call these functions — they never write to the `decisions` table directly. See `api.md` for the complete contract.

### Propagation Scope vs Affected Scope

Two different concepts that share the word "scope" today:

| Concept | Current Name | What It Means |
|---------|-------------|---------------|
| **Propagation** | `DecisionScope` (Local/Shared/Global) | How far the decision travels across projects |
| **Affected scope** | Does not exist yet | Which files/modules/paths this decision governs |

Both are needed. They are orthogonal.

### Authority

Who made this decision and with what weight:

- `human` — explicitly typed by a human
- `agent_approved` — proposed by agent, approved by human
- `agent_proposed` — proposed by agent, not yet approved (lives in inbox)
- `system` — auto-generated (e.g., from sync import)

---

## 4. Canonical Form / Position

```text
┌─────────────────────────────────────────────────────────┐
│                    Decision Model                        │
│                                                         │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐          │
│  │  Object   │───▶│ Lifecycle│───▶│ Mutation │          │
│  │  Schema   │    │  State   │    │ Contract │          │
│  └──────────┘    │ Machine  │    └────┬─────┘          │
│                  └──────────┘         │                  │
│                                       │                  │
├───────────────────────────────────────┼──────────────────┤
│           Consumed By                 │                  │
│                                       ▼                  │
│  ┌─────────┐  ┌──────────┐  ┌────────────┐             │
│  │ Intake  │  │Injection │  │ Governance │             │
│  │         │  │          │  │            │             │
│  │ create  │  │ read     │  │ ALL state  │             │
│  │ only    │  │ only     │  │ transitions│             │
│  └─────────┘  └──────────┘  └────────────┘             │
└─────────────────────────────────────────────────────────┘
```

**Dependency direction is one-way:**

- **Intake** calls `create_candidate()` — writes new candidates with status `proposed`
- **Injection** reads `DecisionView` — read only, never mutates, never touches `DecisionRow`
- **Governance** owns ALL lifecycle transitions — `promote()`, `reject()`, `supersede()`, `freeze()`, `unfreeze()`, `trial()`

No spec calls another spec's mutation functions. All mutations go through Decision Model's contract.

---

## 5. Schema Summary

See `schema-v0.md` for full TypeScript types. Key additions over current `DecisionPayload`:

| Field | Type | Why |
|-------|------|-----|
| `status` | `DecisionStatus` enum | Replace `is_active: bool` with full lifecycle |
| `authority` | `DecisionAuthority` enum | Distinguish human vs agent vs system |
| `affected_paths` | `string[]` | File/module scope (distinct from propagation) |
| `tags` | `string[]` | Custom categorization (architecture, policy, runtime) |
| `review_after` | `string \| null` | ISO 8601 date for scheduled re-evaluation |
| `reversibility` | `"low" \| "medium" \| "high"` | How hard it is to undo |

---

## 6. Relationship to Other Specs

| Spec | Relationship | Contract Surface |
|------|-------------|-----------------|
| **Intake** (`docs/decision-intake/`) | Creates candidates, nothing more | `create_candidate()`, `set_affected_paths()`, `set_tags()` |
| **Injection** (`docs/decision-injection/`) | Reads `DecisionView` for packs | `query_for_context()`, `query_by_paths()`, `to_view()` |
| **Governance** (`docs/decision-governance/`) | Owns ALL lifecycle transitions | `promote()`, `reject()`, `supersede()`, `freeze()`, `unfreeze()`, `trial()`, `transition()` |
| **edda-postmortem** (existing L3) | Separate lifecycle, may reference decisions | Read-only. Rules ≠ decisions |
| **coordination.jsonl** (existing L2) | Session-scoped bindings, may trigger decisions | Bindings are ephemeral; decisions are persistent |

### Source of Truth Declaration

Each spec owns exactly one truth. When in doubt, look here:

| Question | Authoritative Source |
|----------|---------------------|
| What IS a decision? What fields does it have? | **Decision Model** (this spec) — `shared-types.md`, `schema-v0.md` |
| What states can it be in? What transitions are legal? | **Governance** — canonical lifecycle owner |
| How do candidates enter the system? | **Intake** — candidate ingestion |
| When and where do decisions surface? | **Injection** — retrieval, packs, delivery |
| How are conflicts detected and classified? | **Governance** — judgment |
| How are conflicts explained to humans? | **Injection** / UI layer — presentation |

### Boundary Contract (Hard Rules)

These three rules prevent the specs from re-coupling:

**Rule 1: Intake creates candidates, not truth.**
Intake produces `proposed` decisions. It never calls `promote()`, `reject()`, `freeze()`, or any transition. Those belong to Governance.

**Rule 2: Governance owns ALL lifecycle transitions, with one exception.**
Human first-decide (no existing active decision for the same key) goes directly to `active` via `create_candidate()`. This is a bootstrap shortcut — the very first decision for a key doesn't need governance review because there's nothing to conflict with. Once a key has an active decision, ALL subsequent changes go through Governance: human re-decide creates `proposed`, and Governance mediates via `promote()` which triggers supersede.

**Rule 3: Injection consumes `DecisionView`, not `DecisionRow`.**
Injection never parses storage-layer types. It calls `to_view()` and works with the delivery model. If the storage schema changes, only `to_view()` adapts — Injection is unaffected.

---

## 7. Canonical Examples

### Example 1: Simple human decision

```json
{
  "event_id": "evt_01JE7X...",
  "key": "db.engine",
  "value": "sqlite",
  "reason": "embedded, zero-config for CLI tool",
  "status": "active",
  "authority": "human",
  "propagation": "local",
  "affected_paths": ["crates/edda-ledger/**"],
  "tags": ["architecture", "storage"],
  "review_after": null,
  "reversibility": "low",
  "supersedes_id": null,
  "depends_on": [],
  "branch": "main",
  "ts": "2025-12-01T10:00:00Z"
}
```

### Example 2: Agent-proposed decision (inbox candidate)

```json
{
  "event_id": "evt_01JF3Q...",
  "key": "error.pattern",
  "value": "thiserror+anyhow",
  "reason": "extracted from session transcript — consistent pattern across 5 crates",
  "status": "proposed",
  "authority": "agent_proposed",
  "propagation": "local",
  "affected_paths": ["crates/*/src/lib.rs"],
  "tags": ["architecture", "error-handling"],
  "review_after": null,
  "reversibility": "medium",
  "supersedes_id": null,
  "depends_on": [],
  "branch": "main",
  "ts": "2026-01-15T14:30:00Z"
}
```

This decision lives in the inbox until Governance calls `promote()` → status becomes `active`, authority becomes `agent_approved`.

---

## 8. Boundaries / Out of Scope

### In Scope

- Decision object schema and fields
- Status state machine and legal transitions
- Mutation contract (function signatures, preconditions, postconditions)
- Backward compatibility with existing schema v9
- `supersede()` as a state transition (the mechanism)

### Out of Scope

- **How candidates get extracted** → Intake spec
- **When and where decisions surface** → Injection spec
- **How conflicts are detected and resolved** → Governance spec
- **How decisions are rendered in UI/CLI** → presentation layer, not model
- **L3 rules lifecycle** → edda-postmortem, separate system
- **L2 bindings** → coordination layer, separate system

---

## Closing Line

> **Decision Model is the single source of truth for what a decision is, what states it can be in, and what operations can change it. Everything else — capture, surfacing, enforcement — plugs into this contract.**
