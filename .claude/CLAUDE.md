# Edda Project Guidelines

Rust development principles and conventions for the edda project.

## Project Overview

- **Language**: Rust (edition 2021)
- **Structure**: Cargo workspace with 19 crates
- **Runtime**: Zero external runtime dependencies (CLI tool)
- **Storage**: SQLite (rusqlite), JSONL append-only ledger

## Workspace Crate Map

| Crate | Description |
|-------|-------------|
| `edda-core` | Core event model, hash chain, schema |
| `edda-ledger` | Append-only SQLite ledger with hash-chained events |
| `edda-derive` | View rebuilding and tiered history |
| `edda-store` | Per-user store with atomic writes |
| `edda-cli` | CLI and TUI (binary crate, published as `edda`) |
| `edda-serve` | HTTP API server |
| `edda-mcp` | MCP server (JSON-RPC 2.0) |
| `edda-ask` | Cross-source decision query engine |
| `edda-search-fts` | Full-text search (Tantivy) |
| `edda-index` | Transcript index |
| `edda-transcript` | Transcript delta ingest and classification |
| `edda-pack` | Context generation and budget controls |
| `edda-conductor` | Multi-phase plan orchestration |
| `edda-aggregate` | Cross-repo aggregation queries |
| `edda-chronicle` | Chronicle synthesis (recap/cognitive zoom) |
| `edda-postmortem` | L3 post-mortem analysis with TTL decay |
| `edda-notify` | Push notification dispatch |
| `edda-bridge-claude` | Claude Code hooks and transcript ingest |
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

# Run all tests (once per frozen SHA — see Verification ladder)
cargo test --workspace
```

- **Unit tests**: In `#[cfg(test)] mod tests` within source files
- **No integration tests directory** currently — tests are inline
- **No mocking internal crates** — use real SQLite via `tempfile`
- See `edda-core/src/types.rs:261-667` for test patterns

### Verification ladder

Verify once per frozen SHA; cite that result everywhere else. Full workspace
gates are expensive (23 crates; 8–13 GB of build output per target directory),
and CI already re-runs most of them independently on every push.

**Know exactly what CI does and does not cover** (`.github/workflows/ci.yml`):

| CI job | Coverage |
|---|---|
| Format | `cargo fmt --check` |
| Clippy | `cargo clippy --workspace --all-targets` on Linux, macOS **and** Windows |
| Test (Linux, macOS) | `cargo test --workspace` — all 23 crates |
| Test (Windows) | **only 7 crates** — `edda-store`, `edda-ledger`, `edda-search-fts`, `edda-transcript`, `edda-bridge-claude`, `edda-conductor`, `edda` (Windows build/link is ~5x slower; the subset is derived from process-spawn, file-lock, and mmap criteria — GH-433) |

So on Windows every crate is still **compiled and linted** (Clippy is
workspace-wide on all three OSes), but **only those 7 crates have their tests
run**. A Windows-specific *runtime* defect in the other 16 — a path, a file
lock, a spawned process, a temp directory — is caught by no CI job. That is a
stated reason for the verifier to run it locally, and it is why the L1 receipt
from a Windows workstation is load-bearing rather than redundant.

