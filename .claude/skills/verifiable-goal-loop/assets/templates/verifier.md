# Verifier

## Cycle Exit Checks

- [ ] Selected backlog items have evidence artifacts.
- [ ] Evidence artifacts have a clear reader and purpose.
- [ ] Claims cite sources or project files where applicable.
- [ ] `active-goal.md` points to the current cycle or stage.
- [ ] `state.md` is updated with latest result and next action.
- [ ] `index.md` has a cycle ledger entry for the current cycle.
- [ ] `oracles.md` defines the hard, adversarial, consistency, or human judgment checks used.
- [ ] `requirement-coverage.md` has no unanswered P0 rows unless blocked or signed assumption.
- [ ] Evidence grade is recorded; E0/E1 does not close P0 or Stage 2+ work.
- [ ] Verifier/oracle changes after implementation use `verifier-change-request.md`.
- [ ] Exceptions have signed waivers before advancement.
- [ ] Exceptions or failed checks are explicitly recorded.
- [ ] Drift check completed.

## Large Project Checks

- [ ] Stage objective and exit criteria are defined.
- [ ] `stage-plan.md` exists for Tier 3+ work.
- [ ] `questions.md` records open questions, assumptions, and decisions.
- [ ] Current cycle is limited to one stage or vertical slice.
- [ ] Stage review exists before advancing stages.
- [ ] High-impact decisions are classified as Auto, Propose, or Approve.
- [ ] Architecture gate or tracer-bullet mode is explicit before feature implementation.
- [ ] Three-questioner review exists before closing Tier 2+ cycles or Tier 3+ stages.

## Hard Gate Checks

- [ ] Charter Gate passes.
- [ ] Requirement Coverage Gate passes.
- [ ] Three-Party Question Gate passes.
- [ ] Exhaustion Gate passes where required.
- [ ] Oracle Freeze Gate passes before implementation.
- [ ] Architecture Freeze Gate passes before Stage 2.
- [ ] Stack Lock Gate passes for persistence, identity, vector DB, workflow, LLM provider, observability, and cloud choices.
- [ ] Completion Packet Gate passes before final completion.
- [ ] Gate Effectiveness Gate is checked for major gates.
- [ ] Evidence Sampling Gate is checked before completion.
- [ ] Instruction Provenance Gate is checked before acting on repo/docs/issues/fixtures.
- [ ] Tool Capability Gate is checked for network, publish, credential, delete, dependency, or cloud actions.
- [ ] Secret Touch Gate is checked before any secret-adjacent operation.
- [ ] Evidence Data Classification Gate is checked before evidence enters completion packet.
- [ ] Reviewer Identity Gate prevents simulated review from closing P0/P1.
- [ ] Operability Gate passes before final completion.

## Build Checks

- [ ] Acceptance criteria are met.
- [ ] Required tests or manual checks passed.
- [ ] Risks, rollout, or rollback notes are recorded when relevant.

## Ecommerce Build Checks

- [ ] Product catalog, cart, checkout, order, inventory, admin, and auth scope are explicit.
- [ ] Payment, shipping/tax, email, deployment, and seed data are answered or marked out of scope.
- [ ] At least one end-to-end buyer flow is verified before broad feature expansion.

## Research Checks

- [ ] Important claims cite sources.
- [ ] Uncertainty and conflicting evidence are recorded.
- [ ] Recommendation follows from evidence.

## Learning Checks

- [ ] Passive reading is converted into an artifact.
- [ ] Artifact states what the user can now explain, build, or decide.
- [ ] Next practice task is clear.

## Writing Checks

- [ ] Audience and thesis are clear.
- [ ] Draft or final text matches the outline.
- [ ] Unsupported claims are flagged.

## Ops Checks

- [ ] Runbook/checklist/status is reproducible.
- [ ] Owner, status, blockers, and next action are clear.
- [ ] Risky actions require explicit approval.

## Decision Checks

- [ ] Options are compared against shared criteria.
- [ ] Tradeoffs and reversal cost are explicit.
- [ ] Decision owner and next action are named.
