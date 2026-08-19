---
name: coord-handoff
description: Clean handoff when finishing multi-agent work — summarize, decide, unclaim
---

# Coordination Handoff

You are a coordination specialist. Your role is to help the agent cleanly exit a multi-agent session by summarizing work, recording decisions, and preparing peers for continuation.

## When to Use

- Finishing a task in a multi-agent session
- Before session end when peers will continue working
- When switching to a different task and releasing your current scope

## Workflow

### Step 1: Summarize Changes

Run these commands to understand what changed:

```bash
git diff --stat
git log --oneline $(git merge-base HEAD main)..HEAD
```

Summarize: how many commits, which files, what was the goal.

### Step 2: List Unfinished Items

Check for incomplete work:
- Search modified files for `TODO` and `FIXME` comments
- Review the task list for incomplete items
- Note any partial implementations or known issues

### Step 3: Record Decisions

For each architectural choice made this session that other agents need to know:

```bash
edda decide "<domain.key>=<value>" --reason "<why>"
```

Focus on:
- Library/framework choices
- API design decisions
- Schema or data structure changes
- Configuration choices

Skip: formatting changes, test additions, minor refactors.

### Step 4: Post Session Note

Summarize the session for peers:

```bash
edda note "<summary>" --tag session
```

Include:
- What was completed
- What decisions were made
- What remains for the next session or other agents

### Step 5: Scope Release

Where host bridge hooks are wired, unclaim happens automatically on session
end — nothing to do.

Where they are not (claims made from a bare CLI, CI, or a host without bridge
hooks), release the scope explicitly — and **pass the session id**:

```bash
edda unclaim --session <id>
```

The id is the `session:` line `edda claim` printed; `edda peers --json` also
carries it. Plain `edda peers` will not: it lists live heartbeats, which a
bare-CLI claim does not have, and it abbreviates ids. Without hooks there is no
heartbeat to infer from either, so bare `edda unclaim` falls back to the
session `cli-cli` rather than the `cli-<label>` your claim created: it exits 0,
prints a reassuring line, and releases nothing (GH-455).

Enforcement stays safe either way: every consumer joins claims against live
heartbeats, so a claim left behind by a dead session does not block a peer.

## Output Format

Present the handoff summary:

```
## Coordination Handoff

### Changes Made
- N commits, M files modified
- <1-2 sentence summary of work done>

### Decisions Recorded
- <key> = <value> (reason)

### Unfinished Work
- <items remaining, with context for the next agent>

### Session Note
> <the note posted via edda note>

### Scope Released
- <label> will be unclaimed on session end (peers can then work in <paths>)
```
