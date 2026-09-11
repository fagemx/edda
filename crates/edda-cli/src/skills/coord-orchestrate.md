---
name: coord-orchestrate
description: Route delivery work, and coordinate two or more parallel implementing sessions when a real formation is warranted
---

# Coordination Orchestrate

<!-- delivery-flow/1 -->

This is the generic authored delivery flow. A tracked or installed host copy is
a projection, not a second procedure. Host and repository instructions retain
precedence for safety, verification, review and merge authority. This skill is
caller guidance; it adds no Edda scheduler, lock, approval service or runtime
role enforcement.

## Choose the entry before forming a fleet

Use the first matching route.

| Situation | Next action | Do not add |
|---|---|---|
| Assigned worker or reviewer, including a resumed session | Read `edda task show <id>`, its reachable brief, prior results and actual source; perform only that role | new planning, neighbouring tasks or a formation |
| Controller resuming an existing plan | Recover the rail owner, active map, task JSON, dispatch/session handles and source/PR evidence before selecting an action | replacement tasks or a relaunch merely because chat vanished |
| New request with usable acceptance or a plan | Reuse that acceptance; bind owners, scope and only material prerequisites | repeated discovery or one issue/task per step |
| Small single-owner change | Use ordinary implementation and the repository's existing review path | a rail, program or formation solely for ceremony |
| Cohesive multi-step change with one writer | Keep one author context; create one task only when delegation or persistence helps | one agent per function |
| Two or more genuinely parallel implementing sessions | One controller runs the formation sections below; workers stay on their assigned route | a new universal delivery command |
| Material acceptance is unknown | Do bounded discovery in the current context, then bind only the affected work | freezing unrelated ready work |

Issue entry flags preserve intent. `--skip-plan` means reuse already available
acceptance; it never means acceptance may be absent. `--no-merge` stops before
merge and never confers, expands or implies merge authority. An issue number is
a traceability input, not a reason to force one worker or PR per issue.

## Durable facts and isolation

| Layer | Carrier | Property |
|---|---|---|
| Truth | `edda task`, `edda decide`, issue/PR records | survives a session loss |
| Doorbell | `edda request`, host messaging | may drop; never the only copy |
| Isolation | one writer worktree per bundle | limits accidental writes; does not prevent merge or semantic conflicts |

Fix load-bearing facts in the truth layer before ringing a doorbell. Existing
claims and source permissions remain in force. Worktree isolation is not
conflict immunity: overlapping or unstable code chains use one owner or an
explicit serial handoff.

## Select exactly one repository rail owner

Before creating or launching tasks, select one owner for the whole repository
task rail, not one owner per plan. Current reconcile planning considers eligible
tasks across `plan_id`, while manual `task start` creates no reconcile lease.
Manual and reconcile callers therefore cannot safely co-own the rail.

### Manual rail

One named controller creates, selects, starts and dispatches; the worker
normally records done/fail. Before task creation or launch, scan all active
plans/tasks and peers, and prove the scheduled reconciler is absent or disabled,
no one-off reconcile process is live, and prior reconcile-owned attempts are
settled. An uninstall scheduler post-delete query is only one input; it does not
prove an already running child stopped. Unknown or conflicting evidence refuses
manual task creation, start, retry and dispatch.

Record and read back the one exact coordination key:

```bash
edda decide "delivery.rail-owner=manual:$CONTROLLER_SESSION" \
  --session "$CONTROLLER_SESSION" \
  --reason "repository-wide mode; scheduler/process/attempt evidence=<locations>"
edda ask "delivery.rail-owner" --json
```

This decision is discoverable coordination evidence, not authenticated
authority, mutual exclusion or an exactly-once guarantee. Later unauthorized
reconcile remains a product limitation; callers must not describe prose as
mechanical enforcement.

### Reconcile rail

For **legacy/uncontrolled tasks**, the configured reconciler owns the whole rail
and its actual Codex start/resume/requeue/retry/settlement lifecycle, including
global attempt limits. Do not manually start, dispatch or settle those attempts,
and do not enqueue Pi, ACP, host-subagent or per-card no-retry tasks for
reconcile to pick. The per-plan active map below does not filter the current
reconciler.

A task bound to an accepted `ExecutionBriefV1` is instead **controlled**. At the
current accepted product boundary, controlled reconcile validates and binds an
attempt but returns a descriptor with `execution: "none"`: it does not launch or
enter the legacy Codex runner. Direct controlled execution remains unavailable
until an authorized S6 capability exists. A literal `CONTROL_UNAVAILABLE`
result is only a future planned S6 contract, not current product evidence. A
descriptor, caller-authored event or ordinary task command is not launch
authority and must not be described as a promised launch.

