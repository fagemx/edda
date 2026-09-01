# Example: Canonical Form Spec

> Source: `thyra/docs/world-design-v0/canonical-cycle.md`
> Type: Canonical Form — the system's core repeating unit

## What makes this good

### 1. One sentence nails the essence
> **Thyra is not a task orchestrator, but a world governance runtime.**
> Its minimum operable unit is not a task, nor an agent turn,
> but a complete governance cycle of a world within a given time window.

### 2. Ten-stage canonical cycle with ASCII diagram
```text
WORLD → OBSERVE → PROPOSE CHANGE → JUDGE → APPLY / ROLLBACK
→ PULSE → OUTCOME WINDOW → PRECEDENT → LAW / CHIEF ADJUSTMENT → NEXT CYCLE
```

### 3. Every stage says "what it's NOT"
- OBSERVE is not drawing conclusions — it's producing observation material
- PROPOSE is not generic task dispatch — it's a world-level change proposal
- JUDGE doesn't just ask "can it be done" — it also asks about risk, simulation, escalation
- PULSE is not a dashboard visual effect — it's the minimum signal that the world is alive

### 4. Nine canonical artifacts listed at once
```text
1. world_snapshot.json    5. applied_change.json
2. observation_batch.json 6. pulse_frame.json
3. change_proposal.json   7. outcome_report.json
4. judgment_report.json   8. precedent_record.json
                          9. governance_adjustment.json
```

### 5. Has a concrete MVP slice
- 1 world (Midnight Market)
- 3 chiefs
- 5 change types
- 5 metrics
- 15 min cycle cadence

---

> **Pattern notes:**
> - The core of a canonical form spec is compressing the system's repeating unit into nameable stages
> - Every stage must say "what it is + what it's NOT"
> - Must have a canonical artifacts list
> - Must have an MVP slice showing the minimum viable version
> - ASCII diagrams are mandatory — can't just describe in text
