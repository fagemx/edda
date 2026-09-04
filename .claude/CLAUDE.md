# Edda Project Guidelines

Rust development principles and conventions for the edda project.

## Project Overview

- **Language**: Rust (edition 2021)
- **Structure**: Cargo workspace with 23 crates
- **Runtime**: Zero external runtime dependencies (CLI tool)
- **Storage**: SQLite (rusqlite), JSONL append-only ledger

## Workspace Crate Map

| Crate | Description |
|-------|-------------|
| `edda-core` | Core event model, hash chain, schema |
| `edda-ledger` | Append-only SQLite ledger with hash-chained events |
| `edda-derive` | View rebuilding and tiered history |
| `edda-store` | Per-user store with atomic writes |
| `edda` | CLI and TUI (binary crate) |
| `edda-serve` | HTTP API server |
| `edda-mcp` | MCP server (JSON-RPC 2.0) |
| `edda-ask` | Cross-source decision query engine |
| `edda-search-fts` | Full-text search (Tantivy) |
| `edda-index` | Transcript index |
| `edda-transcript` | Transcript delta ingest and classification |
| `edda-ingestion` | Ingestion trigger evaluation |
| `edda-pack` | Context generation and budget controls |
| `edda-conductor` | Multi-phase plan orchestration |
| `edda-aggregate` | Cross-repo aggregation queries |
| `edda-chronicle` | Chronicle synthesis (recap/cognitive zoom) |
| `edda-postmortem` | L3 post-mortem analysis with TTL decay |
| `edda-notify` | Push notification dispatch |
| `edda-bridge-claude` | Claude Code hooks and transcript ingest |
| `edda-bridge-codex` | Codex CLI hooks and context injection |
| `edda-bridge-cursor` | Cursor Agent hooks and context injection |
| `edda-bridge-hermes` | Hermes agent shell hooks and context injection |
| `edda-bridge-openclaw` | OpenClaw hooks and plugin |

## Development Principles

### 3.1 Clippy Zero Warnings

```rust
// ❌ Bad
#[allow(clippy::all)]

// ✅ Good — fix the warning or use targeted allow
#[allow(clippy::result_large_err)]
```

- CI runs: `cargo clippy --workspace --all-targets`
- `RUSTFLAGS: -Dwarnings` in CI — warnings are errors

Workspace method bans:

- Method bans are configured in `clippy.toml` under `disallowed-methods`, each
  entry carrying the concrete failure reason it prevents (e.g. `dirs::home_dir`
  vs `std::env::home_dir` disagreeing on Windows).
- Bans are DefId-matched: aliases, re-exports, and fully-qualified paths are
  all caught.
- Limitation: a `disallowed-methods` entry for a crate absent from a given
  crate's dependency graph is silently inert for that crate — clippy cannot
  resolve the DefId, so the ban does not fire there. A ban on `dirs` therefore
  only protects crates that (transitively) depend on `dirs`.

### 3.2 No unsafe

```rust
// ❌ Bad
unsafe { std::mem::transmute(x) }

// ✅ Good — use safe abstractions
// If unsafe is absolutely necessary, document with SAFETY comment
```

### 3.3 Error Handling — thiserror + anyhow

```rust
// ❌ Bad — unwrap in library code
let data = file.read().unwrap();

// ✅ Good — propagate with ?
let data = file.read()?;

// ✅ OK — unwrap/expect in tests only
#[test]
fn test_read() {
    let data = file.read().expect("test file");
}
```

- Library crates: use `thiserror` for custom error types (see `edda-serve/src/lib.rs:79`)
- Application crates: use `anyhow` for error propagation

### 3.4 Type Safety

```rust
// ❌ Bad — stringly typed
fn process(action: &str) { ... }

// ✅ Good — enum
enum Action { Note, Decide, Query }
fn process(action: Action) { ... }
```

- See `edda-core/src/types.rs` for examples (TaskBriefStatus, TaskBriefIntent, DecisionScope)

### 3.5 Serde Patterns

```rust
// ✅ Good — skip_serializing_if for optional fields
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reason: Option<String>,

#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub blobs: Vec<String>,
```

### 3.6 Module Organization

```rust
// In lib.rs — re-export public API
pub mod types;
pub use types::*;
```

