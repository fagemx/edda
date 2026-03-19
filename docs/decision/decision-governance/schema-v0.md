# Schema v0 — Governance-specific types

> Status: `working draft`
>
> Purpose: Define the types that Governance owns — conflict classification, coverage metrics, governance-specific request/response types. Base types (DecisionStatus, ConflictInfo, ConflictType, MutationResult, TransitionError) live in `../decision-model/shared-types.md`.
>
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Governance owns three categories of types: transition requests, conflict detection results, and coverage/debt metrics. Everything else is imported from Decision Model.**

---

## 2. What It's NOT / Common Mistakes

### NOT a redefinition of shared types

`DecisionStatus`, `ConflictInfo`, `ConflictType`, `MutationResult`, `TransitionError` are all defined in `../decision-model/shared-types.md`. This file defines types that only Governance uses internally or exposes to callers.

### NOT a storage schema

Governance does not own tables. It reads from `decisions` (owned by Decision Model) and writes via the mutation contract. Coverage metrics are computed on-demand, not persisted.

### NOT explanation types

`ConflictInfo` contains structural facts. "Why this matters" and "what to do about it" are presentation-layer concerns. Governance types are judgment, not prose.

---

## 2a. Schema V10 Prerequisite

> **All types in this file that reference `affected_paths`, `tags`, `authority`, `status`, `review_after`, or `reversibility` require Schema V10 as a prerequisite.** These columns do not yet exist in the current schema (v9). The V10 migration must be applied before any Governance operations can use these fields. See `../decision-model/schema-v0.md` §5 for the migration SQL and §5a for the phased rollout plan.
>
> Specifically affected types: `PromoteRequest` (sets `affected_paths`, `tags`, `review_after`), `ConflictDetectionRequest` (checks `affected_paths`), `DriftAlert` (reads `status`), `CoverageReport` (aggregates by `status`), `StaleDecision` (reads `review_after`).

---

## 3. Transition Request Types

### 3.1 TransitionRequest

The standard request envelope for any Governance transition operation.

```typescript
type TransitionRequest = {
  event_id: string;                   // decision to transition
  requested_by: TransitionActor;      // who is requesting
  reason?: string;                    // optional human-readable reason
};

type TransitionActor = {
  authority: DecisionAuthority;       // from shared-types.md §1.2
  session_id?: string;                // agent session, if applicable
  human_approved: boolean;            // explicit human approval recorded?
};
```

### 3.2 PromoteRequest

Extends `TransitionRequest` with promote-specific fields.

```typescript
type PromoteRequest = TransitionRequest & {
  // Governance may enrich these during promotion
  affected_paths?: string[];          // override/set affected paths
  tags?: string[];                    // override/set tags
  review_after?: string;             // set review schedule
};
```

### 3.3 SupersedeRequest

Explicit supersede request (when not triggered by auto-supersede during promote).

```typescript
type SupersedeRequest = {
  old_event_id: string;               // decision to supersede
  new_event_id: string;               // replacement decision
  requested_by: TransitionActor;
  reason?: string;                    // why the old decision is being replaced
};
```

### 3.4 FreezeRequest

```typescript
type FreezeRequest = TransitionRequest & {
  reason: string;                     // REQUIRED: why freeze (e.g., "release freeze")
  unfreeze_after?: string;           // ISO 8601: optional scheduled unfreeze
};
```

---

## 4. Conflict Types (Governance-owned)

Base types (`ConflictInfo`, `ConflictType`) are in `../decision-model/shared-types.md` section 3.2. Governance extends these with detection context:

### 4.1 ConflictDetectionRequest

```typescript
type ConflictDetectionRequest = {
  key: string;                        // decision key to check
  value: string;                      // proposed value
  branch: string;                     // target branch
  affected_paths?: string[];          // proposed paths for scope_overlap check
};
```

### 4.2 ConflictReport

Full report from Governance's conflict detection pipeline.

