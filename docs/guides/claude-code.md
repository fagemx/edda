---
title: Claude Code Integration
---

# Claude Code Integration

Edda integrates with Claude Code through lifecycle hooks. After `edda init`, everything runs automatically.

## Setup

```bash
edda init
```

This installs 5 hooks into `.claude/settings.local.json`:

| Hook | Event | What it does |
|------|-------|-------------|
| SessionStart | Session begins | Digests previous session, injects context |
| UserPromptSubmit | Each prompt | Updates peer heartbeat, injects peer status |
| PreToolUse | Before tool call | Auto-approves edda commands, provides patterns |
| PostToolUse | After tool call | Captures command output, file changes |
| PostToolUseFailure | Tool fails | Records failed commands with exit codes |

## Context injection

At session start, Edda builds a context snapshot and injects it into the agent's system prompt. The agent sees:

- **Recent decisions** from all prior sessions
- **Previous session digest** (what was done, what's next)
- **Active tasks and open threads**
- **Peer activity** (if multi-agent)

Example of what the agent receives:

```markdown
# edda memory pack (hot)

- session_id: 8700aa78-02f1-49c8-93de-6a653cb3bce0
- git_branch: main
- turns: 12

## Recent Turns (deterministic)
### Turn 1 (newest first)
- User: "Add JWT auth middleware"
  - ToolUse: Edit file=src/auth/middleware.rs

## Binding Decisions
- db.engine: sqlite (cli)
- auth.strategy: JWT (cli)
```

View the full context snapshot at any time:

```bash
edda context
```

## Session digests

When a session ends (or the next session starts), Edda automatically analyzes the transcript and extracts:

- **Commits made** during the session
- **Failed commands** with exit codes
- **Files modified** and edit counts
- **Session summary** for the next session

These are stored as structured events in the ledger — no manual input required.

## Multi-agent coordination

When multiple Claude Code sessions work on the same repo, Edda coordinates them automatically.

### Peer discovery

Every session writes a heartbeat file. At each prompt, Edda checks for active peers and injects their status:

```markdown
## Peers (1 active)
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs
```

### Scope claims

Agents can claim ownership of file paths to prevent conflicts:

```bash
edda claim "auth" --paths "src/auth/*"
```

Other agents see claimed paths as off-limits:

```markdown
### Off-limits (other agents active)
- src/auth/* -> Agent auth (30s ago)
```

### Binding decisions

When one agent makes a decision that affects everyone:

```bash
edda decide "db.engine=postgres" --reason "JSONB support needed"
```

All other active sessions see this immediately. If another agent tries to decide differently on the same key, Edda warns about the conflict.

### Peer messaging

Agents can send requests to each other:

```bash
edda request "billing" "Please expose the invoice total as a public method"
```

### Monitoring

```bash
edda watch
```

Opens a real-time TUI showing all active sessions, their current activity, and coordination state.

## Manual commands

Most of the time hooks handle everything. These commands are available when you want to record something manually:

```bash
# Record a decision
edda decide "cache.strategy=redis" --reason "need TTL and pub/sub"

# Record a note
edda note "completed auth refactor; next: add rate limiting" --tag session

# Query past decisions
edda ask "cache"

# Search transcripts
edda search query "auth middleware"

# View event log
edda log --tag decision
```

## Troubleshooting

### Verify hooks are installed

```bash
edda doctor
```

Checks hook installation, ledger integrity, and workspace configuration.

### Reinstall hooks

```bash
edda bridge claude install
```

### Check workspace status

```bash
edda status
```
