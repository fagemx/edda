# GH800 ACP module handoff

Basis: `2a098d8` (reworded history of the `d94f8ad`/`688b2ef` candidates). This
turn (infra-acp-glm, Task #69) completed the CLI integration on top of the
bounded runner and added offline deterministic fake-ACP evidence. Real
provider drills remain open and are listed honestly at the end.

## Implemented (runner crate, no CLI dependency)

- `runner::acp::AcpRunner` performs the typed ACP V1 initialize, new/load,
  prompt, streamed-update, permission, cancellation, and bounded drain flow.
  The per-turn protocol flow lives in `AcpRunner::drive`, split from the
  child-spawning `run` so offline tests exercise the identical flow.
- `runner::acp::AcpPermissionPolicy` permits only `allow_once` inside
  canonical task roots; it rejects traversal, symlink escapes, peer-owned
  paths, unlocated calls, persistent grants, and all verifier requests. ACP
  client filesystem and terminal capabilities remain structurally disabled.
- `LedgerAcpAudit` appends `task.session` immediately after `session/new` and
  durable, scrubbed lifecycle/permission audit notes plus a numeric usage
  receipt (`measured: true|false`, never a zero-cost guess).
- `runner::acp::effective_usage` — measured usage: typed `usage` field first,
  then `_meta.usage` (Grok Build's camelCase carrier); malformed meta never
  invents a measurement.
- `agent::acp_targets::AcpTarget` — per-target spawn table (grok/kilo/pi/
  claude) with verifier read-only flags where the target documents them
  (grok `--sandbox read-only` only; no invented flags elsewhere).
- F7 nested-session guard (TASK_RAIL_V1 §4.3): `spawn_agent` strips
  `CLAUDECODE`, `CLAUDE_CODE`, `CLAUDE_CODE_ENTRYPOINT`, and
  `CLAUDE_CODE_SSE_PORT` from the child environment.

## CLI integration (Task #69, this working tree)

`edda dispatch --agent acp:<grok|kilo|pi|claude> --task-id N` routes to
`cmd_dispatch_acp` after the normal dispatch F1 cwd/prompt preflight and
GitHub claim guard. ACP rejects command-line prompt/session and unsupported
legacy capability options; it derives the bounded #793-style brief/task-facts
prompt, concrete permission roots, `edda mcp serve` stdio endpoint, target,
and prior `task.session` id from the task view. It reuses GH-605's writer
claim and releases that claim before emitting the result; `--detach` is
explicitly refused rather than bypassing its detached-supervisor protocol.

- Task-rail routing: `task.created` `agent_kind` (`acp:<target>`) must match
  the selected target or dispatch refuses; `edda task new --agent-kind
  acp:<target>` creates such tasks.
- First prompt: task title, bounded brief content, scope paths, upstream
  dependency receipts, worktree path, and the `edda task done` completion
  instruction — the ledger is the communication (TASK_RAIL_V1 §5).
- Continuity: turn 1 persists `task.session` before any prompt side effect;
  every later controller turn loads the same session via `session/load`
  (`resume_session_id` from the projected task view) — never a silent new
  session.
- Read-only policy: in-band `AcpPermissionPolicy` for every turn, plus
  documented spawn-level read-only flags for verifier lanes where a target
  ships them.
- Usage: the JSON result carries `measured` and the numeric usage, and the
  ledger receives the same metadata-only receipt.
- Failure cleanup: the runner always drains (and kills) the child before an
  error reaches the caller; the writer claim is released on success and
  failure alike; a failed `session/new` persists no session and no usage.

## Offline deterministic fake-ACP evidence (Task #69, this workstation)

All evidence is offline: an in-process fake agent speaks the same typed ACP
protocol over duplex streams, driven through `AcpRunner::drive` — no
provider binary, no network, no account. Logs:
`docs/evidence/gh800/glm-fake-acp-tests.log` and
`docs/evidence/gh800/glm-cli-acp-tests.log`.

| Evidence | Test |
|---|---|
| Two-step: initialize → session/new → prompt, agent-reported `_meta.usage` | `fake_agent_two_step_uses_measured_usage_and_resumes_one_session` |
| Same-session follow-up: `session/load` resumes the persisted id, usage absent ⇒ `measured: false` | same test |
| Wire-level denial + allowance: server→client permission requests answered through the real request path (in-scope ⇒ `allow_once`, peer-owned ⇒ deny) | `fake_agent_permission_requests_follow_the_policy_on_the_wire` |
| Kill mid-turn: prompt entered, cancelled, error `session/prompt cancelled` audited; restart loads the same session | `fake_agent_cancelled_turn_restarts_into_the_same_session` |
| Failure cleanup: failed `session/new` persists no session event and no usage | `fake_agent_setup_failure_persists_no_session_and_no_usage` |
| F7 strip measured on a real spawned child (`cmd /C set` / `/usr/bin/env` dump) | `spawned_child_env_strips_nested_session_markers` |

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
notification payloads. Runtime-profile isolation is an integration concern,
but no concrete authorized implementation brief exists, so this candidate does
not broaden into it.

## Gates (Task #69, worker-2 lane, this working tree)

- RAN `cargo fmt --all --check`: clean.
- RAN `cargo clippy -p edda-conductor --all-targets -- -D warnings`: clean.
- RAN `cargo clippy -p edda --all-targets -- -D warnings`: clean.
- RAN `cargo test -p edda-conductor acp`: 17 passed (5 new offline evidence
  tests + policy/usage/timeout/connection tests).
- RAN `cargo test -p edda acp` / `agent_kind` / `cmd_dispatch::tests`:
  7 + 6 + 47 passed.
- RAN `scripts/lint-file-length.sh --tree`: clean (ceilings ratcheted for
  `runner/acp.rs` 1503, `cmd_dispatch.rs` 2106, `main.rs` 1463).
- RAN `git diff --check`: clean.
- Prior Terra gates at `d94f8ad` (focused `acp::tests`, conductor clippy,
  `scripts/test-drill-acp-800.ps1` canaries) are recorded in
  `docs/evidence/gh800/terra-l0-logs/`.

## Remaining gaps (honest, blocking `Closes #800`)

- No real provider two-step drill has passed. grok `session/new` exceeded the
  400 s budget; kilo and pi have no entry point on PATH; claude's npx adapter
  produced no output in prior probes. None may be claimed as supported.
- F7 stripping is measured on a spawned child, not yet inside a real nested
  Claude Code session on a live provider turn.
- The kill→restart→`session/load` drill ran against the deterministic fake;
  the same sequence has not been observed on a real agent.
- `edda dispatch --agent acp:<target>` against a live provider, the detached
  supervisor protocol for ACP, and reconciler auto-respawn remain unwired.

## Coordination

- Task #69 grant: `runner/acp.rs`, `agent/acp_targets.rs`, `agent/mod.rs`,
  `agent_kind.rs`, `cmd_dispatch.rs`, `cmd_dispatch_acp.rs`, `main.rs`,
  `ACP_HANDOFF.md`, `scripts/drill-acp-800.ps1`, `docs/evidence/gh800/*` plus
  the file-length ratchet data file. GH-605 shared lifecycle files
  (`dispatch_claim.rs`, lane scripts) were consumed, not modified.
- Orphan cleanup receipt 10:37Z (prior session): stopped PID 33756 and PID
  42360 (own stdin-script probe + its grok child). No generic node/grok
  processes touched.
