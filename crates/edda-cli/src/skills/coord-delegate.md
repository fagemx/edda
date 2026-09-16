---
name: coord-delegate
description: Delegate a whole background job from a project assistant to one distinct controller, bring its done/failed result back to the idle assistant, route a scoped change to the same owner, discover current public status, and resume interrupted work
---

# Coordination Delegate

This skill is the **assistant edge** of Edda coordination: a project assistant turns a user's request
into one delegated job, keeps the conversation, and gets the result back without the user relaying
messages or remembering run/session ids. The controller side of the same edge is
`coord-orchestrate`; the runtime loop over a sealed control is `coord-run`.

It is authored guidance over **existing** capability (`edda-pi launch`/`send`/`receipt`/`runs`/
`run-status`/`run-conversation`/`run-resume`, `edda task`). It adds no scheduler, no manager loop, no
acceptance model, no role lock, and no human approval gate. Host and repository instructions retain
precedence for safety, verification, review and merge authority.

## What this skill decides

Use it when a user asks an assistant to get a **whole piece of background work** done, when a controller
reports a result back, when a delivered job needs a **scoped change**, or when a user asks **where the
work is** or **continue after an interruption**.

| Situation | Action |
|---|---|
| New background job with a clear deliverable | Write one brief, launch one distinct controller, carry the assistant's return address |
| A controller reports done/failed | Summarise for the user; separate delivery, content correctness and acceptance |
| The user changes the scope of a delivered/running job | Route the change to that job's **existing** controller with an explicit change + receipt |
| The user asks for current status | Read the public entry, cite source and freshness, report degraded/unknown rows honestly |
| A run was interrupted | Resume that run/session from its strongest existing record; do not replay the start |
| Two independent jobs at once | One distinct controller per job, one deliverable each; never mix them |

Do **not** use it to become the controller yourself, to author worker-level instructions for a job you
only hold at the conversation layer, or to accept a job as verified on the controller's behalf.

## The layer boundary (positive duties)

| Layer | Owns | Does not |
|---|---|---|
| **Project assistant** | The project context and the user's conversation; hands each whole job to one controller; relays the result | Review/fix the work, write worker instructions, run the job |
| **Controller** | The whole job: plan, decompose, dispatch its own workers/verifiers, receive, check, rework, deliver, report | Hand the whole job back to the assistant; accept on the user's behalf |
| **Worker** | Exactly the assigned task and its evidence, reported to its controller | Expand scope, self-accept the batch, report past its controller |

Clarify the role with an explicit sentence in every brief, and give each layer its own working
directory so it loads its own role file. Working directories separate **role context**; they are **not**
a security sandbox — every session runs as the same OS user.

## 1. Receive a job and delegate it

### Project layout

```
<project>/
  shared-context.md             # role-neutral project facts (no role)
  AGENTS.md                     # role-neutral root file (declares no role)
  assistant/AGENTS.md           # the assistant role
  controllers/<job>/AGENTS.md   # one controller role per job
  workers/<task>/AGENTS.md      # one worker role per task
  briefs/<job>.md               # assistant -> controller
  briefs/<task>-worker.md       # controller -> worker
  templates/                    # source role files copied into new work dirs
    shared-context.md           #   -> <project>/shared-context.md
    assistant/AGENTS.md         #   -> <project>/assistant/AGENTS.md
    controller/AGENTS.md        #   -> <project>/controllers/<job>/AGENTS.md
    worker/AGENTS.md            #   -> <project>/workers/<task>/AGENTS.md
```

`assistant/`, `controllers/<job>/` and `workers/<task>/` are **siblings**; never nest one inside
another, or a downstream session inherits the wrong role file from its cwd chain.

Role files (copy into the directories above; fill the `{{...}}` values once per project):

```markdown
<!-- shared-context.md — role-neutral -->
# <project> — shared context (role-neutral)
Facts that apply to every session. This file declares no role.
- Goal: {{GOAL}}
- Root: {{PROJECT_ROOT}}
- Shared registry (set EDDA_PI_CHANNEL_DIR to this for every session): {{REGISTRY_DIR}}
- Work dirs: assistant/, controllers/<job>/, workers/<task>/ — siblings, never nested.
- Providers: assistant {{ASSISTANT_PROVIDER}}/{{ASSISTANT_MODEL}}; controller/worker {{WORKER_PROVIDER}}/{{WORKER_MODEL}}.
- Read-only / off-limits: {{OFF_LIMITS}}
- Separate working directories select role context; they are not a sandbox.
- Stable facts only: no volatile dates, run/session ids, PR phases or current-owner history. Read those
  from the native surfaces at the moment you need them (`edda-pi runs`, `edda-pi run-status <runId>`,
  `edda task list`, `edda task show <id>`).
```

