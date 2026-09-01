# Control

## Operating Mode

- Mode: proactive
- Current work package:
- Autonomy lane: Green / Yellow / Red
- Meaningful checkpoint:
- Stop condition:

## Command

### Registered Skill Prompt

```text
active goal run Use the verifiable-goal-loop skill for this project. Read goal.md, state.md, index.md, control.md, requirement-coverage.md if present, backlog.md, questions.md, oracles.md, verifier.md, and stage-plan.md if present. Complete the current work package, choosing safe next actions without asking. Produce evidence, run checks, update state.md and index.md, and stop only at checkpoint, failed verifier, Red decision, P0/P1 blocker, or blocker.
```

### Standalone Prompt

```text
active goal run Read [skill path]/SKILL.md, then follow the project files in [workdir]. Complete the current work package, choosing safe next actions without asking. Produce evidence, run checks, update state.md and index.md, and stop only at checkpoint, failed verifier, Red decision, P0/P1 blocker, or blocker.
```

## Skill And Tool Routing

| need | selected skill/tool | reason | status |
|---|---|---|---|
|  |  |  |  |

## Safe Assumptions

-

## Approval Gates

-

## Hard Gates

- Charter Gate
- Requirement Coverage Gate
- Three-Party Question Gate
- Exhaustion Gate
- Oracle Freeze Gate
- Architecture Freeze Gate
- Stack Lock Gate
- Data Permission Gate
- Eval Gate
- Observability Gate
- Fresh Clone Gate
- Rollback Gate
- Completion Packet Gate

## Next Command To Run

-
