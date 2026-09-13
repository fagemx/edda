# Work ownership and review handoff

User-approved first slice: delivery → reviewer handoff → owner closure report.
The user can also intervene directly with the executing agent while the owner
retains a bounded record of the changed instruction.

## Boundaries

An existing Edda task is the durable work identity. Explicit local configuration
selects its project/workspace/task number and responsible manager. Session IDs are
replaceable execution targets, not work IDs. Read the task's title, status and
receipt using its structured CLI. Persist versioned coordination events as Edda
notes associated with that task. SQLite remains the message-effect journal, never
the source of task acceptance. No changes to the Rust task schema are required.

The service supplies fixed-argument, bounded task/log/note calls to configured
workspaces. Browser input cannot select paths, executables or shell commands.
Malformed/unavailable work sources must produce per-work diagnostics without
hiding healthy work or disabling ordinary agent conversations.

## Handoff contract

- Every selected work has a closure owner, current assignee, next step, stage,
  evidence, revision and observation time. An unassigned next step is visible.
- Assignment persists a stable action/message ID and target before sending.
  Explicit user recovery reuses that identity. Restart never blindly replays.
- Transport acceptance is not agent execution; started receipts prove execution.
  Settled receipts mean a reply ended, not delivered work or product acceptance.
- Explicit delivery requires evidence. Explicit owner acceptance records its
  evidence only after existing project gates; it does not execute a merge or
  manufacture a canonical task-done receipt.
- Revision checks and serialized writes prevent two stale operators from
  silently overwriting the handoff. Conflicting or incomplete history is surfaced.
- Reassignment must preserve prior target/evidence and cannot silently duplicate
  an unresolved active operation.

## Direct intervention

The work interface sends a changed instruction to a named selected agent and
records it for the closure owner. The instruction remains awaiting acknowledgement
until an explicit acknowledgement is recorded with evidence. Seeing a reply or
receiving a transport receipt is not semantic agreement. A pending instruction
prevents a misleading accepted-work display; ordinary chat remains available.
No private reasoning or full worker context is copied into the owner view.

## Interface

A work section accompanies the existing agent conversation. It shows canonical
task status separately from handoff stage, owner and next recipient, waiting
reason, last evidence, and pending direction changes. Operators can assign,
intervene, acknowledge, report delivery, record acceptance and report a blocker.
Actions retain their original UUID across a network interruption/browser reload.
An explicit retry reconciles the same request. Stale edits are reported, not
automatically rewritten against a newer revision.

## Acceptance

Exercise a real isolated Edda task and Pi-compatible message channel through
assignment, started receipt, delivery and closure. Prove stale edits conflict,
concurrent retries create one message, restart preserves original targets,
unknown sends are not repeated, directives are not implicitly acknowledged,
and one unavailable task leaves another readable. Verify the browser retains
uncertain requests and that owner/next-step/evidence are visible on desktop and
mobile. No live production task receives synthetic test messages.

This slice implements explicit operations and deterministic tracking. General
autonomous planning, public remote access, notifications and non-Pi adapters are
outside scope; a human or authorized manager still decides the next assignment.
