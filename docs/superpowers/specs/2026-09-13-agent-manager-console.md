# Agent manager console — first usable slice

Operator requested research then implementation: regularize the JS/TS management
layer, show shared progress/intervention, and allow direct conversations with a
backend agent. This design implements that authorized slice without another
approval round. Existing Character/Edda work continues independently.

## Findings and choice

Pi 0.7 integration already has authenticated per-session status/conversation/send,
UUID receipts, managed run status, and private-storage helpers. It has 72 passing
tests and actual Pi evidence. Cross-project selection, browser access, typed DTOs,
operator-facing progress and a unified operation journal are missing. Global Pi
discovery currently aborts on one malformed registry record; explicit selections
avoid letting unrelated damaged records disable the console.

Options considered: extend the Pi CLI with HTML (fast but couples presentation to
transport); rewrite everything in Rust/TypeScript (unnecessary replacement of live
receipt behavior); a typed independent TypeScript service over a Pi adapter
(chosen). New code is strict TypeScript compiled to ESM, with a lockfile and CI.
Legacy imports exist only at the Pi adapter boundary. Native Node HTTP/SQLite keep
runtime dependencies small. Type stripping alone is not type checking, so `tsc`
is an explicit gate, followed by Node tests.

The package is `integrations/agent-manager`. Responsibilities:
- contracts/config: versioned DTOs and runtime validation of operator configuration;
- Pi adapter: selected identity, public projection and existing authenticated APIs;
- store: SQLite management journal, selected configuration revision, send intents;
- manager: observations/freshness, operation reconciliation and bounded polling;
- HTTP/CLI: local authenticated gateway, process ownership and fixed static assets;
- web: Traditional Chinese overview, agent detail/chat, direct message composer.

This service owns operational records only. It does not invent another Edda task
rail, infer acceptance from runtime idleness, or decide that an unresponsive owner
is dead. General automatic delegation, non-Pi adapters, Docker lifecycle operations,
semantic approval replies and remote/public hosting are subsequent slices.

## Visible behavior

Projects group explicitly selected managers/workers. Each agent shows runtime
state, observation/heartbeat/progress times, last public response, optional
manager-written summary and provider-reported usage. Offline/stale/unknown remain
visible alongside healthy agents. An idle response is execution-unverified, never
automatically a completed task. Registered shared resources show their source and
ownership as configuration, not a fabricated live health reading. CPU/RSS sampling
is not in this slice; provider usage and shared-resource bindings are sufficient.

Select an agent to read its public conversation and target that exact agent in the
composer. Normal messages append after current work (`followUp`); priority
instructions use `steer` at Pi's next safe point. Neither action directly kills a
running tool. These are new operator messages, not atomic approval replies bound to
an old question. The viewed cursor is recorded for provenance; no conversation-head
CAS claim is made. Branch-invalid pagination is rejected and the client reloads a
fresh page without discarding its draft. A later explicit decision-response action
must use the existing inbox epoch/identity contract.

Every send carries a client-generated UUID, selection revision and freshly viewed
instance ID. Server rechecks the actual instance, then commits an intent before
sending through Pi. Same UUID/same payload returns the original operation; changed
payload conflicts. A crash/timeout leaves unknown evidence; restart and refresh
query receipts but never resend. HTTP success/queued/started/settled are execution
states, not product acceptance. UI retains uncertain messages and their original
IDs and offers receipt inspection instead of blind new-ID retry.

A bounded refresh (roughly three seconds) needs no model calls. Changes in runtime,
last public response, source error and message receipts populate a durable activity
feed. An unavailable source or summary file cannot disable other agents. Agent chat
is read on demand and refreshed while selected; raw reasoning/tool arguments and
Pi authentication material never enter DTOs.

## Security and storage

Bind IPv4 loopback only. Gateway has its own private random bearer token, distinct
from Pi tokens. Launcher URL uses a fragment; the web app removes it immediately
and keeps the gateway credential in origin-scoped browser storage. API requests
need Authorization; no cookie authentication. Reject nonmatching Host/Origin and
all CORS preflights. Apply no-store, restrictive CSP, no referrers, fixed assets,
bounded JSON bodies, message/page limits and selected-agent-only routing. Messages
render as text, never executable HTML. No arbitrary file path or shell command API.

Configuration and SQLite live in an owner-private service directory, with schema
version validation and no symlink DB/config targets. Single service ownership is
exclusive; stale ownership is never replaced by killing a saved PID. SQLite
transactions protect unique operation IDs and intent-before-effect. Restart opens
the same records and reconciles nonterminal receipts without replay. Event history
is bounded when returned, and observation retention may be compacted independently
of durable operations. Gateway records do not overwrite Pi owner/state files.

## Validation and rollout

Focused tests cover config/DTO privacy, source isolation, stale instance, duplicate
and conflicting sends, uncertain/crashed delivery recovery, branch cursor failures,
HTTP authentication/Origin/Host/body bounds, restart and read-only observation.
Actual offline Pi validates browser -> gateway -> Pi -> receipt -> visible reply.
Browser verification covers desktop/mobile, keyboard, draft retention, agent target
selection and reconnect/offline states. Live registration of the existing
Character/Edda managers is read-only; do not probe them with unsolicited work.

Deploy this slice locally and open the real console after independent review and
required checks. Import the current delegation index plus explicitly selected worker
IDs and the Character shared-service record. Existing management can consume the
same operation/event API later; the console does not claim exclusive control over
all other clients simply because a conversation is open.

Sources: [Node TypeScript execution](https://nodejs.org/api/typescript.html),
[TypeScript strict](https://www.typescriptlang.org/tsconfig/strict.html), and the
checked-in `integrations/pi` APIs/tests. No new transport/model/DB service is
created merely by opening this console.
