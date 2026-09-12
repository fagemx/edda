# Edda Pi session channel

Read a running Pi session's replies and send a message to its existing conversation
from another local process (including Codex). Uses Pi's extension API; no
terminal keystrokes, transcript injection, duplicate agent or separate supervisor model.

Requires Node.js 24 and Pi 0.85.1 (the version used for the runtime smoke test).
The extension has no npm dependencies. Older Pi versions may lack lifecycle
events used here; they are not supported.

## Enable

Try it on a new Pi session without changing settings:

```powershell
pi -e C:/ai_agent/edda-worktrees/pi-session-channel/integrations/pi/extension.mjs --edda-session-label edda-worker
```

Or install the local package using Pi's supported package command:

```powershell
pi install C:/ai_agent/edda-worktrees/pi-session-channel/integrations/pi
```

Local package installation references the directory; keep that worktree until
you move the installation to a merged checkout. Remove that reference with
`pi remove C:/ai_agent/edda-worktrees/pi-session-channel/integrations/pi`.

An already-open Pi needs `/reload` after package installation, at an appropriate
idle point. Loading the extension does not resume work. It cannot silently attach
to arbitrary existing terminals. New sessions load installed packages normally.
Use `/edda-session` inside Pi to see its exact identity and status.

## Start a managed Pi

For new sessions, managed launch owns the process and fixes the integration version
from the beginning. It automatically snapshots the runtime outside the checkout:

```powershell
node integrations/pi/cli.mjs launch --project C:/my-project --provider openrouter --model deepseek/deepseek-v4.1-flash --thinking low --prompt-file task.txt
node integrations/pi/cli.mjs run-status RUN_ID
node integrations/pi/cli.mjs run-stop RUN_ID
node integrations/pi/cli.mjs run-resume RUN_ID
```

The run ID is printed before launch. The result contains a separate Pi session ID,
runner instance, loaded integration version/release/module path, selected model,
session file and initial-message receipt. Use the Pi session ID with `send`,
`conversation`, `adopt` and other session commands. Use the run ID for lifecycle
commands. The calling CLI may exit; the hidden runner and its owned Pi remain alive.

`--provider`, `--model`, `--thinking` and the initial prompt are optional; normal Pi
model configuration applies when omitted. Pi is discovered in a local/global Node
installation, or supply `--pi-entry /path/to/pi/dist/bundle/cli.js` (also configurable
through `EDDA_PI_ENTRY`). A missing installation gives an explicit setup error.
`--agent-dir` can select an existing isolated Pi configuration directory. Extra
trusted extensions can be loaded with `--extension FILE`; `--no-tools` is useful
for isolated protocol tests. This process uses `--no-extensions` plus the fixed
Edda integration and explicitly requested extensions, avoiding duplicate legacy
channels. Other Pi configuration and standard resource discovery still apply.

Runtime files live under the private registry's `releases/<digest>/`. A release
manifest verifies their bytes before launch and recovery. Source edits cannot change
an already-installed release. `runtime-install` creates/verifies a release without
starting Pi or changing global settings; `<release.path>/cli.mjs` is a stable CLI
entry independent of this development worktree. Pi itself remains an installed
dependency: its version is reported, while the integration is digest-pinned.

`run-status` authenticates the actual runner and queries its Pi channel. Readiness
is not proof of work starting: inspect `initialReceipt` and current Pi state. Progress
and provider-reported token/cost totals are bounded metadata, not raw RPC logs.
Model errors expose a category/status code without credential or response-body dumps.

Idle stop closes only the runner's owned Pi child. `run-stop RUN_ID --abort` explicitly
interrupts busy owned work; ordinary stop refuses to interrupt it. Neither command
kills arbitrary saved PIDs. Stop retains session history, inbox and receipts.
`run-resume` restores the same validated session file and observed model/thinking
settings; it does not resend the original task. Send a fresh explicit instruction
when you want work to continue. Retrying `launch` with the same run ID and identical
inputs reconnects instead of starting a duplicate; different inputs conflict.

