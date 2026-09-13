# Edda agent manager

A local Traditional Chinese progress and conversation console for explicitly
selected Pi agents and read-only Codex records. The browser,
gateway, operational journal and Pi transport have separate boundaries.

## Run

Requires Node24.14+ (tested on24.14.0), Pi channel0.7, and the sibling
`integrations/pi` source. New code is TypeScript5.9.3 with strict checks and a
lockfile; `tsc` emits ESM. Native Node SQLite is experimental in Node24 and emits
its normal warning. The operational schema is versioned; an unknown schema version
is refused rather than reset. No Docker, external database or frontend CDN is used.

```powershell
cd integrations/agent-manager
npm ci --ignore-scripts
npm run typecheck
npm test

# Import the currently selected managers; creates config, never launches agents.
node dist/src/cli.js init --from-delegation C:/Users/fagem/.edda-pi-sessions/delegations/20260912-acceleration/index.json --root C:/Users/fagem/.edda-agent-manager
node dist/src/cli.js start --root C:/Users/fagem/.edda-agent-manager
node dist/src/cli.js status --root C:/Users/fagem/.edda-agent-manager
node dist/src/cli.js url --root C:/Users/fagem/.edda-agent-manager
node dist/src/cli.js stop --root C:/Users/fagem/.edda-agent-manager
```

`start` runs a hidden, owned background service; the launching terminal may exit.
`serve` runs it in the foreground. Default port4390; choose `--port 0` for an
available temporary port. The private launch URL includes a gateway credential in
its fragment; the app removes it immediately and stores it only for that browser
origin. Do not publish that URL. Pi bearer tokens never leave the backend.

`stop` stops the console only. Pi workers and their work continue. Restart uses the
same configuration/journal, queries old message receipts, and never replays sends.
A fresh gateway credential is created per service process; reopen its launch URL
after a restart. If a service crashed, `recover` clears ownership only when the
recorded PID is demonstrably absent; it never kills a saved PID. Preserve this
checkout while the service uses its compiled/static files; stop/rebuild/restart for
an upgrade rather than editing a running release in place.

## Configuration and actual capabilities

`config.json` in the private root is an explicit operator selection. Add a known
worker by editing the agents array while the console is stopped, then restart (or
add an already-running run from the console with **加入管理**, see below).
Do not enumerate every directory of an unrelated Pi registry. Each agent has:

```json
{
  "id": "character",
  "name": "Character Sol 負責人",
  "projectId": "character",
  "role": "manager",
  "registryRoot": "C:/path/to/private/pi-registry",
  "sessionId": "actual-pi-session-id",
  "runId": "actual-managed-run-uuid-or-null",
  "workspace": "C:/path/to/actual/worktree",
  "summaryFile": "C:/path/to/selected/status.md"
}
```

The complete file is `{version:1, projects:[...], agents:[...], refreshMs:3000, works:[]}`.
Projects contain `id`, `name`, `priority` (lower first) and `resources` (registered
name/kind/details/owner/source, not live health). `runId` and `summaryFile` accept
JSON null. The importer handles the current Character/Edda delegation index and
Character's registered PostgreSQL service.

The overview distinguishes runtime activity, source availability, old heartbeat
metadata, last public response, manager-written summary and provider-reported
usage. None implies accepted task completion. Refresh and observation make no model
calls. Missing summaries and unavailable agents do not suppress healthy peers.

Open an agent to read recent public messages and forward-cursor updates. Send an
append message (`followUp`) or a priority instruction (`steer`) to the exact viewed
instance. Priority instructions wait for Pi's next safe point; they do not force
kill a tool. These are new operator messages, not atomic approval answers to an
old question. The viewed cursor is recorded as provenance only. No generic
shell/file/URL proxy or service lifecycle action is exposed to the browser.

Each request has a durable UUID before dispatch. Duplicate identical requests
return the same operation; changed payloads conflict. Unknown outcomes retain the
same ID and are queried, never automatically resent. The browser preserves drafts
per recipient and suspends duplicate sending when another tab changes its pending
evidence. If a branch cursor becomes invalid, reload the conversation while keeping
the draft. Completed execution receipts still require product acceptance elsewhere.

