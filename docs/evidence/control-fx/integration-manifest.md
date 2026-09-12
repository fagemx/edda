# Control effects integration manifest (task #213)

Base: accepted `main` `d006c7313d71b709f121481476e74236bc1e6e94`.
Frozen source (execution evidence, not acceptance): `d5fc070ec2e532d2dc976eef6adca483a22742bd`.
Merge-base of main and frozen source: `0512feb45e9ceeaa116b697173ef75a531ce59ac`.

This file freezes every path carried from the frozen source, how it is carried,
and every frozen-source change deliberately *not* carried. It is refined by
compilation; every later revision of this file is the authority over the earlier
seed inventory.

Carry modes:

- **wholesale** — `git checkout d5fc070 -- <path>`; the path is untouched on
  main since the merge-base (or is a new file), so there is no semantic conflict.
- **additive** — main is the base and the frozen source's change is re-applied
  by hand, never overwriting a main-only change.
- **union** — both sides' content is required; the result is verified by the
  repository generator or by compilation.
- **main-kept** — main's (hardened) version is kept and the frozen source's
  version is rejected.

## A. Carried wholesale from `d5fc070`

### A.1 Core event and guided-execution (S5)

| Path | Reason |
|---|---|
| `crates/edda-core/src/guided_execution/mod.rs` | new S5 guided-execution module root |
| `crates/edda-core/src/guided_execution/types.rs` | immutable execution-brief value types |
| `crates/edda-core/src/guided_execution/validate.rs` | brief canonicalization/validation |
| `crates/edda-core/src/guided_execution/render.rs` | deterministic brief rendering |
| `crates/edda-core/src/guided_execution/event.rs` | `execution_brief` event shape |
| `crates/edda-core/src/guided_execution/control_types.rs` | S6 control manifest/intent/receipt value types |
| `crates/edda-core/src/guided_execution/control_validate.rs` | control event validation (uses core continuity validator) |
| `crates/edda-core/src/event.rs` | registers `execution_brief`/control event classification |
| `crates/edda-core/src/event_conformance.rs` | conformance for the new event types |
| `crates/edda-core/src/task_done_event.rs` | controlled-completion fields on `task.done` |
| `crates/edda-core/src/task_session_event.rs` | controlled-session binding on `task.session` |

### A.2 Ledger control state (S6a foundation, S6b effects)

| Path | Reason |
|---|---|
| `crates/edda-ledger/src/control.rs` | compiled control manifest and effect request/result |
| `crates/edda-ledger/src/control_authority.rs` | sealed owner-only authority/merge-capability provisioning |
| `crates/edda-ledger/src/control_events.rs` | control event append helpers |
| `crates/edda-ledger/src/control_projection.rs` | control_manifest/intent/receipt projection |
| `crates/edda-ledger/src/control_projection_targets.rs` | projection target checks |
| `crates/edda-ledger/src/control_review_artifact.rs` | bounded review artifact claims |
| `crates/edda-ledger/src/control_review_claim.rs` | bounded review claim outcomes |
| `crates/edda-ledger/src/control_tests.rs` | S6a foundation tests (`#[cfg(test)]` module) |
| `crates/edda-ledger/src/control_effect_tests.rs` | S6b effect tests (`#[cfg(test)]` module) |
| `crates/edda-ledger/src/guided_execution.rs` | accepted execution-brief ledger readback |
| `crates/edda-ledger/src/ledger.rs` | ledger APIs: execution brief, task lease, control dispatch binding, open_existing |
| `crates/edda-ledger/src/lock.rs` | `TaskDispatchLock` one-live dispatch lock |
| `crates/edda-ledger/src/task_actions.rs` | controlled task lease prefix and actions |
| `crates/edda-ledger/src/tasks.rs` | controlled session/attempt/lease fields on `TaskView` |
| `crates/edda-ledger/src/sqlite_store/control.rs` | SQLite control state store |
| `crates/edda-ledger/src/sqlite_store/events.rs` | statically classified control event types |
| `crates/edda-ledger/src/sqlite_store/mod.rs` | control store wiring |
| `crates/edda-ledger/src/sqlite_store/task_leases.rs` | task-lease persistence |
| `crates/edda-ledger/src/sqlite_store/tests.rs` | updated store tests |
| `crates/edda-ledger/Cargo.toml` | adds `edda-store` dep and Unix `rustix` for sealed authority |

