# Executable tracks

> Task IDs below are plan-local; no allocation of six GitHub issues or rail tasks is implied.
> Read [SPEC.md](SPEC.md) and [CONTRACT.md](CONTRACT.md) before implementation.

## Layers and dependency DAG

- Foundation work: caller/procedure simplification and owner release agreement.
- Integration work: optional review context, review memory and usable release cut.
- Observation work: one bounded Flash attempt; not a release gate.

These are planning groups, not the repository L0/L1/L2 verification ladder.

```text
A1 -> A2 -> C1
A3 (independent)
B1 -> B2
```

Only these arrows are required. A3 and B1 may start with A1; B2 need not wait for A3 or C1.
C1 need not wait for B2, S5, node or control. Shared owner capacity may schedule independent
cards serially; do not encode that as an artifact dependency.
Two compile-needed sessions use different allowed lanes; do not inherit one shared target.

## Task cards / inputs / outputs

| Card | Owner profile | Input | Output | Depends on | Can begin |
|---|---|---|---|---|---|
| [A1](tasks/A1-rolling-bundles.md) | strong workflow implementer | current source + DF-01/02/09 | cohesive rolling skill path + offline fixture | none | immediately |
| [A2](tasks/A2-review-continuity.md) | same A owner | A1 source + actual review carrier/resume behavior | optional context-file + two author lenses + native handoff | A1 | A1 candidate available |
| [A3](tasks/A3-convention-policy.md) | A owner or disjoint policy worker | base REVIEW semantics | U3-only advisory rule + runner regression tests | none | immediately, capacity permitting |
| [B1](tasks/B1-release-cut.md) | existing continuity controller | task113 / GH1141 / current stack | owner-adopted usable-slice amendment | none | read-only now; edit after permission |
| [B2](tasks/B2-continuity-delivery.md) | existing continuity owner + integrator | B1 amendment + accepted local candidates | current-base usable continuity PR(s) | B1 | adoption and source ownership settled |
| [C1](tasks/C1-flash-pilot.md) | strong planner + one Flash author + independent reviewer | A2 guidance + frozen historical bug + selected runtime | result/evidence/cost report, no duplicate product PR | A2 | runtime/timeout/spend bound |

A1–A3 may be one coherent PR if same owner completes them before external review;
otherwise separate usable PRs. No need for three independent research sessions or three
forced review rounds. C1 does not need A2 merged to trial its candidate guidance; it must
record that guidance SHA as experimental, not current repository policy.

## Path/owner map (planned changes, not present pack changes)

| Surface | Writer / task | Responsibility |
|---|---|---|
| `.claude/skills/issue-pipeline/SKILL.md` | A owner / A1 then A2 | rolling dependencies, canonical merge, follow-up resume |
| `.claude/skills/coord-orchestrate/SKILL.md` | A owner / A1 then A2 | project-specific decomposition and shared facts |
| `crates/edda-cli/src/skills/coord-orchestrate.md` | A owner / A1 then A2 | shipped generic guidance, no repository-specific R6 enforcement |
| `docs/guides/operator-runbook.md` | A owner / A1 then A2 | route table, not another duplicated procedure |
| `.claude/skills/issue-action/SKILL.md` / `pr-review-loop/SKILL.md` | A owner / A2 | update only conflicting author/fixer self-check guidance |
| `scripts/test-delivery-guidance.sh` (new test only) | A owner / A1 then A2 | static positive/negative guidance regression checks |
| `crates/edda-cli/src/cmd_review/{args,prepare,brief,mod,tests}.rs` and direct struct fixtures | A owner / A2 | optional bounded context input/notes, no authority/schema change |
| `docs/reference/cli.md` | A owner / A2, then agreed B2 consumer-doc handoff | review input reference; coordinate if B2 also needs edits |
| `REVIEW.md` / `scripts/review-l0.sh` | A3 owner | U3 convention semantics only |
| `scripts/test-review-l0.sh` | A owner / A3 | preserve failures, add U3 nonblocking case |
| GH1141 plan + `docs/superpowers/continuity/program.md` | existing continuity owner / B1 | release amendment, no acceptance erasure |
| existing continuity source/skills/spec/consumer scope | existing owner/integrator / B2 | adopt and integrate candidate; card lists exact surfaces |
| `sdk/**`, producer contract | current SDK owner only, B2 requested seam | slice-specific contract parity, not silent cross-scope work |
| `docs/evidence/delivery-first/pilot.md` (new evidence) | C owner / C1 | pilot facts/results, not production schema |
| disposable historical repair checkout | C author / C1 | bounded code experiment; never current shared checkout |

No automatic edits to `.agents/`; use installed projection mechanisms only.
A2 extends existing review modules with one optional CLI flag. No new endpoint/event is
planned by A/C. B reuses existing modules. Shared CLI documentation changes require a
narrow section handoff, not competing writes or a whole-track barrier.

## Responsibility graph (not a second runtime architecture)

```text
issue/acceptance -> controller's bundle plan -> author session
                           |                      |
                           |                  facts/evidence
                           |                      v
                    existing task rail -> independent reviewer session
                                                   |
                                           existing review verdict
                                                   |
                                      existing canonical R6 merge
```

A changes callers plus optional review context, not task/dispatch/control internals.
B owns feature integration.
C consumes those existing interfaces and reports gaps; it must not grow a new harness.

## Common execution instructions

1. Read your card and linked contract/validation cases only; no need to rediscover all tracks.
2. Pin actual current base and inspect applicable CI/receipts; inspect active peers/claims.
3. Writer gets its own branch/worktree and the smallest accurate scope. Respect existing
   ownership; sharing a Git common dir does not isolate integration conflicts.
4. If a build is needed, controller binds `worker-1` or `worker-2` and the actual lane root;
   verifier uses its assigned verifier lane only for uncovered checks. Docs-only has no lane.
5. Keep one owner across a cohesive chain, one reviewer across rounds where transport allows.
6. Record changed behavior, known evidence and exceptions once in the durable carrier;
   link it from other messages rather than copy all prose.
7. Review/merge follows current rules, not this pack as a bypass. Missing optional facts is
   not a new hard gate. No direct `gh pr merge` fallback.

## Track acceptance / delivery units

- A: V1/V2/V3 cases pass; source projections agree; canonical merge and safety unchanged.
  Deliver one coherent guidance/policy PR or smaller usable PRs, independently of B/C.
- B: local save/restore/skills and required bundle/consumer behavior proven at current head;
  no S3–S10 all-program waiting. Requires B1 adoption, own CI and final verdict.
- C: one attempted task has honest input/runtime/outcome/verification/cost, including failure.
  No success-rate target, broad benchmark or C completion condition on A/B merge.

## Stop / return conditions

Stop only the affected action on scope collision, ambiguous external side effect,
missing paid-runtime allowance, or a newly exposed safety/compatibility defect.
Unknown model cost remains unknown. Two unproductive harness-only cycles end this attempt
with evidence; they do not create extra mandatory remediation tickets for other tracks.
