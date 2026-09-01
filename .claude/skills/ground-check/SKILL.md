---
name: ground-check
description: "Check if codebase supports a planned task — what's ready, what's close, what's missing"
---

# Ground Check

Before starting any task, verify what the codebase actually supports today. This skill bridges the gap between plans and executable reality.

## Usage

```
ground-check <task>
```

Where `<task>` is any of:
- A track subtask: `A2` or `Track A, Ledger Store`
- A task description: `"implement hook entrypoint dispatch"`
- A design doc path: `docs/plan/TRACK_H_AGENT_BRIDGE/H1_STORE_AND_HOOKS.md`
- An issue number: `#15`
- A feature idea: `"add transcript cursor-based delta ingest"`

## What This Skill Does

1. **Reads the task** — understands what needs to be built
2. **Scans actual code** — checks what exists today in the codebase
3. **Reports readiness** — concrete findings, not theoretical analysis
4. **Identifies the gap** — when something isn't ready, names the specific missing piece

## Output Categories

### Ready (can start now)

Infrastructure exists. You can start coding this today.
- Required types/structs are defined
- Required crate APIs are implemented (not stubs)
- Required file store layout exists
- Dependencies compile
- Prerequisite work is done

### Almost (one piece away)

80%+ foundation exists, but one specific thing is missing.
- Report EXACTLY what's missing (file, function, type, crate)
- Estimate effort to fill the gap (trivial / small / medium)
- Suggest: fill the gap first, or work around it?

### Not Ready (foundation missing)

Prerequisite work hasn't been done yet.
- Identify which prerequisite needs to be done first
- Trace the dependency chain back to something that IS ready
- Report the shortest path from "ready" to "this task"

## Scan Procedure

### Step 1: Parse the Task

Read the task definition. Extract:
- **Required types**: What structs, enums, traits does this need?
- **Required crates**: Which crates does this depend on?
- **Required crate APIs**: What functions/methods must be callable?
- **Required file layouts**: What `.edda/` or `~/.edda/` structures must exist?
- **Required CONTRACT rules**: What architecture constraints apply?
- **Required prior work**: What must be built first?

If the task references a track subtask (e.g., `B2`), read the corresponding doc:
`docs/plan/TRACK_<letter>_<name>/<subtask>.md`

Track directory mapping:
- `docs/plan/TRACK_A_LEDGER/` — A1, A2, A3
- `docs/plan/TRACK_B_COMMIT_VIEWS/` — B1, B2, B3
- `docs/plan/TRACK_C_BRANCH_MERGE/` — C1, C2
- `docs/plan/TRACK_D_AUTO_EVIDENCE/` — D1
- `docs/plan/TRACK_E_DRAFT_SYSTEM/` — E1, E2
- `docs/plan/TRACK_F_POLICY_GATE/` — F1
- `docs/plan/TRACK_G_APPROVAL_ROUTING/` — G1, G2
- `docs/plan/TRACK_H_AGENT_BRIDGE/` — H1, H2, H3, H4

If the task is a description, infer requirements from the description and cross-reference with `docs/plan/00_OVERVIEW.md` and crate structure.

### Step 2: Check Each Requirement Against Code

For each requirement, verify in the actual codebase:

**Types & Traits**
```bash
# Does the type exist?
grep -rn "pub struct <TypeName>" crates/
grep -rn "pub enum <TypeName>" crates/
grep -rn "pub trait <TraitName>" crates/

# Is it a stub or real implementation?
# Look for todo!(), unimplemented!(), or empty bodies
```

**Crate Existence**
```bash
# Does the crate exist in workspace?
ls crates/<crate-name>/src/lib.rs

# Is it listed in workspace Cargo.toml?
grep "<crate-name>" Cargo.toml
```

**Crate APIs**
```bash
# Does the crate export what we need?
grep -rn "pub fn\|pub async fn" crates/<crate>/src/

# Is it implemented or stubbed?
grep -rn "todo!\|unimplemented!" crates/<crate>/src/

# Does it compile?
cargo check -p <crate> 2>&1
```

