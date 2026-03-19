# Governance API — How do callers interact with Governance?

> Status: `working draft`
>
> Purpose: Define the complete set of operations that Governance exposes: transition operations, conflict detection, coverage analysis, and drift detection. This is Governance's public contract.
>
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Governance exposes 10 operations: 6 transition, 2 conflict, 2 coverage. If your lifecycle operation isn't on this list, it doesn't exist.**

---

## 2. What It's NOT / Common Mistakes

### NOT the mutation contract

The mutation contract (`../decision-model/api.md`) defines the low-level functions that touch the `decisions` table. Governance's API wraps those functions with precondition checks, authority validation, and orchestration (e.g., supersede during promote).

### NOT an HTTP API (yet)

Like the mutation contract, this is a Rust module-level API. HTTP endpoints in `edda-serve` will wrap these functions, but the contract is library-level.

### NOT a query API for reading decisions

Governance reads decisions to make judgments. It does not expose general-purpose query/search — that's Injection's job. Governance's reads are all judgment-oriented.

---

## 3. API Overview

```text
┌─────────────────────────────────────────────────────────┐
│                  Governance API                          │
│                                                         │
│  Transition Ops (6)       Conflict Ops (2)              │
│  ──────────────────       ───────────────               │
│  gov_promote(req)         detect_conflicts(req)         │
│  gov_reject(req)          detect_drift(key, value,      │
│  gov_trial(req)                        branch)          │
│  gov_freeze(req)                                        │
│  gov_unfreeze(req)        Coverage Ops (2)              │
│  gov_supersede(req)       ───────────────               │
│                           compute_coverage(project_id)  │
│                           find_stale_decisions(opts)    │
│                                                         │
├─────────────────────────────────────────────────────────┤
│  Internal (not exposed)                                  │
│  ───────────────────────                                │
│  validate_authority(actor, transition)                   │
│  check_supersede_needed(branch, key)                    │
│  glob_overlap(paths_a, paths_b)                          │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Transition Operations

All transition operations follow the same pattern:
1. Validate authority (is the caller allowed?)
2. Check precondition (is the current status legal for this transition?)
3. Execute via mutation contract
4. Handle side effects (auto-supersede, provenance, etc.)
5. Return `MutationResult`

### 4.1 `gov_promote`

Promotes a decision from `proposed`/`experimental` to `active`. Orchestrates auto-supersede if needed.

```typescript
function gov_promote(req: PromoteRequest): MutationResult;
```

**Flow:**

```text
gov_promote(req)
  │
  ├─ 1. get_decision(req.event_id)
  │     → decision_not_found? → return error
  │
  ├─ 2. validate_authority(req.requested_by, "promote")
  │     → human_approved == false? → return precondition_failed
  │
  ├─ 3. Check status ∈ { proposed, experimental }
  │     → no? → return illegal_transition
  │
  ├─ 4. check_supersede_needed(decision.branch, decision.key)
  │     → existing is_active=TRUE found? (catches both active AND experimental)
  │       → gov_supersede({ old: existing, new: req.event_id })
  │
  ├─ 5. Enrich: set affected_paths, tags, review_after if provided
  │
  ├─ 6. promote(req.event_id)
  │
  └─ 7. Return MutationResult
```

**Callers:** CLI (`edda inbox approve`), Intake (requesting promotion for human decisions)

### 4.2 `gov_reject`

Rejects a proposed decision.

```typescript
function gov_reject(req: TransitionRequest): MutationResult;
```

**Flow:**

```text
gov_reject(req)
  │
  ├─ 1. get_decision(req.event_id) → not found? → error
  ├─ 2. validate_authority: human only
  ├─ 3. Check status == "proposed" → else illegal_transition
  ├─ 4. reject(req.event_id)
  └─ 5. Return MutationResult
```

**Callers:** CLI (`edda inbox reject`)

### 4.3 `gov_trial`

Moves a decision to `experimental` status.

```typescript
function gov_trial(req: TransitionRequest): MutationResult;
```

**Flow:**

```text
gov_trial(req)
  │
  ├─ 1. get_decision → not found? → error
  ├─ 2. validate_authority: human approval required
  ├─ 3. Check status ∈ { proposed, active } → else illegal_transition
  ├─ 4. transition(req.event_id, "experimental")
  └─ 5. Return MutationResult
```

**Callers:** CLI (`edda inbox trial`, `edda governance trial`)

### 4.4 `gov_freeze`

Freezes an active decision.

```typescript
function gov_freeze(req: FreezeRequest): MutationResult;
```

**Flow:**

```text
gov_freeze(req)
  │
  ├─ 1. get_decision → not found? → error
  ├─ 2. validate_authority: human only
  ├─ 3. Check status == "active" → else illegal_transition
  ├─ 4. Check req.reason is non-empty → else precondition_failed
  ├─ 5. transition(req.event_id, "frozen", { reason: req.reason })
  ├─ 6. If req.unfreeze_after: set_review_after(req.event_id, req.unfreeze_after)
  └─ 7. Return MutationResult
