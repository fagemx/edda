# Contributing to Edda

Thanks for your interest in contributing! This guide covers how to build, test, and submit changes.

## Prerequisites

- Rust stable (1.75+)
- Git

## Git Hooks (optional)

We use [Lefthook](https://github.com/evilmartians/lefthook) for local pre-commit and pre-push checks:

```bash
npm install -g @evilmartians/lefthook
lefthook install
```

This runs `cargo fmt --check` on commit and `cargo clippy --workspace -- -D warnings` on push.
Lefthook is local-only and does not affect CI.

### Git hooks (no extra tools)

Edda also ships zero-dependency POSIX `sh` hooks that enforce the checks locally:

```bash
sh scripts/githooks/install.sh
```

This sets `git config core.hooksPath scripts/githooks` (idempotent; uninstall with `git config --unset core.hooksPath`).

- `pre-commit` — from the staged paths: runs `cargo fmt --all --check` when any `*.rs` or `Cargo.toml`/`Cargo.lock` is staged; runs `cargo clippy -p <crate> --all-targets -- -D warnings` for each touched `crates/<crate>/` (set `SKIP_CLIPPY=1` to skip — the commit message is then tagged `[skip-clippy]`); runs `sh scripts/lint-markdown-content.sh` when any `*.md` is staged; rejects any staged file larger than 1 MB.
- `commit-msg` — enforces `<type>(<scope>): <desc>` or `<type>: <desc>` (types: `feat fix docs refactor test chore ci perf style build`); merge commits, `wip(` lane checkpoints, and `Revert ` messages are allowed.

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

Edda is a Cargo workspace; the crate map lives in the
[Architecture section of the README](README.md#architecture) and is kept
current there — this file deliberately doesn't duplicate it. Quick
orientation: `edda-core` (event model, hash chain) and `edda-ledger`
(SQLite store) are the foundation, `edda-cli` is the entry point,
`edda-bridge-*` integrate each harness, and everything else builds on those.

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