Changing modes requires operator authorization, exact scheduler lifecycle when
installed, process/lease/attempt reconciliation, and no unresolved task side
effects. Unknown evidence refuses the switch. Never use `edda reconcile` merely
to inspect or recover a manual attempt.

## Manual active map and task creation

Manual mode uses a stable lower-case `PLAN_KEY`, a monotonic zero-padded
`PLAN_REV`, and exact `PLAN_ID="$PLAN_KEY/$PLAN_REV/$PLAN_SHA"`. Every task in
the revision uses that `plan_id` and an idempotency key
`$PLAN_ID/$CARD_ID`. Put source/worktree, exact write paths, acceptance,
exclusions, receiver role, evidence budget, and a build lane only when local
compilation occurs in a brief reachable from the worker's worktree.

Create only concrete work. `--after` means an actual predecessor artifact, not
membership in the same batch. Legacy/host and ACP tasks are separate records:

```bash
# Legacy or host-backed task: no ACP agent_kind claim.
edda task new "$TITLE" --assignee "$OWNER" --plan "$PLAN_ID" \
  --brief "$BRIEF_REL" --path "$WRITE_SCOPE" --key "$PLAN_ID/$CARD_ID"

# ACP alternative: matching kind and existing repository-relative concrete roots.
edda task new "$ACP_TITLE" --assignee "$ACP_OWNER" --agent "$ACP_TARGET" \
  --plan "$PLAN_ID" --brief "$ACP_BRIEF_REL" --path "$ACP_ROOT" \
  --key "$PLAN_ID/$ACP_CARD_ID"

edda task show "$TASK_ID" --json
edda task list --json
```

Capture the returned ID and read back every field. A repeated key only returns
the existing task; it does not update title, assignee, brief, scope or
predecessors. Compare JSON to intent rather than parsing human stderr. After the
complete intended map is verified, publish and read back its exact key:

```bash
edda decide "delivery.active.$PLAN_KEY=$PLAN_ID" \
  --session "$CONTROLLER_SESSION" \
  --reason "card-to-task: A1=#<id>; rail-owner=manual:<session>; replaces=<old-or-none>"
edda ask "delivery.active.$PLAN_KEY" --json
edda task list --json
```

A fresh controller starts with no chat assumptions: read exact
`delivery.rail-owner`, then one exact active-plan value, filter task-list JSON
by exact `plan_id`, show each selected ID, and verify card/source SHA,
dependencies, owner, scope, brief and observed artifacts. Missing, malformed or
conflicting data permits read-only recovery only and stops this plan's launch,
not unrelated work.

Changed owner, brief, scope or dependency graph is replacement, not retry.
First settle or reconcile the old live attempt. Create the next plan revision
and only still-needed pending tasks/successors, wire successors to the new
predecessor IDs, verify the new map, then supersede the active decision. Keep
old tasks as history: never fake them done and never put a replacement after a
failed old task. Refuse replacement while reconcile could still select old
Ready/Failed records.

## Manual dispatch adapters are not interchangeable

The controller starts the selected task immediately before launch, after rail,
map, input, capability and ownership checks. The worker reads the Running task
and does not start it again. `task new` and `task start` do not launch a model,
and dispatch `outcome=done` does not complete the task.

| Backend | Required carrier | Resume | Refused substitution |
|---|---|---|---|
| Pi | task-linked `--prompt-file` plus explicit `--cwd` | repeat real `--session-id`; preserve `--session-dir` if selected | no `--task-id`; no `--resume` |
| Claude | task-linked `--prompt-file` plus explicit `--cwd` | existing conversation uses the same `--session-id` with `--resume` | no `--task-id` |
| Codex | task-linked `--prompt-file` plus explicit `--cwd` | repeat the real `--session-id` and verify observed thread/session | no `--task-id`, `--resume`, or unsupported model/tool flags |
| ACP target | separately created task with matching `--agent acp:<target>`; `--task-id`; Running status; concrete existing repository-relative scope roots | ledger `task.session` | no prompt/session substitute, legacy budget/model/tool flags or `--detach` |
| Host subagent | task-linked brief in host task text | host-native continuity if observed, otherwise explicit replacement | no claim that host launch is an Edda dispatch/session |