Messages are bounded to12KiB UTF-8, with additional adapter encoded-frame validation
before dispatch. HTTP bodies are bounded to96KiB to accommodate JSON escaping.
Selections and events are bounded per response. One service owns the private SQLite
journal; operations retain their original target even if a display agent is later
bound to a new session. The journal does not replace Edda task/decision truth.

## Discovering managed Pi runs

The sidebar's **候選執行** section (or `GET /api/candidates`) lists Pi sessions that
already exist in the registry roots this configuration names, so a running workbench
shows a freshly launched run without editing `config.json` or restarting. **更新候選**
re-reads the list; discovery is never polled in the background loop.

Bounds and safety:

- The root set is exactly the effective default Pi registry (`EDDA_PI_CHANNEL_DIR`,
  else `~/.edda-pi-sessions`), kept first, plus the distinct `registryRoot` values of the
  already-selected Pi agents, deduplicated and bounded to at most 32 roots; if the configured
  union exceeds that bound, the dropped roots are reported as a source issue rather than
  silently ignored. There is no other home/project/directory scan, and no raw
  `owner.json`/`state.json`/`managed/**.json` parsing in the manager: each root is read
  through the same validated Pi inventory used elsewhere: `listManagedRuns` (recorded
  managed runs with their `runId`) and `listSessions` (live or offline sessions). A
  managed run and its live session merge into one row, with bounded roots, bounded rows
  per root and bounded run limits.
- It is read-only. Listing only reads status/inventory; it never assigns work, changes an
  owner, launches or resumes a session, calls a model, sends a message or records
  acceptance.
- A candidate whose run is already selected shows **已加入管理** with its agent id
  and cannot be added twice. That match uses the same `(registryRoot, sessionId)` identity
  `config.json` enforces, so the same session id under a different configured root is not
  mislabelled. A managed record and its live session in the same registry root are shown
  once, preferring the live registration; the same session id present under two configured
  roots is two root-scoped candidates, because that is the identity configuration and
  candidate ids use. Offline or stopped runs are shown as recorded
  evidence with a reason, never as proof of completion. If one root cannot be listed,
  its failure is reported and every other root still returns its candidates.

**加入管理** is the only mutation and it is an explicit operator action.
`POST /api/candidates/register {candidateId,id,name,role,projectId}` validates the
request with the same rules as `config.json` and adds the run to this running process
only; it does not edit `config.json`, restart a service, or change any work. The run
then appears in the project list like any selected agent, so the existing
**登記執行 session** (`bind_session`) action, conversations and message sending apply
unchanged. A runtime-added agent is not persisted; after a service restart, add it
again or record it in `config.json`. Merging, delivery and acceptance are still the
project's own gates.

Capability boundary: this removes the manual config edit/restart needed only for
seeing a candidate and for explicitly adding it at runtime. It does not auto-register,
auto-bind, assign, wake an owner, restart a worker, or migrate runs across machines,
and it does not scan arbitrary user or project registries.

## Architecture and checks

- `contracts.ts` / `config.ts`: DTOs, shared limits, versioned configuration validation.
- `pi-adapter.ts`: sole production legacy-module boundary, exact identity and private projection.
- `discovery.ts`: read-only candidate projection, session deduplication and opaque candidate ids.
- `store.ts`: atomic intent/event records and monotonic receipt evidence in SQLite.
- `manager.ts`: bounded observations, freshness, summaries and receipt reconciliation.
- `http.ts` / `cli.ts`: loopback bearer gateway, Host/Origin/body/CSP checks and owned service lifecycle.
- `web/`: text-safe responsive interface; no external scripts, styles or fonts.

`npm test` covers durable intent/restart, stale identity and branch reset, poisoned
unselected registry records, duplicate/unknown sends, public projection, actual Pi
channel HTTP and gateway controls, plus background CLI restart, bounded candidate
discovery/registration and per-root failure isolation.

