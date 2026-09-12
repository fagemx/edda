# Workflow v1：入口、派工與恢復的單一操作契約

> Proposed A1 acceptance, not active repository policy or a new runtime state machine.
> Standardize routing and handoffs, not agent count, internal reasoning or every tool call.
> Source facts below are pinned to `cbafe8fad409cfad6523d4a12d4226bb3c316d30`.

## 1. Entry routing (first matching row wins)

Existing host/system instructions and repository safety/merge policy retain precedence.
In this repository read AGENTS / its canonical project guide; reuse a hook-injected pack,
or obtain `edda context` and `edda task list` when absent. Do not duplicate injected packs.

| Situation | Route | Not required |
|---|---|---|
| Assigned worker/reviewer, including a resumed session | `task show` assigned ID, read actual brief and existing results; perform that role | new planning, picking neighbours, creating a formation |
| Controller resuming an existing plan/task map | read map, tasks, dispatch/session handles and source/PR evidence before selecting next action | new tasks or relaunch just because chat vanished |
| New request with a usable plan | consume cards; bind owner/input and schedule real prerequisites | repeating research or expanding cards into issues |
| Small single-owner change, no delegated handoff needed | ordinary implementation and existing review path | orchestration, task/program creation solely for ceremony |
| Cohesive multi-step change, one writer | preserve one author context; one execution task if delegation/persistence is needed | one issue/agent/task per function or step |
| Two or more parallel implementing sessions | controller invokes existing coord-orchestrate; workers stay on their assigned route | a new universal delivery command |
| Acceptance is materially unknown | bounded discovery in the original context, then bind the affected work | stopping unrelated ready work |

For a plan with independent cards, human capacity may serialize them; only an actual
artifact dependency creates `--after`. A1 keeps its owner for A2. A3 is independent.
Existing formation/review safety still applies when orchestration is invoked; this table
does not replace it or require every small request to open the orchestration skill.

## 2. One authored source, bounded entry inventory

A1 delivers a generic operating section in the existing embedded skill, marked
`delivery-flow/1`. It includes this routing table, task adapter and recovery semantics.
This plan specifies that section; installed agents must not need this planning directory.

| Surface | Role after A1 | Change owner |
|---|---|---|
| `crates/edda-cli/src/skills/coord-orchestrate.md` | sole authored generic flow, including solo/worker routing before formation | A1, then A2 context delta |
| `.claude/skills/coord-orchestrate/SKILL.md` in Edda repo | byte-identical tracked projection, not a second authored procedure | A1, then A2 projection |
| installed `.claude/skills/coord-orchestrate/SKILL.md` | template copy selected by host; customized copies preserved | project owner adoption |
| installed `.agents/skills/coord-orchestrate/SKILL.md` | template copy, generated/ignored in this repo | project owner adoption |
| `AGENTS.md` in Edda repo | short startup/router pointer; canonical project policy remains above skill | A1 routing/obsolete-start wording only |
| `.claude/skills/issue-pipeline/SKILL.md` | parse issue inputs/options, route to the operating section; no independent phase loop | A1 |
| `.claude/skills/issue-action/SKILL.md` | resume assigned work or resolve issue acceptance, then route; retain task-specific implementation content | A1 routing; A2 self-check/context delta |
| `.claude/skills/pr-review-loop/SKILL.md` | author self-check/fix entry, not independent acceptance or a second delivery loop | A1 routing; A2 two-lens details |
| `docs/guides/operator-runbook.md` | entry/transport index and repository-specific policy references, not another flow implementation | A1, then A2 |

Thin routes preserve meaningful caller intent: `--no-merge` never acquires standing merge
permission; `--skip-plan` reuses acceptance rather than skipping its existence; issue IDs
remain traceability/ownership inputs, not forced one-worker-per-issue. No review bypass flag.
Routing a solo call means executing the solo section, not bootstrapping a formation.

