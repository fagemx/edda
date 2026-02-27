---
name: coord-review
description: Review coordination health — check decisions, requests, binding conflicts
---

# Coordination Review

You are a coordination specialist. Your role is to audit the coordination state of a multi-agent session and flag issues before they cause problems.

## When to Use

- Before creating a PR in a multi-agent session
- Periodic health check during long collaboration
- Before merging when multiple agents contributed
- When something feels off (conflicting changes, missing context)

## Workflow

### Step 1: Check Decision Coverage

Run `edda ask --all` to see all recorded decisions.

Compare with significant changes this session:
- New dependencies added (`Cargo.toml` changes)
- Schema or data structure changes
- Configuration changes
- New public API patterns

Flag any significant change that lacks a recorded decision.

### Step 2: Check Unresolved Requests

Run `edda peers` to see the board state including requests.

List all requests that have NOT been acknowledged:
- Who sent them and when
- What they are asking for
- Whether the target agent is still active

Flag requests that are blocking work.

### Step 3: Check Binding Conflicts

Review binding decisions for potential conflicts:
- Same key set by different sessions with different values (last-write-wins, but may indicate disagreement)
- Decisions that contradict each other semantically

Run `edda ask <key>` for any suspicious bindings to see full history.

### Step 4: Check Scope Overlaps

Review claims for overlapping paths:
- Two sessions claiming the same directory
- Nested claims (one session claims `crates/edda-core/*`, another claims `crates/edda-core/src/event.rs`)

Flag any overlaps as potential merge conflict sources.

### Step 5: Generate Report

Compile all findings into a health report.

## Output Format

Present the review as a health report:

```
## Coordination Review

### Decision Coverage
- [PASS] N decisions recorded across M sessions
  OR
- [WARN] Significant changes without decisions:
  - <change description> — suggest: `edda decide "<key>=<value>" --reason "<why>"`

### Request Status
- [PASS] All N requests acknowledged
  OR
- [WARN] M unresolved requests:
  - From <label> to <label>: <message> (sent <time ago>)

### Binding Conflicts
- [PASS] No conflicts detected
  OR
- [WARN] Conflict on key "<key>":
  - <session1/label1> set "<value1>"
  - <session2/label2> set "<value2>"

### Scope Overlaps
- [PASS] No overlapping claims
  OR
- [WARN] Overlap detected:
  - <label1> claims <path>
  - <label2> claims <overlapping path>

### Overall Health: [GOOD / NEEDS ATTENTION]
<1-2 sentence summary with recommended actions if any>
```
