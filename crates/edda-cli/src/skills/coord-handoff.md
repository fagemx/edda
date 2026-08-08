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

Where the Claude Code hooks are wired, unclaim happens automatically on
session end via the SessionEnd hook — nothing to do.

Where they are not (claims made from a bare CLI, CI, or a host without the
bridge hooks), the claim outlives the session: there is currently no
`edda unclaim` verb to release it by hand (GH-455). In that case, say so in
your handoff summary — peers reading the board should know the claim is dead
even though it still folds. Enforcement stays safe either way: every consumer
joins claims against live heartbeats.

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
