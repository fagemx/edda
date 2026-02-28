---
name: coord-request
description: Detect changes affecting peers and send coordination requests
---

# Coordination Request

You are a coordination specialist. Your role is to help the agent identify changes that affect other agents and send appropriate cross-agent requests.

## When to Use

- After making public API changes (new/modified types, functions, traits)
- After changing shared configuration, schema, or data structures
- When you need something from another agent's scope (a type export, an API endpoint, etc.)

## Workflow

### Step 1: Identify Changes

See what files have been modified:

```bash
git diff --name-only
```

For files with API changes (public types, function signatures), inspect the diff:

```bash
git diff <file>
```

Look for:
- New or modified `pub fn`, `pub struct`, `pub enum`, `pub trait`
- Changed function signatures (parameters, return types)
- Removed or renamed public items
- New dependencies in `Cargo.toml`

### Step 2: Check Peer Claims

Run `edda peers` to see who owns what scope.

Cross-reference your changed files with peer claim paths:
- If you modified `crates/edda-store/src/lib.rs` and a peer claims `crates/edda-store/*`, they are affected
- If you added a new public type that a peer's crate depends on, they may need to update imports

### Step 3: Determine Impact

For each affected peer, classify the impact:
- **Breaking**: You changed an API they depend on — they must update their code
- **Additive**: You added something they might want to use — informational
- **Request**: You need them to export/create something in their scope

### Step 4: Draft and Send

For each affected peer, compose a clear request:

```bash
edda request "<peer-label>" "<message>"
```

Message guidelines:
- Start with the action type: `[breaking]`, `[info]`, or `[need]`
- State what changed or what you need
- Be specific about files/types/functions

Examples:
- `edda request "auth" "[breaking]: Changed AuthToken fields in edda-core — update your imports"`
- `edda request "billing" "[need]: Export BillingPlan type from your crate so I can reference it"`
- `edda request "api" "[info]: Added new error variant ApiError::RateLimit — you may want to handle it"`

### Step 5: Verify

Run `edda peers` again to confirm your requests appear in the board state.

## Output Format

Present findings:

```
## Coordination Request

### Changes Detected
- <file>: <what changed (added/modified/removed public API)>

### Affected Peers
- [<label>]: <why affected, what they need to do>

### Requests Sent
- To <label>: <message>

### No Action Needed
- <peers whose scope is not affected by your changes>
```
