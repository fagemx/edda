# A1 — One operating flow, cohesive bundles and rolling progress

> Owner: strong workflow implementer; retain this owner for A2, not necessarily A3.
> Depends on: none. Unblocks: A2. Delivery: usable guidance and entry migration.
> Contract: DF-01/02/09/10/11 in [CONTRACT](../CONTRACT.md).

## Read first / basis

Read [WORKFLOW](../WORKFLOW.md) first: it owns routing, task lifecycle, backend adapters,
issue timing and adoption semantics. Then [SPEC](../SPEC.md) sections 3–5 and V1/V6 in
[VALIDATION](../VALIDATION.md). No fresh decomposition of these already defined cards.

```bash
git rev-parse origin/main
edda task list
```

Ground inputs: existing AGENTS, project/embedded coord skills, issue-pipeline/issue-action/
pr-review-loop, operator-runbook, `cmd_init.rs::{SKILLS,scaffold_skills}`;
READ `cmd_dispatch.rs::run_inner`, `cmd_dispatch_acp.rs` and `task_actions.rs` only to verify
adapter/lifecycle claims. Current base CI is READ evidence, not a fresh workspace rebuild.

## Scope

- `crates/edda-cli/src/skills/coord-orchestrate.md`: sole authored generic operating flow.
- `.claude/skills/coord-orchestrate/SKILL.md`: tracked identical projection.
- `.claude/skills/issue-pipeline/SKILL.md`, `issue-action/SKILL.md`,
  `pr-review-loop/SKILL.md`: entry/dispatch/return routing only; A2 owns self-check details.
- `AGENTS.md`: short entry route, obsolete task-start note and ambiguous local-freeze
  wording only; refer to canonical project policy, do not replace its safety contract.
- `docs/guides/operator-runbook.md`: entry/transport index and only necessary local policy
  clauses preserved from the old project skill. No copied REVIEW procedure.
- `scripts/test-delivery-guidance.sh` (new offline test, not launcher/wrapper).
- `crates/edda-cli/src/cmd_init.rs`: tests only, to prove host projection/parity/preservation;
  inline focused task/dispatch tests may be READ, not expanded into a runtime refactor.

No task/dispatch/control runtime change, universal --task-id, schema, scheduler, workflow
job/hook, new skill name, global installation or peer program edits. Do not force-track
.agents. No production change to init overwrite/detection behavior or new version service.

## Implementation

1. Add `delivery-flow/1` operating section to the canonical embedded skill. Distinguish
   assigned worker, controller resume, existing plan, ordinary solo work and real parallel
   formation. Do not route small tasks through formation merely to claim one entry.
2. Make project coord copy a projection. Inventory unique old clauses, preserve applicable
   local obligations via canonical policy/runbook, remove contradictions. Generic source
   defers to host verification/merge rules; Edda L0/CI/C5/R6 stays local.
3. Turn listed old entries into thin routes for delivery orchestration. Preserve useful
   issue-specific investigation and option meanings, especially --no-merge/--skip-plan.
   Delete their independent all-agent phase waits/fresh-fixer/merge loops, not just add a
   pointer beside contradictory instructions. Inbound affected callers still resolve.
4. Implement WORKFLOW's caller protocol in guidance, not new code. First select ONE
   repository-wide rail owner: manual named controller or existing reconciler. Never use
   per-plan rail-owner keys: current reconcile selects eligible tasks across plan_id. Manual
   mode scans all plans/peers and refuses task creation/
   launch unless scheduled/one-off reconcile and prior reconcile attempts are proven absent;
   reconcile mode owns its actual Codex/retry lifecycle and receives no manual/per-card-
   no-retry tasks. Add rail collision and mode-switch refusal cases to fixtures.
5. For manual mode, use exact-key `delivery.active.<plan>` decision + exact plan_id revision
   as the discoverable map. Fresh controller recovers via `edda ask` and task JSON without
   chat. Add reachable brief, dedup key/readback, prestart, backend launch, worker receipt,
   and same-ID Failed retry. Changed scope/owner gets next revision and remapped pending
   successors before decision supersession; never fake done. Reconcile mode may not rely
   on this map because its current planner does not filter it.
6. Task creation is backend-aware. ACP creation includes matching `--agent acp:<target>` and
   concrete existing scope roots before start; legacy/host task is separate. Fixture covers
   task-new -> readback -> start -> ACP preflight and rejects reuse of a legacy task.
7. Keep same chain with one owner; review-ready candidate proceeds without unrelated work.
   Existing source/claim permissions remain. Worktree isolation is not conflict immunity.
8. Replace direct gh merge path with canonical product path under existing controller R6.
   Independent verdict and current-head rules remain; task done means stated output, not
   obligatory PR/merge. No extra author checks or per-transition board ceremonies are added.
9. Remove imperative full-local-freeze and docs-only build-lane instructions in the listed
   sources/entry summaries. Point to canonical ladder: author focused L0, L1 exact-head CI,
   verifier only uncovered focused checks including applicable Windows C5. No full local
   workspace run solely because a SHA froze.
10. Specify existing init adoption, not magical update: fresh detected hosts get template;
    existing custom files remain. Owner updates selected copy explicitly; never automatically
    --force-skills all five. Marker/hash identifies what was read, not a new admission gate.
11. Add V1/V6 fixtures with positive/negative cases. Static checks inspect active procedural
    sections, allow quoted historical bad examples, catch duplicate active loops. Existing
    init tests prove actual projection bytes and preserved custom files. Isolated task
    fixtures prove repository-wide/cross-plan rail-owner refusal, deterministic active-map
    recovery, manual/reconcile separation, same-ID
    retries, ACP creation/preflight and dependency unlock. No actual model/network launch.

## Acceptance / commands

```bash
sh scripts/test-delivery-guidance.sh
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
git diff --check
```

New test is implemented here; not available before A1. V6 fixtures may use an explicitly
bound freshly built `EDDA_BIN` and temporary isolated ledger/stores; never main task rail.
Check every supported entry reaches equivalent next-action semantics; fake transports
validate arguments/lifecycle, not a real backend. Validate rail-mode conflict refusal from
actual reconcile planning behavior; do not claim prose gives mechanical mutual exclusion.

Run focused `cargo test -p edda` and `cargo clippy -p edda --all-targets -- -D warnings`
in the assigned lane: embedded content is a product blob and init tests compile. Follow
current L0 format/file-length checks. Read exact-head CI at freeze; no second full local
workspace run. V6 live agent observation is optional, separately authorized evidence;
absence is 'not observed', not a blocker on this guidance delivery.

## Delivery / rollback

Intent: `refactor(fleet): converge entry routes on one delivery flow`.
A1 can use coherent subcommits (source/routes, adapter guidance, fixtures); not a new task
per step. Continue directly to A2 in the same context before review if appropriate.
A3 remains independently schedulable. Plan publication does not activate this behavior;
actual source landing and selected host adoption are separate receipt facts.
If regression occurs, fix/revert this scope in a new commit; preserve customized installs,
peer sources and unmerged work. Do not restore unpinned merge or an old hidden phase loop.
