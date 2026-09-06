# GH800 offline deterministic fake-ACP evidence — Task #69 (infra-acp-glm)

Date: 2026-09-05 (UTC). Machine: this Windows workstation. Lane: `worker-2`
(`CARGO_TARGET_DIR=$LOCALAPPDATA/fleet-workstation/lanes/worker-2`, focused
L0 only — no workspace gate).

## What this evidence is

The five `runner::acp` evidence tests drive `AcpRunner::drive` — the exact
per-turn protocol flow used in production — against an in-process
deterministic fake ACP agent (`FakeAcpAgent`) speaking typed ACP V1 over
in-memory duplex streams. The server→client permission path is exercised
through the real `AgentSideConnection::request_permission` wire path, not a
mock. No provider binary, network, or account is involved; no real-provider
success is claimed anywhere.

The F7 test additionally spawns a **real child** (`cmd /C set` on Windows,
`/usr/bin/env` on Unix) through `spawn_agent` and inspects its environment
dump with exact-name matching.

## Evidence map

| Required offline evidence | Test (crates/edda-conductor/src/runner/acp.rs) | Result |
|---|---|---|
| Two-step (initialize → new → prompt) with agent-reported usage | `fake_agent_two_step_uses_measured_usage_and_resumes_one_session` | PASS — turn 1 usage `Some(30/20/10)` via `_meta.usage`; audit `new:fake-session`, `usage:measured=true` |
| Usage present vs absent honesty | same test, turn 2 | PASS — resumed turn reports no usage ⇒ `usage:measured=false`, `measured: false` in result |
| Same-session follow-up (`session/load` resume) | same test | PASS — second connection loads `fake-session`; `state.loads == ["fake-session"]`; audit `session_load:allow=true` |
| Denial (server→client request answered, not ignored) | `fake_agent_permission_requests_follow_the_policy_on_the_wire` | PASS — in-scope ⇒ `allow_once` selected (`permission_outcomes [true, false]`); peer-owned path ⇒ deny; audit carries `permission:allow=true` and `permission:allow=false` |
| Kill mid-turn → restart → same-session load | `fake_agent_cancelled_turn_restarts_into_the_same_session` | PASS — prompt entered then cancelled ⇒ error `ACP session/prompt cancelled` with `cancel:allow=false` audited; restart loads `fake-session`, no new session |
| Failure cleanup at transport level | `fake_agent_setup_failure_persists_no_session_and_no_usage` | PASS — failing `session/new` ⇒ error propagates, audit empty (no session event, no usage) |
| F7 env strip measured on a spawned child | `spawned_child_env_strips_nested_session_markers` | PASS — `CLAUDECODE`, `CLAUDE_CODE`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SSE_PORT` absent from the child environment (exact-name match; unrelated shared-prefix vars like `CLAUDE_CODE_MAX_OUTPUT_TOKENS` are not F7 markers) |

CLI-side wiring evidence (crates/edda-cli): `acp_dispatch_refuses_substitutes_and_unenforceable_options`,
`target_requires_task_agent_kind_match`, `prompt_contains_task_facts_and_receipts`,
`scope_roots_reject_globs_absolute_traversal_and_missing_paths`,
`live_peer_roots_ignore_globs_and_missing_paths_but_include_running_peers`,
`acp_kinds_advertise_no_legacy_capabilities_and_refuse_launcher_construction`,
`dispatch_parses_without_prompt_file_for_acp_and_refuses_it_at_runtime_for_legacy`,
`task_id_is_refused_for_legacy_agents` — all PASS
(`glm-cli-acp-tests.log`, plus the full `cmd_dispatch::tests` module 47/47).

## Logs

- `glm-fake-acp-tests.log` — `cargo test -p edda-conductor acp`: 17 passed.
- `glm-cli-acp-tests.log` — `cargo test -p edda acp` / `agent_kind` /
  `cmd_dispatch::tests`: 7 + 6 + 47 passed.
- `glm-l0-clippy.log` — clippy `-D warnings` for `edda-conductor` and `edda`.
- fmt (`cargo fmt --all --check`), `lint-file-length.sh --tree`,
  `git diff --check` run clean in the same gate pass (terminal receipts in
  the Task #69 session record).

## What this evidence does NOT show

Real-provider two-step drills, a real nested Claude Code F7 session, and a
real agent kill/restart drill remain open — see `ACP_HANDOFF.md` "Remaining
gaps". No `Closes #800`.