## Testing Standards

```bash
# Run tests for a single crate (default while iterating)
cargo test -p edda-core

# Run a specific test
cargo test -p edda-core test_name

# Run all tests — CI's job on Linux + macOS at every push; locally only with a
# stated reason (see Verification ladder)
cargo test --workspace
```

- **Unit tests**: In `#[cfg(test)] mod tests` within source files
- **No integration tests directory** currently — tests are inline
- **No mocking internal crates** — use real SQLite via `tempfile`
- See `edda-core/src/types.rs:261-667` for test patterns

### Verification ladder

Verify once per frozen SHA; cite that result everywhere else. Full workspace
gates are expensive (23 crates; a long-lived target directory for this
workspace measured 40.9 GB — see Build lanes), and CI runs the full workspace
set independently on every push — the ladder's local Cargo work is focused
checks only.

**Know exactly what CI does and does not cover** (`.github/workflows/ci.yml`):

| CI job | Coverage |
|---|---|
| Format | `cargo fmt --check`; `sh -n install.sh`; installer `resolve_version` latest-version stdout test |
| Clippy | `cargo clippy --workspace --all-targets` on Linux, macOS **and** Windows |
| Test (Linux, macOS) | `cargo test --workspace` — all 23 crates |
| Test (Windows) | **only 7 crates** — `edda-store`, `edda-ledger`, `edda-search-fts`, `edda-transcript`, `edda-bridge-claude`, `edda-conductor`, `edda` (Windows build/link is ~5x slower; the subset is derived from process-spawn, file-lock, and mmap criteria — GH-433) |

So on Windows every crate is **type-checked and linted** (Clippy is
workspace-wide on all three OSes), and every crate's *library* is also linked,
because `edda` depends on the whole workspace — `crates/edda-cli/Cargo.toml`
names 21 of the other 22 directly and reaches `edda-ingestion` through
`edda-serve`, so building `-p edda` pulls all of them into its binaries.

What Windows does **not** exercise is the other 16 crates' own test targets:
their runtime behavior goes unrun, and their test-only code and dev-dependencies
are never linked. So a Windows-only defect in a path, a file lock, a spawned
process, or a temp directory in those crates is caught by no CI job. That is a
stated reason for the verifier to run it locally, and it is why the verifier's
focused Windows-gap run is load-bearing rather than redundant.

| Level | When | Run |
|---|---|---|
| L0 iterate | while editing | `cargo fmt --all --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo test -p <crate>` for each touched crate; `scripts/lint-file-length.sh --tree` |
| L1 freeze | once per frozen full SHA, clean tree | **exact-head CI** — Format; Clippy ×3 OS; Test on Linux + macOS full workspace; Test on the Windows 7-crate subset — **plus one focused local run by the verifier**, once per frozen SHA, of `cargo test -p <crate>` on Windows for each touched crate outside the subset (the C5 selector — every touched crate minus the 7-subset; the loop is quoted in the Pre-commit Checklist L1 block and the two are kept identical). The implementer / fix lane runs L0 on touched crates, pushes, posts the `Review Response`, and does not run the workspace gate. Record the CI run id together with the full SHA (gate receipt: `CI run <id> @ <sha>`) |
| L2 review | verifier, once per frozen full SHA | READ the L1 receipt (exact-head CI, `CI run <id> @ <sha>`) and its job results; RAN only focused or adversarial checks they do not cover — **including Windows behavior in any crate outside the CI Windows subset above**, which L1 itself assigns to the verifier as the C5 selector run. A full local rerun needs a stated reason: red or absent exact-head CI, or grounds to distrust it. A coverage gap earns a **focused** check for that gap, not a full rerun — running the workspace to reach one uncovered crate is the cost this ladder exists to remove. Deterministically red CI already blocks the SHA — audit and request changes instead of spending a full run; if the red is environmental, re-run only the failed job |
| L3 pre-merge | merge authority | READ exact-head CI and the final current-head LGTM; RAN only a merge check against the current base. A draft/ready, label, or status flip is not a push — nothing reruns |

Docs-only changes (no code/product blob, `Cargo.lock`, or toolchain change)
run no Cargo gate locally; exact-head CI is the gate.

