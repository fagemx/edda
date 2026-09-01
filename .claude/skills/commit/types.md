# Commit Types - Detailed Reference

This document provides comprehensive details on all valid commit types in the edda project.

## Release-Triggering Types

### `feat:` - New Feature
**Triggers:** Minor version bump (e.g., 1.2.0 → 1.3.0)

Use when:
- Adding a completely new feature or capability
- Introducing new user-facing functionality
- Adding new API endpoints or CLI commands
- Creating new crates or modules

Examples:
- `feat: add user authentication system`
- `feat(api): add session creation endpoint`
- `feat(cli): add deploy command`
- `feat(runner): add sandbox job execution`

### `fix:` - Bug Fix
**Triggers:** Patch version bump (e.g., 1.2.0 → 1.2.1)

Use when:
- Fixing a bug or error in existing functionality
- Correcting unexpected behavior
- Resolving panics or error conditions
- Fixing performance issues

Examples:
- `fix: resolve database connection timeout`
- `fix(auth): prevent token expiration edge case`
- `fix(sandbox): handle vsock disconnect gracefully`
- `fix(db): correct session state transition validation`

**Special case:** You can use `fix:` for refactoring that improves code quality:
- `fix: refactor authentication logic for better maintainability`

### `deps:` - Dependency Updates
**Triggers:** Patch version bump (e.g., 1.2.0 → 1.2.1)

Use when:
- Updating crate dependencies
- Upgrading libraries or frameworks
- Security updates

Examples:
- `deps: update tokio to v1.36`
- `deps: bump axum from 0.7 to 0.8`

## Non-Release Types

These types appear in the changelog but do NOT trigger a new release:

### `docs:` - Documentation
**Triggers:** No release

Use when:
- Updating README files
- Changing doc comments (`///` or `//!`)
- Modifying documentation in `docs/`
- Updating API documentation

Examples:
- `docs: update installation instructions`
- `docs(api): add examples for webhook endpoints`
- `docs: fix typo in contributing guide`

### `style:` - Code Style
**Triggers:** No release

Use when:
- Formatting code (rustfmt changes)
- Style improvements that don't change logic

Examples:
- `style: format code with cargo fmt`
- `style: fix clippy style suggestions`
- `style: adjust import ordering`

### `refactor:` - Code Refactoring
**Triggers:** No release

Use when:
- Restructuring code without changing behavior
- Improving code organization
- Extracting functions or modules
- Renaming for clarity

Examples:
- `refactor: extract validation logic to separate module`
- `refactor: simplify database query logic`
- `refactor(auth): reorganize authentication flow`

**Note:** If you want the refactor to trigger a release, use `fix: refactor ...` instead.

### `test:` - Test Changes
**Triggers:** No release

Use when:
- Adding new tests
- Modifying existing tests
- Fixing test failures
- Improving test coverage

Examples:
- `test: add integration tests for session service`
- `test(db): add migration rollback tests`
- `test: fix flaky integration test`

### `chore:` - Build/Tool Changes
**Triggers:** No release

Use when:
- Updating build scripts
- Modifying CI/CD configuration
- Changing development tools
- Updating Cargo workspace config

Examples:
- `chore: update workspace Cargo.toml`
- `chore: configure clippy lints`
- `chore: add cargo-watch for development`

### `ci:` - CI Configuration
**Triggers:** No release

Use when:
- Modifying GitHub Actions workflows
- Updating CI/CD pipelines
- Changing release automation
- Adjusting build matrix

Examples:
- `ci: optimize release workflow dependencies`
- `ci: add caching for cargo dependencies`
- `ci: update rust toolchain in workflow`

### `perf:` - Performance Improvements
**Triggers:** No release (unless breaking)

Use when:
- Optimizing performance
- Reducing binary size
- Improving response times
- Optimizing algorithms

Examples:
- `perf: optimize database query batching`
- `perf: reduce api response time with connection pooling`
- `perf(storage): implement streaming upload`

### `build:` - Build System
**Triggers:** No release

Use when:
- Changing build configuration
- Modifying Cargo.toml settings
- Updating build.rs scripts
- Changing feature flags

Examples:
- `build: enable lto for release builds`
- `build: configure workspace dependencies`

### `revert:` - Revert Previous Commit
**Triggers:** No release

Use when:
- Reverting a previous commit
- Rolling back changes

Examples:
- `revert: revert "feat: add dark mode"`
- `revert: roll back database migration`

## Breaking Changes

**Triggers:** Major version bump (e.g., 1.2.0 → 2.0.0)

Any type can be a breaking change by adding `!` after the type or including `BREAKING CHANGE:` in the footer:

```
feat!: change api response format to include metadata

BREAKING CHANGE: API responses now return {data, metadata} instead of raw data
```

Use breaking changes when:
- Changing public API contracts
- Removing features or endpoints
- Changing behavior in incompatible ways
- Requiring migration steps

## Scopes (Optional)

Scopes provide additional context about what area of the codebase was affected:

Examples:
- `feat(api): add session endpoint`
- `fix(auth): resolve token refresh issue`
- `docs(cli): update command reference`
- `test(db): add session query tests`

Common scopes in this project:
- `bridge` - Claude Code bridge hooks (edda-bridge-claude)
- `cli` - CLI tool (edda-cli)
- `core` - Core types and events (edda-core)
- `derive` - View derivation (edda-derive)
- `index` - Event indexing (edda-index)
- `ledger` - Append-only event ledger (edda-ledger)
- `mcp` - MCP server (edda-mcp)
- `pack` - Memory packs (edda-pack)
- `search` - Full-text search (edda-search-fts)
- `store` - File store (edda-store)
- `transcript` - Transcript parsing (edda-transcript)
