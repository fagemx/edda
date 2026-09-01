---
name: project-plan
description: "Generate a structured planning pack (tracks, tasks, contracts, validation) for any project"
---

# Project Plan

You are a project planning architect. Your job is to analyze a project's scope and generate a complete planning pack — the kind of document set that lets multiple agents (or one agent across sessions) execute a complex build without losing context.

## Reference Files (CRITICAL)

Before generating ANY planning pack, you MUST read the reference files in this skill's directory. They contain real examples from production projects that define the quality bar.

```bash
# Read ALL reference files first — this is what makes the output good
cat .claude/skills/project-plan/reference-overview.md
cat .claude/skills/project-plan/reference-contract.md
cat .claude/skills/project-plan/reference-track.md
cat .claude/skills/project-plan/reference-tracks-full.md
cat .claude/skills/project-plan/reference-task.md
cat .claude/skills/project-plan/reference-validation.md
```

Your output must match the depth and specificity shown in these references. Pay special attention to the `> **Pattern notes for generation:**` sections at the bottom of each file — they explain what makes each document effective.

**If your output is shallower than the references, you're doing it wrong.**

## Usage

```
project-plan <description or scope doc path>
project-plan add-track <track letter> <track name>
project-plan validate
```

- **Default**: Generate full planning pack from a description, scope doc, or conversation context
- **add-track**: Add a new track to an existing planning pack
- **validate**: Check an existing planning pack for completeness and consistency

## When to Use This

- Starting a new project or major feature that spans multiple modules
- The work needs more than ~5 tasks and has dependency ordering
- You want agents to execute independently with clear boundaries
- You're about to use Karvi to dispatch parallel tracks

**Do NOT use this for**: single-feature issues, bug fixes, refactors, or anything that fits in one PR.

---

# Operation: Generate Planning Pack

## Workflow

### Step 1: Gather Context

Read available project context to understand scope, tech stack, and constraints:

```bash
# Check for existing project config
cat .claude/project.yaml 2>/dev/null
cat .claude/CLAUDE.md 2>/dev/null

# Check for scope doc if path provided
cat <scope-doc-path>

# Check existing code structure
ls -la src/ lib/ core/ server/ 2>/dev/null
```

If the user provided a description instead of a path, use that directly.

