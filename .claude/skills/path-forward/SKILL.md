---
name: path-forward
description: "Find the most unblocking next step given current codebase state and goals"
---

# Path Forward

Find the single next action that unblocks the most downstream work. Use this after completing a task, when stuck on priorities, or when returning to the project after a break.

## Usage

```
path-forward                      # What should I work on next?
path-forward completed <task>     # I just finished <task>, what's unblocked?
path-forward stuck <problem>      # I'm blocked, find an alternate path
path-forward status               # Show current state of all tracks
```

## Core Principle

**Maximum unblocking, not minimum effort.**

The right next step is the one that enables the MOST subsequent work. This is different from:
- "Easiest task" — easy tasks may not unblock anything
- "Most important feature" — importance doesn't mean buildable now
- "What the docs say is next" — docs may not reflect actual code state

## Operations

### Default: What Should I Do Next?

#### Step 1: Scan Current State

Check each crate's implementation status:

```bash
# For each crate, check: lines of code, stub count, test count
for crate in edda-core edda-ledger edda-store edda-transcript edda-index edda-derive edda-pack edda-bridge-claude edda-mcp edda-search-fts edda-cli; do
  echo "=== $crate ==="
  # Actual code lines (excluding blanks and comments)
  find crates/$crate/src -name "*.rs" 2>/dev/null | xargs wc -l 2>/dev/null | tail -1
  # Stub count
  grep -rn "todo!\|unimplemented!" crates/$crate/src/ 2>/dev/null | wc -l
  # Test count
  grep -rn "#\[test\]\|#\[tokio::test\]" crates/$crate/ 2>/dev/null | wc -l
done
```

```bash
# Overall health
cargo check 2>&1 | tail -5
cargo test 2>&1 | tail -10
```

#### Step 2: Map What's Blocked

Read project docs for the dependency graph and critical path:

```
Transcript ingest → Event ledger → Derive views → Pack generation → Bridge injection
```

Crate dependency order:
```
edda-core (types, events)
  → edda-store (file storage)
  → edda-ledger (append-only event log)
  → edda-transcript (Claude transcript parsing)
  → edda-index (event indexing)
  → edda-derive (view derivation from events)
  → edda-pack (memory pack generation)
  → edda-search-fts (full-text search)
  → edda-bridge-claude (Claude Code hook injection)
  → edda-mcp (MCP server)
  → edda-cli (CLI entry point)
```

For each incomplete track, determine:
- **Blocked by**: upstream crates that aren't done
- **Blocks**: downstream crates waiting on this
- **Downstream count**: total tasks that this enables (directly + transitively)

#### Step 3: Score Candidates

For each candidate next task:

```
Unblocking Score = (downstream tasks enabled) × (prerequisite readiness)
```

- Task enabling 5 downstream tasks > task enabling 1
- Task with 100% prerequisites ready > task with 50%
- Tie-breaker: prefer critical path (Transcript → Ledger → Derive → Pack → Bridge)

#### Step 4: Verify Top Candidate

For the highest-scoring candidate, quick ground-check:
- Do required types exist?
- Do required crate APIs exist?
- Does it compile with the prerequisite crates?

If not buildable, move to next candidate.

#### Step 5: Report

```markdown
## Path Forward

### Current State

| Crate | LOC | Tests | Stubs | Status |
|-------|-----|-------|-------|--------|
| edda-core | 450 | 12 | 0 | Complete |
| edda-ledger | 230 | 5 | 3 | In Progress |
| edda-store | 180 | 8 | 0 | Complete |
| edda-transcript | 300 | 10 | 2 | In Progress |
| ... | ... | ... | ... | ... |

### Recommended Next Step

**<task description>**

Why this one:
- Prerequisites: <what's already done>
- Unblocks: <what this enables>
- Readiness: <evidence it's buildable now>
- Scope: <estimated size>

### After That

1. <next priority> — unblocks <what>
2. <next priority> — unblocks <what>
3. <next priority> — unblocks <what>
```

---

### Operation: completed <task>

After finishing a task:

1. **Verify completion**: compile + tests pass for the changed crate
2. **Check what's newly unblocked**: which downstream tasks now have all prerequisites met?
3. **Recommend the newly-unblocked task with highest unblocking score**

```markdown
## Completed: <task>

### Newly Unblocked
- <task> (was waiting on <prerequisite> — now done)
- <task> (was waiting on <prerequisite> — now done)

### Recommended Next: <task>
- Just unblocked by your completion of <previous>
- Unblocks: <downstream tasks>
- All prerequisites now met
```

---

### Operation: stuck <problem>

When blocked on something:

1. **Identify the blocker** — what specifically can't you do and why?
2. **Find alternative paths**:
   - Can you achieve the same goal differently?
   - Can you do 80% and defer the blocked part?
   - Can you mock/stub the blocker temporarily?
3. **Find the unblocking path** — what would remove the blocker?

```markdown
## Stuck on: <problem>

### Blocker Analysis
<What's blocking and why>

### Alternative Paths
1. **Work around it**: <approach that avoids the blocker>
2. **Partial implementation**: <do what you can, stub the rest>
3. **Unblock it**: <prerequisite work to remove the blocker>

### Recommended Path
<Which alternative and why>
```

---

### Operation: status

Quick overview, no recommendations:

```markdown
## Project Status

| Crate | LOC | Tests | Stubs | Status |
|-------|-----|-------|-------|--------|
| edda-core | 450 | 12 | 0 | Complete |
| edda-ledger | 230 | 5 | 3 | In Progress |
| edda-store | 180 | 8 | 0 | Complete |
| ... | ... | ... | ... | ... |

### Build Health
- Compilation: pass/fail
- Tests: X passing, Y failing

### Critical Path Progress
core [===] → ledger [== ] → derive [   ] → pack [   ] → bridge [   ]
```

## Scale Awareness

The skill adapts its recommendations to the project's growth phase:

**Early phase** (most crates <100 LOC, many stubs):
- Focus on foundation crates (core, store, ledger)
- Recommend groundwork that enables the most downstream crates
- Prefer depth (finish one crate) over breadth (start many)

**Mid phase** (core crates implemented, bridge/mcp have stubs):
- Focus on integration (derive, pack, bridge)
- Recommend connecting implemented pieces
- Prefer end-to-end paths over isolated features

**Late phase** (all crates compile and have tests):
- Focus on polish (CLI, MCP, search)
- Recommend end-to-end validation and edge cases
- Prefer hardening over new features

**Ongoing development** (post-initial build):
- Focus on what the user is actively working on
- Check recent git history to understand current direction
- Recommend based on what was just completed and what's in flight

Phase detection:
```bash
# Check LOC distribution across crates
for crate in crates/*/; do
  echo "$(basename $crate): $(find $crate/src -name '*.rs' 2>/dev/null | xargs wc -l 2>/dev/null | tail -1)"
done
```

## What This Skill Does NOT Do

- Does NOT check if a specific task is ready (that's `/ground-check`)
- Does NOT create issues or PRs
- Does NOT modify code
- Does NOT question architecture decisions — works within them

## References

- Project scope: `docs/SCOPE_V1.md`
- Roadmap: `docs/ROADMAP_V1_1.md`
- Bridge contract: `docs/BRIDGE_CONTRACT_P1.md`
- Critical path: Transcript → Ledger → Derive → Pack → Bridge