If the runner crashes, status reports unavailable rather than inferring ownership
from a PID. Recovery refuses when a previous runner/Pi may still be alive, the
session file is missing/mismatched, or a launch/resume lock is ambiguous. Inspect
that evidence rather than creating another run ID on a timeout. A crash before
the initial prompt is sent can leave an unknown initial intent; recovery never
blindly replays it. Newly created empty sessions may not yet have a persisted file.

This is the process/event entry point for management. It does not autonomously
approve work, poll a manager model, or wake the Codex desktop conversation.

## Start here: adopt an existing session

When the task has a structured management brief, one command prepares its context,
enrolls the session and follows the task plus its transitive `after` prerequisites:

```powershell
node integrations/pi/cli.mjs doctor
node integrations/pi/cli.mjs adopt SESSION_PREFIX --task 17 --scope "Observe the assigned task within existing authority; no new spending." --preview
node integrations/pi/cli.mjs adopt SESSION_PREFIX --task 17 --scope "Observe the assigned task within existing authority; no new spending." --notify
```

Use an exact ID or a unique prefix of at least eight characters; offline collisions
also count. Project defaults to that live session's working directory. Add
`--project PATH` for another task project, `--include 18,19` for explicit review/fix
roots, or `--context FILE` for structured metadata when the task brief is prose.
The metadata format is documented under task-to-handoff composition below.

Adoption reads all prerequisites before applying setup, with an eight-task maximum.
Cycles, missing tasks and overflow fail visibly; use `follow --tasks` to deliberately
select a smaller observation set. Coverage is an `explicit_after_snapshot`: new
review/fix tasks and changed graph edges need another adoption. Names, receipts and
`done` status are not used to infer acceptance, ownership or missing edges.

`--preview` reads/caches source snapshots but makes no enrollment, handoff, follow
or message changes. Missing metadata reports exactly what to supply. Busy sessions
and old extensions report idle/reload steps. Replacing an existing handoff or
rebinding after reload needs `--expected REVISION`, obtained from `brief`; this is
an explicit compare-and-set against the existing context, not new work authority.

Repeated adoption reuses a semantically unchanged current manifest and its original
immutable source snapshot. A task-status-only update does not reset notification
caps. A changed task graph or explicit configuration is a new subscription; its
initial alert may consume a normal model turn when `--notify` is present.

Apply is a sequence of existing operations, not a transaction: `adoption_incomplete`
lists confirmed steps and the possibly-applied last request. Inspect `doctor`,
`brief` and `dependencies` before retrying. No rollback or blind resend occurs.
Existing observation stays active until its configuration is changed. `adopted`
means setup succeeded, not work started; notify mode reports `workStarted: null`
until its separate message receipt is inspected. Without `--notify`, adoption
configures observation only. Use an explicit `send` for an authorized work instruction.

The receiver now persists stopping/decision events in a local manager inbox (below).
It does not wake an idle Codex thread. `conversation` still reads replies on demand;
`brief` reads structured reports when prepared. Dependency alerts wake Pi only.

## Manager inbox: observe, read and respond

Pi publishes a decision event when it records `waiting_decision`; other structured
stopping reports are also retained. Without a structured report, settlement saves
an unread-activity event with at most 2400 UTF-8 bytes of the final public assistant
text. No approval is inferred from prose and no private reasoning is captured.
Workers do not need to fill metadata or stop for another approval to continue work.

```powershell
node integrations/pi/cli.mjs inbox
node integrations/pi/cli.mjs inbox-read EVENT_ID
node integrations/pi/cli.mjs inbox-ack EVENT_ID
node integrations/pi/cli.mjs inbox-respond EVENT_ID --message "The existing task scope is approved; proceed with the specified next step."
```

