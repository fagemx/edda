# Verifiable Goal Loop Framework

## Principle

Long-running work becomes reliable when execution, state, ledger, and stopping criteria are separated. `active goal run` is the worker loop; `goal.md`, `state.md`, `index.md`, `backlog.md`, `oracles.md`, and `verifier.md` are the external control system.

## Why This Works

- `goal.md` keeps the master outcome stable.
- `state.md` survives context loss and names the next action.
- `index.md` records cycle history and metrics.
- `backlog.md` absorbs distractions without derailing the current cycle.
- `oracles.md` defines resistance that the executing agent does not fully control.
- `verifier.md` prevents self-graded completion.
- `evidence/` turns progress into inspectable artifacts.

## Universal Loop

1. Observe current state.
2. Pick exactly one next action.
3. Produce or update evidence.
4. Verify against objective checks.
5. Update state and append the ledger.
6. Continue, stop, or ask for input based on verifier and oracle results.

## Scale Handling

The method can coordinate large work, but a single active cycle should stay small. Large projects fail when "do the project" becomes the active instruction. Use:

- one master `goal.md`
- one durable `index.md`
- explicit `oracles.md`
- stage-level reviews
- cycle-level execution
- verifier checks per stage
- a question/decision log

### Practical Capacity

- Small: one bug fix, one memo, one feature, one study card. One cycle is enough.
- Medium: several related artifacts or a small app. Use 3-8 cycles.
- Large: full-stack app, repo modernization, research program, portfolio build. Use stages and reviews.
- Program: multiple apps or months of work. Split into separate loops and keep a coordination loop.

The method is not a substitute for missing product decisions, credentials, budget, or domain judgment. Those become explicit blockers or questions.

## Questioning System

Use questions as control surfaces:

- Intake: missing information that changes architecture, scope, risk, or acceptance criteria.
- Challenge: assumptions that may make the plan wrong.
- Review: whether evidence is good enough to advance.

Question quality bar:

- Ask fewer questions.
- Ask only questions that change the next plan.
- Record assumptions when proceeding.
- Convert unanswered questions into verifier checks or blockers.

## Drift Audit

Run a drift audit at stage boundaries and whenever execution feels too smooth for too long. Check:

- Goal drift: the current work no longer supports `goal.md`.
- Scope drift: new features entered the active cycle without backlog selection.
- Quality drift: artifacts exist but do not pass verifier checks.
- Architecture drift: implementation decisions contradict earlier ADRs or assumptions.
- Evidence drift: state says progress happened but no artifact changed.

When drift is found, pause execution, update state/backlog/verifier, and ask the user only if the direction changed.

## Lulin Compatibility

This method can borrow Lulin-style controls without depending on Lulin files. If a project already has `LOOP.md`, `INDEX.md`, or `QUESTIONS.md`, treat those as authority and use this skill as a Codex `active goal run` adapter. If no such files exist, use this skill's own `goal.md`, `index.md`, `questions.md`, `oracles.md`, and `verifier.md`.

## Profile Adaptation

### Build

Evidence: code diff, tests, screenshots, deployment notes, PR summary.

Verifier examples:

- tests pass
- app runs locally
- screenshots show target behavior
- acceptance criteria met
- rollback or risk notes recorded

For large full-stack builds, use these default stages:

1. Scope and acceptance criteria.
2. Architecture and data model.
3. End-to-end vertical slice.
4. Feature expansion.
5. Hardening and launch readiness.

For ecommerce specifically, do not skip decisions about catalog, cart, checkout, payment, orders, inventory, admin, auth, shipping/tax, email, deployment, seed data, and test data.

### Research

Evidence: source-backed memo, source table, finding confidence, open questions.

Verifier examples:

- claims cite sources
- uncertainty is explicit
- conflicting evidence is recorded
- recommendation follows from evidence

### Learning

Evidence: study notes, repo teardown, diagrams, ADRs, interview scripts.

Verifier examples:

- user can explain the concept
- artifact answers "what can I now build or decide?"
- passive reading is not counted as done

### Writing

Evidence: outline, draft, revision ledger, final copy, publishing checklist.

Verifier examples:

- audience and thesis are clear
- draft satisfies outline
- unsupported claims are flagged
- final checklist passes

### Ops

Evidence: runbook, audit log, checklist, status report.

Verifier examples:

- steps are reproducible
- owner/status/blockers are clear
- risky actions require approval

### Decision

Evidence: option matrix, ADR, risk table, recommendation.

Verifier examples:

- options share the same criteria
- tradeoffs are explicit
- decision owner is named
- reversal cost is recorded

## Stop Conditions

Good stop conditions:

- verifier passes
- required access is missing
- user judgment is needed
- repeated failure has reproduction details
- irreversible action needs approval

Bad stop conditions:

- "I worked on it"
- "the plan looks good"
- "the agent thinks it is complete"
- "I read enough"