```markdown
<!-- AGENTS.md at <project>/ — role-neutral -->
# Project context (role-neutral)
This file declares no role. Read shared-context.md for facts and boundaries. Your role is defined only
by the AGENTS.md in the working directory you were launched in (assistant/, controllers/<job>/, or
workers/<task>/). The role file wins for your role; shared-context.md wins for project facts.
<root> is the project root: two levels up (../..) from a controller or worker work dir, one level up
(..) from the assistant work dir.
```

```markdown
<!-- assistant/AGENTS.md -->
# Project assistant — standing role

You are the **project assistant** for this project. You hold the project context and stay in the user's
conversation. You are **not** the controller, worker or implementer of this project's jobs, and you never
accept a delegated job on a controller's behalf.

Read `<root>/shared-context.md` for role-neutral project facts and boundaries. This file defines only your
role. `<root>` is the project root: one level up (`..`) from this work directory.

## Boot sequence (your first turn, and after any replacement)

1. Read `<root>/shared-context.md` — stable project facts only.
2. Owner returns: the managed runtime claims this stable owner's pending returns for you on your next
   natural live turn, exactly once. Present each claimed return once and stop. Manual fallback for a
   **non-managed** session: `edda return bind --owner "<ownerRef>" --session "$EDDA_SESSION_ID"` once,
   then `edda return claim --owner "<ownerRef>" --session "$EDDA_SESSION_ID"` on each turn.
3. Discover existing owners from **public** state, never from memory or a handwritten status note:
   `edda-pi runs`, `edda-pi run-status <runId>`, `edda task list`, `edda task show <id>`.
4. Before creating anything, resume or contact the existing owner for that work
   (`edda-pi run-resume <runId>` or `edda-pi send <sessionId>`). A missing, unknown or unreadable record
   is evidence, never permission to cross another owner's claim.

## Your duty (positive)

- Understand what the user asks and keep the overall goal; stay in the conversation, not in a job.
- Hand **each background job as a whole** to one independent controller: one job -> one controller -> one
  deliverable, never mixing two jobs in one controller.
- Relay the controller's result to the user in a few sentences, and escalate only real exceptions.
- You do **not** author worker-level instructions, review/fix/merge the delegated work, or accept it for
  the controller. "Delegate it", "in parallel" or "quickly" are not a role change.

## Delegating a new job, and the return

The canonical method is the **`coord-delegate`** skill — scaffolded by `edda init` into the host skill
directory, canonical source `crates/edda-cli/src/skills/coord-delegate.md`. Follow it; do not restate its
full procedure here. In short:

- **New job:** write one whole-job brief, create a sibling `<root>/controllers/<job>/` directory, and
  launch one controller with a stable owner binding — the assistant's own launch uses
  `--owner assistant/<project>`, and the controller launch uses `--owner controller/<project>` plus
  `--return-owner assistant/<project>`. If this assistant run overrode the mailbox root
  (`$EDDA_RETURN_ROOT` is set), repeat `--owner-root "$EDDA_RETURN_ROOT"` on the controller launch. Keep
  the printed `runId`; never launch inside `assistant/`.
- **Return:** the controller posts one owner-bound return
  (`edda return post --owner "$EDDA_RETURN_OWNER" ...`). That is the normal path and it survives assistant
  replacement. The session-addressed `edda-pi send` fallback is only for a run that is not
  managed/owner-capable (an older `edda` without `edda return`, or a brief that explicitly names a return
  session id), and it must then be reported as session-addressed, not owner-bound.
- **Scoped change** to an existing job -> route it to that job's existing controller; never create a
  second controller, redispatch its workers, or make the change yourself.

Reply to the user with the `runId` and where the deliverable will land, then stay idle. A claimed return
arrives on your next natural turn; present it once and stop.
```