The index is bounded (`--limit 1..50`, default 20); use `--after EVENT_ID` for the
next page. Pages are observation snapshots, not a streaming delivery cursor; start
from the beginning when polling again. `--consumer NAME` separates each manager's
read acknowledgements. All local consumers share the same inbox; names are labels,
not authentication or assignment. Ack means read, not approval or resolution;
unanswered questions and historical responses remain available across restarts.
`inbox-read --budget-bytes 32768` can expand one event without replaying a transcript.

An explicit response uses one deterministic message ID per event. It checks the
original idle instance and current work/scope before sending; old questions cannot
silently send into a new run. Its intent precedes delivery and uncertainty never
causes a blind resend. Inspect `inbox-read` for the actual message receipt. A
started/settled message proves processing began, never independent task acceptance.
Enrollment and a prepared manifest are not prerequisites for responding to a
reportless event. Missing setup is not a denial; an explicit pause or change to an
existing bound scope still protects against stale responses. The ordinary `send`
command remains available for a fresh authorized instruction.

### Optional reuse of an existing authorization

This is manager evidence, not an additional permission gate. A manager can save a
reference to an already-approved exact action/resource using a JSON file:

```json
{
  "requestedAction": "fixture_continue",
  "resource": "offline-only",
  "source": { "uri": "fixture://operator", "revision": "approved-v1" },
  "note": "This action is already approved within the existing scope."
}
```

```powershell
node integrations/pi/cli.mjs authorization-record EVENT_ID --record approval-reference.json
node integrations/pi/cli.mjs inbox-respond EVENT_ID --authorization RECORD_ID --message "Proceed with the already-approved step."
node integrations/pi/cli.mjs authorization-revoke RECORD_ID
```

Replace fixture values with the request's exact action/resource and actual evidence
reference. A later matching request under the same session/run/manifest/scope shows
the saved reference as a candidate. The receiver cannot create it through its report
tool, and matching never sends a response automatically. References are manager
declarations, not independently verified grants; their contents are not fetched or
executed. Missing/mismatched/revoked optional evidence produces a warning and is
omitted from the explicit response, not a new request for permission.

### Delivery limits and failures

`inbox-wake` currently returns `unsupported` with `notified: false`: this package
has no configured adapter to the existing Codex desktop host. It starts no process,
model or schedule. An active manager can read the durable inbox; persistence is not
proof that an idle manager was notified. Host wake routing is still required for
unattended supervision.

Inbox telemetry is a side channel. Storage/notification errors appear in session
status but do not reject valid handoff reports, handoff updates or ordinary channel
messages. Persisted events, read acknowledgements, response intents and authorization
records live in the private registry and are not automatically deleted. A pending
publication is replayed idempotently on startup/heartbeat; the latest durable report
can also be recovered. If storage failed before anything was persisted, an observation
may be unavailable after a crash; no successful delivery is claimed. Hard-link atomic
publication requires a filesystem supporting hard links; unsupported storage reports
an error and original work continues.

## Select dependencies directly: follow, inspect, pause

```powershell
node integrations/pi/cli.mjs doctor
node integrations/pi/cli.mjs follow SESSION_ID --project C:/ai_agent/edda --tasks 17,18 --scope "Observe these dependencies within the existing task scope; no new work or spending authority." --notify --max-notifications 10
node integrations/pi/cli.mjs dependencies SESSION_ID
node integrations/pi/cli.mjs unfollow SESSION_ID
```

Use the exact session ID shown by `doctor`/`list`, and your actual upstream task
IDs. `doctor` tells you whether the original Pi is offline, needs an idle `/reload`,
needs enrollment, has a handoff, or is following dependencies. `follow --scope`
can create/update the local enrollment; omit scope to reuse an enabled enrollment.
Invalid setup is validated before changing enrollment.

