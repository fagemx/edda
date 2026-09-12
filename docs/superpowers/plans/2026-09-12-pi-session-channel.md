# Pi session channel implementation plan

**Goal:** Let Codex inspect and message an existing opted-in Pi session.

**Architecture:** A Pi extension owns a local authenticated HTTP endpoint. A
private registry supplies discovery, a UUID-bound receipt supplies deduplication,
and a standalone CLI exposes the channel without changing the shared Rust CLI.

**Tech stack:** Node built-ins, Pi extension API, node:test; no Cargo build lane.

The operator has approved implementation. Execute inline in the isolated
codex/pi-session-channel worktree; retain existing agents and their sources.

- [x] Add private storage and exclusive owner handling in integrations/pi/store.mjs.
  Test owner collision, permissions and explicit dead-owner recovery.
- [x] Add channel.mjs and client.mjs. Test real loopback requests, authentication,
  bounded payloads, exact targeting, idempotency and fail-closed uncertainty.
- [x] Add extension.mjs using session_start/shutdown, runtime/tool/UI events,
  message_start and agent_settled. Test lifecycle via a fake Pi API and real store.
- [x] Add cli.mjs with list/status/send/receipt/recover, JSON stdout and nonzero
  errors; add README with exact commands and truthful scope/receipt semantics.
- [x] Run node --test integrations/pi/*.test.mjs and git diff --check. Verify the
  installed Pi loads the extension with a no-model command; obtain independent
  bounded review and resolve findings. Record exact local delivery SHA and gates.

Success requires a usable local channel and passing tests, not merely files
generated. Existing busy sessions are not restarted to demonstrate installation.

Round 1 independent review found two receipt-truth defects. The correction uses
`unconfirmed` for Pi's void send wrapper and converts all previous-instance
nonterminal receipts to durable `unknown` before serving. Windows tests include
actual owner-process death/recovery and real Pi missing-credential rejection.
Final review verdict and installation evidence are recorded on Edda task #174.