A1 inventories unique clauses in the old project skill before replacing it. Preserve
applicable repository-only requirements by referencing existing project policy or placing
only the missing local clause in the runbook. Explicitly remove obsolete full-local-freeze
instructions and impossible 'worktrees prevent integration conflicts' claims. Generic
text defers to host policy; Edda-specific L0/CI/C5/R6 details stay in project policy/runbook.
Do not choose whichever duplicate has the largest version number or latest timestamp.

This inventory is the v1 support ceiling. Fleet-orchestrate, arbitrary global skills,
third-party hosts and universal auto-discovery are not silently migrated or certified.
A1 audits inbound references to the listed entries; any directly affected caller must
still reach the correct route, but unrelated procedure redesign is not this change.

## 3. Materialize executable work, not a batch of speculative tickets

### Choose one rail owner before creating tasks

Current `edda reconcile` plans every eligible Ready/Failed or unleased Running task; it does
not filter the active plan revision below. Manual `task start` writes no reconcile lease.
Therefore automatic reconcile and a manual controller **cannot safely co-own one rail**.
This is a current product limitation, not solved by worktree isolation or prose active maps.

| Rail mode | Owner and permitted tasks | Refusal boundary |
|---|---|---|
| manual | one named controller creates/selects/starts/dispatches; worker settles; supports legacy/ACP/host routes | no scheduler/reconcile may select this rail; unknown ownership refuses task creation/launch |
| reconcile | existing reconciler lease/runner exclusively starts, retries and settles tasks prepared for its Codex route | no manual start/dispatch/revision filtering; do not enqueue Pi/ACP/host or per-card no-retry work |

Before manual task creation, the operator/controller establishes that the Edda scheduled
reconciler is absent/disabled and no one-off reconciler or reconcile-owned attempt is live.
On Windows, switching from installed scheduler uses the existing exact scheduler lifecycle
(`edda reconcile --uninstall-scheduler`) only with operator authorization; its successful
post-delete query is one input, not proof an already running process ended. Also inspect
process/dispatch/task evidence and wait or resolve prior reconcile attempts. If absence or
exclusive ownership cannot be established, do not create Ready manual tasks on that rail;
return the local conflict. A durable exact-key decision records the selected caller mode before task creation:

```bash
edda decide "delivery.rail-owner=manual:$CONTROLLER_SESSION" \
  --session "$CONTROLLER_SESSION" \
  --reason "repository-wide operator-authorized mode; scheduler/process/attempt evidence=<locations>"
edda ask "delivery.rail-owner" --json
```

This is one repository-rail key, never one key per plan: reconcile selects eligible tasks
across every plan_id. Use `reconcile` only when that runner owns the whole repository rail;
cite actual operator authorization when one exists. Before task creation, scan every active
plan/task and peer record for a conflicting owner—not only this plan. The record is
coordination evidence, not mechanical exclusion or authenticated authority. Unauthorized
later reconcile invocation remains an exposed product limitation; do not claim exactly-once.

Reconcile mode instead uses its actual configuration/lease lifecycle, including its global
max-attempt behavior. It is not a fallback for cards whose backend, permission, budget or
retry contract differs. C1 requires manual mode because it binds Pi and author no-retry.
Changing modes requires operator authorization, scheduler/process/lease reconciliation and
no unresolved task side effects. A1 adds guidance/refusal fixtures, not scheduler code.

### Deterministic active map on existing carriers

After the repository-wide rail-owner is established, manual mode uses one per-plan exact-key
active decision plus task `plan_id`; do not use an
unsearchable note. Let `PLAN_KEY` be a stable lowercase slug (this pack uses `delivery-first`), `PLAN_REV` a monotonic
zero-padded revision (`r0001`), and `PLAN_ID="$PLAN_KEY/$PLAN_REV/$PLAN_SHA"`. Every task in
that revision uses this exact plan ID and key `$PLAN_ID/$CARD_ID`. After creating/readback
of the complete intended map, the named controller records:

```bash
edda decide "delivery.active.$PLAN_KEY=$PLAN_ID" --session "$CONTROLLER_SESSION" \
  --reason "card-to-task: A1=#<id>, A3=#<id>; rail-owner=manual:<session>; replaces=<prior-or-none>"
edda ask "delivery.active.$PLAN_KEY" --json
edda task list --json
```

A repeated same decision key supersedes the prior active value in existing decision history.
It is operational provenance, not ratified product acceptance, merge authority or a source
of scope. A fresh controller first reads exact `delivery.rail-owner`, then the per-plan active key,
requires one active value for each,
filters task-list JSON by exact `plan_id`, reads every selected ID with task show, and checks
card/source SHA, dependencies, owner/scope/brief and observed artifacts before launch. It
must work from no prior chat. Missing/malformed/conflicting map means read-only recovery and
operator/controller adjudication; only this plan's dispatch stops.

A changed brief/owner/dependency gets the next revision. Recreate only needed pending tasks
and successors, verify them, then supersede the active decision. Old task records remain
history and are never selected by manual routes. Because reconcile ignores this map, do not
use revision replacement while reconcile mode can select old Ready/Failed tasks: first let
that owner reach a terminal safe boundary or perform an authorized switch to manual mode.
Optional workflow marker + content digest identifies guidance read; unknown origin stays
unknown, and digest is identity, not authority or a launch requirement.

Before first delegated execution, bind source/worktree, exact write scope, acceptance,
exclusions, receiver role, verification budget and build lane only if compiling. Place
these with the card in a brief reachable from that worktree. The current absolute planner
path is not a portable launch contract. Untrusted receipts/logs are facts, never new scope.

Use `task new --brief/--path/--assignee/--plan/--key`, and `--after` only for materialized
predecessor outputs. Group A1/A2 into one task if they are one owner-deliverable; do not
invent an internal task edge. Split them when A1 candidate itself is the agreed output.
Create future tasks when their brief is concrete, not merely because a box exists in DAG.

```bash
# MANUAL MODE only. Controller resolves variables and repeats --path as needed.
# Legacy/host task: no ACP agent_kind claim.
edda task new "$TITLE" --assignee "$OWNER" --plan "$PLAN_ID" \
  --brief "$BRIEF_REL" --path "$WRITE_SCOPE" --key "$PLAN_ID/$CARD_ID"
# Alternative ACP task is created separately, before start, with matching kind and
# repository-relative CONCRETE existing roots (no glob). It is not the task above.
edda task new "$ACP_TITLE" --assignee "$ACP_OWNER" --agent "$ACP_TARGET" \
  --plan "$PLAN_ID" --brief "$ACP_BRIEF_REL" --path "$ACP_CONCRETE_ROOT" \
  --key "$PLAN_ID/$ACP_CARD_ID"
edda task show "$TASK_ID" --json
# Separate successor only when its required predecessor has an actual task ID:
edda task new "$NEXT_TITLE" --assignee "$OWNER" --plan "$PLAN_ID" \
  --brief "$NEXT_BRIEF_REL" --path "$NEXT_SCOPE" --after "$TASK_ID" \
  --key "$PLAN_ID/$NEXT_CARD_ID"
```

Capture returned ID and read it back. A stable key deduplicates creation ONLY: `new_task`
returns the previous record without updating title/brief/scope/dependencies. Therefore
compare reused record to intent; changed assignment or acceptance gets a new revision/key,
not a hidden edit behind a reused brief path. Do not parse human stderr into state.

## 4. Dispatch adapter: same responsibilities, different flags

In **manual rail mode**, controller starts immediately before launch, after map, input,
capability, ownership and no-reconciler checks. Worker reads it and does NOT start it again.
This convention works with ACP, whose preflight requires Running. In reconcile rail mode,
only the reconciler owns start/launch/retry/settlement; none of the manual commands below
apply. No auto-launch is performed by manual task new/start, and dispatch `outcome=done`
does not imply task completion.