Actual installed Pi with an offline provider (no paid model invocation):

```powershell
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js
# Keep an isolated browser fixture until POST /api/service/stop:
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js --serve
```

The smoke preserves its private receipt/workspace path for inspection. Real agents
are never used as test recipients. Autonomous delegation, non-Pi message delivery, structured
inbox approvals, automatic Docker operations, CPU/RSS sampling and remote hosting
remain subsequent slices. Opening a conversation does not claim exclusive control
over independent clients or the parent's existing monitoring automation.

## Work ownership and handoff

Select existing Edda tasks in the optional `works` array while the console is
stopped. The `workspace` must resolve the intended Edda project; it need not be
the current assignee's worktree. `ownerAgentId` names a selected manager in the
same project. No task is automatically created or completed by the console.

```json
{
  "id": "review-delivery",
  "projectId": "edda",
  "taskId": 217,
  "workspace": "C:/ai_agent/edda",
  "ownerAgentId": "edda"
}
```

Use an actual task number from `edda task list`, not the illustrative number
above. The task's title/status/receipt are read from `edda task show --json`.
Versioned coordination notes tagged `manager-work-<taskId>` in that same ledger
retain assignments, direction changes, evidence and closure. The task rail
remains canonical; coordination acceptance neither runs a merge nor sets task
status to done. Preserve original notes rather than editing their JSON by hand.

The work card separates the closure owner, current assignee, next step and task
status from message delivery. Choose **安排下一步**, then **交辦給代理** with a
bounded brief. A channel accepting a message does not mean the agent has started.
A started receipt proves execution; a settled receipt means the reply ended and
delivery evidence is still due. Record **交付** with evidence, then have the
authorized owner record **驗收與收尾** after the project's existing gates.

Use **變更工作指示** for a direction change that must remain visible to the owner.
It sends a priority message to the current assignee and keeps a pending instruction
on the work card. **記錄指示已確認** requires evidence of the agent's explicit
acknowledgement. This is an operator attestation, not a model-inferred agreement.
Ordinary questions can still use the agent conversation beneath the work card.
Viewing or acknowledging a work item does not grant a new runtime permission.

The browser preserves full action requests before POST. If the result is unknown,
**查詢／恢復原操作** submits that same request and ID; do not manufacture a new ID.
A stale revision rejects the edit and offers to preserve the draft for review
against the latest work. Uncertain writes retain their original request. Service
restart reads the same notes and message journal; it never automatically replays
a send. Losing the original message journal is not proof that no send occurred.

API (same loopback bearer/Host/Origin boundaries as conversations):

- `GET /api/works` → `{works, generatedAt}`; per-work unavailable/error state.
- `POST /api/works/:id/actions` → the updated `WorkView` and `confirmedActionId`.
  Every action includes `actionId` (UUID) and the observed `revision`.
- Actions: `initialize {nextStep}`, `assign {agentId,nextStep,send}`,
  `intervene {send}`, `acknowledge {instructionId,evidence}`,
  `deliver {evidence,nextStep}`, `accept {evidence}`, `block {reason,nextStep}`.
  `send` is the existing `SendRequest` including its own stable operation UUID,
  current selection revision and exact instance. See `workflow-contracts.ts`.

Workspaces/executables cannot be supplied in an HTTP action. The adapter calls
only fixed `task show`, `log`, and `note` argument vectors with no shell. Each
selected work supports at most 256 coordination events; exceeding that limit
reports an explicit error, preserving history for a successor task. Per-task
cross-process locks contain no task state and are released by the OS after a crash.
Serialized coordination events are limited to 12,000 characters, including JSON
escaping and target metadata, to fit Windows command arguments. A larger event is
rejected before recording or sending; shorten its message/evidence. Work snapshots
return within 1.5 seconds with loading rows if needed; a shared background refresh
uses at most two concurrent work reads and a ten-second per-work cache.

Explicit real Edda + installed Pi offline lifecycle (temporary isolated project,
no paid model invocation and no production test recipients):

```powershell
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js --workflow
# Optional interactive browser fixture:
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js --workflow --serve
```