Without `--notify`, follow is **observation only**. With it, you explicitly permit
an initial state message plus later changed-state messages to the current Pi model.
Normal model usage applies; the default cap is 10 notification attempts per
subscription, not a dollar-budget guarantee. Existing task/spend/approval limits
still apply. No notification is a new grant or a declaration that all gates passed.

The observer runs inside the existing Pi extension, every 60 seconds while that
Pi process is running. It reads the selected tasks through the installed Rust
`edda task show --json`. It does not wake a separate manager model to poll.
Changed status, attempts, receipts or evidence can create an alert; timestamp-only
noise does not. Select the execution **and** relevant review/correction task IDs:
this version does not infer new review tasks or crawl the dependency graph.

If the receiver is busy or waiting on an extension UI prompt, changes are coalesced
until it is idle. The alert contains task facts and bounded, explicitly untrusted
receipt excerpts. A review task marked `done` can still contain Changes Requested;
the receiving controller must inspect its actual meaning and original authority.

Each source transition has a monotonic sequence and its own message ID. Repeated
checks of the same facts do not resend; A→B→A still has a fresh identity. One
uncertain/unconfirmed delivery blocks additional notifications until resolved or
explicitly reconfigured. `dependencies` and `doctor` expose that receipt state;
never assume the model received or acted on an unconfirmed notification.

`unfollow` writes a durable pause marker even if Pi is unreachable. A notification
already handed to Pi cannot be recalled. Scope changes, paused enrollment or a
changed handoff manifest suspend notification. After Pi reloads/restarts, use
`follow` again explicitly; old pending changes do not replay into a new instance.
The same active follow configuration does not reset its cap. To authorize another
subscription after the cap, deliberately unfollow and follow again.

`check-dependencies SESSION_ID` requests an immediate check using the saved mode;
it may notify only if `--notify` was already enabled. `dependencies SESSION_ID` is
read-only. A source read error retains the prior baseline and sends nothing.
Diagnostics remain available to explain stopped/paused/error/limit states.

## Use from Codex or a local shell

From the checkout containing this integration:

```powershell
node integrations/pi/cli.mjs list
node integrations/pi/cli.mjs status SESSION_ID
node integrations/pi/cli.mjs send SESSION_ID --message "Continue the assigned task within its existing scope." --sender codex
node integrations/pi/cli.mjs receipt SESSION_ID --id MESSAGE_UUID
node integrations/pi/cli.mjs conversation SESSION_ID --limit 20
```

Replace `SESSION_ID` with the exact value from `list`; labels are display-only.
The send command prints a generated UUID to stderr **before** network delivery
and the receipt to stdout. On uncertain output, query that receipt. If retrying,
use `--id MESSAGE_UUID` with the identical message, sender and mode; never mint a
new ID just because a request timed out. A reused ID with different content is
rejected. For multiline text use `--message-file PATH` (UTF-8).

`followUp` is the default: a busy Pi processes the message after its current work.
`--mode steer` requests delivery at Pi's next steering boundary. An idle Pi starts
a new turn in the same conversation. Both modes visibly prefix the user message
with the Edda message ID and sender. Sender is a label, not an authorization role.
Slash commands/templates are not expanded. Sending a message may incur the
current Pi model's normal usage; this channel sets no new model or spend allowance.

## Interpret status and receipts

| Field/state | What it proves |
| --- | --- |
| `live: true` | The authenticated instance just answered a status request |
| `idle` | No tracked Pi run/tools/UI prompt; does not mean its task is complete |
| `running` | Pi emitted a run start and has not settled |
| `executing_tool` | One or more tools have not ended; `toolNames` lists names, not arguments |
| `waiting_user` | An extension UI prompt is open; new messages are refused |
| `stopped` | The extension shut down normally |
| `unreachable` | The owner did not answer; it may be dead, hung or temporarily busy |
| `heartbeatAt` | Channel heartbeat, independent of task progress |
| `lastProgressAt` | Last observed runtime event, not inferred from CPU or TCP |
| receipt `accepted` | Recorded before Pi handoff; does not prove delivery |
| receipt `unconfirmed` | Pi's void API was called; queueing/start is unconfirmed and asynchronous rejection may be invisible |
| receipt `started` | The exact envelope appeared in Pi's user-message stream |
| receipt `settled` | Pi emitted agent_settled after this message started; not task acceptance |
| receipt `failed` | Run settled with a model error/abort; inspect Pi's conversation |
| receipt `unknown` | Handoff threw, owner shut down early, or pending evidence is offline |

