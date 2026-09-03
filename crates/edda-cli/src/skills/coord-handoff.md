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
hooks), release the scope by naming it. The id is the `session:` line
`edda claim` printed:

```bash edda-doctest
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda unclaim --session cli-auth
> Unclaimed scope for session: cli-auth
```

Omit `--session` and it refuses rather than guessing, listing every claim with
its id — so the value you need is in the error:

```bash edda-doctest
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda unclaim
! cannot tell which claim is yours
! cli-auth
```

That refusal is deliberate. A caller with no session of its own cannot tell
whose claim it is, and releasing the wrong one would drop the off-limits
protection a live peer is relying on.

`unclaim` also never reports success for a session that holds nothing — if it
prints a released scope, that scope is gone:

```bash edda-doctest
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda unclaim --session cli-nobody
! holds no claim
```

In CI, where teardown runs whether or not a claim was made, add `--if-claimed`
so having nothing to release is not a failure. It still names which of the two
nothings it found — no claim it could identify as yours, or a session you named
that holds none — because a blanket success is the false report this verb exists
to remove:

```bash edda-doctest
$ edda unclaim --if-claimed
> Released nothing
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda unclaim --session cli-nobody --if-claimed
> Nothing to unclaim for session cli-nobody
```

`edda peers --json` carries the id too, under `claims[].session_id`. Plain
`edda peers` does not, because it lists live heartbeats and a bare-CLI claim
has none:

```bash edda-doctest
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda peers --json
> "session_id": "cli-auth"
$ edda peers
> No active sessions
```

A bare CLI claim is machine-visible for as long as it stands, not for a
freshness window: the claimant is a one-shot process, so no heartbeat age can
prove it gone, and `edda claim check` counts the claim — exit 1, and
`unjudgeable_claims` in `--json` — until it is unclaimed (GH-705). That is
deliberately fail-closed: the gate never reads an occupied surface as clear.

```bash edda-doctest
$ edda claim "auth" --paths "src/auth/*"
> session: cli-auth
$ edda claim check "src/auth/login.rs"
! bare-CLI claim, liveness cannot be judged
```

So release the scope when the work is done (Step 5 above); until then the
occupation is visible to every peer. Hooked sessions are the other shape:
their claims age out with their heartbeats, so a claim left behind by a dead
session does not block a peer.

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
