# Demo Path — Prove Governance controls the lifecycle end-to-end

> Status: `working draft`
>
> Purpose: Walk through concrete Governance scenarios — conflict detection, supersede orchestration, authority enforcement, coverage analysis — proving that all lifecycle control flows through Governance.
>
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Five scenarios, zero bypasses — every transition goes through Governance, every conflict is detected, every authority rule is enforced.**

---

## 2. What It's NOT

### NOT a unit test plan

This is a conceptual walkthrough to prove that Governance's API covers all lifecycle scenarios. Unit tests come during implementation.

### NOT a repeat of Decision Model's demo

Decision Model's demo proves the mutation contract works. This demo proves Governance correctly wraps those mutations with authority, conflict detection, and orchestration.

### NOT an integration test with Injection

This demo does not test whether decisions surface correctly. It tests whether Governance transitions them correctly and detects conflicts.

---

## 3. Setup

```text
Project: edda (this repo)
Branch: main
Schema: v10 (migration applied)
Existing decisions:
  evt_A: { key: "db.engine", value: "sqlite", status: "active",
           authority: "human", affected_paths: ["crates/edda-ledger/**"] }
  evt_B: { key: "auth.method", value: "none", status: "active",
           authority: "human", ts: "2025-10-15T08:00:00Z" }
  evt_C: { key: "error.pattern", value: "thiserror+anyhow",
           status: "proposed", authority: "agent_proposed" }
```

---

## 4. Demo Walkthrough

### Scenario 1: Agent proposal cannot self-promote

**Goal:** Prove that `agent_proposed` decisions require human approval.

```typescript
// Agent tries to promote its own proposal
const r1 = gov_promote({
  event_id: "evt_C",
  requested_by: {
    authority: "agent_proposed",
    session_id: "sess_01JF...",
    human_approved: false,
  },
});
```

**Expected result:**

```json
{
  "ok": false,
  "error": "authority_denied: human approval required for promotion"
}
```

**Verify:**
- `get_decision("evt_C")` → still `status: "proposed"` (unchanged)
- No event written
- Authority matrix: `agent_proposed` + `promote` → NO

---

### Scenario 2: Human promotes with auto-supersede

**Goal:** Prove that promoting a candidate with the same key as an active decision triggers supersede.

**Setup:** Create a candidate that conflicts with evt_A:

```typescript
// Intake created this candidate earlier
// evt_D: { key: "db.engine", value: "postgres", status: "proposed",
//          authority: "agent_proposed" }

const r2 = gov_promote({
  event_id: "evt_D",
  requested_by: {
    authority: "human",
    human_approved: true,
  },
  affected_paths: ["crates/edda-ledger/**", "crates/edda-serve/**"],
  tags: ["architecture", "storage"],
  review_after: "2026-09-01",
});
```

**Expected result:**

```json
{
  "ok": true,
  "event_id": "evt_E_promote",
  "decision_id": "evt_D",
  "superseded_id": "evt_A"
}
```

**Verify step by step:**

```text
1. get_decision("evt_D") → status: "proposed" ✓ (precondition met)
2. validate_authority: human, human_approved=true ✓
3. check_supersede_needed("main", "db.engine")
   → found evt_A (active, same key, same branch)
4. gov_supersede({ old: "evt_A", new: "evt_D" })
   → evt_A: status = "superseded", is_active = false
5. promote("evt_D")
   → evt_D: status = "active", is_active = true
   → authority: "agent_proposed" → "agent_approved"
   → supersedes_id = "evt_A"
   → affected_paths = ["crates/edda-ledger/**", "crates/edda-serve/**"]
6. Provenance: evt_D.refs.provenance includes { target: "evt_A", rel: "supersedes" }
7. Events written: decision_superseded + decision_promoted (2 events)
```

**State after:**

```json
{
  "evt_A": { "status": "superseded", "is_active": false },
  "evt_D": { "status": "active", "is_active": true,
             "authority": "agent_approved",
             "supersedes_id": "evt_A",
             "affected_paths": "[\"crates/edda-ledger/**\", \"crates/edda-serve/**\"]",
             "review_after": "2026-09-01" }
}
```

---

### Scenario 3: Conflict detection — three checks

**Goal:** Prove all three conflict detection checks work.

#### Check 1: Value divergence (same key, different value)

```typescript
const report1 = detect_conflicts({
  key: "db.engine",
  value: "mysql",
  branch: "main",
});
```

**Expected:**

```json
{
  "conflicts": [
    {
      "existing_event_id": "evt_D",
      "existing_value": "postgres",
      "existing_branch": "main",
      "existing_authority": "agent_approved",
      "conflict_type": "value_divergence"
    }
  ],
  "checks_performed": [
    { "check_type": "key_match", "matches_found": 1 },
    { "check_type": "path_overlap", "matches_found": 0 },
    { "check_type": "cross_branch", "matches_found": 0 }
  ]
}
```

#### Check 2: Scope overlap (different key, overlapping paths)

```typescript
// Suppose evt_F exists: { key: "storage.format", value: "json",
//   status: "active", affected_paths: ["crates/edda-ledger/src/**"] }

const report2 = detect_conflicts({
  key: "storage.format",
  value: "msgpack",
  branch: "main",
  affected_paths: ["crates/edda-ledger/src/sqlite_store.rs"],
});
```

**Expected:**

