# Zero Tolerance for Warnings

**All code must compile without warnings. Clippy warnings are errors.**

## Core Principle

In this project, compiler warnings and clippy lints are **non-negotiable**. They exist to maintain code quality, consistency, and prevent bugs.

**If clippy complains, fix the code - don't silence clippy.**

## The Four Rules

### 1. Never Add `#[allow(...)]` Attributes

**Never do this:**
```rust
#[allow(dead_code)]
fn unused_function() {
    // ...
}

#[allow(unused_variables)]
fn process(data: Vec<u8>) {
    // data is never used
}

#[allow(clippy::needless_return)]
fn compute() -> u32 {
    return 42;
}
```

**Why this is bad:**
- Disabling warnings means accepting poor code quality
- Creates technical debt
- The warning exists for a reason
- Future developers will copy this pattern

**Do this instead:**
```rust
// Remove unused function entirely - git history preserves it

fn process(data: Vec<u8>) {
    // Actually use data, or remove the parameter
    process_bytes(&data);
}

fn compute() -> u32 {
    42 // No needless return
}
```

### 2. Never Add `#[allow(clippy::...)]` Attributes

**Never do this:**
```rust
#[allow(clippy::too_many_arguments)]
fn create_session(
    name: String,
    project_id: String,
    user_id: String,
    timeout: u64,
    retries: u32,
    env: HashMap<String, String>,
    labels: Vec<String>,
    config: Config,
) -> Session {
    todo!()
}

#[allow(clippy::unwrap_used)]
fn parse_config(s: &str) -> Config {
    serde_json::from_str(s).unwrap()
}
```

**Why this is bad:**
- Clippy catches real issues
- Too many arguments means the function needs refactoring
- `.unwrap()` in library code can panic at runtime

**Do this instead:**
```rust
// Group related parameters into a struct
struct CreateSessionParams {
    name: String,
    project_id: ProjectId,
    user_id: UserId,
    timeout: Duration,
    env: HashMap<String, String>,
}

fn create_session(params: CreateSessionParams) -> Session {
    todo!()
}

// Handle errors properly
fn parse_config(s: &str) -> Result<Config, serde_json::Error> {
    serde_json::from_str(s)
}
```

### 3. Fix the Underlying Issue

Don't suppress warnings - address the root cause.

**Bad - Suppressing the symptom:**
```rust
#[allow(unused_imports)]
use std::collections::HashMap;

#[allow(unused_mut)]
let mut counter = 0;

#[allow(clippy::redundant_clone)]
let name = session.name.clone();
```

**Good - Fixing the cause:**
```rust
// Remove unused import entirely

// Remove mut if never reassigned
let counter = 0;

// Don't clone if not needed
let name = &session.name;
```

### 4. Respect All Clippy Lints

Every clippy lint serves a purpose. Don't disable lints in config.

**Bad:**
```toml
# Cargo.toml or clippy.toml
[lints.clippy]
pedantic = { level = "allow" }
unwrap_used = { level = "allow" }
```

**Good:**
```toml
# Cargo.toml
[workspace.lints.clippy]
all = "warn"
```

## Common Clippy Warnings and Solutions

### Dead Code

**Bad:**
```rust
fn helper_function() -> u32 {
    // This function is never called
    42
}
```

**Good - Remove it:**
```rust
// Delete the function. Git history preserves it if needed later.
```

### Unused Variables

**Bad:**
```rust
fn process(session: Session, config: Config) {
    // Only using session, config is unused
    println!("{}", session.name);
}
```

**Good - Remove or prefix:**
```rust
// If parameter is needed for trait compliance
fn process(session: Session, _config: Config) {
    println!("{}", session.name);
}

// Better: remove if not needed
fn process(session: Session) {
    println!("{}", session.name);
}
```

### Needless Return

**Bad:**
```rust
fn get_name(session: &Session) -> &str {
    return &session.name;
}
```

