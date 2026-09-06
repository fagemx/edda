# GH-810 evidence — `[profile.dev]` and `.cargo/config.toml` measurements

**Status note (read first).** The substance of issue #810 was already
implemented and merged as PR #811 (commit `03c604f`, merged 2026-09-04):
`[profile.dev] debug = "line-tables-only"` in the root `Cargo.toml`, and
`.cargo/config.toml` carrying `linker = "rust-lld.exe"` for
`x86_64-pc-windows-msvc`, with a full before/after measurement table in the
PR body. That measurement was taken at base `f0750ad` on rustc 1.93.1,
before GH-809 pinned the toolchain. This file re-measures the same pairs on
the **pinned toolchain at the current base**, so the recorded numbers hold
for the toolchain every future gate runs on, and gives the measurements a
durable home in the tree (the PR body is the original carrier; this file
does not replace it).

## Environment (both runs identical)

| item | value |
|---|---|
| worktree | `C:/ai_agent/edda-wt-gh810` (branch `chore/gh810-dev-profile-tuning`) |
| measured tree | base `0b5083f9bc29529667c106799f75ddde90dbe073` (=`origin/main`); baseline run has the #811 build config reverted, working tree otherwise byte-identical |
| rustc / cargo | 1.98.1 (48a229cea 2026-09-01) / 1.98.1 (797e8a9bc 2026-08-05) |
| toolchain pin | `rust-toolchain.toml` → `channel = "1.98.1"`, components rustfmt+clippy, profile minimal |
| OS | Windows 11 Pro 10.0.26200.9168 (MINGW64 NT-10.0-26200, Git Bash) |
| CPU | 32 logical cores |
| build dir | this worktree's own `target/` (worktree default; no `CARGO_TARGET_DIR`) |
| incremental | `CARGO_INCREMENTAL=0` for every measured run |
| clean protocol | `cargo clean` before each measured build |

## Commands

## Run 1 — baseline (pre-#811 build config)

Config: current tree at `0b5083f` with the `[profile.dev]` section removed
from the root `Cargo.toml` and `.cargo/config.toml` deleted — i.e. exactly
the build configuration #811 replaced. (The pre-#811 root `Cargo.toml`
`03c604f~1` cannot be used verbatim: current workspace members inherit
`rust-version` from `workspace.package`, which it predates. Only the profile
and linker deltas differ; everything else is byte-identical to HEAD.)

```
$ export CARGO_INCREMENTAL=0 && cargo clean && time cargo build --workspace --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 28s
```

