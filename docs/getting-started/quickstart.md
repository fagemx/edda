---
title: Quick Start
---

# Quick Start

## Initialize

```bash
cd your-project
edda init
```

`edda init` does three things:

1. Creates `.edda/` with an empty SQLite ledger
2. Installs lifecycle hooks into `.claude/settings.local.json`
3. Adds decision-tracking instructions to `.claude/CLAUDE.md`

That's it. Start a Claude Code session and Edda runs in the background.

## What happens automatically

| When | What Edda does |
|------|---------------|
| Session starts | Digests previous session, injects prior context |
| Agent runs commands | Hooks capture activity (commands, edits, errors) |
| Session ends | Writes session digest to the ledger |
| Next session starts | Agent sees relevant decisions from all prior sessions |

You don't need to do anything. Hooks handle the entire lifecycle.

## Inspect memory

When you want to see what Edda remembers:

```bash
# See what the agent sees at session start
edda context

# Query past decisions
edda ask "cache"

# View event log
edda log

# Filter by tag
edda log --tag decision

# Full-text search across transcripts
edda search query "auth middleware"
```

## Real-time monitoring

```bash
edda watch
```

Opens a terminal UI showing active sessions, recent events, and coordination state.

## Health check

```bash
edda doctor
```

Verifies ledger integrity, hook installation, and workspace configuration.

## Next step

Read the [Claude Code integration guide](../guides/claude-code.md) for details on hooks, context injection, and multi-agent coordination.
