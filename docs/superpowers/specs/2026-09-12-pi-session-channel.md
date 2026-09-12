# Pi session channel

Operator approved the first slice in the 2026-09-12 conversation: register
interactive Pi sessions, report runtime state, and deliver a message from Codex
to the original conversation. This is a local integration, not a new remote
service, automatic supervisor, task authority, or replacement dispatcher.

## Contract

- An opt-in Pi extension opens an authenticated loopback endpoint and registers
  its actual Pi session ID, instance ID, working directory and optional label.
- Registry and receipts live outside the repository in a private per-user
  directory. Unix uses owner-only modes; Windows applies a user-only ACL before
  writing credentials. Same-user processes are trusted. Browser-origin requests
  are refused. No public listen address or credential in status output.
- A session has one channel owner. A durable exclusive lock prevents duplicate
  registration; a crashed owner requires explicit recovery, never silent takeover.
- State separates idle, running, executing tools, waiting for an extension UI
  answer, stopped, and unreachable. Heartbeats prove channel liveness only;
  runtime-event timestamps indicate progress. Task completion is never inferred.
- Send requires exact session and instance binding, a caller-visible UUID,
  sender label, nonempty bounded text and an explicit delivery mode (followUp
  by default; steer optional). Runtime unavailability and UI prompts refuse new
  delivery. Slash commands are not expanded.
- Write an acceptance receipt before handing a message to Pi. Reusing an ID
  with identical content returns its receipt; different content is a conflict.
  An uncertain handoff is not retried. Receipts distinguish accepted, unconfirmed,
  started, settled, failed and unknown. Settled means Pi finished the run, not
  that the requested task passed. A bounded receipt count prevents unbounded use.
- The extension observes the exact message envelope at user-message start;
  settlement uses agent_settled rather than agent_end (which can precede retries).
- Codex uses a Node CLI in integrations/pi. Native Rust command integration is
  deferred because another worker owns the CLI. Existing terminal sessions need
  the extension loaded; a session file alone is not an attached control channel.

## Acceptance

Real HTTP and filesystem tests cover authentication, wrong instance, duplicate
IDs, uncertain delivery, UI waits, owner collisions, stale state and lifecycle
cleanup. Extension event tests cover unconfirmed-to-started-to-settled and failure.
A real installed Pi load is verified without a paid model call. Documentation
provides installation, status, send, receipt and crash-recovery instructions.
