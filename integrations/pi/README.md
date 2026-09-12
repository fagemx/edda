# Edda Pi session channel

Read a running Pi session's replies and send a message to its existing conversation
from another local process (including Codex). Uses Pi's extension API; no
terminal keystrokes, transcript injection, duplicate agent or paid supervisor.

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
new authority, a second Edda task state machine or a monitoring scheduler.
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
not fetched or resolved into grants. Plan-to-manifest extraction is manual in
this slice; reading every plan and automatically joining task/authority records
is deferred to the canonical Edda service integration.

Preparation requires an idle live Pi instance. A changed manifest requires
`--expected CURRENT_MANIFEST_REVISION`; reload/resume also requires explicitly
rebinding with that revision. Old reports are not reused as current progress.
The content digest is stable for identical manifests and does not claim a
cryptographic authorization signature.

Newly loaded Pi extensions expose:

- `edda_handoff`: read the prepared brief and its current manifest revision.
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
node --test integrations/pi/channel.test.mjs integrations/pi/extension.test.mjs integrations/pi/conversation.test.mjs integrations/pi/supervision.test.mjs integrations/pi/handoff.test.mjs
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --reject
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --supervise
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --handoff
```

The smoke test starts an isolated actual Pi with only this extension and a
deterministic offline provider. It sends two messages through the real channel,
checks two replies and receipts in the same session, then stops only that test
subprocess. No credentials, user sessions or paid model calls are needed.