### A.3 CLI control product surface (S6a/S6b) and S5 dispatch

| Path | Reason |
|---|---|
| `crates/edda-cli/src/cmd_control.rs` | `edda control` command tree |
| `crates/edda-cli/src/cmd_control_effects.rs` | control effect execution/receipt binding |
| `crates/edda-cli/src/cmd_control_effects_tests.rs` | effect tests |
| `crates/edda-cli/src/cmd_control_verification.rs` | verifier identity/model+session binding |
| `crates/edda-cli/src/cmd_control_review_bundle.rs` | bounded review bundle |
| `crates/edda-cli/src/control_dispatch_safety.rs` | dispatch safety guards |
| `crates/edda-cli/src/control_effect_pipeline_tests.rs` | end-to-end controlled pipeline tests |
| `crates/edda-cli/src/claim_guard.rs` | non-fatal claim guard |
| `crates/edda-cli/src/claim_guard_tests.rs` | claim guard tests |
| `crates/edda-cli/src/detached_dispatch.rs` | detached supervisor stdout/stderr separation |
| `crates/edda-cli/src/cmd_dispatch_acp.rs` | controlled ACP preflight/run with brief + lease binding |
| `crates/edda-cli/src/cmd_reconcile/manifest.rs` | guided reconcile manifest |
| `crates/edda-cli/src/cmd_reconcile/mod.rs` | guided reconcile wiring |
| `crates/edda-cli/src/cmd_reconcile/plan.rs` | guided reconcile plan |
| `crates/edda-cli/src/cmd_reconcile/runner.rs` | guided reconcile runner |
| `crates/edda-cli/src/cmd_reconcile/tests/guided.rs` | guided reconcile tests |
| `crates/edda-cli/src/cmd_reconcile/tests/mod.rs` | tests module |
| `crates/edda-cli/src/cmd_reconcile/tests/plan.rs` | plan tests |
| `crates/edda-cli/src/cmd_reconcile/tests/runner.rs` | runner tests |
| `crates/edda-cli/src/cmd_reconcile/tests/scheduler.rs` | scheduler tests |
| `crates/edda-cli/src/cmd_task.rs` | task-rail command surface |
| `crates/edda-cli/src/cmd_task_guided.rs` | guided task helpers |
| `crates/edda-cli/src/skills/task-prepare.md` | task-prepare canonical skill |
| `crates/edda-cli/resources/dispatch-task.ps1` | controlled Windows dispatch task script |
| `crates/edda-cli/tests/control_foundation.rs` | S6a foundation integration test |
| `crates/edda-cli/tests/dispatch_claim_guard.rs` | dispatch claim guard integration test |
| `crates/edda-cli/tests/prs_check_merge_compat.rs` | PR check merge compatibility test |
| `crates/edda-cli/tests/review_merge_advisory.rs` | review merge advisory test |
| `crates/edda-cli/src/cmd_review/claim.rs` | new bounded review claim module |
| `crates/edda-cli/src/cmd_review/claim_tests.rs` | bounded review claim tests |
| `crates/edda-cli/src/cmd_review/deliver.rs` | bounded verdict delivery (`claim`-aware) |
| `crates/edda-cli/src/cmd_review/github.rs` | authenticated GitHub reads bound to portable identity + remote hint |
| `crates/edda-cli/src/cmd_review/merge.rs` | merge-advisory output for review merge |
| `crates/edda-cli/src/cmd_review/merge_tests.rs` | review merge tests |

### A.4 Conductor and bridge

| Path | Reason |
|---|---|
| `crates/edda-conductor/src/runner/acp.rs` | controlled ACP launch/audit with execution brief |
| `crates/edda-conductor/src/runner/acp/audit.rs` | ACP audit sink |
| `crates/edda-bridge-claude/src/task_nudge.rs` | task nudge integration |

### A.5 Spec, fixtures, registry, pinned spec (contract D-union)

