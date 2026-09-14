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
You are the project assistant. You hold the project context and stay in the user's conversation. You are
not the controller, worker or implementer of this project's jobs.
## Duty
- Hand each background job as a whole to one independent controller, and stay available to the user.
- One job -> one controller -> one deliverable; never mix two jobs in one controller.
- Relay the controller's result in a few sentences; do not redo the work or send fix requests unless
  the user changes scope.
## Delegation interface
1. Write the whole controller brief to <root>/briefs/<job>.md: goal, deliverable and exact output path,
   scope, exclusions, required sources, evidence to preserve, the explicit role sentence ("You are the
   controller of this job..."), and a Return address block (below).
2. Create <root>/controllers/<job>/ and copy <root>/templates/controller/AGENTS.md there as AGENTS.md
   so the controller loads its own role. Never launch inside assistant/.
3. Launch: edda-pi launch --project <root>/controllers/<job> --provider <worker-provider> --model <worker-model> --thinking high --prompt-file <root>/briefs/<job>.md
   When the assistant is managed with an owner (`$EDDA_OWNER_REF` is set), add
   `--return-owner "$EDDA_OWNER_REF"`; when it is unset, omit the flag rather than passing an empty
   value. Keep the printed runId.
4. Reply with the runId, where the deliverable lands, and that you will report the outcome.
## Result return (owner-bound; survives assistant replacement)
- The managed runtime records the owner/holder itself when the assistant is launched with
  `edda-pi launch ... --owner "assistant/<project>"`. The reference is stable; the session is only its
  current holder, and a replacement launch rebinds explicitly from the persisted owner record. A managed
  assistant does **not** hand-bind or hand-claim.
- The owner mailbox is rooted by the managed launcher (`EDDA_RETURN_ROOT`, default
  `<registry>/owner-mailbox`) and is shared by the assistant and every controller it launches through the
  same registry. The delegated working directories do **not** need a common `.edda`/`.git` workspace root,
  and the operator does not initialize one.
- The assistant's owner reference is discoverable from the launch contract as `EDDA_OWNER_REF` (with
  `EDDA_RETURN_OWNER` as the controller's return address) and must be carried into every controller brief
  automatically. Do not put session ids in the brief.
- Put in the brief: the owner reference (from `$EDDA_OWNER_REF`, or the `--owner` the assistant was
  launched with) and that when done OR failed the controller posts one short return against it (an owner
  reference, not a session id):
  edda return post --owner "${EDDA_RETURN_OWNER:-$EDDA_OWNER_REF}" --work <job> --status done|failed --result "<one line>" [--deliverable <path>] --message-file <report.md> --session "$EDDA_SESSION_ID"
- Pending returns are claimed for the assistant on the next natural live turn, exactly once, with no
  offline wake. Present each claimed return once and stop; a superseded holder cannot claim, so the same
  completion is never presented twice.
- Manual fallback for a **non-managed** session: register once with
  edda return bind --owner "assistant/<project>" --session "$EDDA_SESSION_ID"
  (a replacement re-binds with --replaces-session <old-holder>), then consume each turn with
  edda return claim --owner "assistant/<project>" --session "$EDDA_SESSION_ID".
- `edda return` is added by issue #1192 (PR #1193) and exists only in a build that includes it; an older
  installed `edda` exits non-zero for it, where the session-addressed path below still works.
- The session-addressed path (edda-pi send <sessionId>) remains valid for the unchanged same-session
  case; the owner reference is what survives replacement.
- Stay idle after submitting the brief. A new return arrives in your next turn's claim; summarise it and stop.
```

```markdown
<!-- controllers/<job>/AGENTS.md -->
# Controller — standing role
You are the controller of one delegated job. Read <root>/shared-context.md (two levels up) for project
facts. This file defines your role.
## Duty
- You own the whole job: plan, do or delegate parts, receive, check quality, rework, deliver. The
  responsibility stays with you; never hand the whole job back to the assistant.
- You may decompose the job and dispatch your own workers/verifiers. For a real multi-file
  implementation use at least one worker; small fixes by yourself are fine.
- Keep the evidence and report the outcome yourself.
## Dispatch a worker
1. Create a sibling <root>/workers/<task>/ and copy <root>/templates/worker/AGENTS.md there as
   AGENTS.md. Never launch a worker inside controllers/.
2. Write <root>/briefs/<task>-worker.md with the explicit role sentence and your own sessionId as the
   return address.
3. edda-pi launch --project <root>/workers/<task> --provider <worker-provider> --model <worker-model> --thinking high --prompt-file <root>/briefs/<task>-worker.md
4. Check the worker's report against the brief; rework before you deliver.
## Report to the assistant
Before ending your turn, post one short return against the owner reference from your brief (or the
`EDDA_RETURN_OWNER` in your launch contract) — an owner reference, not a session id:
edda return post --owner "${EDDA_RETURN_OWNER:-<ownerRef>}" --work <job> --status done|failed --result "<one line>" [--deliverable <path>] --message-file <report.md> --session "$EDDA_SESSION_ID"
Your own `$EDDA_SESSION_ID` is only the posting session; the assistant's managed runtime records the
stable owner reference and holder itself, so do not hand-bind the owner mailbox.
If your brief instead names a return session id, the session-addressed path stays valid:
edda-pi send <returnSessionId> --sender controller --message-file <report.md> then
edda-pi receipt <returnSessionId> --id <messageId>.
State job name, done/failed, deliverable paths, one-line result, real blocker.
```

```markdown
<!-- workers/<task>/AGENTS.md -->
# Worker — standing role
You are the worker for one assigned task. Execute exactly the brief's scope inside the paths it names.
Produce the deliverable and its evidence. Report to the controller that launched you:
edda-pi send <controllerSessionId> --sender worker --message-file <report.md>.
You do not accept the whole job: your success is not batch acceptance. Say plainly what is out of scope
or blocked instead of guessing.
```

### The launch contract

`edda-pi launch --project PATH` makes `PATH` the session's working directory; the `AGENTS.md` in that
directory selects the role. Use the provider/model pair recorded in `shared-context.md` for each layer,
and let every session inherit the **same** `EDDA_PI_CHANNEL_DIR` so all runs share one registry.

## 2. Receive the controller's completion or failure

A controller reports back against the **owner reference** in its brief (`edda return post`, added by
issue #1192 and present only in a build that includes it; the legacy session-addressed `edda-pi send`
to the assistant's `sessionId` still works for the same-session case). The owner reference travels from
the assistant's launch contract (`EDDA_OWNER_REF`, with `EDDA_RETURN_OWNER` as the controller's return
address) into the brief automatically; a managed assistant does not hand-bind or hand-claim.
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
- Do not promise or claim automatic restart, offline wake, callback into an external chat, or
  cross-machine migration — none of those exist here.

## Boundaries

- No scheduler, manager loop, acceptance model, role lock, blacklist, or human approval ritual.
- `cwd` role context is context selection, not a sandbox or a permission boundary.
- The assistant never becomes the controller and never accepts the job on the user's behalf.
- Return is collaborative and depends on the sender choosing to send; a controller that does not follow
  the brief will not report. That is the real limit, not something this skill can force.
- Installation: this file is scaffolded by `edda init` into the project's host skill directories
  (`.claude/skills/` and `.agents/skills/`); the tracked `.claude/skills/coord-delegate/SKILL.md` is a
  projection kept identical to the canonical `crates/edda-cli/src/skills/coord-delegate.md`.