For crates whose tests spawn their own binary (such as `crates/edda-cli`),
process-spawning tests live in `tests/*.rs` and use `env!("CARGO_BIN_EXE_<bin>")`
so Cargo guarantees the binary is compiled fresh (`cargo test -p <crate>`).
The narrower `cargo test -p <crate> --bin <bin>` runs only pure unit tests
and does not depend on the binary, eliminating warm-lane stale-binary hazards (GH-789).

### Build lanes

**When this section applies:** only to sessions that compile this workspace on
this machine. It is a rule about local Cargo build cache, not about edda or the
fleet in general. For scale: *using* edda costs about 26 MB for the binary plus
0.4–9 MB of ledger per project, while *compiling* this 23-crate workspace costs
tens of GB per target directory. Everything below is about the second number.
A fleet session that runs no local build has no lane and needs none.

- Never create ad-hoc `CARGO_TARGET_DIR`s per round, per SHA, or per
  timestamp. Build output is disposable cache, but unbounded copies are not: an
  audit of one workstation counted 15 such directories and ~194 GB.
- Solo work uses the worktree's default `target/`.
- **Lane root** is the `LOCALAPPDATA` directory plus `\fleet-workstation\lanes`
  — `"$env:LOCALAPPDATA\fleet-workstation\lanes"` in PowerShell (quoted: a
  bare assignment of that path is a parser error),
  `$LOCALAPPDATA/fleet-workstation/lanes` in Git Bash — unless `FLEET_LANE_ROOT`
  is set or the brief names a different absolute path. A lane is always
  `<lane-root>\<lane-name>`; resolve it from that rule, never invent a path.
- Fleet sessions that build locally use only the lane named in their brief —
  one of `worker-1`, `worker-2`, `verifier`, `verifier-2` — for their whole
  lifetime. The workstation lane tool (`lane.ps1 -Lane <assigned> -Gate
  focused|freeze`) is not shipped yet; until it is, set `CARGO_TARGET_DIR` to
  `<lane-root>\<assigned-lane>` yourself and reuse it every round. If your brief
  names no lane or no lane root and you are about to build for a fleet, ask the
  controller first rather than creating a directory.
- Verifier lanes set `CARGO_INCREMENTAL=0` — that covers the focused
  Windows-gap run, which is L1's only local Cargo work. Fix lanes run only L0
  on touched crates, with the incremental cache left enabled: a focused `-p`
  build is a small fraction of the workspace footprint, and the orphaned
  incremental sessions in the Footprint section below came from workspace-scale
  runs, not focused ones.
- Build output is disposable and non-destructive to delete; per
  `fleet.merged-artifact-cleanup`, an artifact whose PR is **merged** — the
  merged PR's remote branch and its lane worktree — may also be reclaimed,
  because the squash commit is on `main` and GitHub keeps `refs/pull/N/head`,
  so SHA-pinned verdicts stay resolvable. Anything unmerged stays untouched:
  open or closed-unmerged branches, worktrees carrying uncommitted work,
  another session's active branch or worktree, and sources.

#### Footprint and thresholds — measured, and larger than they should be

A long-lived `target/debug` for this workspace was measured at **40.9 GB**
across 88,789 files:

| Part | Size | What it is |
|---|---:|---|
| `incremental` | 21.6 GB | per-session codegen caches (`.o` + `.bin`) |
| `.rlib` + `.rmeta` | 10.3 GB | compiled libraries and metadata |
| `.pdb` | 8.1 GB | Windows debug symbols — `edda.pdb` alone is 310 MB against a 51 MB `edda.exe` |
| `.exe` | 0.9 GB | the 84 test and binary executables that actually run |

Two thirds of that is avoidable rather than intrinsic. Every one of the 509
incremental session directories was older than 24 hours — orphans left by
interrupted or killed builds, which nothing in Cargo ever reclaims. The
workspace sets `debug = "line-tables-only"` for dev builds (GH-810); before
that it tuned `[profile.release]` only and debug builds carried full symbols
into every statically linked test binary.

**Any lane pool ceiling stated before that footprint is fixed is provisional.**
A single warm lane can reach 40 GB, so a four-lane pool at today's settings
would need well over 100 GB — a ceiling calibrated against the 194 GB pathology
would refuse during normal operation. Do not treat a fixed number here as
authority yet: reclaim stale `incremental` sessions by age, and measure again
before setting a limit. Stop and report if the pool is clearly growing without
bound.

