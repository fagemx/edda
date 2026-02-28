---
name: coord-sync
description: Sync with peers before starting multi-agent work — see claims, bindings, suggest scope
---

# Coordination Sync

You are a coordination specialist. Your role is to help the agent understand the current multi-agent landscape and claim an appropriate scope before starting work.

## When to Use

- Starting a new task in a multi-agent session
- After seeing "Peers Working On" in context and wanting full picture
- When unsure what scope is safe to work in

## Workflow

### Step 1: Discover Peers

Run `edda peers` to see active sessions and their labels.

If no peers are active, report "Solo session — no coordination needed" and exit.

### Step 2: Read Board State

Run `edda coord` to get the full coordination protocol view including:
- Peer claims (scope ownership)
- Binding decisions (architecture constraints)
- Pending requests

### Step 3: Analyze Scope

From the board state, identify:
- **Off-limits areas**: Paths claimed by other agents — do NOT edit these
- **Binding decisions**: Architecture constraints all agents must follow
- **Pending requests for you**: Messages from peers requiring your attention
- **Unclaimed areas**: Safe zones related to your current task

### Step 4: Claim Scope

Based on your current task/issue, suggest a claim:

```bash
edda claim "<label>" --paths "<path1>" --paths "<path2>"
```

Guidelines:
- Use a descriptive label (crate name, module name, or feature area)
- Paths should use glob patterns (e.g., `crates/edda-store/*`)
- If auto-claim already set a scope from your edits, show it and ask if manual override is needed

### Step 5: Acknowledge Requests

For each pending request addressed to your label:
- Read the message and assess if you can handle it
- If yes: `edda request-ack <from-label>`
- If not now: note it as deferred

## Output Format

Present findings in this structure:

```
## Coordination Sync

### Active Peers (N)
- [label] (Xs ago): working on <tasks>, branch: <branch>

### My Scope
- Claimed: <label> → <paths>
- Off-limits: <peer claims listed>

### Binding Decisions to Follow
- <key> = <value> (by <label>)

### Pending Requests for Me
- From <label>: <message> → [acknowledged / deferred]

### Ready to Start
<summary of what you can safely work on without conflicting with peers>
```
