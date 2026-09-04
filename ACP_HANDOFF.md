# GH800 ACP module handoff

Basis: `467e8be02eb98fb47d0eca82e9dd400d91d67e6a`. Candidate commit: `d94f8ad`
+ this working tree (target table, drill driver, usage meta path). No CLI
wiring — see Remaining gates.

## Implemented (runner crate, no CLI dependency)

- `runner::acp::AcpRunner` performs the typed ACP V1 initialize, new/load,
  prompt, streamed-update, permission, cancellation, and bounded drain flow.
- `runner::acp::AcpPermissionPolicy` permits only `allow_once` inside
  canonical task roots; it rejects traversal, symlink escapes, peer-owned
  paths, unlocated calls, persistent grants, and all verifier requests. ACP
  client filesystem and terminal capabilities remain structurally disabled.
- `LedgerAcpAudit` appends `task.session` immediately after `session/new` and
  durable, scrubbed lifecycle/permission audit notes.
- `runner::acp::effective_usage` — measured usage: typed `usage` field first,
  then `_meta.usage` (Grok Build's camelCase carrier); malformed meta never
  invents a measurement. Absent on the wire ⇒ `None` ⇒ `measured: false`.
- `agent::acp_targets::AcpTarget` — per-target spawn table (grok/kilo/pi/
  claude) with verifier read-only flags where the target documents them
  (grok `--sandbox read-only` only; no invented flags elsewhere).

## Windows probe/drill evidence (2026-09-04, this workstation)

Driver: `scripts/drill-acp-800.ps1` (standalone, no edda CLI). It speaks
newline-delimited JSON-RPC over stdio, answers `session/request_permission`
read-only (reject_once, else cancelled), retains the agent-supplied session id
in memory only for step 2, and writes an allowlisted metadata trace. The trace
contains only time, direction, known ACP method/request ids, response status,
and allowlisted numeric protocol/usage facts; it never serializes message,
error, header, userinfo, session-id, or unknown-field values. Results in
`docs/evidence/gh800/` are compact method/status evidence only.

| Target | Probe/drill result | Exact evidence |
|---|---|---|
| grok 1.0.13 | `initialize` OK (protocolVersion 1, loadSession=true); `session/new` **no response within 400 s** — observed spawn times vary: one run's own log recorded `session.new_session elapsed 182 s`, later runs silent >400 s. **Not classified**: neither universal-unsupported nor passing; needs longer budgets and one-at-a-time retries | `drill-result-grok.json`, `transcript-grok-step1.methods.txt` |
| kilo | **spawn failure**: `kilo` not on PATH; `cmd /C kilo acp` exits 1 immediately, 0 lines exchanged | `drill-result-kilo.json` |
| pi | **spawn failure**: `pi-acp` not on PATH (`where pi-acp` fails; unrelated `pi` RPC launcher is not the ACP entry) | `drill-result-pi.json` |
| claude | npx `@agentclientprotocol/claude-agent-acp` produced no output within 60 s in two prior probes (Terra, recorded below); not re-probed | prior handoff note |

Security note (observed leak class): grok echoes operator-global MCP
configuration in `_x.ai/mcp/servers_updated`. A prior raw local transcript was
scrubbed. The driver now persists no arbitrary JSON or message text at all;
synthetic canaries cover nested unknown fields, headers/userinfo, arrays, and
malformed lines. The **runner** persists only audit kind strings, never
notification payloads — integration must keep that property. Runtime-profile
isolation is an integration concern, but no concrete authorized implementation
brief exists, so this candidate does not broaden into it.

Runner integration consequence: `session/new` on grok can exceed 3 minutes;
per-method budgets must exceed the driver's 180 s first-attempt cap, and
session setup must not share the prompt-timeout constant (`REQUEST_TIMEOUT`
30 s in the candidate covers prompt only; initialize/new/load are currently
unbounded — add bounded large budgets before CLI wiring).

## Gates (Terra, at `d94f8ad`)

- RAN `cargo test -p edda-conductor acp::tests --lib`: 4 passed (fake typed
  ACP agent covers initialize/new/prompt/load).
- RAN `cargo clippy -p edda-conductor --all-targets -- -D warnings`: passed.
- RAN `cargo test -p edda-conductor`: 379 tests exercised; host stream
  truncated before the final line, no failing test shown.
- RAN `cargo fmt --all --check`, `scripts/lint-file-length.sh --tree`,
  `git diff --check`: passed.

## Remaining gates for this working tree (lane release needed)

- Terra L0 is recorded in `docs/evidence/gh800/terra-l0-logs/` using the
  released verifier lane with `CARGO_INCREMENTAL=0`. Initial focused ACP test
  and Clippy logs record the real `effective_usage` borrow failure; after the
  focused fix, all final logs exit 0: `cargo test -p edda-conductor acp`,
  `cargo test -p edda-conductor acp_targets`, conductor Clippy
  warnings-denied, fmt, and file-length. No full conductor/workspace suite was
  rerun.
- `scripts/test-drill-acp-800.ps1` safe-trace canaries and `git diff --check`
  also exit 0; no live target probe was rerun.
- Real 2-step drills (needs safe temp worktree + known provider auth):
  grok with ≥400 s session/new budget; kilo/pi/claude only after a real
  entry point exists (package-local attempt is an installation trial — out of
  bounds this turn). Unsupported claims must carry the exact failing method.
- F7 nested-session env strip drill; kill-mid-turn → restart → `session/load`
  resume drill.

## Controller integration (not authorized yet)

CLI cmd_dispatch/agent_kind wiring waits on the #782/#605 owner's frozen-SHA
grant. Integration must construct `AcpTaskRequest` from the task view (task
id, worktree, rendered #793 brief/facts prompt, live peer roots, `edda-mcp`
stdio endpoint, `resume_session_id` from `task.session`), select targets via
`agent::acp_targets::AcpTarget`, and map `AcpTurnResult::usage` into receipt
cost fields with explicit `measured: true|false`. Old dispatch backends are
untouched. No `Closes #800` until drill acceptance.

## Coordination

- Scope: `runner/acp.rs`, `agent/acp_targets.rs`, `agent/mod.rs`,
  `scripts/drill-acp-800.ps1`, `docs/evidence/gh800/*`, `ACP_HANDOFF.md`.
  Off-limits peers respected: cmd_dispatch.rs, claim_guard.rs, lane scripts
  (CLI/lifecycle), sdk/* (SDK), adapter-conformance surfaces.
- Orphan cleanup receipt 10:37Z: stopped PID 33756 (own stdin-script probe
  python, hung on broken pipe) and PID 42360 (its `grok agent stdio` child).
  No generic node/grok processes touched.