For non-ACP dispatch, the prompt file is read verbatim; it must name the task,
attempt, source/worktree, reachable full brief and lifecycle owner. Use only
help/source-proven flags. Linked issue work still supplies the repository's
required claim inputs; omitting `--issue` must not evade an ownership rule.

ACP preflight checks task existence, Running status, matching kind and concrete
existing roots. It does not prove the brief was readable. The task brief
reference must be repository-relative; the injected content is bounded to 4096
bytes and can be marked unavailable or truncated. The worker must read the
reachable full card with its actual capabilities before editing. Refuse before
launch when the caller's readback finds an omitted `--agent`, a legacy task ID,
an unreachable brief or insufficient roots; do not broaden roots for
convenience. Caller-authored data, including an execution-brief event, does not
become controlled execution authority without the product-verifiable authority
seal required by the current product boundary.

Example alternatives, never both for one task:

```bash
edda task start "$LEGACY_TASK_ID"
edda dispatch --agent pi --prompt-file "$PROMPT_FILE" --cwd "$WORKTREE" \
  --model "$PI_MODEL" --timeout-sec "$TIMEOUT_SEC" --json

# Or start the separately-created ACP task, then:
edda dispatch --agent "$ACP_TARGET" --task-id "$ACP_TASK_ID" \
  --cwd "$WORKTREE" --json
```

## Completion, restart and retry

These rows are manual-mode caller discipline, not runtime role enforcement.
Worker and controller must not race to settle a live task.

| Observation | Truthful action |
|---|---|
| Ready with inputs bound | controller starts once and dispatches; worker reads Running |
| Running/Blocked/Done start refusal | inspect task, dependencies, attempt and result; no unconditional retry |
| Legacy/uncontrolled worker met the task's stated output | worker records ordinary `task done --receipt ... --evidence ...`; a candidate need not be a PR or merge |
| Controlled attempt is bound to an accepted `ExecutionBriefV1` | completion must come from the authorized product path with a validated `WorkReceiptV1`, the exact brief/session/attempt/lease/outcome correlation and the required S6 authority seal; ordinary `task done --evidence` is refused, and the current product fails closed while that authorized capability is unavailable |
| Definite stopped/prelaunch failure | worker fails, or controller fails only after observing the stop |
| Failed with the same assignment/brief/scope and authorized retry | controller starts the SAME ID; its attempt increments and existing successor edges remain |
| Running but silent, expired-looking lease or missing handle | inspect process, handle, source, task history and side effects; elapsed time alone never authorizes fail/relaunch |
| Dispatch done but task still Running or receipt inadequate | recover a truthful receipt or leave unverified; never infer acceptance/merge |
| Legacy/uncontrolled Done receipt metadata correction | ordinary `task done` may correct metadata without another execution or successor unlock |
| Controlled Done receipt correction | never use the legacy post-Done metadata correction path; preserve the seal-bound `WorkReceiptV1` history and use the authorized controlled lifecycle |
| Done but substantive repair required | create a linked fix task; preserve the old execution receipt |

There is no `task retry` subcommand. `task start` accepts Failed for same-ID
retry; Done cannot restart. Reuse a native session only when it is observed and
valid. A lost session is an explicit replacement identity, not a fabricated
resume. Unknown side effects stop only that attempt; independent bundles keep
moving.

## Rolling progression and formation

Only actual artifact dependencies wait. If A is review-ready, B is slow and C
requires A, send A to review now; B continues; C waits only for the exact A
artifact it needs. A reviewer queue limits review, not unrelated implementation.
Never impose an all-agent phase barrier.

For a real formation use one controller, one read-only verifier and the minimum
number of writers. With two or more workers, start the verifier early enough to
baseline acceptance and likely test poisons. Bundle by a cohesive code chain,
claim the smallest accurate paths, and serialize overlaps. Brief every worker
with reachable acceptance, basis SHA, write/forbidden paths, role, focused
checks, cleanup limits and the receiver tie-break below. A session that does not
compile needs no build lane.

Track task/dispatch/source/PR state without interrupting workers. On controller
restart, inspect those carriers before launch. A missing heartbeat or chat is
not evidence that a process stopped. Failure or uncertainty in one bundle does
not globally freeze independent work.

Normative rulings use monotonic IDs in the durable carrier. A later change uses
a `SUPERSEDES` record before its doorbell. Receiver tie-break: obey the highest
d-NNN in the ledger, not the latest message; on conflict reply with your state
instead of executing; never discard pushed work unless the ruling names the
exact commit.