| Level | When | Run |
|---|---|---|
| L0 iterate | while editing | `cargo fmt --all --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo test -p <crate>` for each touched crate |
| L1 freeze | once per frozen full SHA, clean tree, before push / PR update | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`, with `CARGO_INCREMENTAL=0`; record the result together with the full SHA (gate receipt) |
| L2 review | verifier, once per frozen full SHA | READ the L1 receipt and exact-head CI; RAN only focused or adversarial checks they do not cover — **including Windows behavior in any crate outside the CI Windows subset above**. A full local rerun needs a stated reason: no receipt, red or absent CI, grounds to distrust the receipt, or coverage CI genuinely lacks. Deterministically red CI already blocks the SHA — audit and request changes instead of spending a full run; if the red is environmental, re-run only the failed job |
| L3 pre-merge | merge authority | READ exact-head CI and the final current-head LGTM; RAN only a merge check against the current base. A draft/ready, label, or status flip is not a push — nothing reruns |

Docs-only changes (no code/product blob, `Cargo.lock`, or toolchain change)
run no Cargo gate locally; exact-head CI is the gate.

### Build lanes

- Never create ad-hoc `CARGO_TARGET_DIR`s per round, per SHA, or per
  timestamp. Build output is disposable cache, but unbounded copies are not: an
  audit of one workstation counted 15 such directories and ~194 GB, with a
  single directory at 12.79 GB.
- Solo work uses the worktree's default `target/`.
- **Lane root** is the `LOCALAPPDATA` directory plus `\fleet-workstation\lanes`
  — `$env:LOCALAPPDATA\fleet-workstation\lanes` in PowerShell,
  `$LOCALAPPDATA/fleet-workstation/lanes` in Git Bash — unless `FLEET_LANE_ROOT`
  is set or the brief names a different absolute path. A lane is always
  `<lane-root>\<lane-name>`; resolve it from that rule, never invent a path.
- Fleet sessions build only in the lane named in their brief — one of
  `worker-1`, `worker-2`, `verifier`, `verifier-2` — for their whole lifetime.
  The workstation lane tool (`lane.ps1 -Lane <assigned> -Gate focused|freeze`)
  is not shipped yet; until it is, set `CARGO_TARGET_DIR` to
  `<lane-root>\<assigned-lane>` yourself and reuse it every round. If your brief
  names no lane and you are running gates for a fleet, ask the controller for
  one rather than creating a directory.
- Verifier lanes and every L1 run set `CARGO_INCREMENTAL=0`.
- Deleting lane build output is authorized and non-destructive; deleting
  worktrees, branches, or sources is not. Stop and report instead of building
  when the lane pool exceeds 50 GB.

## Pre-commit Checklist

```bash
# Before every commit (L0 — touched crates only)
cargo fmt --check
cargo clippy -p <touched crate> --all-targets -- -D warnings
cargo test -p <touched crate>

# Before freezing a SHA for push / PR update (L1 — once per frozen SHA,
# clean tree, incremental off). Record the result with the full SHA.
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# then record the gate receipt: full SHA, gate set, toolchain, lane, result
```

Set `CARGO_INCREMENTAL=0` in the environment for the whole L1 run — not as an
inline prefix, which only POSIX shells accept:

```powershell
$env:CARGO_INCREMENTAL = "0"   # PowerShell
```

```bash
export CARGO_INCREMENTAL=0     # bash / Git Bash
```

The L1 block is the ladder's L1 row — keep the two identical. Skipping the
receipt is not a shortcut: without it the reviewer has nothing to READ and must
run the whole set again, which is the cost this ladder exists to remove.

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

Before merging any GitHub PR, the PR itself must show the complete loop:

This is a bounded complete review, never a minimal review. It governs sessions
invoking the coordination review/orchestration skills; it is not an Edda
runtime rule imposed on every project.

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
3. `Code Review: Round N` is pinned to the reviewed full SHA and records
   `IN SCOPE`, `FOLLOW-UP ISSUE`, blocking P0/P1, `RAN` versus `READ` evidence,
   available elapsed/token/tool cost, and `Changes Requested` or `LGTM`.
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
- Every worker/verifier brief names a build lane, a verification budget (L0
  while iterating; L1 once per frozen SHA), and cleanup authority (lane build
  cache is disposable; worktrees, branches, and sources are never deleted).
- One verifier identity per PR: rounds resume the same session and lane; a
  replacement reads receipts and CI before running anything.
- Over-verification — a second RAN for an already-recorded SHA without a
  reason, workspace gates for a docs-only push, an ad-hoc target directory —
  is a process finding: record it in the handoff cost line, route it as a
  `FOLLOW-UP ISSUE`, correct the next brief. It does not block a product-green
  PR.