```typescript
type ConflictReport = {
  request: ConflictDetectionRequest;
  conflicts: ConflictInfo[];          // from shared-types.md §3.2
  checked_at: string;                 // ISO 8601
  checks_performed: ConflictCheck[];  // which checks ran
};

type ConflictCheck = {
  check_type: "key_match" | "path_overlap" | "cross_branch";
  matches_found: number;
  duration_ms: number;                // for performance tracking
};
```

### 4.3 DriftAlert

Emitted when a new proposal or PR contradicts an active decision.

```typescript
type DriftAlert = {
  alert_id: string;                   // ULID
  trigger: DriftTrigger;
  conflicting_decision: {
    event_id: string;
    key: string;
    value: string;
    status: DecisionStatus;           // from shared-types.md §1.1
  };
  proposed_change: {
    source: "pr" | "proposal" | "decide_command";
    key: string;
    value: string;
    branch: string;
  };
  conflict_type: ConflictType;        // from shared-types.md §3.2
  detected_at: string;                // ISO 8601
};

type DriftTrigger =
  | "new_candidate"                   // create_candidate() found conflict
  | "pr_scan"                         // PR/commit scan found contradiction
  | "sync_import";                    // cross-project sync found divergence
```

---

## 5. Coverage & Debt Metrics

### 5.1 CoverageReport

Computed on-demand by Governance. Not persisted.

```typescript
type CoverageReport = {
  project_id: string;
  computed_at: string;                // ISO 8601
  summary: CoverageSummary;
  domains: DomainCoverage[];
  stale_decisions: StaleDecision[];
  high_churn_decisions: ChurnDecision[];
};

type CoverageSummary = {
  total_active: number;               // active + experimental decisions
  total_superseded: number;           // historical superseded count
  total_frozen: number;
  total_proposed: number;             // inbox backlog
  domains_covered: number;            // unique domains with active decisions
  stale_count: number;                // active decisions older than threshold
  avg_decision_age_days: number;      // mean age of active decisions
};
```

### 5.2 DomainCoverage

Per-domain health check.

```typescript
type DomainCoverage = {
  domain: string;                     // "db", "auth", "error", etc.
  active_decisions: number;
  total_decisions: number;            // including superseded/rejected
  last_decision_ts: string | null;    // most recent decision timestamp
  churn_rate: number;                 // supersede count / total decisions
  health: CoverageHealth;
};

type CoverageHealth =
  | "healthy"                         // recent decisions, low churn
  | "stale"                           // no decision change in > 90 days
  | "volatile"                        // churn_rate > 0.5 (frequently changing)
  | "uncovered";                      // domain detected in code but no decisions
```

### 5.3 StaleDecision

Active decision that hasn't been reviewed or touched in a long time.

```typescript
type StaleDecision = {
  event_id: string;
  key: string;
  value: string;
  status: DecisionStatus;
  age_days: number;                   // days since creation
  review_after: string | null;        // if set and past, even more stale
  staleness: "approaching" | "stale" | "critical";
};
```

**Staleness thresholds:**
- `approaching`: 60-89 days without review/change
- `stale`: 90-179 days
- `critical`: 180+ days

### 5.4 ChurnDecision

Decision key that has been superseded multiple times.

```typescript
type ChurnDecision = {
  key: string;
  domain: string;
  supersede_count: number;            // how many times this key was superseded
  latest_value: string;
  latest_event_id: string;
  first_decided_ts: string;
  latest_decided_ts: string;
  avg_lifetime_days: number;          // mean time before supersede
};
```

**Churn threshold:** `supersede_count >= 3` flags a key as high-churn.

---

## 6. Governance Event Tags

Events written by Governance use these tags for queryability:

```typescript
const GOVERNANCE_EVENT_TAGS = {
  promoted:   "decision_promoted",
  rejected:   "decision_rejected",
  superseded: "decision_superseded",
  frozen:     "decision_frozen",
  unfrozen:   "decision_unfrozen",
  trial:      "decision_trial",
  conflict:   "decision_conflict_detected",
  drift:      "decision_drift_alert",
} as const;
```