| Path | Reason |
|---|---|
| `spec/events/control_intent.schema.json` | new control intent contract |
| `spec/events/control_manifest.schema.json` | new control manifest contract |
| `spec/events/control_receipt.schema.json` | new control receipt contract |
| `spec/events/execution_brief.schema.json` | new execution brief contract |
| `spec/events/task.done.schema.json` | controlled-completion fields |
| `spec/events/task.session.schema.json` | controlled-session fields |
| `spec/events/registry.json` | union: keeps `continuity_capsule`, adds the four control events (36 total) |
| `tests/fixtures/events/control_intent.jsonl` | fixture |
| `tests/fixtures/events/control_manifest.jsonl` | fixture |
| `tests/fixtures/events/control_receipt.jsonl` | fixture |
| `tests/fixtures/events/execution_brief.jsonl` | fixture |
| `sdk/spec-pin/spec/events/control_intent.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/control_manifest.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/control_receipt.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/execution_brief.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/task.done.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/task.session.schema.json` | pinned-spec cache |
| `sdk/spec-pin/spec/events/registry.json` | pinned-spec cache (36 events) |
| `sdk/spec-pin/tests/fixtures/events/control_intent.jsonl` | pinned fixture |
| `sdk/spec-pin/tests/fixtures/events/control_manifest.jsonl` | pinned fixture |
| `sdk/spec-pin/tests/fixtures/events/control_receipt.jsonl` | pinned fixture |
| `sdk/spec-pin/tests/fixtures/events/execution_brief.jsonl` | pinned fixture |
| `sdk/generator/generate-types.mjs` | supersedes main's change: handles const, `$ref`, conditionals/`dependentRequired` |
| `sdk/generator/generate_types.py` | supersedes main's change: same |
| `sdk/ts/test/enum-types.test.ts` | union: control/brief assertions **plus** main's continuity const assertions |
| `sdk/python/tests/test_types_gen.py` | union: control/brief assertions **plus** main's continuity const assertions |
| `sdk/ts/src/types.gen.ts` | regenerated from the union `spec/events` |
| `sdk/python/src/edda_sdk/types_gen.py` | regenerated from the union `spec/events` |

### A.6 Docs

| Path | Reason |
|---|---|
| `docs/superpowers/continuity/program.md` | CE-only S0-S6 program record |
| `docs/superpowers/plans/2026-09-11-context-save-edda-integration.md` | CE-only continuity integration plan |
| `docs/reference/ledger-event-spec.md` | schema table rows for the new event types |

## B. Hand-merged additively (main is the base)

| Path | Merge rule |
|---|---|
| `crates/edda-core/src/lib.rs` | keep main's `pub mod continuity;`; add `pub mod guided_execution;`, `mod task_done_event;`, `mod task_session_event;` |
| `crates/edda-core/src/types.rs` | keep main's `continuity_capsule` classification; add CE's `execution_brief`, `control_manifest`, `control_intent`, `control_receipt` arms |
| `crates/edda-ledger/src/lib.rs` | keep main's `pub mod continuity` re-exports; add control modules and published control types |
| `crates/edda-cli/Cargo.toml` | keep main's `libc`/`same-file` target deps; add CE's `fs2` |
| `Cargo.toml` (root) | add CE's workspace `rustix` (main has none) |
| `Cargo.lock` | regenerate with `cargo metadata`-driven resolution; keep main's additions and add control deps |
| `crates/edda-cli/src/main.rs` | keep main's module/command set; add `cmd_control`, `cmd_control_effects`, `#[cfg(test)] control_effect_pipeline_tests`, and the `Control` command/registration |
| `crates/edda-cli/src/cmd_init.rs` | keep main's file (both-host skill scaffolding + orchestration projection test); add `("task-prepare", …)` to `SKILLS` |
| `crates/edda-cli/src/cmd_review/mod.rs` | keep main's PR1164 launcher/persistence/warning changes; add CE's `claim` module, `review_comments_argv`/`review_lines`/`review_union` exports, `pub(crate) mod github` |
| `crates/edda-cli/src/cmd_dispatch.rs` | keep main's doc, `--task-id` ACP guard, claim-guard-before-turn, codex persistence options; add CE's `--brief-event-id`/`--brief-digest` and ACP preflight handoff. The carried claim-outcome helper and main's `claim_guard_refusal` are relocated into `claim_guard.rs` so the file holds main's 2106-line ratchet without contortion |
| `crates/edda-cli/src/claim_guard.rs` | CE version plus `dispatch_claim_outcome` and `claim_guard_refusal` relocated from `cmd_dispatch.rs` |
| `COMPATIBILITY.md` | keep main's text; recompute `main.rs` line citations after adding the `Control` variant |
| `docs/reference/ledger-event-spec.md` | see A.6 (frozen source is main plus these rows) |