**Good:**
```rust
fn get_name(session: &Session) -> &str {
    &session.name
}
```

### Redundant Clone

**Bad:**
```rust
let name = session.name.clone();
drop(session); // session is moved, clone was unnecessary
```

**Good:**
```rust
let name = session.name; // Move instead of clone
```

### Too Many Arguments

**Bad:**
```rust
fn create(a: String, b: String, c: u64, d: bool, e: Vec<u8>, f: Option<String>) {
    // ...
}
```

**Good:**
```rust
struct CreateParams {
    a: String,
    b: String,
    c: u64,
    d: bool,
    e: Vec<u8>,
    f: Option<String>,
}

fn create(params: CreateParams) {
    // ...
}
```

### Unwrap Used

**Bad:**
```rust
let config: Config = serde_json::from_str(json_str).unwrap();
let session = sessions.get(0).unwrap();
```

**Good:**
```rust
let config: Config = serde_json::from_str(json_str)?;
let session = sessions.first().ok_or(Error::EmptyList)?;
```

### Manual Map / Manual Flatten

**Bad:**
```rust
let result = match option {
    Some(v) => Some(v.to_string()),
    None => None,
};
```

**Good:**
```rust
let result = option.map(|v| v.to_string());
```

### Wildcard Imports

**Bad:**
```rust
use std::collections::*;
use crate::models::*;
```

**Good:**
```rust
use std::collections::HashMap;
use crate::models::{Session, Run, SessionId};
```

## Handling Warnings

### Step 1: Read the Warning Message

Compiler and clippy warnings tell you what's wrong:

```
warning: unused variable: `config`
  --> src/main.rs:10:9
   |
10 |     let config = load_config();
   |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_config`

warning: this function has too many arguments (8/7)
  --> src/session.rs:20:1
```

### Step 2: Understand Why the Lint Exists

Before fixing, understand the purpose:
- `dead_code`: Remove unused code to reduce maintenance burden
- `unused_variables`: Clean up dead assignments
- `clippy::unwrap_used`: Prevent runtime panics
- `clippy::too_many_arguments`: Functions with many args are hard to use correctly

### Step 3: Fix the Root Cause

Don't suppress the warning - fix the underlying issue.

### Step 4: Run Clippy Again

Verify the fix:

```bash
cargo clippy -- -D warnings
```

## Pre-Commit Workflow

### Always check before committing:

```bash
# Format code
cargo fmt

# Check clippy (warnings as errors)
cargo clippy -- -D warnings

# Run tests
cargo test

# Verify compilation
cargo check --all-targets
```

All must pass before committing.

## Auto-Fix Where Possible

Some warnings can be auto-fixed:

```bash
# Auto-fix formatting
cargo fmt

# Auto-fix some clippy issues
cargo clippy --fix --allow-dirty
```

**But verify the fixes!** Don't blindly accept auto-fixes.

## When You Think a Lint is Wrong

If you believe a clippy lint is incorrect:

1. **First, assume the lint is right** - It probably is
2. **Understand the lint's purpose** - Read the documentation
3. **Try to fix the code** - There's usually a better way
4. **Discuss with team** - Don't just suppress it

**In this project, we maintain zero tolerance.** Don't suppress lints.

## Benefits of Zero Warning Tolerance

### Consistent Code Quality
- All code follows same standards
- No exceptions create inconsistency
- Easy to understand codebase

### Catch Bugs Early
- Unused variables might indicate logic errors
- Clippy catches common mistakes
- Best practices enforced

### Better Collaboration
- No arguments about code style
- Automated enforcement
- Clear expectations

### Easier Maintenance
- Find dead code easily
- Refactor with confidence
- No technical debt accumulation

## Remember

**Lint rules exist to help you write better code.**

- Never add `#[allow(...)]`
- Fix the root cause
- Respect all lints
- Zero tolerance means zero tolerance

**"If clippy complains, the code is wrong - not clippy."**
