# Edda-scoped Pi event supervisor

**Goal:** One manager and two managed Pi workers carry explicitly assigned Edda
tasks through normal continuation and dependency dispatch without human reminders.

## Policy and ownership

Configuration pins one canonical Edda project, one or two task IDs with distinct
worker directories, operator-approved instruction/source reference, Pi profiles,
and finite decision/response counts. These are manager setup inputs, not new worker
forms. Tasks remain on the Rust rail; workers retain start/done/fail ownership.
Use assigned worktrees for repository work. No arbitrary task creation or acceptance
transition is exposed to the manager. Existing task `done` remains an execution
fact and never becomes an independent LGTM or merge verdict.

Ready task dispatch is deterministic. Derive run IDs from canonical project and
task creation identity. Existing running tasks with no known managed binding are
reported as owner-elsewhere, not duplicated. Explicit existing run mappings are
validated against worker cwd. The existing managed-run and inbox idempotency
contracts protect launch and response. No automatic restart of stopped workers.

## Event decisions

The service examines task state and inbox files on a bounded interval. No new event
or semantic task change means no model call. Busy worker events remain pending.
The manager is itself a managed Pi with no builtin execution tools, exposing only
read-selected-task/brief and submit-proposal tools. It receives a bounded packet:
operator authority, current task summaries, one current stopping event, existing
authorization evidence where available, and allowed action names.

Actions are continue, wait, escalate and observe. The manager selects a task/action
and gives advisory reasoning; continuation messages are built from trusted operator
instructions and current task identity. Free model text does not become a new grant
or arbitrary executable command. Missing data can be read through the task tool;
invalid/failed decisions are surfaced, never automatically retried or turned into
permission. Existing workers continue if manager observation fails.

## Durability and lifecycle

One service generation owns a controller lock and authenticated status/stop endpoint.
Configuration, task-creation bindings, dispatch intents, packet/decision identities,
manager message IDs and worker response receipts are durable. Restart scans these
records and existing managed runs before any effect. Unknown attempts are inspected,
not replayed. Finite budgets bound manager invocations/responses, not worker work.
Stopping the supervisor stops new effects and preserves workers. All selected tasks
done produces tasks_done/acceptance-unverified; it does not invent more work.

## Implementation and verification

- Add config/storage helpers, deterministic task packet/policy functions, manager
  extension tools, service engine and CLI start/status/stop wiring.
- Reuse managed launch/recovery/status and inbox response primitives. Add a manager
  resource-discovery option only as needed to avoid unrelated skill context.
- Tests use real private files, task-reader subprocesses and loopback channels for
  event freshness, stable identities, no duplicate dispatch, busy deferral, paused
  service, unknown receipts, unchanged-state quietness and unsupported proposals.
- Native Edda/Pi offline-provider smoke creates two temporary tasks and worker dirs,
  proves automatic question/answer and dependent dispatch, and checks terminal state.
  A bounded DeepSeek trial may separately validate manager decisions; no broad model
  benchmark or repeated paid trials. No user tasks or unrelated sessions changed.
- Freeze Node checks, independent PR-visible review, exact-head CI and R6 merge under
  standing operator authority. No local Cargo build lane.
