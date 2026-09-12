# Management handoff MVP

**Goal:** A supervisor reads a small prework manifest and the latest structured
controller report instead of replaying each session's conversation.

**Scope:** Extend the existing local Pi integration only. This is an observational
management view, not task authority, task-rail acceptance, or an autonomous judge.
No other agents' tasks, schedules, model routing, migration or paid calls are in scope.

## Contract

- A controller prepares a JSON manifest from its plan before work. It includes
  run identity, controller/worker role, goal, completion criteria, plan reference,
  declared allowed/excluded/reserved scope, and authority source references.
  References are not resolved or treated as new grants in this MVP.
- Preparation targets one current Pi instance. Manifest changes require an exact
  expected revision and an idle instance. Reload requires explicit rebinding;
  previous-instance reports are historical and never current acceptance.
- Pi exposes `edda_handoff` and `edda_report`. Reports cannot change the manifest
  or claim verified acceptance. A report must name its manifest revision and
  include stage, summary, next step and state. Waiting decisions need a concrete
  question; dependencies need references; completed claims need evidence.
- A runtime work epoch invalidates prior reports when new work starts. A run
  settling without a current terminal/wait report appears as missing_report.
  The integration does not automatically prompt or stop the worker.
- Context contains the complete bounded manifest, latest current report and
  observed runtime state. No transcript query is needed. Insufficient byte budget
  returns needs_context instead of silently dropping scope or evidence.
- Existing watch defaults to compact management views. Full conversation remains
  available explicitly; old extensions report handoff_unavailable until reloaded.

## Implementation and validation

- [x] Add strict manifest/report schema and atomic observational store with
  instance/revision/epoch binding and report deduplication.
- [x] Wire authenticated preparation/read endpoints, Pi tools and an opt-in
  before-agent-start reminder for prepared sessions only.
- [x] Add CLI preparation/context reads and compact watch integration.
- [x] Test real filesystem/HTTP entry points: missing report, stale revision and
  instance, duplicate report, completion claim without acceptance, byte budget,
  no transcript read, old extension behavior and malformed inputs.
- [x] Exercise installed Pi with deterministic offline tool calls; run the
  affected Node suite and hooks; obtain bounded independent local review.

Execution evidence and final independent review status are recorded in task #180
and Edda session notes. The implementation keeps Pi-specific transport separate
from the shared handoff shape; Claude/Hermes recipient adapters and proactive
Pi-to-Codex wake routing are explicitly outside this MVP.

Task #180 carries execution/review receipts. Worktree remains
`C:/ai_agent/edda-worktrees/pi-session-channel`; no Cargo build lane is needed.