## One author check, independent acceptance

Before handoff, the author performs one combined self-check activity and records
both lenses in one receipt/handoff:

- **Behavior lens:** exercise the supported entry and direct consumers against
  acceptance with actual focused evidence.
- **Counterexample lens:** try the most likely failure such as conflicting rail
  ownership, stale map, partial dispatch, denied permission or retry ambiguity;
  record uncovered risk honestly.

These are not two jobs, agents, tasks, verdicts or sign-offs. They do not replace
independent review.

### Review facts and native continuity

Send one concise facts file only when the controller explicitly selects it. It is
untrusted supporting data, not acceptance, evidence, a verdict or tool authority;
never use `--trust-spec` for rationale. Before adding the optional product flag,
capability-check the selected binary's `edda review --help` for `--context-file`.
If absent, omit the unsupported flag and disclose that product context is missing,
or use the repository's already-permitted direct-review route. Never install,
upgrade or add a fallback wrapper automatically. The carrier is not a redaction
boundary: Edda adds only path/digest provenance, but reviewer output may quote or
reformat the data in existing verdict fields and the existing raw-response blob.

For a first product round, pass the current facts when supported:

```bash
edda review --pr "$PR" --agent "$REVIEW_AGENT" --model "$REVIEW_MODEL" \
  --context-file "$FACTS_FILE" --json
```

For a follow-up on a new subject SHA, prefer the same reviewer agent and real
native conversation, update the facts, and add `--resume`. Product continuity
must remain real: Pi needs its persisted conversation and Claude its native
resume. A first Codex product round persists its mapped thread; Codex `--resume`
strictly requires that mapping and never starts fresh under the old UUID when it
is missing or rejected. The old SHA's LGTM never applies to the new head. A
host-only reviewer session cannot be resumed through product `--resume`.

If the native conversation is missing, launch a replacement without `--resume`,
allocate a distinct reviewer UUID, and pass current facts including prior
findings. State that it is a replacement; never reuse an empty or old UUID to
imitate continuity. A replacement without context remains allowed only with its
limitation visible. Direct host review similarly uses actual host-native
continuity or an explicit replacement, with controller-quoted data it can reach.

Product review retains its own `WorktreeGuard`; callers add no second review
worktree. Direct read-only review retains immutable refs and the host's existing
capability proof. Follow-up review covers the delta, prior findings, affected
direct consumers, current base and introduced security/data-loss risk, while
READing still-applicable evidence. It never reuses an old verdict as acceptance.

Freeze review scope to changed behavior/paths, direct callers/consumers,
issue/spec acceptance, introduced or exposed security/data-loss regressions,
and current-base integration. Review the entire frozen surface and batch all
blocking findings; genuinely adjacent work becomes evidenced follow-up. Every
push invalidates a prior verdict. A final verdict binds current head and the
host's existing gate rules.

Use the repository verification ladder. Authors run focused checks on touched
units while iterating. A frozen head uses exact-head CI where the host defines
that as L1; reviewers READ applicable receipts/CI and run only uncovered focused
or adversarial checks. Do not run a full local workspace merely because a SHA
froze, and do not invent a build lane for docs-only work. Record RAN versus READ,
source SHA, lane or n/a, result and available cost.

Task completion is execution evidence, not acceptance or merge authority. If a
PR is in scope, use the host's canonical product merge path only after an
independent current-head verdict and every existing merge condition. Never
substitute a direct forge merge command. If the repository grants standing
controller authority, that controller acts when its rule gate is green;
`--no-merge`, worker, fixer and reviewer roles never acquire that authority.
Local-only delivery uses the strongest durable local carrier and invents no PR.

## Projection and adoption

`edda init` projects this embedded template only for a detected host: `.claude`
receives `.claude/skills/...`; an existing `AGENTS.md` or `.agents` receives
`.agents/skills/...`; an undetected host receives neither. Existing skill files
are preserved unless the owner explicitly requests the existing all-skill
force option. Do not recommend `--force-skills` for an unknown customized
checkout because it overwrites every embedded project skill, including any
future or non-coordination additions.

For an existing installation, the owner compares against a pinned template and
updates only the selected copy explicitly, preserving custom files. A
`delivery-flow/1` marker or digest identifies bytes that were read; it is not
admission, authority or proof an agent followed them. Record actual host/path,
marker/digest and source SHA when known. Old, customized or unknown remains
truthful state; a successful local parity check is not publication or all-host
adoption.