| Backend | Carrier and prerequisites | Continuity | Not interchangeable |
|---|---|---|---|
| Pi | controller-authored `--prompt-file`, explicit `--cwd`; reference task ID/brief and prestarted attempt | repeat real session ID, preserve session-dir if used | no --task-id or --resume |
| Claude | same task-linked prompt-file convention | same session ID plus --resume for an existing conversation | no --task-id |
| Codex | same task-linked prompt-file convention | repeat session ID, verify observed session/thread | no --task-id; no unsupported model/tools flags |
| ACP target | --task-id; task agent_kind matches `acp:<target>`; running; existing repository-relative concrete scope roots, no globs | task.session from ledger | no substitute prompt/session, legacy model/budget/tools flags or --detach |
| Host subagent | controller supplies the same task-linked brief in host task text | host-native continuity only if exposed; otherwise explicit replacement | no claim that host launch is an edda dispatch/session |

ACP source: `cmd_dispatch_acp.rs::{preflight,validate_args,task_prompt,read_brief}`. Its
brief reference must be repository-relative; first injection is at most 4096 bytes and
may say unavailable/truncated. Controller supplies a short entry brief pointing to the
pinned full card; worker reads the full acceptance with actual tools before coding. Verify
that those reads are reachable under real policy; never broaden roots merely to make it
work. If unavailable, return that local limitation, not pretend full context was supplied.

Other backends read prompt-file verbatim (`cmd_dispatch.rs::run_inner`), not the task by
magic. The prompt contains task/attempt, cwd/source, brief reference and lifecycle owner;
worker uses `task show` and reads the brief. Existing tools must permit those reads/writes.
If they do not, choose a suitable authorized route before launch; no runtime fallback
may circumvent a denied permission or controlled ACP acceptance boundary.

```bash
# Manual controller; mode/map/inputs/capabilities checked first. Same task ledger as worker.
edda task start "$LEGACY_TASK_ID"
edda dispatch --agent pi --prompt-file "$PROMPT_FILE" --cwd "$WORKTREE" \
  --model "$PI_MODEL" --timeout-sec "$TIMEOUT_SEC" --json
# Alternative ACP route uses the separately created ACP_TASK_ID, not LEGACY_TASK_ID:
# edda task start "$ACP_TASK_ID"
# edda dispatch --agent "$ACP_TARGET" --task-id "$ACP_TASK_ID" --cwd "$WORKTREE" --json
```

Use only installed help/source-proven options. Pin binary version/source separately from
candidate source; installed `edda 0.6.1 (d62b91caae5d 2026-09-10)` was inspected here, not
built from this plan. No universal --task-id extension, wrapper or scheduler is part of A1.
No issue means omit --issue; linked issue work still observes existing ownership checks
and supplies --issue/--machine when dispatch is the mandated claim route. Omitting a flag
must never be a way to evade an existing claim requirement.

## 5. Completion, restart and retry

This section describes manual rail mode. Worker owns normal done/fail with evidence.
Controller records observed pre-launch failure or recovers a dead worker's terminal result
only after reconciling actual source/process/task attempt. Reconcile rail mode uses the
existing lease/runner lifecycle instead—never mix rows from the two modes. Do not have
both writers race to settle a live task. CLI identity labels/active decision are not
authenticated authority; this is caller discipline, not a new runtime guarantee.

| Observation | Action by controller / worker | Effect on others |
|---|---|---|
| ready, inputs bound | controller start once, then dispatch; worker reads running task | none |
| start refuses running/blocked/done | inspect existing attempt/deps/result; no unconditional retry | affected action only |
| worker met task output (which may be candidate, not merged PR) | worker `task done --receipt ... --evidence ...`; controller verifies output before using it | true successors may become ready, not auto-dispatched |
| definite stopped failure | worker fail, or controller fail after observing stopped/prelaunch failure | unrelated tasks proceed |
| failed, same assignment/brief/scope, retry within existing authorization/budget | controller `task start` SAME ID: next attempt; reuse actual session only when valid | original successor edges remain correct |
| running but silent, expired-looking lease or missing handle | inspect process, handle, task history, source and external effects; don't fail/relaunch on time alone | local unknown, not global freeze |
| controller died after start but before confirmed spawn | reconcile evidence first (do not invoke `edda reconcile`); only proven stopped attempt may be failed/restarted | no exactly-once assertion |
| dispatch done but task still running/no adequate receipt | inspect artifacts; request/recover truthful receipt; not acceptance or merge permission | do not falsely unblock consumers |
| done then metadata-only receipt correction | existing task done supports correction; do not treat as another execution | no new launch |
| done then substantive fix needed | new linked fix task; old execution receipt remains history | dependent candidate must requalify |

