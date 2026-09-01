# Example: Demo Path Spec

> Source: `thyra/docs/world-design-v0/midnight-market-demo-path.md`
> Type: Demo Path — step-by-step walkthrough proving the system can close the loop

## What makes this good

### 1. Split into phases, each with kill criteria
- Phase 1: World Spine — if state can't even be built, stop
- Phase 2: Judgment Spine — if judge can't run, stop
- Phase 3: Living Surface — if pulse isn't perceivable, stop
- Phase 4: Outcome & Memory Closure — if outcome can't feed back into governance, stop

Each phase has an explicit "don't continue if this fails" condition.

### 2. Demo script has a timeline
6-minute demo:
- 0:00-1:00 World initialization
- 1:00-2:30 Observation + proposal
- 2:30-3:30 Judgment + apply
- 3:30-4:30 Pulse visible
- 4:30-6:00 Outcome + precedent

### 3. API calls are concrete
```
POST /api/v1/worlds
POST /api/v1/cycles/:id/proposals
POST /api/v1/proposals/:id/judgment
POST /api/v1/proposals/:id/apply
GET  /api/v1/worlds/:id/pulse
```

### 4. Each step says "what to look for"
Not just "execute this API" but "what you should see after execution."

### 5. Issue map links to engineering tasks
At the end, the demo path maps each phase to specific issues —
this is the bridging point from spec stack → project-plan.

---

> **Pattern notes:**
> - The core of a demo path: proving the system can close the loop
> - Split into phases, each with kill criteria (don't continue if it fails)
> - Demo script has a concrete timeline
> - Every API call includes expected results
> - Links to engineering issues at the end — this is the natural interface for promotion to project-plan
> - If you can't write a demo path = the system can't close the loop = can't enter project-plan
