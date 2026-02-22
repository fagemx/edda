# Contributing to Edda

Thanks for your interest in contributing! This guide covers how to build, test, and submit changes.

## Prerequisites

- Rust stable (1.75+)
- Git

## Build and Test

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Check formatting and lint
cargo fmt --check
cargo clippy --workspace --all-targets
```

## Project Structure

Edda is a Cargo workspace with 15 crates:

| Crate | Purpose |
|-------|---------|
| `edda-core` | Event model, hash chain, schema |
| `edda-ledger` | SQLite ledger, blob store |
| `edda-cli` | All CLI commands |
| `edda-tui` | Real-time TUI (`edda watch`) |
| `edda-bridge-claude` | Claude Code hooks and context injection |
| `edda-bridge-openclaw` | OpenClaw hooks and plugin |
| `edda-mcp` | MCP server (7 tools) |
| `edda-ask` | Cross-source decision query engine |
| `edda-derive` | View rebuilding |
| `edda-pack` | Context generation and budget controls |
| `edda-transcript` | Transcript ingest and classification |
| `edda-store` | Per-user store, atomic writes |
| `edda-search-fts` | Full-text search (Tantivy) |
| `edda-index` | Transcript index |
| `edda-conductor` | Multi-phase plan orchestration |

## Making Changes

### Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(cli): add --json flag to edda log
fix(bridge): resolve session identity via heartbeat
test(ledger): add hash chain integrity tests
docs: update README quick start section
chore: apply cargo fmt across workspace
refactor(store): simplify atomic write logic
```

**Prefixes**: `feat`, `fix`, `test`, `docs`, `chore`, `refactor`, `perf`

**Scope** (optional): the crate or area being changed — `cli`, `bridge`, `ledger`, `tui`, `mcp`, `search`, `store`, `core`

### Pull Requests

1. Create a feature branch: `git checkout -b feat/your-change`
2. Make your changes with conventional commit messages
3. Ensure CI passes: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --check`
4. Open a PR against `main`

### Adding a New Bridge

To add support for a new agent platform:

1. Create `crates/edda-bridge-{name}/` with `Cargo.toml` and `src/`
2. Implement hook dispatch (`dispatch.rs`) and admin install/uninstall (`admin.rs`)
3. Follow the patterns in `edda-bridge-claude` or `edda-bridge-openclaw`
4. Add the crate to the workspace `Cargo.toml`
5. Wire CLI subcommands in `edda-cli/src/cmd_bridge.rs`

### Adding a New MCP Tool

1. Add the tool function in `crates/edda-mcp/src/server.rs`
2. Register it in the `#[tool]` impl block
3. Follow the pattern of existing tools (e.g., `edda_decide`, `edda_query`)

## License

By contributing, you agree that your contributions will be licensed under the MIT OR Apache-2.0 dual license.
