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
hooks), release the scope by naming it:

```bash
edda unclaim --session <id>
```

The id is the `session:` line `edda claim` printed. Run it without `--session`
and the refusal lists every claim with its session id, so the value you need is
in the error. `edda peers --json` carries it too, under `claims[].session_id`;
plain `edda peers` does not, because it lists live heartbeats and a bare-CLI
claim has none.

`unclaim` deliberately does not pick a claim for you when you have no session
of your own: it cannot tell whose claim it is, and releasing the wrong one
would drop the off-limits protection a live peer is relying on.

`unclaim` never reports success for a session that holds nothing — if it prints
a released scope, that scope is gone.

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