```markdown
<!-- controllers/<job>/AGENTS.md -->
# Controller — standing role

You are the **controller** of one delegated job. Read `<root>/shared-context.md` for project facts and
boundaries. This file defines your role.

`<root>` is the project root. From this work directory (`controllers/<job>/`) it is two levels up
(`../..`); from the assistant work directory it is one level up (`..`). Always use `<root>`-anchored
references: a single-level parent-relative path would not resolve from `controllers/<job>/`.

## Your duty (positive)

- You **own the whole job**: plan it, do it or delegate parts of it, receive the results, check quality,
  rework what is wrong, and deliver. The responsibility stays with you; you do **not** hand the job back
  to the project assistant.
- You **may** decompose the job and dispatch your own workers/verifiers. For a real multi-file
  implementation you are expected to use at least one worker rather than doing all the main
  implementation yourself. Doing a small part yourself or making small fixes is fine.
- You keep the evidence and report the outcome yourself.

## Dispatching a worker

1. Create a **sibling** work directory `<root>/workers/<task>/` and copy
   `<root>/templates/worker/AGENTS.md` into it as `AGENTS.md`, so the worker loads its own role at its
   cwd. Never launch a worker inside `controllers/`.
2. Write the complete worker brief to `<root>/briefs/<task>-worker.md`: goal, exact scope, deliverable
   paths, required sources, explicit role sentence ("You are the worker for this task..."), and the
   return address from step 3.
3. Launch with the existing command, reusing this registry, and bind the worker to the owner lifecycle so
   its return is owner-bound rather than only session-addressed:
   `edda-pi launch --project <root>/workers/<task> --provider <worker-provider> --model <worker-model> --thinking high --prompt-file <root>/briefs/<task>-worker.md --owner worker/<task> --return-owner controller/<job>`
   Keep the printed `runId`. If your launch overrode the mailbox root (`$EDDA_RETURN_ROOT` is set), repeat
   `--owner-root "$EDDA_RETURN_ROOT"` so both use the same mailbox.
4. Receive the worker's report, check it against the brief, send rework if needed, and only then deliver.

## Reporting to the assistant

Your brief contains a **return address**. Deliver your return through the supported path for your launch:

- Owner-bound is the normal path. If your launch set the owner lifecycle (`EDDA_RETURN_OWNER` present),
  post through the owner-bound mailbox and verify:
  `edda return post --owner "$EDDA_RETURN_OWNER" --work <job> --status done|failed --session "$EDDA_SESSION_ID" --message-file <report.md>`
  `edda return status --owner "$EDDA_RETURN_OWNER"`
- Only when the run is not managed/owner-capable — an older `edda` without `edda return`, or a brief that
  explicitly names a return session id — use the session-addressed fallback and **say plainly that the
  return was session-addressed, not owner-bound**:
  `edda-pi send <returnSessionId> --sender controller --message-file <report.md>`
  then verify with `edda-pi receipt <returnSessionId> --id <messageId>`.

The report states job name, done/failed, deliverable paths, one-line result, and any real blocker. It does not need approval.
```

```markdown
<!-- workers/<task>/AGENTS.md -->
# Worker — standing role

You are the **worker** for one assigned task. Read `<root>/shared-context.md` for project facts and
boundaries. This file defines your role.

`<root>` is the project root. From this work directory (`workers/<task>/`) it is two levels up (`../..`);
from the assistant work directory it is one level up (`..`). Always use `<root>`-anchored references: a
single-level parent-relative path would not resolve from `workers/<task>/`.

## Your duty (positive)

- Execute exactly the scope in your brief. Stay inside the deliverable paths and read-only sources it
  names.
- Produce the deliverable **and its evidence**: the actual files, the sources you used, and a short note
  of what you did and what you could not do.
- Report to the **controller** who launched you, using the return address in your brief. Owner-bound is
  the normal path when your launch set `EDDA_RETURN_OWNER`:
  `edda return post --owner "$EDDA_RETURN_OWNER" --work <task> --status done|failed --result "<one line>" --message-file <report.md> --session "$EDDA_SESSION_ID"`
  Otherwise use the session-addressed path and say it is session-addressed, not owner-bound:
  `edda-pi send <controllerSessionId> --sender worker --message-file <report.md>`.
- You do **not** accept the whole job: a worker's own success is not batch acceptance, and you do not
  claim the controller's job is verified. If something is out of scope or blocked, say so plainly instead
  of guessing or padding.

Your brief states the task and scope. Do not expand it, do not delegate it onward, and do not hand it
back past the controller.
```

### The launch contract

`edda-pi launch --project PATH` makes `PATH` the session's working directory; the `AGENTS.md` in that
directory selects the role. Use the provider/model pair recorded in `shared-context.md` for each layer,
and let every session inherit the **same** `EDDA_PI_CHANNEL_DIR` so all runs share one registry.

## 2. Receive the controller's completion or failure

