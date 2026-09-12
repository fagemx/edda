# Managed Pi launch and recovery

**Goal:** Create a usable managed Pi from one command, own its lifecycle, and keep
session/event evidence available when the calling client exits or reconnects.

**Architecture:** Snapshot flat integration runtime modules, package metadata and
the Windows ACL helper to a digest-addressed private release. A hidden detached
Node runner launches one Pi RPC child with `--no-extensions` plus the pinned Edda
extension. It owns stdin/stdout and an authenticated loopback status/idle-stop
endpoint. The existing Pi channel owns work messages and inbox publication.

## Interfaces and boundaries

- `runtime-install`: canonical release path, version and file digest manifest.
- `launch --project PATH [--pi-entry FILE] [--provider NAME --model NAME]
  [--prompt-file FILE] [--run-id UUID]`: create-only run configuration and printed
  run ID before start; launch existing matching ID returns existing evidence.
- `run-status RUN_ID`: challenge runner instance; show Pi/session/version, current
  channel state, initial receipt and inbox event pointer. Never equate PID existence
  with authenticated ownership. No transcript/tool-argument dump.
- `run-stop RUN_ID [--abort]`: verify runner token/instance and live idle Pi, then end only its
  owned child (bounded stdin close with owned-child termination fallback). Preserve
  session files and inbox. Busy work requires explicit --abort and is reported as
  interrupted, never completed. No arbitrary saved-PID kill.
- `run-resume RUN_ID`: explicit recovery only after both runner and Pi owners are
  absent/dead; conservative refusal on ambiguous liveness. Reuse the exact checked
  session file and identity. No original-prompt replay and no automatic task resume.
- LF-only JSONL parser with bounded frames; tolerate Pi diagnostics separately and
  consume get_state responses for session identity/file. Do not retain reasoning,
  tool arguments or raw stdout; existing extension publishes bounded public inbox.
- Durable startup/initial-message intent handles duplicate invocation and uncertain
  delivery. Crashed launch locks are visible and never blindly stolen.
- Ordinary user credentials/settings remain usable, while extension selection is
  explicit for this new process. Additional trusted extensions may be passed for
  fixtures/custom providers. No mutation of already-running user sessions/settings.
- This runner is the lifecycle/event entry point, not an autonomous decision maker
  or a Codex desktop wake adapter. No model polling or new reporting prerequisites.

## Work and validation

- Implement release snapshot/config/record helpers and bounded LF RPC parser.
- Implement owned background runner, authenticated status/stop and launch/resume CLI.
- Test immutable-release verification, framing, duplicate identity, wrong endpoint
  identity and failure outcomes with temporary process-boundary fixtures.
- Actual Pi offline provider: client disconnect/reconnect, question/response, idle
  stop/resume same session, retained inbox, no duplicate initial message.
- Bounded configured-model dogfood writes only an explicitly assigned temporary
  fixture and stops. Report tool/model/token evidence separately; no trial retries.
- Focused Node tests then full suite once; frozen independent PR review, exact-head
  CI and R6 merge under existing operator authorization. No local Cargo build.