```

**Callers:** CLI (`edda governance freeze`)

### 4.5 `gov_unfreeze`

Unfreezes a frozen decision back to active.

```typescript
function gov_unfreeze(req: TransitionRequest): MutationResult;
```

**Flow:**

```text
gov_unfreeze(req)
  │
  ├─ 1. get_decision → not found? → error
  ├─ 2. validate_authority: human approval required
  ├─ 3. Check status == "frozen" → else illegal_transition
  ├─ 4. detect_conflicts for this key (conflicts created during freeze period)
  ├─ 5. transition(req.event_id, "active", { reason: req.reason })
  └─ 6. Return MutationResult (include conflicts in response if any)
```

**Callers:** CLI (`edda governance unfreeze`)

### 4.6 `gov_supersede`

Explicit supersede (usually called internally by `gov_promote`, but can be called directly).

```typescript
function gov_supersede(req: SupersedeRequest): MutationResult;
```

**Flow:**

```text
gov_supersede(req)
  │
  ├─ 1. get_decision(req.old_event_id) → not found? → error
  ├─ 2. get_decision(req.new_event_id) → not found? → error
  ├─ 3. validate_authority: human or agent_approved
  ├─ 4. Check old.is_active == TRUE → else illegal_transition
  │     (catches both "active" and "experimental" — see check_supersede_needed §7.2)
  ├─ 5. Check old.key == new.key → else precondition_failed
  ├─ 6. transition(old_id, "superseded", { superseded_by: new_id })
  ├─ 7. Write provenance link
  └─ 8. Return MutationResult { superseded_id: old_id }
```

**Callers:** `gov_promote` (internal), CLI (`edda governance supersede`)

---

## 5. Conflict Operations

### 5.1 `detect_conflicts`

Full conflict detection pipeline: key match, path overlap, cross-branch.

```typescript
function detect_conflicts(
  req: ConflictDetectionRequest
): ConflictReport;
```

**Pipeline:**

```text
detect_conflicts(req)
  │
  ├─ Check 1: Key Match
  │   find_conflicts(req.key, req.value)
  │   → ConflictInfo[] with conflict_type = "value_divergence"
  │
  ├─ Check 2: Path Overlap
  │   For each active decision in req.branch:
  │     If same domain AND glob_overlap(existing.paths, req.paths)
  │       AND existing.value != req.value
  │     → ConflictInfo with conflict_type = "scope_overlap"
  │
  ├─ Check 3: Cross-Branch
  │   For each active decision on OTHER branches:
  │     If same key AND different value
  │     → ConflictInfo with conflict_type = "cross_branch"
  │
  └─ Return ConflictReport { conflicts, checks_performed }
```

**Callers:** `gov_promote` (internal check), CLI (`edda governance check`), Bridge (on `edda decide` hook)

### 5.2 `detect_drift`

Lightweight drift check for a single key/value — used during PR scans and `edda decide` hooks.

```typescript
function detect_drift(
  key: string,
  value: string,
  branch: string,
): DriftAlert | null;
```

**Returns `null` if no conflict.** Returns a `DriftAlert` if the proposed value contradicts an active decision.

**Callers:** Bridge (`edda hook claude` pre-commit scan), CI integration (future)

---

## 6. Coverage Operations

### 6.1 `compute_coverage`

Computes the full coverage report for a project.

```typescript
function compute_coverage(
  project_id: string,
  opts?: {
    stale_threshold_days?: number;    // default: 90
    churn_threshold?: number;         // default: 3 supersedes
  }
): CoverageReport;
```

**Implementation sketch:**

```text
compute_coverage(project_id, opts)
  │
  ├─ 1. Query all decisions (all statuses)
  │
  ├─ 2. Group by domain → compute DomainCoverage[]
  │     For each domain:
  │       active_decisions: count where status ∈ (active, experimental)
  │       total_decisions: count all
  │       churn_rate: superseded_count / total
  │       health: classify by churn_rate + last_decision_ts
  │
  ├─ 3. Find stale decisions
  │     WHERE status = 'active'
  │       AND age > stale_threshold_days
  │       OR (review_after IS NOT NULL AND review_after < now)
  │
  ├─ 4. Find high-churn keys
  │     GROUP BY key
  │     HAVING count(status = 'superseded') >= churn_threshold
  │
  ├─ 5. Compute summary stats
  │
  └─ 6. Return CoverageReport
