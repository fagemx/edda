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
# Run all tests
cargo test --workspace

# Run tests for single crate
cargo test -p edda-core

# Run specific test
cargo test -p edda-core test_name
```

- **Unit tests**: In `#[cfg(test)] mod tests` within source files
- **No integration tests directory** currently — tests are inline
- **No mocking internal crates** — use real SQLite via `tempfile`
- See `edda-core/src/types.rs:261-667` for test patterns

## Pre-commit Checklist

```bash
cargo fmt --check      # Format check
cargo clippy           # Lint (CI uses -Dwarnings)
cargo test --workspace # All tests
```

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
