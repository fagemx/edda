# Edda agent manager

A local Traditional Chinese progress and conversation console for explicitly
selected Pi agents. This is the first typed management-service slice: the browser,
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
worker by editing the agents array while the console is stopped, then restart.
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

The complete file is `{version:1, projects:[...], agents:[...], refreshMs:3000}`.
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

## Architecture and checks

- `contracts.ts` / `config.ts`: DTOs, shared limits, versioned configuration validation.
- `pi-adapter.ts`: sole production legacy-module boundary, exact identity and private projection.
- `store.ts`: atomic intent/event records and monotonic receipt evidence in SQLite.
- `manager.ts`: bounded observations, freshness, summaries and receipt reconciliation.
- `http.ts` / `cli.ts`: loopback bearer gateway, Host/Origin/body/CSP checks and owned service lifecycle.
- `web/`: text-safe responsive interface; no external scripts, styles or fonts.

`npm test` covers durable intent/restart, stale identity and branch reset, poisoned
unselected registry records, duplicate/unknown sends, public projection, actual Pi
channel HTTP and gateway controls, plus background CLI restart.

Actual installed Pi with an offline provider (no paid model invocation):

```powershell
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js
# Keep an isolated browser fixture until POST /api/service/stop:
node dist/test/offline-smoke.js C:/path/to/pi/dist/bundle/cli.js --serve
```

The smoke preserves its private receipt/workspace path for inspection. Real agents
are never used as test recipients. General delegation, non-Pi runtimes, structured
inbox approvals, automatic Docker operations, CPU/RSS sampling and remote hosting
remain subsequent slices. Opening a conversation does not claim exclusive control
over independent clients or the parent's existing monitoring automation.