---

## 7. Canonical Examples

### Example 1: ConflictReport with value_divergence

```json
{
  "request": {
    "key": "error.pattern",
    "value": "custom_enum",
    "branch": "main",
    "affected_paths": ["crates/*/src/lib.rs"]
  },
  "conflicts": [
    {
      "existing_event_id": "evt_01JF3Q...",
      "existing_value": "thiserror+anyhow",
      "existing_branch": "main",
      "existing_authority": "human",
      "existing_ts": "2026-01-15T14:30:00Z",
      "conflict_type": "value_divergence"
    }
  ],
  "checked_at": "2026-03-19T10:00:00Z",
  "checks_performed": [
    { "check_type": "key_match", "matches_found": 1, "duration_ms": 2 },
    { "check_type": "path_overlap", "matches_found": 0, "duration_ms": 5 },
    { "check_type": "cross_branch", "matches_found": 0, "duration_ms": 3 }
  ]
}
```

### Example 2: CoverageReport showing domain health

```json
{
  "project_id": "proj_edda",
  "computed_at": "2026-03-19T10:00:00Z",
  "summary": {
    "total_active": 12,
    "total_superseded": 4,
    "total_frozen": 1,
    "total_proposed": 3,
    "domains_covered": 6,
    "stale_count": 2,
    "avg_decision_age_days": 45
  },
  "domains": [
    {
      "domain": "db",
      "active_decisions": 3,
      "total_decisions": 5,
      "last_decision_ts": "2026-03-01T10:00:00Z",
      "churn_rate": 0.4,
      "health": "healthy"
    },
    {
      "domain": "auth",
      "active_decisions": 1,
      "total_decisions": 1,
      "last_decision_ts": "2025-10-15T08:00:00Z",
      "churn_rate": 0.0,
      "health": "stale"
    }
  ],
  "stale_decisions": [
    {
      "event_id": "evt_01JE7X...",
      "key": "auth.method",
      "value": "none",
      "status": "active",
      "age_days": 155,
      "review_after": null,
      "staleness": "stale"
    }
  ],
  "high_churn_decisions": [
    {
      "key": "ui.framework",
      "domain": "ui",
      "supersede_count": 4,
      "latest_value": "svelte",
      "latest_event_id": "evt_01JG...",
      "first_decided_ts": "2025-11-01T10:00:00Z",
      "latest_decided_ts": "2026-03-10T10:00:00Z",
      "avg_lifetime_days": 32
    }
  ]
}
```

### Example 3: DriftAlert from PR scan

```json
{
  "alert_id": "alert_01JH...",
  "trigger": "pr_scan",
  "conflicting_decision": {
    "event_id": "evt_01JF3Q...",
    "key": "error.pattern",
    "value": "thiserror+anyhow",
    "status": "active"
  },
  "proposed_change": {
    "source": "pr",
    "key": "error.pattern",
    "value": "custom_enum",
    "branch": "feat/new-errors"
  },
  "conflict_type": "value_divergence",
  "detected_at": "2026-03-19T10:00:00Z"
}
```

---

## 8. Boundaries / Out of Scope

### In Scope

- Transition request/response types
- Conflict detection request/report types
- Drift alert types
- Coverage and debt metric types
- Governance event tags
- Staleness and churn thresholds

### Out of Scope

- **Base types** (DecisionStatus, ConflictInfo, etc.) -- `../decision-model/shared-types.md`
- **Decision object schema** -- `../decision-model/schema-v0.md`
- **SQLite migration** -- `../decision-model/schema-v0.md`
- **Presentation/rendering of metrics** -- UI/CLI layer
- **LLM-generated explanations** -- future enhancement

---

## Closing Line

> **Governance owns three type families: transition requests (how to ask), conflict reports (what was found), and coverage metrics (how healthy the landscape is). Base types come from Decision Model. Explanation comes from Injection. Governance deals in structural facts.**
