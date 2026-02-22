---
title: "Introducing Edda: Decision Memory for Claude Code"
date: "2026-02-22"
author: "fagemx"
category: "Product Launch"
summary: "Edda adds cross-session decision memory to Claude Code by storing key decisions and rationale in a local, deterministic ledger."
---

# Introducing Edda: Decision Memory for Claude Code

The missing layer for agent workflows is not more context. It is decision continuity.

Claude Code can compact context within a session, but important decisions can still get buried in transcript noise, and session context does not persist across sessions by default. That creates a familiar failure mode for multi-session work:

- the agent forgets why a choice was made
- rejected paths are retried
- summaries lose rationale and tradeoffs
- momentum resets between sessions

Today I am launching **Edda**, a local-first decision memory system for Claude Code.

Edda extracts key decisions and their rationale, stores them in a local ledger, and restores relevant context when the next session starts. The goal is simple: let your agent continue work with continuity instead of re-deriving project history from scratch.

## Why Edda Exists

Context compaction solves a token problem. It does not fully solve a continuity problem.

When you build software across multiple agent sessions, the most valuable information is often not raw transcript text. It is:

- what was decided
- why it was decided
- what alternatives were rejected
- what the current direction is

That is the layer Edda focuses on.

Instead of trying to preserve everything, Edda preserves the parts that matter most for future execution: decisions, rationale, notes, and session digests.

## What Edda Does

At a high level, Edda adds an automatic memory loop around your agent workflow:

1. During a session, hooks capture activity — commands run, files changed, decisions made.
2. At session end, Edda digests the transcript and stores structured events in a local ledger.
3. At the next session start, Edda assembles relevant prior context and injects it back.

This gives the agent a working memory of project decisions across sessions without requiring a hosted memory service, embeddings pipeline, or repeated LLM summarization.

## Design Principles

Edda is built around a few constraints:

### 1) Local-first

All data stays in `.edda/` in your workspace (SQLite + local files). No cloud service, no accounts, no background sync.

### 2) Deterministic Retrieval

Edda uses structured data and local querying instead of opaque semantic retrieval as the default path. The same query should produce the same result.

### 3) Preserve the Why

A summary that drops rationale is often not enough for future work. Edda is designed to preserve reasoning, not just outcomes.

### 4) Agent-native Workflow

Edda is meant to run in the background with hooks and lightweight commands, so the workflow feels automatic rather than "another tool to maintain."

## How It Works (Today)

Edda integrates with Claude Code and supports OpenClaw / MCP clients.

For Claude Code, `edda init` installs lifecycle hooks that run automatically. Hooks capture session activity, digest transcripts, and inject prior context — no manual intervention needed.

The ledger stores events such as:

- session digests (automatic, from transcript analysis)
- decisions and notes (recorded during work)
- command outputs (captured by hooks)

These events are stored in a local SQLite-backed ledger with hash-chained records for integrity.

At session start, Edda builds a context snapshot from the ledger so the agent can see relevant prior decisions before continuing work.

## Quick Start

```bash
edda init
```

That is enough to get Edda running in the background for Claude Code in a typical setup.

Useful commands when you want to inspect or query memory directly:

```bash
edda ask "cache"
edda context
edda log --tag decision
```

## What Edda Is (and Is Not)

Edda is not a generic chat memory layer that stores everything and hopes retrieval works later.

Edda is a **decision memory system**: it prioritizes preserving technical decisions, rationale, and continuity across sessions.

It also does not require embeddings, a vector database, or recurring LLM calls to function. For many workflows, that makes it easier to reason about, cheaper to run, and simpler to trust.

## Why Launch Now

The pace of agent-assisted development is increasing, but cross-session continuity is still weak in many real workflows.

This is a good time to make the "decision ledger" approach explicit and practical:

- local enough to use immediately
- structured enough to query
- lightweight enough to run in the background

The current release is intentionally focused: **Claude Code first**, with OpenClaw and MCP client support already available as part of the broader direction.

## Multi-Agent Coordination

Edda also solves a related problem: when multiple Claude Code agents work on the same repo simultaneously, they have no awareness of each other.

Edda adds lightweight coordination — peer discovery, scope claims, and shared binding decisions — so agents can see what others are working on and avoid conflicts. This runs entirely through local files with zero configuration beyond `edda init`.

See the [companion post on multi-agent coordination](/blog/2026-02-23-multi-agent-coordination) for details.

## What's Next

Near-term work includes:

- improving context recall quality and evaluation
- multi-agent coordination improvements (hard enforcement, cross-machine)
- expanding cross-project search and retrieval workflows
- deeper integrations across agent clients

The long-term direction is broader than one client: Edda is building toward a reusable decision memory and coordination layer for agentic engineering.

## Try Edda

If you build with Claude Code across multiple sessions, Edda should make your workflow more consistent immediately.

- GitHub: `https://github.com/fagemx/edda`
- Quick start: run `edda init`

Feedback and issues are welcome.