## C. Continuity preservation (critical) — additive only

| Path | Rule |
|---|---|
| `crates/edda-store/src/continuity.rs` | add only `derive_portable_repository_remote_hint`, `portable_alias_contains`, and the `remote_hint_remains_available_when_a_configured_identity_key_takes_precedence` test from `d5fc070` |
| `crates/edda-cli/src/cmd_continuity/{io,git,mod,render}.rs` | **main-kept** (hardened PR1156 implementation) |
| `crates/edda-core/src/continuity/**` | **main-kept** (hardened validation/bundle) |
| `crates/edda-cli/tests/continuity*.rs` | **main-kept** |
| `crates/edda-cli/src/skills/continuity-*.md` | **main-kept** |
| `crates/edda-ledger/src/continuity.rs` | **main-kept** |
| `crates/edda-store/src/lib.rs` | **main-kept** (only `pub mod continuity;` surface, present in both) |
| `spec/events/continuity_capsule.schema.json`, `tests/fixtures/events/continuity_capsule.jsonl`, `sdk/spec-pin/.../continuity_capsule.*` | **main-kept** (main's accepted contract) |

## D. SDK pin procedure

1. Product + spec commit (pre-SDK integration commit) carries all `crates/`,
   `spec/events`, `tests/fixtures/events` changes. Its SHA is the pin.
2. A later commit regenerates `sdk/ts/src/types.gen.ts` and
   `sdk/python/src/edda_sdk/types_gen.py` from the union `spec/events`, copies
   `spec`/`tests/fixtures` into `sdk/spec-pin/`, and sets
   `sdk/SPEC_PIN.json:spec_sha` and `sdk/generator/SPEC_PIN.env:SPEC_SHA` to
   that pre-SDK commit. Never the SDK commit's own SHA; never self-referential.

## E. Task #201 corrective requirements preserved

- owner/repo authority sealed at compile and compared at every effect;
  `crates/edda-cli/src/cmd_review/github.rs` uses the validated portable
  identity plus `derive_portable_repository_remote_hint`.
- `EDDA_GH_BIN`/transport overrides rejected in production paths; injected mock
  seam only.
- verifier `model_observed`/`session_observed` bound to the exact requested
  profile/session (`cmd_control_verification.rs`).
- truthful cost caps/accounting; no claim of a backend hard cap when absent.
- ACP controlled launch refuses forbidden detach/session flags and records start
  only after spawn/handshake.
- expired action token / pending intent performs zero mutation.
- detached stdout JSON separated from stderr diagnostics.
- delegated merge remains unavailable before intent
  (`CONTROL_UNAVAILABLE`); no permission shortcut added.

## F. Deliberately deferred (frozen-source changes not carried)

| Path | Reason |
|---|---|
| `crates/edda-core/src/continuity/**` | superseded by main's hardened PR1156 continuity; CE version would weaken/regress main |
| `crates/edda-cli/src/cmd_continuity/**` | superseded by main's hardened PR1156 continuity |
| `crates/edda-cli/tests/continuity*.rs`, `skills/continuity-*.md` | superseded by main's hardened PR1156 continuity |
| `crates/edda-ledger/src/continuity.rs`, `crates/edda-store/src/lib.rs` | superseded by main's hardened PR1156 continuity |
| `sdk/SPEC_PIN.json`, `sdk/generator/SPEC_PIN.env` (frozen values) | not carried verbatim; re-pointed at this branch's pre-SDK commit per D |
| `crates/edda-cli/src/skills/coord-orchestrate.md` and other main-only files | main's versions are newer; frozen source is older |
| main-only PR1164/PR1166/PR1156 files (`.claude/skills/**`, `integrations/**`, `REVIEW.md`, `docs/plan/delivery-first/**`, etc.) | not part of this integration; main is authoritative |

## G. Scope exclusions

- No S3 work, no S7-S10 work.
- No CE continuity replacement: main's continuity hardening and before/after
  publication semantics stay intact.
- Delegated merge stays unavailable before intent.
