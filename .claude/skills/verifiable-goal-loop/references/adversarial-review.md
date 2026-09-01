# Adversarial Review

## Purpose

Prevent the executor from acting as both player and referee. The executor may draft, implement, and integrate, but it must not be the only judge of stage completion.

## Required Reviewers

Use three reviewers for Tier 2+ work and every Tier 3+ stage:

1. Product/PM reviewer: attacks user value, scope, MVP, non-goals, acceptance criteria, and decision ownership.
2. Architecture/Ops reviewer: attacks architecture, data flow, permissions, deployment, rollback, security, maintainability, and operations.
3. Verifier/Method reviewer: attacks evidence, oracle hardness, self-grading, stop conditions, drift detection, and premature completion.

If subagents are available and authorized, use them. If not, simulate the roles in separate sections and mark the review as simulated. Simulated review is weaker evidence and should not clear high-stakes gates alone.

## Reviewer Prompt Shape

Give each reviewer only the artifacts they need and ask for attack output, not praise:

```text
Read [artifact paths]. Attack the current plan from [role]. Output:
1. Five concrete vulnerabilities.
2. Ten questions that could change the plan.
3. Checks that are too soft or self-graded.
4. Hard gates required before advancing.
Do not edit files. Do not praise. Do not summarize generally.
```

## Integration Rule

The executor must create `reviews/adversarial-review-YYYY-MM-DD.md` with:

- source artifacts reviewed
- reviewer outputs or summaries
- accepted fixes
- parked questions
- rejected findings with reasons
- new Red decisions
- verifier/oracle updates needed
- advance decision

Critical findings cannot be ignored. If the executor rejects a finding, it must state a concrete reason and an observable check that keeps the risk controlled.

## Advance Rule

Advance only when:

- all critical findings are fixed, converted into Red decisions, or parked with explicit risk
- `questions.md`, `oracles.md`, `verifier.md`, `state.md`, and `index.md` reflect the review
- the review file states whether it used independent subagents, human review, or simulation

Do not mark a stage complete solely because the executor thinks the plan is good.