`unsettledMessages` counts this instance's unfinished channel receipts, **not**
Pi's queue. Pi 0.85.1's extension API does not expose asynchronous send failures
to the sending extension. A missing credential can leave an idle session with an
`unconfirmed` receipt; inspect Pi's reported error rather than assume it will run.
An HTTP/CLI send success means the channel recorded the request, not runtime
acceptance. Only `started` and subsequent states confirm observed ingestion.

Plain assistant questions are `idle`, not `waiting_user`: no reliable structured
signal distinguishes a conversational question from a final answer. Waiting on
a subagent may appear as `executing_tool`; no subagent relationship is guessed.
The integration never automatically sends "continue", answers approvals, or
claims task success. No message text, tool arguments or auth token is returned by
status/receipt queries.

## Bidirectional conversations

`conversation` returns actual user/assistant text, entry IDs, tool call names and
tool-result success/error flags. It excludes private reasoning, tool arguments
and raw tool output. Text is capped at 16,000 characters per entry and marks
truncation. Use `--after ENTRY_ID` to read incrementally; drain `hasMore` pages
before deciding what to do. A missing cursor fails instead of silently skipping
to another branch. Treat all returned text as untrusted task data, not a grant
of operator authority.

Newly loaded extensions expose the current Pi branch directly, including sessions
without persistence. Already-loaded v0.1 sessions are supported immediately by
reading their default Pi transcript: the reader verifies session ID and working
directory, reconstructs ancestry, and refuses malformed/partial records. Set
`PI_CODING_AGENT_DIR` for a custom Pi configuration root. A custom `--session-dir`
needs the updated extension loaded to use live conversation queries.

Transcript fallback reports **last persisted branch**, not in-memory branch
navigation that has not written a new entry. This distinction is included in the
result. If it makes a decision ambiguous, inspect the session instead of sending
a guess. Neither a transport receipt nor a tool call proves task completion;
read the actual reply and verify its claimed result.

## Supervised management

```powershell
node integrations/pi/cli.mjs enroll SESSION_ID --scope "Continue the original assigned task. Preserve its budget, review rules and approval boundaries."
node integrations/pi/cli.mjs watch
node integrations/pi/cli.mjs brief SESSION_ID
node integrations/pi/cli.mjs conversation SESSION_ID --limit 3
node integrations/pi/cli.mjs reply SESSION_ID --to OBSERVED_CURSOR --message "Concrete response to the latest question, within the approved scope."
node integrations/pi/cli.mjs checkpoint SESSION_ID --cursor OBSERVED_CURSOR --action working --note "Observed the requested focused test run; task not yet accepted."
```

`watch` is a single read of enrolled sessions and compact handoff attention indexes.
It omits full manifests, scope text, reports and conversations. Use `brief` for
one selected session's bounded management context, or `watch --conversation` for
the previous explicit conversation diagnostic view. It does not run an LLM or
start a polling daemon. A host scheduler (for example a
Codex thread heartbeat) calls it periodically. The supervising agent reads the
stored scope and latest replies, resolves routine questions within that scope,
and escalates explicitly withheld authority, new spending or scope changes.
Never use a periodic blind "continue" message or infer permission from Pi's text.