```

**Callers:** CLI (`edda governance coverage`), Serve (`GET /governance/coverage`)

### 6.2 `find_stale_decisions`

Focused query for stale decisions only (lighter than full coverage report).

```typescript
function find_stale_decisions(opts?: {
  threshold_days?: number;            // default: 90
  include_approaching?: boolean;      // default: true (60-89 days)
}): StaleDecision[];
```

**Callers:** CLI (`edda governance stale`), scheduled checks

---

## 7. Internal Functions (not exposed)

### 7.1 `validate_authority`

```typescript
function validate_authority(
  actor: TransitionActor,
  transition: string,
): { ok: boolean; error?: string };
```

Checks the authority matrix from `canonical-form.md` section 4.

### 7.2 `check_supersede_needed`

```typescript
function check_supersede_needed(
  branch: string,
  key: string,
): DecisionRow | null;
```

Returns the existing active decision for `(branch, key)` if one exists, signaling that promotion should trigger auto-supersede.

**Implementation note:** This function checks `is_active = TRUE`, NOT `status == "active"`. Because `is_active = (status IN active, experimental)` (see Invariant I2 in `../decision-model/api.md`), this correctly catches `experimental` decisions that also need to be superseded when a new decision is promoted for the same key. Checking only `status == "active"` would miss experimental decisions, creating orphan active-like decisions for the same key.

### 7.3 `glob_overlap`

```typescript
function glob_overlap(
  paths_a: string[],
  paths_b: string[],
): boolean;
```

Returns `true` if any glob pattern in `paths_a` overlaps with any glob pattern in `paths_b`. Uses standard glob semantics:
- `**` matches any depth of directories
- `*` matches any single path segment
- Exact paths match literally

---

## 8. Error Handling

All operations return `MutationResult` (from `../decision-model/shared-types.md` section 3.1). Governance adds context to errors:

```typescript
// Error prefixes for debugging
const ERROR_PREFIXES = {
  authority: "authority_denied",       // caller lacks permission
  precondition: "precondition_failed", // status/field check failed
  transition: "illegal_transition",    // state machine violation
  not_found: "decision_not_found",     // event_id doesn't exist
  conflict: "conflict_detected",       // informational (not a failure)
} as const;
```

### Error examples

| Operation | Error | Message |
|-----------|-------|---------|
| `gov_promote` | Authority denied | `"authority_denied: human approval required for promotion"` |
| `gov_reject` | Illegal transition | `"illegal_transition: active -> rejected (only proposed can be rejected)"` |
| `gov_freeze` | Precondition | `"precondition_failed: reason is required for freeze"` |
| `gov_supersede` | Not found | `"decision_not_found: evt_01JX... does not exist"` |
| `gov_supersede` | Precondition | `"precondition_failed: old.key (db.engine) != new.key (auth.method)"` |

---

## 9. Canonical Examples

### Example 1: Promote with conflict detection and auto-supersede

```typescript
// Agent proposed a candidate, human approves from inbox
const result = gov_promote({
  event_id: "evt_01JF3Q...",
  requested_by: {
    authority: "human",
    human_approved: true,
  },
  affected_paths: ["crates/*/src/lib.rs"],
  tags: ["architecture", "error-handling"],
});

// Governance internally:
// 1. get_decision("evt_01JF3Q...") → status: "proposed", key: "error.pattern"
// 2. validate_authority: human_approved = true ✓
// 3. check_supersede_needed("main", "error.pattern") → existing active found
// 4. gov_supersede(old, new) → old becomes "superseded"
// 5. promote("evt_01JF3Q...") → status: "active"
// 6. Return: { ok: true, event_id: "evt_01JG...", superseded_id: "evt_OLD" }
```

### Example 2: Coverage report reveals issues

```typescript
const report = compute_coverage("proj_edda", {
  stale_threshold_days: 90,
  churn_threshold: 3,
});

// report.summary.stale_count = 2
// report.domains[1] = { domain: "auth", health: "stale", last_decision_ts: "2025-10-15..." }
// report.high_churn_decisions[0] = { key: "ui.framework", supersede_count: 4 }
//
// Human sees: "auth domain is stale, ui.framework is volatile — review needed"
```

### Example 3: Drift detection on edda decide hook

```typescript
// During `edda decide "error.pattern=custom_enum"`, Bridge calls:
const alert = detect_drift("error.pattern", "custom_enum", "main");

// alert = {
//   alert_id: "alert_01JH...",
//   trigger: "new_candidate",
//   conflicting_decision: { key: "error.pattern", value: "thiserror+anyhow", status: "active" },
//   proposed_change: { source: "decide_command", key: "error.pattern", value: "custom_enum" },
//   conflict_type: "value_divergence",
//   detected_at: "2026-03-19T10:00:00Z"
// }
//
// Bridge shows warning to user. User confirms → proceed with supersede flow.
```

---

## 10. Boundaries / Out of Scope

### In Scope

- 6 transition operations with authority validation
- 2 conflict operations (full report + lightweight drift)
- 2 coverage operations (full report + stale-only query)
- Error handling with structured prefixes
- Internal helpers (authority validation, supersede check, glob overlap)

### Out of Scope

- **Mutation contract internals** -- `../decision-model/api.md`
- **Decision query/search API** -- Injection spec
- **HTTP routing** -- `edda-serve` wraps these
- **CLI subcommand definitions** -- `edda-cli` implementation
- **LLM-assisted conflict explanation** -- future enhancement
- **Policy engine** (custom rules like "no changes during release") -- future layer

---

## Closing Line

> **10 operations, 3 categories, 1 invariant: every transition validates authority, checks preconditions, and writes an audit event. Governance is the single gateway for all decision state changes.**