## Pre-commit Checklist

```bash
# Before every commit (L0 — touched crates only)
cargo fmt --all --check
cargo clippy -p <touched crate> --all-targets -- -D warnings
cargo test -p <touched crate>
# GH-779 file-length ratchet: scripts/lint-file-length.sh --tree (CI / L0);
# pre-commit runs --staged on each staged *.rs blob.

# L1 (once per frozen full SHA, clean tree) = exact-head CI (Format; Clippy
# ×3 OS; Test on Linux + macOS full workspace; Test on the Windows 7-crate
# subset) + one focused local run by the verifier: `cargo test -p <crate>` on
# Windows for each touched crate outside the subset — the C5 selector loop.
# The implementer / fix lane runs L0 on touched crates, pushes, posts the
# `Review Response`, and does not run the workspace gate.
for crate in $(git diff --name-only "origin/$BASE...$SHA" \
  | grep '^crates/' | cut -d/ -f2 | sort -u \
  | grep -Ev '^(edda-store|edda-ledger|edda-search-fts|edda-transcript|edda-bridge-claude|edda-conductor|edda-cli)$'); do
  cargo test -p "$crate"   # on Windows, verifier lane, CARGO_INCREMENTAL=0
done
# then record the gate receipt: full SHA, CI run id, toolchain, lane, result
```

Set `CARGO_INCREMENTAL=0` in the environment for the verifier's focused C5 run
— not as an inline prefix, which only POSIX shells accept:

```powershell
$env:CARGO_INCREMENTAL = "0"   # PowerShell
```

```bash
export CARGO_INCREMENTAL=0     # bash / Git Bash
```

The L1 block is the ladder's L1 row — keep the two identical. The gate receipt
is `CI run <id> @ <full SHA>`; the fix lane posts it in the `Review Response`.
Skipping the receipt is not a shortcut: without it the reviewer has nothing to
READ, which is the cost this ladder exists to remove.

## Commit Conventions