Extract from context:
- **What is being built** (product / feature / system)
- **Tech stack** (language, framework, runtime)
- **Existing code** (what already exists vs what's new)
- **Architecture constraints** (patterns, rules, boundaries)
- **Quality gates** (type checking, tests, linting)

If critical info is missing, ask the user. Maximum 3 questions.

### Step 2: Decompose into Tracks

Break the scope into **Tracks** (feature groups) and **Tasks** (implementable units).

**Thinking framework:**
1. What are the independent feature areas? → Tracks
2. What depends on what? → Layer ordering (L0 base → L1 core → L2 systems → L3 integration)
3. Within each track, what's the build order? → Tasks (T1 → T2 → T3)
4. What can run in parallel? → Batch grouping

**Rules:**
- Each Track should have 1-4 Tasks (not more, unless it's a complex integration track — max 5)
- Each Task should be completable in one agent session (~1-2 hours of work)
- Tasks within a track are sequential; tracks at the same layer can be parallel
- Every task must have a concrete DoD (Definition of Done) with verifiable checks

**Step Decomposition Patterns** — use these to decide how to break a track into tasks:

| Pattern | When to use | Steps |
|---------|-------------|-------|
| **DB-first** | Modules with persistence (most CRUD) | Schema+DB → Core Logic → Validators/Helpers → Routes+Tests |
| **Assessor** | Stateless checkers, evaluators | Types+Constants → Core Logic → Tracking/Aggregation → Routes+Tests |
| **Engine** | Complex business logic with multiple concerns | Schema+DB → Compliance/Validation → Engine Core → Routes+Tests |
| **Integration** | Bridges to external systems | Single file (expand when building) |

The last step is always **Routes+Tests** — it's the integration point that proves the track works.
Phase 1/2 tracks stay as single files — don't pre-expand what hasn't been designed yet.

### Step 3: Define Architecture Constraints

Extract or derive constraints that apply across ALL tracks. These are rules that, if violated, mean the task is not done.

**Categories to consider:**
- Type safety rules (e.g., no `any`, zero tsc errors)
- Architecture boundaries (e.g., core/ cannot import rendering/)
- Data integrity rules (e.g., append-only, atomic writes)
- Quality gates (e.g., zero warnings, test pass rate)
- Convention rules (e.g., commit format, naming)

Each constraint needs: Rule ID, description, verification command, affected tracks.

### Step 4: Write the Planning Pack

Generate the following files in `docs/plan/`:

#### File 1: `00_OVERVIEW.md`

```markdown
# <Project Name> — Planning Pack

## Goal
<1-3 sentences: what this build achieves>

## Dependency DAG
<ASCII art showing track dependencies by layer>

## Track Summary
| Track | Name | Layer | Tasks | Dependencies | Status |
|-------|------|-------|-------|-------------|--------|
| A | ... | L0 | 2 | — | _ |
| B | ... | L1 | 3 | A | _ |

**Total: X Tracks, Y Tasks**

## Parallel Execution Timeline
<Batch grouping showing what can run simultaneously>

## Progress Tracking
<Checkbox list grouped by batch>

## Module Map
<directory tree showing what gets created>
```

#### File 2: `CONTRACT.md`

```markdown
# <Project Name> — Architecture Constraints

> These rules cannot be violated during development.
> Any task that violates these rules is considered incomplete.

## Rules

| Rule ID | Description | Verification | Affected Tracks |
|---------|------------|--------------|-----------------|
| TYPE-01 | ... | `command` | All |
| ARCH-01 | ... | `command` | A, B |
```

For each rule, add a detailed section with: description, rationale, verification steps, consequence of violation.

#### File 3: `TRACKS.md`

This file is the **agent navigation hub** — it answers "where am I?" and "what depends on what?". It has 5 sections:

1. **Layer definitions** — one line per layer explaining its role
2. **DAG** — ASCII art showing all tracks grouped by layer with dependency arrows
3. **Track→Step mapping** — subdirectory structure showing how each track decomposes into steps
4. **Module import path map** — every source file annotated with which step creates it
5. **Cross-module dependency graph** — import direction rules + layering constraints

See `reference-tracks-full.md` for a complete example.

For each track entry within the Track→Step mapping:
```markdown
### T1: Village Manager（L0）
\`\`\`
T1_VILLAGE_MANAGER/
  T1_01_PROJECT_INIT.md      ← 專案骨架 + tsconfig + deps
  T1_02_DB_LAYER.md          ← SQLite 連線 + schema + audit_log
  T1_03_VILLAGE_CORE.md      ← VillageManager class（CRUD + version）
  T1_04_ROUTES_AND_TESTS.md  ← API routes + 測試 + 驗收
\`\`\`
```

And a separate section for each track with full metadata (see `reference-track.md`):
```markdown
## Track X: <Name>

**Layer**: L0/L1/L2/L3
**Goal**: <what this track delivers>

**Input**: <what it needs to exist before starting>
**Output**: <what exists when done>

**Dependencies**:
- blocks: <tracks this unblocks>
- blocked-by: <tracks that must finish first>

**DoD**:
- [ ] <verifiable check with command>
- [ ] <verifiable check with command>
```

#### File 4: `VALIDATION.md`

```markdown
# Validation Plan

## Track Acceptance Criteria
<Table per track with: item, pass criteria, verification command>

## Golden Path Scenarios
<2-5 end-to-end scenarios that prove the system works>

## Quality Benchmarks
<Table: rule, metric, baseline, verification>
```

#### File 5: Parent Task Files — `TRACK_X_<NAME>.md` (one per track)

For projects with substantial code, create a **parent task file** per track that contains the full implementation code. Step files then reference it instead of duplicating code.

```markdown
# T4: Law Engine

> Batch 3（依賴 T2 + T3）
> 新建檔案：`src/law-engine.ts`, `src/schemas/law.ts`

## 核心設計
<design explanation — what and why>

## 實作步驟
### Step 1: Database Schema
<full SQL + code>

### Step 2: Zod Schema
<full code>

### Step 3: Engine Core
<full class implementation>

### Step 4: API Routes
<full route code>

### Step 5: Tests
<full test suite>
```

#### File 6+: `TRACK_X_<NAME>/<TASK_ID>.md` (step files, one per task)

**Two patterns** — choose based on project size:

**Pattern A: Self-contained** (small projects, <5 tracks) — each step file has all the code inline. See `reference-task.md`.

**Pattern B: Parent-reference** (large projects, 5+ tracks) — step files are navigation + context + verification pointers. Full code lives in the parent task file. This avoids duplicating 15KB of code across 4 step files.

```markdown
# <Task ID>: <Task Title>

> **Layer**: L0/L1/L2/L3
> **Dependencies**: <task IDs with names, e.g. T2_02（ConstitutionStore）>
> **Blocks**: <task IDs with names>
> **Output**: `path/to/output` — ClassName or function

---

## 給 Agent 的起始指令

\`\`\`bash
cat docs/plan/CONTRACT.md             # relevant rules
cat docs/plan/TRACK_X_NAME.md         # Step N full code
cat src/dependency-module.ts           # imports needed
bun run build                          # verify baseline
\`\`\`

---

## 實作

<key code excerpts — types, signatures, critical logic>
<NOT the full implementation — that's in the parent file>

完整程式碼見 `TRACK_X_NAME.md` Step N。

---

## 驗收

\`\`\`bash
<verification commands>
\`\`\`
```

**Key differences from Pattern A:**
- Dependencies include **names** not just IDs: `T2_02（ConstitutionStore）` tells the agent what to import without looking it up
- Agent startup instructions point to the parent file + dependency modules
- Code is excerpted (signatures, key logic) not duplicated
- "完整程式碼見..." links back to parent file
- Completion checklist at the end of the last step shows all steps for the track

### Step 5: Self-Check

Before presenting, verify:

| Check | How |
|-------|-----|
| Every task has a concrete DoD | No vague "works correctly" — specific commands |
| Dependencies are acyclic | No circular deps between tracks/tasks |
| CONTRACT rules are verifiable | Every rule has a command, not just "review" |
| Tasks are right-sized | Each completable in one session |
| Parallel batches are correct | Tasks in same batch have no mutual deps |
| Module map matches tasks | Every file in the map is created by exactly one task |
| Golden paths are testable | Each scenario references specific tasks/modules |

## Output Format

After generating all files, present a summary:

```markdown
## Planning Pack Generated

**Location**: `docs/plan/`
**Tracks**: X | **Tasks**: Y | **Constraints**: Z rules

| Track | Tasks | Layer | Can Start After |
|-------|-------|-------|----------------|
| A | A1, A2 | L0 | immediately |
| B | B1, B2, B3 | L1 | Track A |

### Files Created
- `docs/plan/00_OVERVIEW.md`
- `docs/plan/CONTRACT.md`
- `docs/plan/TRACKS.md`
- `docs/plan/VALIDATION.md`
- `docs/plan/TRACK_A_<NAME>/A1_<TASK>.md`
- ...

### Next Steps
- Review the plan, then dispatch Track A
- Or use Karvi: `POST /api/project` with tasks from each track
```

---

# Operation: Add Track

## Workflow

1. Read `docs/plan/00_OVERVIEW.md` and `docs/plan/TRACKS.md` for existing structure
2. Determine the new track's layer and dependencies relative to existing tracks
3. Generate the track entry in TRACKS.md and individual task files
4. Update 00_OVERVIEW.md (DAG, summary table, progress tracking)
5. Update VALIDATION.md with acceptance criteria for the new track

---

# Operation: Validate

## Workflow

1. Read all files in `docs/plan/`
2. Check for:
   - Missing task files (referenced in TRACKS.md but no file exists)
   - Broken dependency references (task depends on non-existent task)
   - Duplicate task IDs
   - CONTRACT rules without verification commands
   - Tasks without DoD
   - Circular dependencies
3. Report findings

## Output Format

```markdown
## Plan Validation: <project>

| Check | Status | Detail |
|-------|--------|--------|
| All task files exist | pass/fail | ... |
| Dependencies acyclic | pass/fail | ... |
| All rules verifiable | pass/fail | ... |
| All tasks have DoD | pass/fail | ... |

### Issues Found
1. ...

### Verdict
**<PASS/FAIL>** — <one sentence>
```

---

## Decision Framework

| Situation | Decision |
|-----------|----------|
| Scope too vague to decompose | Ask user for scope doc or concrete feature list (max 3 questions) |
| Not sure how many tracks | Start with fewer — 3-5 tracks is ideal, 8+ is a smell |
| Task too big (>4 files created) | Split into sub-tasks |
| Task too small (<1 file) | Merge with adjacent task |
| Unclear tech stack | Read package.json / Cargo.toml / go.mod / existing code first |
| Existing planning pack at docs/plan/ | Ask: extend or replace? |

## Anti-Patterns

1. **Vague DoD** — "works correctly" is not verifiable. Use specific commands: `npx tsc --noEmit`, `cargo test -p crate`
2. **God Track** — one track with 8 tasks means it should be split into 2-3 tracks
3. **Over-planning** — if the whole project is <5 tasks, you don't need this skill. Just write the code.
4. **Copy-paste CONTRACT** — constraints must come from the actual project, not generic best practices
5. **Missing dependencies** — if Task B2 reads files Task A3 creates, there must be a dependency A→B
6. **Speculative tracks** — don't add "future" tracks. Plan what's being built now.
