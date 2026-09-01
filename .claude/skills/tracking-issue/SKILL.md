---
name: tracking-issue
description: Analyze complex problems structurally — map actors, find root causes, create tracking issue with parallel task decomposition
context: fork
---

# Tracking Issue — Structural Problem Analysis

When you encounter a complex bug or architectural problem that touches multiple modules, don't jump to fixing. First build a complete mental model, then create a structured tracking issue that decomposes the problem into parallel fixable tasks.

**Inspired by**: vm0 lancy's pattern — "畫完全景再動手"

Your args are: `$ARGUMENTS`

Parse the args:
- A description of the problem, bug report, or area of concern
- Optionally a GitHub issue number to investigate

---

## Phase 1: Map the System

### 1a: Identify All Actors

List every component/module/service that participates in the problem area.

```markdown
### Actors in the system
| Actor | Role | Entry point |
|-------|------|-------------|
| API handler | Creates/triggers the operation | `src/routes/X.ts` |
| Engine | Executes business logic | `src/X-engine.ts` |
| DB layer | Persists state | `src/db.ts` |
| Background job | Async processing | `src/loop-runner.ts` |
| External service | Bridge call | `src/X-bridge.ts` |
```

For each actor, find the actual code:
```bash
# Search for the relevant entry points
grep -rn "function\|class\|export" src/ --include="*.ts" | grep -i "<keyword>"
```

### 1b: Trace All Interaction Paths

For each pair of actors that interact, trace the code path:

```markdown
### Interaction paths
| From | To | Trigger | Code path |
|------|----|---------|-----------|
| API | Engine | POST /api/X | routes/X.ts:25 → engine.doX() |
| Engine | DB | state change | engine.ts:50 → db.run() |
| Cron | Engine | timer | loop-runner.ts:100 → engine.check() |
```

### 1c: Draw Timeline

For the problem scenario, draw a timeline showing what happens and where things go wrong:

```markdown
### Race/Failure Timeline

T0:     Actor A does X (state = S1)
T0+1s:  Actor B reads state (sees S1)
T0+2s:  Actor A completes (state = S2)
T0+3s:  Actor B acts on stale S1 ← BUG: should see S2
Result: Inconsistent state
```

---

## Phase 2: Find Structural Root Causes

Don't list symptoms. Find the **structural patterns** that cause the problems.

Common structural root causes:
- **Missing guards**: state transitions without `WHERE status = ?` conditions
- **Dimension mismatch**: checking per-X but enforcing per-Y
- **Stale reads**: reading state outside transaction, acting later
- **Missing atomicity**: multi-step mutation without transaction
- **Implicit ordering**: assuming A happens before B without enforcement
- **Missing heartbeat**: long operation without liveness signal

For each root cause found:
```markdown
### Root Cause N: <name>

**Pattern**: <what's structurally wrong>
**Evidence**: <file:line references>
**Blast radius**: <which actors/paths are affected>
**Fix pattern**: <one-sentence description of the fix approach>
```

---

## Phase 3: Design Fix Strategy

### 3a: Define the Reference Pattern

Find the ONE place in the codebase where this problem is already handled correctly. That's the reference pattern to generalize.

```markdown
### Reference Pattern
<file:line> already does this correctly because:
- It uses conditional update: `WHERE status IN (...)`
- It runs inside a transaction
- It checks return value

The fix generalizes this pattern to all N other sites.
```

### 3b: Decompose into Tasks

Each task must be:
- **Independent** — can be developed and merged without other tasks
- **Verifiable** — has a concrete test or check
- **Right-sized** — completable in one session

```markdown
## Tasks

| # | Issue | Description | Fixes | Independent? |
|---|-------|-------------|-------|-------------|
| 1 | #NNN | Add guards to all state transitions | #bug1, #bug2 | Yes |
| 2 | #NNN | Align dimension X with Y | #bug3 | Yes |
| 3 | #NNN | Add heartbeat to long pipeline | #bug4 | Yes (enhanced by #1) |
```

### 3c: Dependency & Ordering

```markdown
## Dependency & Ordering

All N tasks are **independent** and can be developed in parallel.

[If there are dependencies:]
Task 1 provides the strongest safety net and should be prioritized —
it prevents the widest class of bugs. Task 3's effectiveness is
enhanced by Task 1 (defense-in-depth).
```

---

## Phase 4: Create the Tracking Issue

Create the GitHub tracking issue:

```bash
gh issue create --title "fix: <problem area> — tracking issue" --body "$(cat <<'EOF'
## Overview

<2-3 sentences: what's broken and why>

<N> bug issues identified. They stem from <M> structural problems:

1. **<Root cause 1>** — <one sentence>
2. **<Root cause 2>** — <one sentence>

## Architecture Context

### Actors in the system
<table from Phase 1>

### Core pattern that works correctly
<reference pattern from Phase 3a>

## Tasks

<table from Phase 3b>

## Dependency & Ordering

<from Phase 3c>

## Bug Issues

| Bug Issue | Severity | Description | Fixed By |
|-----------|----------|-------------|----------|
| #NNN | Medium | ... | Task 1 |

EOF
)"
```

Then create individual task issues:

```bash
gh issue create --title "fix: <task description>" --body "..."
```

---

## Phase 5: Report

Display summary to user:

```markdown
## Tracking Issue Created

**Tracking**: #NNN — <title>
**Root causes**: N structural problems identified
**Tasks**: M independent fix tasks created
**Bug coverage**: K bug issues will be fixed

### Root Causes
1. <name> — <one sentence>

### Tasks (can run in parallel)
- #NNN — <description>

### Recommended order
<which to do first and why>
```

---

## Key Principles

1. **Never fix symptoms** — always find the structural root cause
2. **One pattern to fix many bugs** — if you're writing N different fixes for N bugs, you haven't found the pattern yet
3. **Reference from existing code** — the codebase usually already has the right pattern somewhere, just not everywhere
4. **Independence enables parallelism** — design tasks so they can be merged in any order
5. **Tracking issue is the source of truth** — all context lives there, not in individual bug issues
