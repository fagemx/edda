# Edda Pi session channel

Inspect a running Pi session and send a message to its existing conversation
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
node --test integrations/pi/channel.test.mjs integrations/pi/extension.test.mjs
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js
node integrations/pi/pi-smoke.mjs C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js --reject
```

The smoke test starts an isolated actual Pi with only this extension and a
deterministic offline provider. It sends two messages through the real channel,
checks two replies and receipts in the same session, then stops only that test
subprocess. No credentials, user sessions or paid model calls are needed.