The receipt proves one send despite duplicate requests, settled versus accepted
separation, explicit delivery/closure and missing-task isolation. Browser work
cards are available to the owner even when another agent is selected in the same
project. They do not yet schedule an autonomous owner wake-up or remote notification.

## Selected sessions and the owner inbox

In **登記執行 session**, select an existing agent, its work role, expected
next report and optional deadline. A reviewer must name a full reviewed commit SHA.
The UI uses the current owner as parent; the API can name another selected parent.
The work's Edda note chain retains the exact session identity and selection revision;
changing an agent's configuration does not rewrite that historical binding. Retire
an old relation explicitly with **結束 session 追蹤**. These actions neither start
agents nor dispatch tasks. A missing relation or heartbeat never authorizes a second
writer. Each work permits32 active relations and128 historical relations.

Pi continues to use the selected authenticated channel. Its public reply-end/error
events and known managed stop metadata are projected without private reasoning,
raw tool arguments/results, or provider error strings. The bounded recent sample
is marked history-incomplete; it is not a lossless native event subscription.

Codex can be selected as a read-only recorded source:

```json
{
  "id": "codex-reviewer",
  "name": "Codex reviewer",
  "projectId": "edda",
  "role": "worker",
  "transport": "codex",
  "sessionId": "actual-desktop-thread-uuid",
  "workspace": "C:/path/to/recorded/workspace",
  "transcriptFile": "C:/path/to/explicitly-selected-rollout.jsonl"
}
```

The adapter validates session metadata and workspace against this configuration.
It reads at most1MiB per observation, including metadata/checkpoints, retains up to
64 public entries and128 native events, and persists a bounded cursor projection.
It handles incomplete tails, corrupt/oversized rows, truncation and replacement
visibly. A tail bootstrap reports incomplete history. `task_started`,
`task_complete`, `turn_aborted`, and native child references are observed data;
history ancestry is not ownership. Recorded activity is never labeled live process
proof. Codex sends are unsupported in this slice; continue its conversation through
the native app. Unknown provider-event shapes remain unsupported rather than guessed.

The **負責人事件收件匣** persists reply-ended, interruption, normalized provider
error, source-unavailable and overdue notices before showing them. Stable native
identity and binding identity prevent duplicates after restart. Provider categories
and HTTP status are bounded metadata only. Explicit acknowledgements retain their
own UUID/evidence and mean the notice was read, not that an instruction was understood
or a task accepted. A child event newer than the owner's summary marks that summary
stale. Deadlines are unresolved expectations until retired/replaced or independently
delivered; random tool activity cannot satisfy their free-text meaning. Overdue
means suspected stalled, never automatic termination or reassignment.

**移交收尾負責人** transfers the work to a selected manager with evidence, retaining
pending directions, original message targets, session relations and unresolved
alerts. The bounded context endpoint provides that handoff packet. It does not
automatically rotate a model based on cumulative token usage.

Authenticated API additions (paths/executables still come only from local config):

- Work actions: `bind_session {agentId,role,parentAgentId,reviewedSha,expectedEvent,nextExpectedAt}`,
  `unbind_session {bindingId}`, `handoff_owner {ownerAgentId,evidence}`; all use the
  existing `actionId` and observed `revision`. Optional SHA/deadline/parent use null.
- `GET /api/owner-inbox?ownerAgentId=<selected-id>` returns bounded events.
- `POST /api/owner-inbox/ack` accepts `{eventId,actionId,evidence}`.
- `GET /api/owners/:id/context` returns at most20 work packets within64KiB, bounded
  recent work history, pending instruction previews and at most10 unresolved alerts
  per work, plus truncation. Each packet's `detailPath` points to the complete
  selected work at `GET /api/works/:id`; source evidence is never truncated in storage.

The journal schema upgrades add observational tables without replacing canonical
Edda work events. Polling runs in the service (normally every3 seconds) and is
independent of whether a browser is visible; disconnected sources remain unknown.
Persistent inbox availability is not proof that a sleeping owner has read it.
Native push/wake and remote notifications remain separate capabilities.

