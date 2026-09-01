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

If the send fails with "no active session answers to ...", the label is wrong —
check `edda peers` for the live labels. Only pass `--force` when you are
deliberately queuing for a peer that has not started yet.

### Step 5: Verify

The send command itself is the verification: a label no active session answers
to is an error (with the live labels listed), and a label held by several
sessions prints a warning naming how many will see it. A clean exit with
`Request sent to [<label>]` means the request is on the board. Do NOT check
`edda peers` for it — that command lists sessions, not requests.

### Registered letter, not doorbell

`edda request` is a registered letter addressed to a **role**: it is durable,
acknowledged, and survives the peer dying — a replacement session holding the
same label receives it on arrival. What it can never do is **wake** anyone:
delivery happens at the peer's next hook event, and an idle session has no
next hook event.

If your host has cross-session messaging (a way to send a message that starts
the peer's next turn), ring after sending: fixate first (`edda request`), then
send a short host message that only points at it ("coordination request
pending — run `edda coord`"). Letter first, bell second — the letter is the
truth, the bell is latency.

### Answering requests addressed to you

Requests you receive stay pending until you acknowledge them — being shown the
message does not clear it, and the sender has no other signal that it landed.
Once you have acted on a peer's request:

```bash
edda request-ack "<their-label>"
```

This retires only what is outstanding now, so a later message from that same
peer still reaches you.

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
