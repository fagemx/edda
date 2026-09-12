# Pi manager inbox and response loop

**Goal:** Pi stopping/decision events become durable, bounded manager work items,
with explicit authorization evidence reuse and receipt-linked responses.

**Architecture:** Keep the existing same-user private channel store. Pi is the
event producer; `local-manager` is the inbox recipient. A caller-supplied consumer
ID distinguishes manager read acknowledgements; it is a label, not authentication.
Events never grant authority. Only an explicitly invoked manager response sends a
message through the existing exact-instance channel and durable receipt path.

## Contract

- An immutable event carries session/instance/report or work identity, run and
  manifest revision, enrollment-scope digest, timestamp, report and evidence refs.
  Structured waiting_decision reports become decision requests. Reportless
  settlement becomes unread activity/missing report with a UTF-8 bounded public
  assistant excerpt. Do not infer requested authority from prose or expose reasoning.
- Attempt publication after a structured report is recorded. Reconcile pending
  publication and the last durable handoff report at startup and on heartbeat.
  Telemetry failure never rejects a valid report, handoff update, channel startup
  or task execution. Events are create-only and
  deduplicated by stable identity. Retain event/ack/response/authorization files;
  no automatic pruning of unread or unanswered requests.
- `inbox` returns a bounded index. `inbox-read` fetches one complete event with
  its authorization candidates and response evidence. `inbox-ack` acknowledges
  reading only. An acknowledged unanswered decision stays pending.
- Authorization records are manager-authored declarations with exact requested
  action/resource, run/manifest/scope binding, and an explicit source URI/revision.
  Worker reports cannot create records. Matching is exact and returns evidence to
  the manager, never an automatic approval. Revocation removes reuse candidates.
  A missing/mismatched/revoked optional record produces a warning and omits its
  citation; it does not block the manager's otherwise valid explicit response.
- `inbox-respond` explicitly chooses a message and optional authorization record.
  Revalidate enabled enrollment, exact idle instance, current report/work identity
  and manifest/scope before a write-ahead response intent. One event has one
  deterministic message ID; uncertain attempts query receipts and never resend.
  Read acknowledgement, response intent, accepted/started/settled receipt and task
  acceptance remain separate. A response receipt is never task acceptance.
- Current host wake capability is `unsupported`: no desktop adapter is configured.
  `inbox-wake` returns that status and performs no process/model call. An active
  manager can poll/read/respond; there is no claim that an idle Codex is notified.
  The [official App Server documentation](https://learn.chatgpt.com/docs/app-server)
  describes transports and turn/start, but this integration has no configured
  connection to the existing desktop host. Do not spawn a second runtime to fake it.

**Operator clarification:** This is side-channel observation, not a new execution
gate. Structured reports, acknowledgements and authorization records are optional;
missing metadata must never make the worker stop to fill forms or seek approval.
Only wrong/stale destination, explicit pause/scope changes and duplicate delivery
checks constrain the managed response. Ordinary authorized send remains available.
Storage errors are visible and may leave unpersisted observations unavailable after
a crash; already-persisted events and pending publication are never silently erased.

## Implementation and tests

- Add small storage, producer and manager-response modules under integrations/pi.
- Wire report/settlement producer into channel and capture only assistant text from
  message_end. Surface producer errors/capability in status; preserve pending source.
- Add CLI index/read/ack/authorization-record/revoke/respond/wake and README workflow.
- Test exact duplicate events, missing report excerpt, no reasoning, per-consumer
  acknowledgement, restart recovery, unacknowledged requests, stale epoch/instance/
  scope, revoked or mismatched authorization, response dedupe/unknown receipts and
  unsupported wake. Use real private files and authenticated loopback transport.
- Actual Pi offline-provider scenario: request -> outbound event -> manager reads
  and records fixture authorization -> response -> same-session execution; a repeated
  request under the same binding finds the existing declaration. No paid calls.
- Full Node suite and exact-head CI; independent PR-visible review; R6 controller
  merge under standing operator approval. No Rust/Cargo build or live auto-subscription.