`reply` requires a live idle session and the latest inspected cursor. It checks
the snapshot again and pins the instance before sending. This is a preflight
check, not an atomic lock on a user concurrently editing the conversation;
send only instructions that remain valid within the original scope. It uses a
deterministic message ID per session/cursor and persists an intent before sending.
Repeated identical requests return the receipt; changed replies at the same
cursor refuse. Interrupted attempts without a receipt stay unknown, never replay.

Checkpoints remember the cursor and evidence note; actions are `observed`,
`working`, `waiting_user`, `complete` and `paused`. The last two disable enrollment.
`complete` requires a live idle session and the current conversation head (new
unread activity refuses completion), but the supervisor must still verify task
acceptance evidence. `waiting_user` keeps the session enrolled without repeatedly
raising the same already-read question. A dropped lifecycle lock requires manual
inspection; no recovery path silently deletes controller intent.

To stop managing a session, use `checkpoint SESSION_ID --action paused --note
"Operator requested pause"`. No cursor or reachable runtime is required for
pausing; the last verified cursor is preserved. The
supervision records live in the private registry, not in the project ledger or
Git; enrollment is a local controller policy, not new project/task authority.

## Management handoff MVP

The next small layer is a prework management manifest plus structured reports.
It separates what a **supervisor** needs from a controller's full implementation
brief. It does not implement automatic judgments, Flash/strong-model routing,
new authority or a second Edda task state machine. The opt-in dependency observer
above supplies deterministic receiver-local polling, not a model-driven manager.
Codex-to-Codex messages should continue using native session tools; this package
is the Pi transport adapter and local observational prototype.

Prepare a bounded JSON manifest from the already-authorized plan, using
[management-manifest.json](./fixtures/management-manifest.json) as a shape example
(the example itself grants no real-world authority). Then run:

```powershell
node integrations/pi/cli.mjs prepare SESSION_ID --manifest path/to/management.json
node integrations/pi/cli.mjs brief SESSION_ID --budget-bytes 16384
```

The manifest includes `runId`, `role` (controller/worker), `goal`, `doneWhen`,
`planRef` with a revision, and declared `scope.allowed/excluded/reserved` plus
`authorityRefs`. It is capped at 8 KiB. Source references are retained verbatim,
not fetched or resolved into grants. `compose` can now populate task facts and
extract explicitly marked management metadata. Semantic interpretation of
arbitrary prose and authority resolution remain outside this integration.

Preparation requires an idle live Pi instance. A changed manifest requires
`--expected CURRENT_MANIFEST_REVISION`; reload/resume also requires explicitly
rebinding with that revision. Old reports are not reused as current progress.
The content digest is stable for identical manifests and does not claim a
cryptographic authorization signature.

Newly loaded Pi extensions expose:

- `edda_handoff`: read the prepared brief and its current manifest revision.
- If the tool returns `needs_context`, call `edda_handoff` with
  `budgetBytes: 32768` (or another explicit 512..32768-byte budget). The tool
  validates the bound; it never silently increases the budget or truncates scope.
- `edda_report`: report a milestone or stopping reason against that revision.

Only prepared sessions receive a short context reminder, once per manifest
revision. Other sessions continue normally. Reporting failure affects management
visibility; it does not stop the worker's task, create a retry, call another model
or mark an Edda task done.

Reports carry `reportedState`, `stage`, `summary`, `nextStep`, `evidence` and
`dependencies`. A `waiting_decision` report also needs a concrete `decision` with
question, requested action/resource and recommendation. `waiting_dependency`
needs references; a `completed` claim needs evidence. Neither can change the
manifest or write verified acceptance. Tool call identity, current Pi instance
and a runtime work epoch bind each report; duplicate IDs cannot change content.

After new work starts, previous-epoch reports are no longer current. If Pi settles
without a current stopping report (or only says `working`), attention is
`missing_report`. Waiting decisions/dependencies, failed/paused reports and
`completion_pending` are separately visible. No transcript keyword inference is
used; the reports remain worker claims, and `acceptance` remains `unverified`.
Work epochs remain monotonic within an instance across manifest replacement,
including restoring earlier manifest content; historical report IDs cannot
become eligible for a later run merely because the content digest matches again.