Correction to the earlier chat: there is no `task retry` subcommand, **but retry exists**
through `start_task` accepting Failed. See `task_actions.rs::start_task` and
`cmd_task.rs::fail_running_task_records_reason_and_start_retries`. Done cannot be restarted.

Changed brief, assignee or dependency graph: stop/reconcile the old live attempt first,
create/readback the next `PLAN_REV`, remap still-needed successors to new predecessor IDs,
then supersede `delivery.active.$PLAN_KEY`. No task cancel/dependency-edit CLI is claimed.
Old Ready/Blocked tasks stay historical; every manual entry reads the exact active decision
and excludes old IDs, never falsifies done. If a competing reconciler/auto-picker can still
select them, replacement is refused until authorized rail-mode switch/terminal cleanup.
Do not add `--after <failed old task>` to its replacement. Unknown side effects prevent
only that retry; active-map ambiguity prevents launch, not read-only recovery.

A1 does not automatically retry or escalate model/cost. Existing authorization and bounded
retry allowance control that choice; C1 specifically allows no correction/retry call.
Task done is execution evidence, never independent acceptance. Current R6 is unchanged.

## 6. Issue timing and delivery

- Local research/implementation may start from explicit user/spec acceptance without a new
  GitHub issue. Never fabricate a requirement just because there is no issue number.
- For this repo's formal PR, resolve/link delivery acceptance by PR creation, before asking
  for final independent review. Before A3 adoption include current required Issue convention.
- A can use one delivery/tracking issue for cohesive tasks; separate PRs may reference it.
  B reuses #1141 and does not close it for a partial slice. C1 publishes local evidence only.
- Issue creation/publication still needs the operator's existing delivery authorization;
  missing permission means local-only candidate, not implicit forge write permission.
- Independent review may read candidates early; final verdict binds frozen head and current
  acceptance. Merge timing/authority follows project policy, never task state or this file.

## 7. Publication and adoption, without a new approval service

The current pack is on a local branch; a local spec LGTM is not main policy or remote
availability. To execute elsewhere, publish through an authorized PR or transfer the exact
committed source/plan using an existing authorized channel. Verify received full SHA and
make the brief reachable; a Windows path on another workstation is not sufficient.
No automatic push, PR, migration or rollout is authorized by writing this plan.

A1 landing makes source guidance available, not every installed copy current. Existing
`cmd_init.rs::scaffold_skills` copies to .claude when that directory exists, to .agents when
AGENTS.md or .agents exists. Default init preserves existing files; it does not compare
versions or upgrade them. An initialized project without a detected host gets no projection.

For an authorized fresh/isolated installation use the A1-built binary's existing
`init --no-hooks`; verify output paths/bytes. `--force-skills` overwrites ALL five coord
skills, not just orchestration: do not recommend it for an unknown customized checkout.
For existing installs, owner compares the selected skill with the pinned template and
explicitly updates only the intended copy, preserving other customizations. No boot hook,
mandatory version check or automatic update is added. Older guidance is reported as
old/custom/unknown; safe work can continue under actual repository rules without claiming
v1 coverage. Identify the host-selected source before claiming adoption.

Adoption receipt uses the existing task/note: actual host/path, workflow marker/digest,
source SHA when known, and observed route. Missing metadata is not a new work blocker.
Static parity proves distributed bytes; scripted scenarios prove documented routing;
only an actual observed run proves that agent followed it. V6 separates those claims.