- **Format**: `<type>(<scope>): <description>`
- **Scope**: crate name (e.g., `feat(edda-core):`, `fix(edda-ledger):`)
- **Types**: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`

Examples:
```
feat(edda-core): add DecisionScope enum
fix(edda-ledger): hash chain verification edge case
docs(edda-cli): update README with new commands
test(edda-serve): add HTTP endpoint coverage
```

## Dependency Management

- Workspace dependencies defined in root `Cargo.toml`
- Use `workspace = true` in crate Cargo.toml
- Current shared deps: `anyhow`, `thiserror`, `serde`, `serde_json`, `sha2`, `hex`, `ulid`, `time`, `clap`, `tracing`, etc.

---

## Decision Recording

This project uses **edda** for decision tracking across sessions.

When you make an architectural decision (choosing a library, defining a pattern,
changing infrastructure), record it:

```bash
edda decide "domain.aspect=value" --reason "why"
```

### What to record

- Choosing a database, ORM, or storage engine
- Picking an auth strategy or session management approach
- Defining error handling or logging patterns
- Adding or changing deployment configuration
- Creating new modules or establishing code structure

### What NOT to record

- Formatting changes, typo fixes, minor refactors
- Dependency version bumps (unless switching libraries)
- Test additions that don't change architecture

### Expectations

- **Record at least 1-2 decisions per session** — if you chose a library, defined a pattern, or changed config, that's a decision
- Record decisions AS you make them, not at the end
- When in doubt, record it — too many decisions is better than too few

### Examples

```bash
edda decide "db.engine=sqlite" --reason "embedded, zero-config for MVP"
edda decide "auth.strategy=JWT" --reason "stateless, scales horizontally"
edda decide "error.pattern=enum+IntoResponse" --reason "axum idiomatic, typed errors"
```

## Session Notes

Before ending a session, summarize what you did:

```bash
edda note "completed X; decided Y; next: Z" --tag session
```

<!-- edda:decision-tracking -->

<!-- edda:coordination -->
## Multi-Agent Coordination (edda)

When edda detects multiple agents, it injects peer information into your context.

**You MUST follow these rules:**
- **Check Off-limits** before editing any file — if a file is listed under "Off-limits", do NOT edit it
- **Claim your scope** at session start: `edda claim "label" --paths "src/scope/*"`
- **Request before crossing boundaries**: `edda request "peer-label" "your message"`
- **Respect binding decisions** — they apply to all sessions

Ignoring these rules causes merge conflicts and duplicated work.

### PR review-fix loop

**How to review is `REVIEW.md` at the repo root — run it top to bottom.** It is
the single executable spec: fetch, diff, mechanical class routing, per-rule
severity and check command, `[判斷]` escalation, the wiring verdict slot, and
the fixed output format. Do not restate its procedure anywhere else; point at
it, as this section does.

This section states the **contract** that procedure serves — the loop a PR must
show before it is merged, and the source `REVIEW.md` cites for it. It is a
bounded complete review, never a minimal review. It governs sessions invoking
the coordination review/orchestration skills; it is not an Edda runtime rule
imposed on every project.

1. Each handoff freezes `IN SCOPE`: changed behavior/paths, direct
   callers/consumers, issue/spec acceptance, security/data-loss regressions
   introduced or exposed by the change, and current-base integration.
   Adjacent, pre-existing, and speculative findings are evidenced
   `FOLLOW-UP ISSUE`s that do not extend the PR. Every frozen-surface failure
   is mandatory; only findings genuinely outside it qualify for follow-up.
2. Before `Changes Requested`, finish the whole scoped audit and batch every
   blocking P0/P1. Later-round blockers must be fix-caused or previously
   unobservable; otherwise route follow-up. The issue/spec is the acceptance
   ceiling, except evidence needed to prove a required fact or safety boundary.
3. `Code Review: Round N` is pinned to the reviewed full SHA. Its fields and
   table shape are fixed by `REVIEW.md` §7, and the verdict rule by §8 — any
   P0 or P1 is `Changes Requested`.
   Gate selection follows code/product-blob, base, and toolchain changes;
   docs/evidence-only pushes reuse applicable code gates and run only relevant
   validation plus exact-head CI.
4. Every `Changes Requested` round has an implementer `Review Response: Round
   N` that answers each blocking finding, names the new full SHA, and reports
   ran gates. Follow-up issues are linked but require no response.
5. Every push invalidates the prior verdict and requires another review round.
6. Stop after two non-product/harness-only cycles without useful progress or
   at diminishing returns; classify/route the finding instead of continuing.
7. The final comment is current-head `LGTM`, P0=0, P1=0, with exact gates.

Internal verifier reports, task receipts, and CI do not replace PR comments.
Merge still requires explicit operator authority.

For local-only delivery, record the same round/response/verdict fields in the
strongest durable local carrier; do not invent a PR.

### Verification cost

Rules regulate cost and reclamation, not only evidence:

- Verify once per frozen SHA on the ladder above. READ recorded gate results
  and exact-head CI before any RAN, and state the reason whenever you rerun a
  recorded gate.
- Every worker/verifier brief names a verification budget (L0 in the fix lane;
  L1 = CI + focused Windows-gap run by the verifier) and cleanup authority (build output is disposable and
  stale cache should be reclaimed by age; per `fleet.merged-artifact-cleanup`,
  an artifact whose PR is **merged** — the merged PR's remote branch and its
  lane worktree — may be reclaimed, since the squash commit is on `main` and
  GitHub keeps `refs/pull/N/head`, so SHA-pinned verdicts stay resolvable;
  anything unmerged — open or closed-unmerged branches, worktrees carrying
  uncommitted work, another session's active branch or worktree, and sources —
  stays untouched). It names a build lane **only when the session compiles
  locally** — see Build lanes; a session that builds nothing has no lane and
  reports `n/a`.
- One verifier identity per PR: rounds resume the same session, and the same
  lane where one applies; a replacement reads receipts and CI before running
  anything.
- Over-verification — a second RAN for an already-recorded SHA without a
  reason, workspace gates for a docs-only push, an ad-hoc target directory —
  is a process finding: record it in the handoff cost line, route it as a
  `FOLLOW-UP ISSUE`, correct the next brief. It does not block a product-green
  PR.