`brief` composes the full required manifest + latest current report + observed
runtime + local supervisor enrollment scope, if present. It never reads a
transcript. The default budget is 16 KiB of compact JSON (maximum 32 KiB); byte
size is not a model-token guarantee, and pretty CLI formatting adds whitespace.
If required fields cannot fit, the result is `needs_context` with the required
size; constraints/evidence are never silently truncated. `watch` remains an
index: call `brief` only for the session that needs attention.
Non-ready `brief` results (including missing preparation, stale binding and
insufficient budget) still print JSON and exit 2, so a caller cannot treat them
as a ready decision context based on command success alone.

Persistence keeps the current manifest/report plus up to 1,000 deduplication
receipts in a private atomic file. It is not a full report-history archive or
Edda ledger. Original plan/evidence references and Pi's own transcript remain
the audit sources. Full role/authority resolution and semantic progress scoring
belong to later slices; this MVP deliberately does not infer them.

Existing loaded extensions need `/reload` at idle for the new handoff capability.
Their previous bidirectional conversation functions remain usable; `watch` shows
`handoff_unavailable` instead of silently replaying history on their behalf.

## Compose from existing Edda tasks

This adapter is JavaScript; the task engine remains Rust. `compose` executes only
the installed `edda task show ID --json` with a supplied project directory and
argument-safe process execution. It never changes a task, launches a session or
prepares a Pi automatically.

```powershell
node integrations/pi/cli.mjs compose --project C:/ai_agent/edda --task 17 --output handoff.json
node integrations/pi/cli.mjs compose --project C:/ai_agent/edda --task 17 --context management.md --output handoff.json
node integrations/pi/cli.mjs prepare SESSION_ID --manifest handoff.json
```

Replace the example project/task with your intended source. The separate prepare
step selects the destination session. `--edda-bin PATH` (or `EDDA_BIN`) selects a
native executable when it is not on PATH; no shell or command-string fallback is
used. Task IDs outside JavaScript's safe integer range are refused rather than
silently rounded. Task engine transitions and numeric contracts remain Rust-owned.

Task identity, goal/title, path facts, dependencies and plan/work-unit references
come from Edda. The remaining metadata is JSON with exactly `role`, `doneWhen`
and `scope` (allowed/excluded/reserved/authorityRefs), or one top-level fenced
`edda-management` block containing that JSON. See
[management-context.md](./fixtures/management-context.md) for the format.
Documentation examples nested inside another code fence do not count.

Without `--context`, the adapter reads a regular UTF-8 file named by the task's
`brief_ref` only when its resolved path stays inside the project. A missing or
inline brief, an external path, or a URL does not trigger broader discovery or
network access. `--context FILE` is an explicit file selection and can refer to
a file elsewhere. Each source is limited to 256 KiB; full composition previews
are bounded to 32 KiB. Ambiguous blocks, malformed input and absent mandatory
fields produce `needs_context` (exit 2) or a read/validation error (exit 1).
Ordinary prose is never interpreted as authority or completion criteria.

Complete results print `status: ready` and a validated manifest. `--output`
creates a **new** manifest file only for ready results; an existing file is never
overwritten. Without this option, ready output is returned as JSON without a
manifest file. Missing data does not create a partial output file.

`scope.taskPaths` preserves Edda's original path facts separately from declared
allowed actions. An empty list means no path declaration was supplied, not
unrestricted permission. Excessive path lists remain a context gap; they are not
silently shortened. Role is supplied explicitly, not inferred from an assignee's
name. A ready composition is readable context, not an authorization verdict.

