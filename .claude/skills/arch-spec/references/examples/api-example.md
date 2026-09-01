# Example: API Spec

> Source: `thyra/docs/world-design-v0/world-cycle-api.md`
> Type: API — how the system connects externally

## What makes this good

### 1. API language must align with product language
Opens by stating: the API should not make people feel like they're operating jobs/tasks/workers/runs,
but rather like they're operating worlds/cycles/observations/judgments.

### 2. Canonical cycle maps directly to routes
```text
/worlds
/worlds/:id/cycles
/cycles/:id/observations
/cycles/:id/proposals
/proposals/:id/judgment
/proposals/:id/apply
/applied-changes/:id/rollback
/worlds/:id/pulse
/outcome-windows/:id
/precedents
/governance-adjustments
```

### 3. Resource graph shown as ASCII
```text
World
├─ has many Cycles
├─ has many PulseFrames
├─ has many AppliedChanges
└─ has many PrecedentRecords

Cycle
├─ belongs to World
├─ has one ObservationBatch
├─ has many ChangeProposals
└─ may trigger next Cycle
```

### 4. Every route has complete request/response JSON
Not just listing route paths — includes concrete request body and response body.

### 5. Follows the project's unified error envelope (THY-11)
```json
{ "ok": true, "data": { ... } }
{ "ok": false, "error": { "code": "...", "message": "..." } }
```

### 6. State transition diagram
Change proposal state transitions shown in ASCII diagram,
including all verdicts (approved, approved_with_constraints, deferred...).

---

> **Pattern notes:**
> - Core principle of an API spec: API language = product language = data model language
> - Every stage of the canonical cycle should correspond to a route
> - Resource graph uses ASCII tree
> - Every route has request + response examples
> - Error format must match the project's unified format
> - State transitions use diagrams, not prose
