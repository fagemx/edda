---
title: Claude Code Integration
---

# Claude Code Integration

Edda integrates with Claude Code through lifecycle hooks. After `edda init`, everything runs automatically.

## Setup

```bash
edda init
```

This installs 12 hooks into `.claude/settings.local.json` (the full event list
is `HOOK_EVENTS` in `crates/edda-bridge-claude/src/admin.rs`; each row's
behavior is the corresponding `hook_event_name` match arm in
`crates/edda-bridge-claude/src/dispatch/mod.rs`):

| Event | When it fires | What edda does |
|-------|---------------|----------------|
| PreToolUse | Before each tool call | Auto-approves `edda` commands (on by default; the `EDDA_CLAUDE_AUTO_APPROVE` env var gates it), blocks risky ones with a reason, and attaches pattern, learned-rule, and pending peer-request context |
| PostToolUse | After each tool call | Captures command output and file changes as signals and auto-writes commit/merge events to the ledger; may inject a write-back nudge via `additionalContext` (cooldown-gated) |
| PostToolUseFailure | A tool call fails | Installed, but the dispatch arm currently returns an empty result — no hook output. The raw payload is still recorded in the append-only session ledger |
| SessionStart | Session begins | Auto-digests previous sessions, ingests the transcript, builds the full hot pack (turns + workspace), ensures the peer heartbeat, and injects the pack via `additionalContext` |
| UserPromptSubmit | Each prompt | Injects lightweight workspace context (~2.5K budget) with peer status and a coordination diff; after a compaction it only re-ingests instead (the full pack was already injected by the preceding `SessionStart:compact` hook) |
| PreCompact | Before context compaction | Side-effect only: ingests and rebuilds the pack and flags compaction pending; returns no output because Claude Code's hook schema does not allow `hookSpecificOutput` on this event. The next `SessionStart:compact` consumes the rebuilt pack |
| SessionEnd | Session ends | Auto-digests the transcript and cleans up: removes the peer heartbeat, releases scope claims, resets per-session state |
| Stop | Assistant turn ends | Delivers the task-rail nudge through the block/reason channel (this event cannot inject context); watermarked to at most once per task per session |
| SubagentStart | A sub-agent spawns | Injects active-peer context via `additionalContext` before writing the sub-agent's heartbeat, so the sub-agent does not see itself in the peer list |
| SubagentStop | A sub-agent finishes | Records the sub-agent's completion (summary, files touched, decisions, commits), optionally writes a ledger note event, removes its heartbeat |
| TaskCompleted | A task completes | Records task completion in the coordination state and optionally writes a ledger note event |
| TeammateIdle | A teammate goes idle | Marks the teammate's phase as idle and writes an idle event to `coordination.jsonl` |

Independently of the per-event behavior above, every hook invocation is
appended to the append-only session ledger before dispatch.

## Context injection

When Edda injects context, it returns a `hookSpecificOutput.additionalContext`
payload to Claude Code (wrapped in `edda:` boundary markers). Claude Code
decides where that content lands — Edda controls what is returned, not where
the host places it. The injected pack includes:

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

Reasoning checkpoints are durable, vendor-neutral ledger records. Record one with
`edda checkpoint --hypothesis "..." --rejected "hypothesis|reason" --open "..." --next "..."`.
The hot pack includes the latest checkpoint under `Open Checkpoints`; budget
degradation drops complete sections and reports the omitted-item count.

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