Run the isolated actual Pi/Edda owner-inbox path with:

```powershell
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js --session-events
```

This binds the real isolated Pi session before assignment, observes its native end
event in the owner's inbox and acknowledges it twice with one durable result before
independent delivery/acceptance. Synthetic Codex tests cover file and event boundaries;
a selected actual Codex rollout was also observed read-only without exposing content.

## Native necessary context and recovery

The workbench now reuses native Edda continuity. Configure the optional absolute
`continuityExecutable` path in local config when PATH has an older executable.
A missing native capability is reported; no alternative capsule format is written.
Keep the configured executable pinned outside mutable build output.

Open **必要上下文與接手** on a selected work. Save goal/current evidence/next action,
copy readable text for a new session, or copy the native JSON to another configured
workspace. Native import checks repository identity and integrity. Restore displays
native warnings, including missing commits and dirty files; warnings do not silently
change into extra execution gates. Required artifacts still need to be accessible.
The capsule is data only and never imports runtime authority or starts a worker.

After checking the destination and old writer release, **記錄接手** reuses the existing
owner handoff. The original task lifecycle and next execution action stay under their
existing owner. Ordinary work assignment remains the separate work action.

Saving and importing persist the original operation identity and non-plaintext verification proofs before the native effect. After
an interrupted response, reopening this panel reads native capsule records and
reconciles verifiable saved state without repeating save. Known pre-write validation
refusals allow editing a new request; ambiguous I/O remains visible with its original
ID. **核對原生紀錄並恢復連結** accepts an exact capsule ID when bounded discovery cannot
find it, and verifies correlation before attachment. Browser persistence keeps only operation IDs, route and revision; unsaved prose remains in page memory. After reload, recover the original operation from native records or re-enter the original request using the same ID. Saved context is read back from Edda. Clipboard-denied environments have a selectable JSON view.

`start` now automatically reclaims a demonstrably dead, matching console owner under
a cross-process lifecycle mutex. It preserves config, SQLite and pending operations.
A live/reused PID or mismatched generation is refused without killing a process.
This is recovery when start is invoked; it does not install an OS startup task or
restart downstream agents. Existing receipt reconciliation resumes automatically.

Authenticated additions:

- `GET/POST /api/works/:id/continuation`: read/reconcile or save native input.
- `POST /api/works/:id/continuation/import`: import a native bundle.
- `GET /api/works/:id/continuation/:capsuleId`: exact native restore.
- `POST /api/works/:id/continuation/recover`: reconcile original action and optional capsule ID.
- `POST /api/works/:id/continuation/takeover`: record existing owner handoff.

The import body limit accommodates the native 528 KiB bundle plus request metadata;
ordinary message limits remain unchanged. Generic work actions cannot attach an
unchecked capsule reference. Executable and workspace come only from local config.

### Layer ownership after capability audit

The console is a view/intervention adapter, not a replacement scheduler. Native
controlled tasks remain owned by `edda control` (sealed manifest, lease, attempt,
dispatch identity, receipts and adjudication). Existing Pi managed/supervised tasks
remain owned by their managed run/supervisor. Host/manual tasks retain their actual
host/controller. A missing session map does not authorize a new lifecycle or writer.

`crates/edda-cli/src/skills/coord-orchestrate.md` is the canonical strong-planning
skill; `coord-run.md` guides the native runtime loop. Installed `.agents` copies
can lag behind the canonical source because initialization preserves existing files.
Development controller/worker/verifier roles are not another product scheduler.

The current workbench observes interruption/provider/deadline events and stores the
owner inbox, but this does not prove owner wake delivery or lossless Pi event capture
while offline. Native controlled advancement, Pi supervisor routing, automatic worker
restart, OS boot scheduling and cross-machine synchronization are distinct downstream
capabilities; this change does not replace or claim to implement them.

- `GET /api/works/:id/actions/:actionId`: read whether an original work action was recorded; an absent record remains unknown and never authorizes a replacement action.
