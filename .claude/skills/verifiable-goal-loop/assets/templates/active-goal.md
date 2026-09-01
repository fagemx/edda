# Active Goal Prompt

Use one of these as the current `active goal run` prompt.

## If The Skill Is Registered

```text
active goal run Use the verifiable-goal-loop skill for this project. Read goal.md, state.md, index.md, control.md, requirement-coverage.md if present, backlog.md, questions.md, oracles.md, verifier.md, and stage-plan.md if present. Complete the current work package named in state.md. Choose safe next actions without asking. Produce or update the required evidence artifact, run applicable oracle and verifier checks, update state.md, append index.md, and continue inside the same package until checkpoint, failed verifier, Red decision, P0/P1 blocker, or blocker.
```

## If The Skill Is Standalone

Use this when the skill has not been installed or registered.

```text
active goal run Read [skill path]/SKILL.md, then follow the project files in [workdir]. Complete the current work package named in state.md. Choose safe next actions without asking. Produce or update the required evidence artifact, run applicable oracle and verifier checks, update state.md, append index.md, and continue inside the same package until checkpoint, failed verifier, Red decision, P0/P1 blocker, or blocker.
```

## Notes

- Replace the prompt when the stage changes.
- Keep `goal.md` stable unless the master objective changes.
- Do not use one active goal for an entire large project.
- Do not stop after a trivial subtask; stop at checkpoint, failed verifier, Red decision, or blocker.
- Do not advance with unanswered P0 requirements, unsigned exceptions, or executor-only closure evidence for high-impact gates.
