# BRIEF_SCHEMA: Karvi ↔ Edda Field Mapping

This document defines the canonical field mapping between **karvi task-engine** (management layer) and **edda conductor** (execution layer). It is the source of truth for implementing `runtime-edda.js` (#124) and the edda brief schema (#125).

## Overview

```
karvi board.json
  │
  │  buildDispatchPlan(task, { runtimeHint: "edda" })
  ▼
┌──────────────────────────────────────────────┐
│  DispatchPlan                                 │
│  { taskId, message, timeoutSec, artifacts }   │
└──────────┬───────────────────────────────────┘
           │
           │  runtime-edda.js
           │  translates to plan YAML + spawns conductor
           ▼
┌──────────────────────────────────────────────┐
│  edda conductor                               │
│  phases → checks → retries → events.jsonl     │
└──────────┬───────────────────────────────────┘
           │
           │  runtime-edda.js tails events.jsonl
           │  translates events to brief PATCHes
           ▼
┌──────────────────────────────────────────────┐
│  PATCH /api/brief/:taskId                     │
│  { phases, currentPhase, cost, ... }          │
└──────────────────────────────────────────────┘
           │
           │  PlanCompleted / PlanAborted
           ▼
┌──────────────────────────────────────────────┐
│  karvi task.status update                     │
│  completed / blocked                          │
└──────────────────────────────────────────────┘
```

**Bridge point**: edda conductor writes structured events to `.edda/conductor/{plan_name}/events.jsonl`. runtime-edda.js tails this file — no stdout parsing, no Rust changes needed.

---

## 1. Glossary

| Karvi Term | Edda Term | Scope |
|------------|-----------|-------|
| Task | Plan | Work unit (1 task = 1 plan execution) |
| task.status | PlanStatus | Lifecycle state |
| — | Phase | Sub-step within a plan (no karvi equivalent) |
| Brief (scoped board) | — | Domain-specific working memory (JSON file) |
| DispatchPlan | — | Dispatch instruction from karvi to runtime |
| — | PlanState | Persisted execution state (`.edda/conductor/{name}/state.json`) |
| task.dispatch.state | — | Dispatch lifecycle (prepared → dispatching → completed/failed) |
| task.depends | phase.depends_on | Dependencies (inter-task vs intra-plan) |
| task.skill | Plan template | Determines plan structure |
| task.spec | plan.purpose | Goal / driving prompt |
| Signals / Insights / Lessons | Ledger events | Learning and history |

---

## 2. DispatchPlan → Edda Plan

### Field Mapping

| DispatchPlan field | → Edda Plan field | Notes |
|---|---|---|
| `taskId` | `tags: ["karvi:{taskId}"]` | Tracking only |
| `planId` | `tags: ["dispatch:{planId}"]` | Tracking only |
| `message` | `purpose` | Injected into every phase prompt |
| `timeoutSec` | `timeout_sec` | Plan-level timeout |
| `controlsSnapshot.max_review_attempts` | `max_attempts` | Phase retry limit |
| `artifacts[].summary` | `phases[0].context` | Upstream context injection |
| `requiredSkills` | `phases[].allowed_tools` | Tool filtering |
| `mode` ("redispatch") | `env: { REDISPATCH: "1" }` | Retry signal |

### Fields NOT Mapped (karvi-only, not in edda plan)

| Field | Reason |
|---|---|
| `runtimeHint` | Selector for which runtime to use — not part of plan |
| `agentId` | Edda manages its own agent sessions |
| `modelHint` | Passed as env var if needed, not a plan field |
| `codexRole` | Not applicable — edda uses Claude Code |
| `sessionId` | Edda generates deterministic UUIDv5 per phase+attempt |
| `upstreamTaskIds` | Resolved into `artifacts` before dispatch |
| `controlsSnapshot.auto_review` | Karvi handles review post-completion |
| `controlsSnapshot.quality_threshold` | Karvi handles quality gating |

### Example: DispatchPlan → plan.yaml

**Input: karvi DispatchPlan**

```json
{
  "kind": "task_dispatch",
  "version": 1,
  "planId": "disp_abc123",
  "taskId": "T5",
  "mode": "dispatch",
  "runtimeHint": "edda",
  "agentId": "engineer_pro",
  "modelHint": null,
  "timeoutSec": 600,
  "message": "Add user authentication with JWT. Use middleware pattern.",
  "artifacts": [
    { "id": "T3", "title": "Setup database", "status": "approved", "summary": "PostgreSQL with JSONB, migrations in src/db/" }
  ],
  "requiredSkills": [],
  "codexRole": "worker",
  "controlsSnapshot": {
    "quality_threshold": 70,
    "auto_review": true,
    "auto_redispatch": false,
    "max_review_attempts": 3
  }
}
```

**Output: generated plan.yaml**

```yaml
name: karvi-T5
purpose: "Add user authentication with JWT. Use middleware pattern."
timeout_sec: 600
max_attempts: 3
tags:
  - "karvi:T5"
  - "dispatch:disp_abc123"

phases:
  - id: implement
    prompt: |
      Add user authentication with JWT. Use middleware pattern.

      ## Upstream Context
      T3 (Setup database): PostgreSQL with JSONB, migrations in src/db/
    check:
      - type: git_clean
        allow_untracked: true

  - id: test
    depends_on: [implement]
    prompt: |
      Write tests for the authentication implementation.
      Ensure all tests pass.
    check:
      - type: cmd_succeeds
        cmd: "cargo test"

  - id: docs
    depends_on: [implement]
    prompt: |
      Update documentation to reflect the new authentication feature.
    check:
      - type: file_contains
        path: README.md
        pattern: "auth"
```

---

## 3. Edda Brief Schema

The edda brief uses karvi's scoped board format (`meta` + `controls` + `log` skeleton) with edda-specific domain fields (`plan` + `phases` + `cost`).

```json
{
  "meta": {
    "boardType": "brief",
    "version": 1,
    "taskId": "T5",
    "runtime": "edda",
    "updatedAt": "2026-02-28T03:05:00Z"
  },

  "plan": {
    "source": "generated",
    "name": "karvi-T5",
    "totalPhases": 3,
    "budget_usd": 5.0
  },

  "phases": {
    "implement": {
      "status": "passed",
      "attempts": 1,
      "duration_ms": 120000,
      "cost_usd": 0.85,
      "startedAt": "2026-02-28T03:00:00Z",
      "completedAt": "2026-02-28T03:02:00Z"
    },
    "test": {
      "status": "running",
      "attempts": 2,
      "startedAt": "2026-02-28T03:02:05Z"
    },
    "docs": {
      "status": "pending"
    }
  },

  "currentPhase": "test",
  "completedPhases": 1,

  "cost": {
    "total_usd": 1.23,
    "by_phase": {
      "implement": 0.85,
      "test": 0.38
    }
  },

  "artifacts": ["src/auth.rs", "src/middleware.rs", "tests/test_auth.rs"],

  "log": [
    { "time": "2026-02-28T03:00:00Z", "agent": "conductor", "action": "plan_start", "detail": "3 phases" },
    { "time": "2026-02-28T03:00:00Z", "agent": "conductor", "action": "phase_start", "detail": "implement" },
    { "time": "2026-02-28T03:02:00Z", "agent": "conductor", "action": "phase_passed", "detail": "implement (120s, $0.85)" },
    { "time": "2026-02-28T03:02:05Z", "agent": "conductor", "action": "phase_start", "detail": "test (attempt 2)" }
  ]
}
```

### Field Reference

| Field | Type | Source | Update Frequency |
|---|---|---|---|
| `meta.boardType` | `"brief"` | Constant | Once |
| `meta.version` | `1` | Constant | Once |
| `meta.taskId` | string | DispatchPlan.taskId | Once |
| `meta.runtime` | `"edda"` | Constant | Once |
| `meta.updatedAt` | ISO8601 | Clock | Every PATCH |
| `plan.source` | `"generated"` | Constant | Once |
| `plan.name` | string | Generated plan name | Once |
| `plan.totalPhases` | number | Plan.phases.len() | Once |
| `plan.budget_usd` | number | Plan.budget_usd | Once |
| `phases.{id}.status` | string | Event type | Per phase event |
| `phases.{id}.attempts` | number | Event.attempt | Per PhaseStart |
| `phases.{id}.duration_ms` | number | PhasePassed.duration_ms | On pass |
| `phases.{id}.cost_usd` | number | PhasePassed.cost_usd | On pass |
| `phases.{id}.startedAt` | ISO8601 | PhaseStart.ts | Per PhaseStart |
| `phases.{id}.completedAt` | ISO8601 | PhasePassed.ts | On pass |
| `phases.{id}.error` | string | PhaseFailed.error | On fail |
| `phases.{id}.reason` | string | PhaseSkipped.reason | On skip |
| `currentPhase` | string | Latest PhaseStart.phase_id | Per PhaseStart |
| `completedPhases` | number | Count of Passed + Skipped | Per pass/skip |
| `cost.total_usd` | number | PlanCompleted.total_cost_usd | On plan complete |
| `cost.by_phase.{id}` | number | PhasePassed.cost_usd | Per pass |
| `artifacts` | string[] | Agent output (parsed) | Per pass |
| `log[]` | array | All events | Per event |

### Phase Status Values

Maps 1:1 from edda `PhaseStatus` enum:

| Brief `phases.{id}.status` | Edda `PhaseStatus` |
|---|---|
| `"pending"` | `Pending` |
| `"running"` | `Running` |
| `"checking"` | `Checking` |
| `"passed"` | `Passed` |
| `"failed"` | `Failed` |
| `"skipped"` | `Skipped` |
| `"stale"` | `Stale` |

---

## 4. Status Mapping

### Forward: Edda → Karvi task.status

| Edda PlanStatus | → Karvi task.status | Trigger |
|---|---|---|
| `Pending` | `dispatched` | Plan accepted, not yet started |
| `Running` | `in_progress` | First `PhaseStart` event |
| `Blocked` | `blocked` | Phase failed after max retries |
| `Completed` | `completed` | All phases passed or skipped |
| `Aborted` | `blocked` | User abort or budget exceeded |

### Forward: Edda → Karvi dispatch.state

| Edda PlanStatus | → Karvi dispatch.state |
|---|---|
| `Pending` | `dispatching` |
| `Running` | `dispatching` |
| `Completed` | `completed` |
| `Blocked` | `failed` |
| `Aborted` | `failed` |

### Forward: Edda errors → Karvi dispatch.lastError

| Edda ErrorType | → dispatch.lastError |
|---|---|
| `CheckFailed` | `"Phase {id} check failed: {message}"` |
| `AgentCrash` | `"Phase {id} agent crashed: {message}"` |
| `Timeout` | `"Phase {id} timed out"` |
| `BudgetExceeded` | `"Budget exceeded ($X spent of $Y)"` |
| `UserAbort` | `"Plan aborted by user"` |

### Reverse: Karvi actions → Edda effects

| Karvi Action | Edda Effect |
|---|---|
| Redispatch task | New `edda conduct run` (fresh plan) |
| Manual block | `edda conduct abort {plan}` |
| No action needed | Edda retries are internal — no karvi intervention |

---

## 5. Phase → Brief Progress Mapping

How each `events.jsonl` event translates to a brief PATCH:

| events.jsonl Event | → PATCH /api/brief/:taskId |
|---|---|
| `PlanStart { plan_name, phase_count }` | `{ plan: { name, totalPhases: phase_count }, completedPhases: 0 }` |
| `PhaseStart { phase_id, attempt }` | `{ phases: { [phase_id]: { status: "running", attempts: attempt, startedAt: ts } }, currentPhase: phase_id }` |
| `PhasePassed { phase_id, attempt, duration_ms, cost_usd }` | `{ phases: { [phase_id]: { status: "passed", attempts: attempt, duration_ms, cost_usd, completedAt: ts } }, completedPhases: N, cost: { by_phase: { [phase_id]: cost_usd } } }` |
| `PhaseFailed { phase_id, attempt, duration_ms, error }` | `{ phases: { [phase_id]: { status: "failed", attempts: attempt, error } } }` |
| `PhaseSkipped { phase_id, reason }` | `{ phases: { [phase_id]: { status: "skipped", reason } }, completedPhases: N }` |
| `PlanCompleted { phases_passed, total_cost_usd }` | `{ cost: { total_usd: total_cost_usd } }` |
| `PlanAborted { phases_passed, phases_pending }` | No brief update — task status changes instead |

**Note**: `completedPhases` counts phases with status `"passed"` or `"skipped"`.

---

## 6. Error/Retry Decision Tree

```
Phase check fails
│
├── attempts < max_attempts?
│   │
│   ├── YES → edda auto-retries internally
│   │         brief: phases.{id}.status remains "running", attempts++
│   │         karvi: no change (still in_progress)
│   │
│   └── NO → depends on plan.on_fail / phase.on_fail
│       │
│       ├── AutoRetry (exhausted) → plan status = Blocked
│       │   karvi task.status = "blocked"
│       │   karvi dispatch.state = "failed"
│       │   karvi dispatch.lastError = error message
│       │   karvi task.blocker = { reason: error, askedAt: now }
│       │
│       ├── Skip → phase status = Skipped, plan continues
│       │   brief: phases.{id}.status = "skipped"
│       │   karvi: no task status change
│       │
│       └── Abort → plan status = Aborted
│           karvi task.status = "blocked"
│           karvi dispatch.state = "failed"
│
├── AgentCrash → treated as non-retryable failure
│   Same flow as "max_attempts exhausted"
│
├── Timeout → treated as non-retryable failure
│   Same flow as "max_attempts exhausted"
│
└── BudgetExceeded → plan Blocked immediately (no retry)
    karvi task.status = "blocked"
    karvi dispatch.lastError = "budget exceeded"
```

**Key principle**: edda handles phase-level retries internally. Karvi only sees terminal outcomes (completed or blocked).

---

## 7. Complete Examples

### Example A: Happy Path

**1. Karvi dispatches task T5**

```json
{
  "kind": "task_dispatch",
  "version": 1,
  "planId": "disp_001",
  "taskId": "T5",
  "mode": "dispatch",
  "runtimeHint": "edda",
  "message": "Add user authentication with JWT.",
  "timeoutSec": 600,
  "artifacts": [],
  "controlsSnapshot": { "max_review_attempts": 3 }
}
```

**2. runtime-edda.js generates plan and spawns conductor**

Task status: `dispatched` → `in_progress`

**3. events.jsonl sequence**

```jsonl
{"seq":1,"ts":"2026-02-28T03:00:00Z","type":"PlanStart","plan_name":"karvi-T5","phase_count":3}
{"seq":2,"ts":"2026-02-28T03:00:01Z","type":"PhaseStart","phase_id":"implement","attempt":1}
{"seq":3,"ts":"2026-02-28T03:02:00Z","type":"PhasePassed","phase_id":"implement","attempt":1,"duration_ms":119000,"cost_usd":0.85}
{"seq":4,"ts":"2026-02-28T03:02:05Z","type":"PhaseStart","phase_id":"test","attempt":1}
{"seq":5,"ts":"2026-02-28T03:03:30Z","type":"PhasePassed","phase_id":"test","attempt":1,"duration_ms":85000,"cost_usd":0.42}
{"seq":6,"ts":"2026-02-28T03:03:35Z","type":"PhaseStart","phase_id":"docs","attempt":1}
{"seq":7,"ts":"2026-02-28T03:04:15Z","type":"PhasePassed","phase_id":"docs","attempt":1,"duration_ms":40000,"cost_usd":0.18}
{"seq":8,"ts":"2026-02-28T03:04:15Z","type":"PlanCompleted","phases_passed":3,"total_cost_usd":1.45}
```

**4. Brief PATCH sequence (8 PATCHes)**

```
PATCH 1: { plan: { name: "karvi-T5", totalPhases: 3 }, completedPhases: 0 }
PATCH 2: { phases: { implement: { status: "running", attempts: 1 } }, currentPhase: "implement" }
PATCH 3: { phases: { implement: { status: "passed", duration_ms: 119000, cost_usd: 0.85 } }, completedPhases: 1 }
PATCH 4: { phases: { test: { status: "running", attempts: 1 } }, currentPhase: "test" }
PATCH 5: { phases: { test: { status: "passed", duration_ms: 85000, cost_usd: 0.42 } }, completedPhases: 2 }
PATCH 6: { phases: { docs: { status: "running", attempts: 1 } }, currentPhase: "docs" }
PATCH 7: { phases: { docs: { status: "passed", duration_ms: 40000, cost_usd: 0.18 } }, completedPhases: 3 }
PATCH 8: { cost: { total_usd: 1.45 } }
```

**5. Terminal state**

- dispatch resolves: `{ code: 0 }`
- karvi task.status: `completed`
- karvi dispatch.state: `completed`

---

### Example B: Failure After Retries

**1. Same dispatch as Example A**

**2. events.jsonl sequence (test phase fails twice)**

```jsonl
{"seq":1,"ts":"...","type":"PlanStart","plan_name":"karvi-T5","phase_count":3}
{"seq":2,"ts":"...","type":"PhaseStart","phase_id":"implement","attempt":1}
{"seq":3,"ts":"...","type":"PhasePassed","phase_id":"implement","attempt":1,"duration_ms":119000,"cost_usd":0.85}
{"seq":4,"ts":"...","type":"PhaseStart","phase_id":"test","attempt":1}
{"seq":5,"ts":"...","type":"PhaseFailed","phase_id":"test","attempt":1,"duration_ms":60000,"error":"check failed: cmd `cargo test` exited 1"}
{"seq":6,"ts":"...","type":"PhaseStart","phase_id":"test","attempt":2}
{"seq":7,"ts":"...","type":"PhaseFailed","phase_id":"test","attempt":2,"duration_ms":55000,"error":"check failed: cmd `cargo test` exited 1"}
{"seq":8,"ts":"...","type":"PhaseStart","phase_id":"test","attempt":3}
{"seq":9,"ts":"...","type":"PhaseFailed","phase_id":"test","attempt":3,"duration_ms":50000,"error":"check failed: cmd `cargo test` exited 1"}
```

Plan status becomes `Blocked` (max_attempts=3 exhausted, on_fail=AutoRetry).

**3. Brief PATCH sequence**

```
PATCH 1: { plan: { ... }, completedPhases: 0 }
PATCH 2: { phases: { implement: { status: "running" } } }
PATCH 3: { phases: { implement: { status: "passed", ... } }, completedPhases: 1 }
PATCH 4: { phases: { test: { status: "running", attempts: 1 } } }
PATCH 5: { phases: { test: { status: "failed", error: "cmd `cargo test` exited 1" } } }
PATCH 6: { phases: { test: { status: "running", attempts: 2 } } }
PATCH 7: { phases: { test: { status: "failed", error: "cmd `cargo test` exited 1" } } }
PATCH 8: { phases: { test: { status: "running", attempts: 3 } } }
PATCH 9: { phases: { test: { status: "failed", error: "cmd `cargo test` exited 1", attempts: 3 } } }
```

**4. Terminal state**

- dispatch rejects: `{ code: 1, error: "Phase test failed after 3 attempts" }`
- karvi task.status: `blocked`
- karvi task.blocker: `{ reason: "Phase test failed: cmd `cargo test` exited 1", askedAt: "..." }`
- karvi dispatch.state: `failed`
- karvi dispatch.lastError: `"Phase test failed after 3 attempts: cmd `cargo test` exited 1"`

---

## 8. Relationship to Other Issues

| Issue | Relationship |
|---|---|
| #123 Engineering Brief | Edda-side materialized view (read-only, historical). **Complementary**: #123 aggregates ledger events after the fact; this brief is written in real-time during execution. |
| #124 runtime-edda adapter | **Consumer**: implements the mappings defined in this document. |
| #125 edda brief schema | **Subset**: #125 defines the karvi-side brief format; Section 3 of this doc is the edda variant. |

```
                    ┌─────────────────────┐
                    │ This doc (#127)      │
                    │ BRIEF_SCHEMA.md      │
                    │ (canonical mapping)  │
                    └──┬──────────┬────────┘
                       │          │
          implements   │          │  defines schema for
                       ▼          ▼
              ┌────────────┐  ┌────────────┐
              │ #124       │  │ #125       │
              │ runtime-   │  │ brief      │
              │ edda.js    │  │ schema     │
              └────────────┘  └────────────┘
                                   │
                          complements│
                                   ▼
                          ┌────────────┐
                          │ #123       │
                          │ task_briefs│
                          │ (history)  │
                          └────────────┘
```
