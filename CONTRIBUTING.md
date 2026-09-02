# Contributing to Edda

Thanks for your interest in contributing! This guide covers how to build, test, and submit changes.

## Prerequisites

- Rust stable (1.75+)
- Git

## Git Hooks

Enable the git-native pre-commit and commit-msg hooks (zero external
dependencies — no lefthook, no npm, nothing to install):

```bash
sh scripts/githooks/install.sh
```

On every commit this enforces the L0 gates from `.claude/CLAUDE.md` on the
staged paths:

- any staged file larger than 1 MB is rejected
- staged `*.rs` or `Cargo.*` → `cargo fmt --all --check`
- staged `crates/<crate>/…` → `cargo clippy -p <crate> --all-targets -- -D warnings` for each touched crate
- staged `*.md` → `sh scripts/lint-markdown-content.sh`
- conventional-commit subject check (`<type>(<scope>): <description>`)

Merge commits and `wip(…)` lane checkpoints pass the message check.
`SKIP_CLIPPY=1 git commit …` skips the clippy gate; the hook then appends
`[skip-clippy]` to the commit message so reviewers can see the skip.
`git commit --no-verify` bypasses all local hooks — CI still gates every push.

The hook scripts are POSIX sh and live in `scripts/githooks/`; a self-test
that exercises all scenarios in a throwaway repo is `sh scripts/githooks/test.sh`.

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
docs(contributing): update README quick start section
chore(repo): apply cargo fmt across workspace
refactor(store): simplify atomic write logic
```

**Prefixes**: `feat`, `fix`, `test`, `docs`, `chore`, `refactor`, `ci`, `perf`,
`style`, `build` — enforced by the `commit-msg` hook above (merge commits and
`wip(…)` lane checkpoints are exempt)

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
