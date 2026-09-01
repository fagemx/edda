---
name: Project Principles
description: Core architectural and code quality principles that guide all development decisions in the edda project
---

# Project Principles Skill

This skill defines the fundamental design principles and coding standards for the edda project. These principles are MANDATORY for all code written in this project and should guide every development decision.

## The Four Core Principles

### 1. YAGNI (You Aren't Gonna Need It) - CORE PRINCIPLE

**Don't add functionality until it's actually needed.**

Quick rules:
- Start with the simplest solution that works
- Avoid premature abstractions
- Delete unused code aggressively
- No "just in case" features

**When coding:** Ask "Do we need this NOW?" If not, don't add it.

> For detailed guidelines and examples, read `yagni.md`

### 2. Proper Error Handling (Not Defensive Programming)

**Use `Result`/`Option` and `?` propagation. Don't panic or swallow errors.**

Quick rules:
- Only handle errors when you can meaningfully recover
- Let errors propagate with `?` to where they can be properly addressed
- Avoid `.unwrap()` in library code - use `?` or `.expect("reason")`
- Trust the type system and Rust's ownership model

**When coding:** Only add explicit error handling when you have specific recovery logic.

> For detailed guidelines and examples, read `no-defensive.md`

### 3. Leverage Rust's Type System

**Use the type system to enforce correctness at compile time.**

Quick rules:
- Use newtypes for domain concepts (`SessionId(Uuid)` not raw `Uuid`)
- Make invalid states unrepresentable with enums
- Use `thiserror` for library errors, `anyhow` for application errors
- Define structs and enums for all data structures

**When coding:** If a domain concept uses a primitive type, wrap it in a newtype.

> For detailed guidelines and examples, read `type-safety.md`

### 4. Zero Tolerance for Warnings

**All code must compile without warnings. Clippy warnings are errors.**

Quick rules:
- Never add `#[allow(...)]` to suppress warnings
- Never add `#[allow(clippy::...)]` to bypass clippy lints
- Fix the underlying issue, don't suppress the warning
- Run `cargo clippy -- -D warnings` before committing

**When coding:** If clippy complains, fix the code, not the linter.

> For detailed guidelines and examples, read `zero-lint.md`

## Quick Reference: Code Quality Checklist

Before writing any code, verify:
- Is this feature needed NOW? (YAGNI)
- Am I propagating errors properly with `?`? (Error Handling)
- Are domain concepts wrapped in newtypes? (Type Safety)
- Will this pass `cargo clippy -- -D warnings`? (Zero Warnings)

## When to Load Additional Context

- **Starting a new feature?** Read `yagni.md` first
- **Handling errors?** Read `no-defensive.md`
- **Defining types?** Read `type-safety.md`
- **Getting clippy warnings?** Read `zero-lint.md`

## Integration with Workflow

These principles should be applied:
1. **Before writing code** - Plan with YAGNI in mind
2. **While writing code** - Follow type safety and proper error handling
3. **Before committing** - Ensure zero warnings with `cargo clippy -- -D warnings`
4. **During code review** - Verify adherence to all principles

## Philosophy

These principles exist to:
- Keep the codebase simple and maintainable
- Prevent technical debt accumulation
- Ensure high code quality
- Make the project easy to understand and modify

They may feel restrictive at first, but they lead to cleaner, more maintainable code.

## Conflict Resolution

If principles seem to conflict:
1. YAGNI takes precedence - simplicity wins
2. Type safety is non-negotiable
3. When in doubt, choose the simpler solution