**File Store Layout**
```bash
# Per-repo store (.edda/) — does init create the expected structure?
grep -rn "create_dir_all\|ensure_dirs" crates/edda-ledger/src/
grep -rn "create_dir_all\|ensure_dirs" crates/edda-store/src/

# Per-user store (~/.edda/) — does the store layout exist?
grep -rn "store_root\|project_dir" crates/edda-store/src/
```

**Event Builders**
```bash
# Does the event builder exist?
grep -rn "pub fn new_<type>_event" crates/edda-core/src/event.rs

# Does the event type appear in the registry?
grep -rn "\"<event_type>\"" crates/edda-core/src/
```

**CONTRACT Rule Compliance**
```bash
# Check specific CONTRACT rules relevant to this task
# See docs/plan/CONTRACT.md for full rule table

# HASH-01: hash chain
grep -rn "parent_hash\|compute_event_hash" crates/edda-core/src/

# LEDGER-02: append-only
grep -rn "append(true)\|OpenOptions" crates/edda-ledger/src/

# LOCK-01: exclusive lock
grep -rn "LOCK\|lock_file\|WorkspaceLock" crates/edda-ledger/src/

# VIEW-01: deterministic rebuild
grep -rn "rebuild_branch\|rebuild_all" crates/edda-derive/src/
```

### Step 3: Cross-Reference Dependencies

Check the dependency graph from `docs/plan/TRACKS.md`:
```
A → B → C → D → E → F → (G, H parallel)
```

For each prerequisite:
- Source files exist AND have non-stub implementations
- Tests exist AND pass (or at minimum, code compiles)
- No `todo!()` or `unimplemented!()` in the paths this task will call

### Step 4: Compile & Test

```bash
# Does the project compile?
cargo check 2>&1

# Do tests pass for relevant crates?
cargo test -p <relevant-crate> 2>&1

# Clippy clean? (RUST-01)
cargo clippy -p <relevant-crate> -- -D warnings 2>&1
```

### Step 5: Report

Generate the report:

```markdown
## Ground Check: <task name>

### Prerequisites

| Requirement | Status | Detail |
|------------|--------|--------|
| edda-core Event struct | Ready | Event, Refs, EventId defined in types.rs |
| edda-ledger append | Ready | Ledger::append() in ledger.rs |
| edda-derive rebuild | Almost | rebuild_branch() exists, render_context() missing |
| edda-store project_id | Not Ready | edda-store crate doesn't exist yet |

### CONTRACT Rules

| Rule | Applies | Status |
|------|---------|--------|
| HASH-01 (hash chain) | Yes | Ready — compute_event_hash() implemented |
| LOCK-01 (workspace lock) | Yes | Ready — WorkspaceLock in lock.rs |
| DRAFT-01 (draft not in ledger) | No | N/A for this task |

### Verdict

**Almost ready** — 1 blocker: render_context() not implemented.

### Recommended Action

1. Implement render_context() in edda-derive first → then this task is fully ready
2. OR: start with parts that don't need context output

### Buildable Sub-Pieces

Even without the blocker, these parts are buildable now:
- Event builder for new event type
- CLI subcommand skeleton
- File store layout changes
```

## Ongoing Development Mode

For tasks beyond initial track work (new features, refactoring, bug fixes):

1. **New feature**: Check if the types, file layouts, and crate APIs needed already exist
2. **Refactoring**: Check what depends on the code being refactored (blast radius)
3. **Bug fix**: Check if tests exist for the affected code path
4. **Integration**: Check if both sides of the integration are implemented

The scan procedure is the same — always check actual code, not just docs.

## What This Skill Does NOT Do

- Does NOT suggest what to build (that's `/path-forward`)
- Does NOT create issues or PRs
- Does NOT modify code
- Does NOT evaluate design quality — only infrastructure readiness

## References

- Track definitions: `docs/plan/TRACK_*/`
- Overview + dependency graph: `docs/plan/00_OVERVIEW.md`
- Track breakdown + DAG: `docs/plan/TRACKS.md`
- Architecture constraints: `docs/plan/CONTRACT.md`
- Validation plan: `docs/plan/VALIDATION.md`
- Detailed specs (archive): `docs/plan/archive/plan*.md`