```json
{
  "conflicts": [
    {
      "existing_event_id": "evt_F",
      "existing_value": "json",
      "conflict_type": "scope_overlap"
    }
  ],
  "checks_performed": [
    { "check_type": "key_match", "matches_found": 1 },
    { "check_type": "path_overlap", "matches_found": 1 },
    { "check_type": "cross_branch", "matches_found": 0 }
  ]
}
```

#### Check 3: Cross-branch divergence

```typescript
// On branch "feat/new-db", someone decided db.engine=mysql
// Meanwhile main has db.engine=postgres

const report3 = detect_conflicts({
  key: "db.engine",
  value: "postgres",
  branch: "feat/new-db",
});
```

**Expected:**

```json
{
  "conflicts": [
    {
      "existing_event_id": "evt_D",
      "existing_value": "postgres",
      "existing_branch": "main",
      "conflict_type": "cross_branch"
    }
  ]
}
```

---

### Scenario 4: Freeze, attempt illegal transition, then unfreeze

**Goal:** Prove freeze/unfreeze works and illegal transitions are blocked.

#### Step 1: Freeze

```typescript
const r3 = gov_freeze({
  event_id: "evt_D",
  requested_by: { authority: "human", human_approved: true },
  reason: "release freeze — no infra changes until v2.0 ships",
  unfreeze_after: "2026-04-01",
});
```

**Expected:** `{ ok: true }`, evt_D status = `frozen`, is_active = `false`.

#### Step 2: Attempt illegal transition (frozen → proposed)

```typescript
const r4 = gov_trial({
  event_id: "evt_D",
  requested_by: { authority: "human", human_approved: true },
});
```

**Expected:**

```json
{ "ok": false, "error": "illegal_transition: frozen -> experimental" }
```

**Verify:** Only `frozen → active` (unfreeze) is legal from frozen. `frozen → experimental` is not in the state machine.

#### Step 3: Unfreeze

```typescript
const r5 = gov_unfreeze({
  event_id: "evt_D",
  requested_by: { authority: "human", human_approved: true },
  reason: "v2.0 shipped",
});
```

**Expected:** `{ ok: true }`, evt_D status = `active`, is_active = `true`.

---

### Scenario 5: Coverage analysis reveals stale and volatile decisions

**Goal:** Prove coverage analysis detects real issues.

```typescript
const coverage = compute_coverage("proj_edda", {
  stale_threshold_days: 90,
  churn_threshold: 3,
});
```

**Expected report (based on setup + scenario 2):**

```json
{
  "summary": {
    "total_active": 2,
    "total_superseded": 1,
    "total_proposed": 1,
    "stale_count": 1,
    "domains_covered": 3
  },
  "domains": [
    { "domain": "db", "active_decisions": 1, "total_decisions": 2,
      "churn_rate": 0.5, "health": "healthy" },
    { "domain": "auth", "active_decisions": 1, "total_decisions": 1,
      "churn_rate": 0.0, "health": "stale" },
    { "domain": "error", "active_decisions": 0, "total_decisions": 1,
      "churn_rate": 0.0, "health": "uncovered" }
  ],
  "stale_decisions": [
    { "event_id": "evt_B", "key": "auth.method", "value": "none",
      "age_days": 155, "staleness": "stale" }
  ],
  "high_churn_decisions": []
}
```

**Verify:**
- `auth` domain flagged stale (155 days, no review_after set)
- `error` domain flagged uncovered (only proposed decision, no active)
- `db` domain healthy despite supersede (churn_rate 0.5 < 0.5 threshold, recent activity)

---

## 5. Closure Proof

| Capability | Demonstrated | Scenario |
|-----------|-------------|----------|
| Authority enforcement (agent cannot self-promote) | YES | Scenario 1 |
| Promote with auto-supersede | YES | Scenario 2 |
| Authority escalation (agent_proposed → agent_approved) | YES | Scenario 2 |
| Provenance linking on supersede | YES | Scenario 2 |
| Conflict: value_divergence | YES | Scenario 3, Check 1 |
| Conflict: scope_overlap | YES | Scenario 3, Check 2 |
| Conflict: cross_branch | YES | Scenario 3, Check 3 |
| Freeze with reason | YES | Scenario 4, Step 1 |
| Illegal transition blocked | YES | Scenario 4, Step 2 |
| Unfreeze back to active | YES | Scenario 4, Step 3 |
| Coverage: stale detection | YES | Scenario 5 |
| Coverage: domain health | YES | Scenario 5 |
| Coverage: uncovered domain | YES | Scenario 5 |

---

## 6. What This Demo Does NOT Cover

| Gap | Covered By |
|-----|-----------|
| Candidate creation / extraction | Decision Model demo, Intake spec |
| Decision surfacing in packs | Injection spec |
| `trial()` flow (experimental lifecycle) | Future iteration |
| Cross-project drift (sync conflicts) | Existing `edda-ledger/src/sync.rs` |
| LLM-assisted conflict explanation | Future enhancement |
| Scheduled unfreeze (review_after automation) | Future: cron/scheduler layer |
| High-churn detection (needs 3+ supersedes) | Implicitly defined; needs more data |

---

## Closing Line

> **5 scenarios, 13 capabilities proven, zero bypasses. Governance is the sole gateway for every transition, every conflict check, and every coverage analysis. No decision changes state without Governance's approval.**