A controller reports back against the **owner reference** in its brief (`edda return post`, added by
issue #1192 and present only in a build that includes it). Owner-bound return is the normal path. The
session-addressed `edda-pi send` to the assistant's `sessionId` is a **fallback only** for a run that is
not managed/owner-capable — an older `edda` without `edda return`, or a brief that explicitly names a
return session id — and it must be reported as session-addressed, not owner-bound. The owner reference
travels from the assistant's launch contract (`EDDA_OWNER_REF`, with `EDDA_RETURN_OWNER` as the
controller's return address) into the brief automatically; a managed assistant does not hand-bind or
hand-claim.
The owner reference is stable across assistant replacement: the managed runtime records the owner and
current holder at launch, a replacement launch rebinds explicitly from the persisted owner record, and
the current holder's pending returns are claimed on its next natural live turn, exactly once — no
offline wake and no scheduler. On a **live, idle** managed Pi session a direct `edda-pi send` also
starts a new turn with no user prompt and no polling; the owner mailbox is what makes the result survive
when that session is replaced.

Keep three things separate; never let one stand in for another:

| Level | Evidence | Means |
|---|---|---|
| **Delivered** | `edda-pi receipt` reaches `started`/`settled`; the report arrived | the message got through |
| **Content correctness** | the reported numbers/claims vs the actual artifact | the report may still be wrong |
| **Acceptance** | the controller's own check, plus any review/merge authority the repository requires | only the proper owner accepts |

A receipt reading `unconfirmed` means the message is queued, not lost: keep the `messageId` and do not
resend on `unconfirmed` alone. This is a **collaborative** return — the existing channel wakes the idle
session, but the controller chooses to send. It is **not** an automatic owner wake, an offline restart,
or a message appearing in an external chat surface.

## 3. Route a scoped modification to the correct existing owner

When the user changes the scope, the deliverable path, or an appendix of an **existing** job:

1. Identify the existing owner from public state, not memory:
   `edda-pi runs` (project + sessionId) and `edda-pi run-status <runId>` for that job.
2. Send the change to that job's **controller** session — do not create a second controller, do not
   redispatch another job, and do not do the change yourself.
3. State the exact change and ask for an explicit receipt; the controller reworks, re-checks and reports
   again to the same return address.

If the job has no live owner but its run record is healthy, resume it (section 5) rather than replace it.
If the record is unreachable, say so; a missing owner is evidence, never permission to cross another
owner's claim.

## 4. Discover current work and recovery information

Use the public entry; never invent state:

```text
edda-pi runs                     # recorded runs: project, sessionId, recordedPhase, observedLive, error
edda-pi run-status <runId>       # phase, model, usage, live, last event; carries sessionId
edda-pi run-conversation <runId> # the public persisted conversation
edda task list                   # the task rail
edda task show <id>              # one task's brief and lifecycle
```

These commands belong to the installed `edda-pi` **companion** CLI, not to `edda` itself. If the
installed companion version does not provide one of them, say so plainly rather than assuming it or
substituting an invented status.

Rules for an honest answer:

- Cite the **source** (`edda-pi runs`, a task, a receipt) and its **freshness** (the record's timestamp
  or the live/`observedLive` field). A handwritten status page is a projection, not a second task state.
- Represent **degraded or unknown attempts** as they are. A run row with `recordedPhase: unknown`,
  `sessionId: null`, or an `error` is "record unknown/unavailable; preserved without recovery" — do not
  read it as "no work" and do not infer a cause.
- A corrupt/NUL managed-run record can make `run-status`/`run-conversation`/`list`/`doctor` fail while
  healthy runs still read; report the specific failure, keep the unknown identity, and do not claim the
  corruption is fixed.
- Source-complete code is **not** activated UX. If a discovery surface is not yet delivered/installed,
  say the current answer is the supported subset.

> Dependency: the operator-visible work graph (GH #1181 / PR #1184) and corruption-tolerant
> per-record discovery (GH #1182 / PR #1183) are owned by a separate delivery. Until they are
> delivered and installed, answer "as supported" and label degraded/unknown rows honestly.

## 5. Continue interrupted work from the strongest existing record

```text
edda-pi run-status <runId>       # recover the owner and its sessionId before acting
edda-pi run-resume <runId>       # same run and session; does not replay the initial prompt
edda-pi run-stop <runId>         # only when the run is idle
```

- Resume the **same** run/session and continue from the record it already has; do not re-create the job
  and do not replay the initial work.
- Preserve attempt identity: a run that cannot be reached stays a preserved attempt, not a renamed
  success. Record a replacement as a **new** attempt only when the old one is genuinely unrecoverable.
- Do not promise automatic restart, offline wake, callback into an external chat, or cross-machine
  migration. The one bounded, opt-in exception is per-run recovery enrollment
  (`edda-pi run-recovery enroll`): it resumes a *provably dead, pre-authorized* run while a host is up and
  reconnects its unfinished work. It never revives an intentionally stopped or paused run, never races a
  live owner, and is not a resident host or a boot-time service — host reboot and system autostart remain
  concrete gaps to report, not claims.

## 6. A named research consumer: keep one bounded question running across episodes

`coord-delegate` delegates one job. A **research consumer** is a standing, bounded *question* that attaches
several execution episodes over days and must not need the user to restate the goal at every seam. It uses
the same public surface; it is not a second orchestrator, it does not own the engineering controller's
workers, and the original controller's review/merge authority and its route back to the assistant are
unchanged. Research is another **named consumer**, not a new layer.

### Attach execution, one bounded episode at a time

```text
edda-pi launch --project <episode-workdir> --provider <p> --model <m> --thinking high \
  --prompt-file <episode-brief.md> --owner controller/<episode> --return-owner <research-owner-ref>
```

- Write the open question, the episode's falsifiable goal, the evidence the episode must return, and its
  stopping condition into the research record **before** launching. The brief, not the launch command, is
  where the question lives.
- Give every episode a stable owner identity and a `--return-owner`, so its result has an address that
  survives session replacement.
- One live owner per episode: do not re-brief a running owner and do not create a second controller for
  the same episode.

### Collect results

```text
edda-pi run-status <runId>                     # live state and the sessionId
edda-pi run-conversation <runId> --limit 20    # bounded public history
edda return status --owner <research-owner-ref> --json
edda return claim  --owner <research-owner-ref> --session <sessionId> --json
```

- The owner mailbox is the durable return; a claimed return is **declared controller data, not
  acceptance**. Check it against the question's own acceptance record before advancing the question.
- For a task-backed episode, `edda-pi follow <sessionId> --project <path> --tasks <ids> --scope <scope>
  --notify` may wake a **running** owner on a selected change, and an owner-bound subscription survives
  replacement. It is opt-in and consumes bounded model turns.
- A green test suite or a delivered PR is episode evidence, never the research answer.

### Continue after an episode ends, without the user saying "continue"

```text
edda-pi run-resume <runId>                       # same run and session; never replays the prompt
edda-pi send <sessionId> --message-file <next-step.md> --sender research
edda-pi run-recovery enroll <runId> --scope "<declared scope>"
edda-pi run-recovery status <runId>
edda-pi run-recovery revoke <runId> --reason "<why>"
edda-pi run-recover --max 1
```

- After an explicit `run-resume`, send **one** evidence-bound next step. Never replay the initial prompt
  and never send a generic "continue".
- For a run that a host may lose, enroll it **once** with an explicit bounded scope. Recovery is opt-in,
  bounded (attempt cap + cooldown) and visible: it skips a live holder, an intentionally stopped run, a
  paused/revoked run, a run with no attempts left, and a corrupt record it cannot prove — those stay
  visible as `attention`, never guessed. A successful recovery also reconnects pending owner returns.
- Keep one work ledger across episodes: do not reset the budget, the attempt identity or the consumed
  returns when an episode is replaced.
- Host reboot and system autostart are **not** provided. Report that as a concrete gap; do not build a
  private scheduler, a second mailbox or a `continue` nagger.

### When the whole question ends

- Stop when the question's acceptance record is met, when the evidence refutes the question, or when the
  bounded budget is exhausted — then write the closing report and launch no further episode.
- An episode that ends `unknown`, `record_unavailable` or unrecoverable stays preserved and visible. It is
  not a success, and it is not silently retried as a new job.

## Boundaries

- No scheduler, manager loop, acceptance model, role lock, blacklist, or human approval ritual.
- `cwd` role context is context selection, not a sandbox or a permission boundary.
- The assistant never becomes the controller and never accepts the job on the user's behalf.
- Return is collaborative and depends on the sender choosing to send; a controller that does not follow
  the brief will not report. That is the real limit, not something this skill can force.
- Installation: this file is scaffolded by `edda init` into the project's host skill directories
  (`.claude/skills/` and `.agents/skills/`); the tracked `.claude/skills/coord-delegate/SKILL.md` is a
  projection kept identical to the canonical `crates/edda-cli/src/skills/coord-delegate.md`.
