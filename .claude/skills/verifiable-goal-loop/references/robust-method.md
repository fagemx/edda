# Robust Method

## Independence

This skill is an independent Codex method. It can borrow Lulin-style control ideas, but it must not depend on the Lulin repository, Lulin skills, or a specific folder layout at runtime.

When a project already has Lulin files, treat them as authority:

| Existing project file | Use as |
|---|---|
| `LOOP.md` | master operating contract |
| `INDEX.md` | progress ledger and change history |
| `QUESTIONS.md` | questions, decisions, and blockers |
| `BACKLOG.md` | prioritized work queue |

In that mode, this skill only adapts the project for Codex `active goal run`: generate or update `active-goal.md`, preserve the existing contract, and avoid replacing project files unless the user asks.

## Control Planes

Keep five control planes separate:

1. Objective: `goal.md` defines what success means.
2. Execution: `active-goal.md` defines the current `active goal run` only.
3. State: `state.md` names the current unit and next action.
4. Ledger: `index.md` records cycle history, metrics, and durable progress.
5. Verification: `oracles.md` defines resistance; `verifier.md` defines pass/fail checks.

Do not collapse these into one prompt. A long prompt is not a long-running system.

## Bootstrap

For a new project:

1. Classify profile and scale tier.
2. Write `goal.md` with definition of done and non-goals.
3. Write `oracles.md` before execution starts.
4. Write `backlog.md` with small, verifiable units.
5. Write `state.md` with exactly one active unit.
6. Write `index.md` with a blank cycle ledger.
7. Write `active-goal.md` for the current unit only.

For large build work, insert an architecture gate before implementation. For owner-led exploratory work, use tracer-bullet mode and record owner reactions as the oracle.

## One-Unit Loop

Each `active goal run` run must:

1. Read state and contract files.
2. Select exactly one unit.
3. Produce or modify evidence.
4. Run verifier and oracle checks.
5. Integrate findings into the current unit only.
6. Write `state.md` and append `index.md`.
7. Stop.

Never start the next unit just because time remains.

## Oracle Ladder

Use the hardest available oracle:

1. Automated test, linter, executable check, or reproducible dry run.
2. Source-backed fact check or real external data.
3. Adversarial challenge against spec, assumptions, and edge cases.
4. Consistency scan against previous project files.
5. Human judgment or owner reaction.

For high-stakes decisions, agent simulation can prepare the decision but cannot approve it.

## Governance Modes

Use strict mode when output affects production, money, users, reputation, security, legal risk, or public launch:

- approval points block execution
- human-signature decisions are recorded
- risky actions require dry-run evidence
- repeated failures stop the loop

Use unattended mode only for low-stakes experiments:

- execution can continue with pending validation
- metrics and warnings still stay in `index.md`
- simulated evidence must remain labeled as simulated

## Build Projects

Build projects need architecture resistance before implementation because tests can pass for the wrong product.

Architecture gate outputs:

- users and scenarios
- non-goals
- module boundaries
- data flow and source of truth
- data model
- auth, integrations, deployment
- test strategy
- first vertical slice
- risk and rollback notes

Spec challenge must attack:

- boundary cases
- simpler designs
- maintenance in six months
- failure recovery
- hidden scope

After the architecture gate, the loop can fill backlog items. Architecture changes return to the gate.

## Failure Signals

Stop and diagnose when any signal appears:

- same failure repeats three times
- state claims progress but evidence did not change
- verifier becomes softer to let work pass
- current work no longer maps to a backlog item
- open questions grow while closed questions stay at zero
- implementation changes architecture without a recorded decision
- several cycles produce no contradiction, new question, or failed check