Source task JSON, the selected metadata file and a bundle of their references are
saved by content digest in the private `sources/` cache. `planRef` points to that
immutable bundle so the initial inputs remain inspectable after the task changes.
The run ID is scoped by canonical source directory and task creation event; this
is a local view identity, not a new global Edda project/task ID. Recomposition
reads a fresh snapshot but never silently rebinds a running handoff. Snapshots
remain local and are not automatically deleted or published.

## Storage, identity and recovery

Registry and receipts live under `~/.edda-pi-sessions`, outside Git. To isolate
tests or hosts set `EDDA_PI_CHANNEL_DIR` to a **dedicated** directory in both Pi
and the client. Startup restricts that directory's permissions before writing
credentials: owner-only Unix modes or a protected current-user Windows ACL.
Windows requires the built-in Windows PowerShell 5.1 for the ACL helper.

The endpoint binds only `127.0.0.1` on an OS-assigned port, authenticates a random
token, rejects requests carrying browser Origin, and requires the exact instance
ID. Same-user processes and extensions are trusted; this is not a sandbox between
agents running under your account. Do not expose or forward the port to a public
network. A remote Codex front end must already have authenticated execution on
this host; phone connectivity itself is outside this package.

An exclusive owner record prevents two channel instances for one Pi session.
Normal session switch/reload/shutdown closes the endpoint and releases ownership.
After a crash inspect the session and, only when its PID is dead, run:

```powershell
node integrations/pi/cli.mjs recover SESSION_ID --instance OLD_INSTANCE_UUID
```

Recovery validates the stored instance and dead PID and preserves receipts.
PID reuse fails closed. Recovery does not launch Pi, replay messages or restore
its in-memory queue. Resume the session normally; old message IDs still dedupe.
On reopening, previous-instance nonterminal receipts become durable `unknown`
with `lastRecordedStatus` retained; they never return to an apparent queue.
If the process crashes during the very short lifecycle lock operation, a
`lifecycle.lock` may need manual inspection/removal after proving no owner lives.
Receipts are bounded to 1,000 per session; retain/archive evidence before resetting
a session's registry. No automatic deletion or unbounded offline queue is provided.
Atomic file replacement protects against partial process writes; power-loss
durability and filesystem corruption recovery are not guaranteed.

## Verify

```powershell
npm --prefix integrations/pi test
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --reject
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --supervise
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --handoff
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --handoff-budget
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --inbox
node integrations/pi/managed-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js
node integrations/pi/dependency-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js C:/Users/fagem/.cargo/bin/edda.exe
```

The smoke test starts an isolated actual Pi with only this extension and a
deterministic offline provider. It sends two messages through the real channel,
checks two replies and receipts in the same session, then stops only that test
subprocess. No credentials, user sessions or paid model calls are needed.

The dependency smoke creates an isolated real Edda workspace/store and a real Pi
using the offline provider, changes only its synthetic task, waits for the actual
60-second timer to notify, verifies the same-session reply, then pauses and cleans
only that test's processes/directories.

The managed smoke checks client reconnection, same-session stop/resume, inbox
retention and no initial-prompt replay. Its default provider is offline. An explicitly
authorized real-model fixture can run with `--live PROVIDER MODEL`; it writes and
verifies one file only in its temporary project, records observed usage, and cleans
up owned processes. Failed smoke evidence is preserved in its printed temporary
directory. The real model test is separate from deterministic CI and is not a model
quality benchmark.

## Remaining usability/product work

- Automatic selection of newly created review/fix tasks and structured acceptance joins.
- Bootstrap/update convenience for already-running old extensions; one idle reload
  is still needed to load a new capability.
- Full authority resolution and manager/strong-model escalation policy; no natural
  language permission guessing is implemented.
- Proactive Pi-to-Codex wake routing, Claude/Hermes recipient adapters and remote hosts.
- Published package distribution and upgrade management for the Pi dependency itself;
  managed integration releases already have a stable path outside the worktree.

These are separate stages. Current dependency alerts are usable with explicit
selected tasks and limits; they do not claim those broader capabilities.