| metric | value |
|---|---:|
| wall clock | **89 s** (rc=0) |
| `target/debug` total | **4.55 GB** (4,883,183,### bytes; 3,578 files) |
| `.pdb` total | **2.85 GB** (155 files) |
| largest `.pdb` | `edda.pdb` 331 MB (against a final binary far smaller) |
| executables | 132 `.exe` |

Note on wall clock: PR #811 recorded 187 s for the same arm on rustc 1.93.1
at `f0750ad`. This run is 89 s on 1.98.1 — the machine/toolchain are faster
than at the original measurement, so absolute times are not comparable
across the two evidence sets; only the same-file-set pairs within this file
are.

## Run 2 — after (committed #811 config: `line-tables-only` + `rust-lld`)

Config: tree restored byte-identical to `0b5083f` (`git checkout HEAD --
Cargo.toml .cargo/config.toml`), `cargo clean`, same environment.

```
$ export CARGO_INCREMENTAL=0 && cargo clean && time cargo build --workspace --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 13s
```

| metric | value |
|---|---:|
| wall clock | **74 s** (rc=0) |
| `target/debug` total | **2.35 GB** (3,555 files) |
| `.pdb` total | **1.17 GB** (155 files) |
| largest `.pdb` | `edda.pdb` 158 MB |
| executables | 132 `.exe` |

## Summary — base `0b5083f`, rustc/cargo 1.98.1, clean target both sides

| metric | baseline (default `debug=2`, `link.exe`) | after (`line-tables-only`, `rust-lld`) | delta |
|---|---:|---:|---:|
| `target/debug` total | 4.55 GB | 2.35 GB | **−48%** |
| `.pdb` total | 2.85 GB | 1.17 GB | **−59%** |
| `edda.pdb` | 331 MB | 158 MB | −52% |
| wall clock (`cargo build --workspace --all-targets`) | 89 s | 74 s | **−17%** |

Identical artifact topology on both sides (132 `.exe`, 155 `.pdb`), so the
totals are directly comparable. This confirms, on the pinned toolchain, the
direction and rough magnitude of PR #811's original measurement
(`f0750ad`, 1.93.1: 4.14→2.07 GB total, 2.64→1.04 GB `.pdb`, wall 187→193 s
for the profile half alone and 153 s with the linker). The wall-clock win
here (−15 s) is attributable mainly to `rust-lld`, consistent with #811's
linker measurement; single runs per arm, so treat −17% as indicative, not a
bound. The workstation plan's "~40 GB → ~10 GB" prediction concerned
steady-state lanes including 21.6 GB of orphan `incremental` caches; clean
builds were never 40 GB. On clean builds the measured steady input is
4.55→2.35 GB here, and the `line-tables-only` change also cuts the
per-session incremental cache that grows on top of it (GH-810's own framing:
"the difference between a ~40 GB and a ~10 GB steady state" counted both
halves).

## Backtrace quality — re-verified on the pinned toolchain

`line-tables-only` drops type and variable information, so the acceptance
risk is losing usable failure output. A deliberately panicking probe test
(`crates/edda-core/tests/tmp_backtrace_probe.rs`, added temporarily, removed
after capture — same method as PR #811) run under the committed profile with
`RUST_BACKTRACE=1` on rustc 1.98.1 still reports the panic site and a fully
symbolicated stack with file:line on every workspace frame:

```
thread 'probe_panics_to_show_backtrace_quality' (41764) panicked at crates\edda-core\tests\tmp_backtrace_probe.rs:3:5:
gh810 backtrace probe
stack backtrace:
   0: std::panicking::panic_handler
             at /rustc/48a229ceaefd4985c50990b14116b6d856af0985/library\std\src\panicking.rs:679
   1: core::panicking::panic_fmt
             at /rustc/48a229ceaefd4985c50990b14116b6d856af0985/library\core\src\panicking.rs:80
   2: tmp_backtrace_probe::probe_panics_to_show_backtrace_quality
             at .\tests\tmp_backtrace_probe.rs:3
   3: tmp_backtrace_probe::probe_panics_to_show_backtrace_quality::closure$0
             at .\tests\tmp_backtrace_probe.rs:2
   4: core::ops::function::FnOnce::call_once<tmp_backtrace_probe::probe_panics_to_show_backtrace_quality::closure_env$0,tuple$<> >
             at /rustc/48a229ceaefd4985c50990b14116b6d856af0985/library\core\src\ops\function.rs:250
```

## Gate receipt — `0b5083f`, rustc 1.98.1, `CARGO_INCREMENTAL=0`, worktree default `target/`

The committed build config is unchanged from merged PR #811, which ran full
L1 at its own SHA (fmt / clippy pass; `cargo test --workspace` 2680 passed,
0 failed — see the #811 PR body). These runs re-confirm the gates at the
current base with the pinned toolchain; keep the two build logs
(`baseline-build.log`, `after-build.log`) alongside this file as raw output.

| gate | result |
|---|---|
| `cargo fmt --all --check` | **pass** (run 2026-09-05) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** (47 s warm, rc=0) |
| `cargo test --workspace` | **pass — 2801 passed, 0 failed, 62 suites** (94 s warm, rc=0; full stdout: `test-stdout.log`, not retained) |

Raw build logs retained next to this file: `baseline-build.log`,
`after-build.log` (stderr of the two measured `cargo build` runs).

