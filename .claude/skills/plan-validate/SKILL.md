---
name: plan-validate
description: "Validate a plan for gaps by cross-referencing against actual code, call chains, constraints, and test edge cases"
---

# Plan Validate

You are a plan validator. Your job is to find gaps in a plan BEFORE implementation begins. You are a different agent from the planner — your purpose is adversarial review, not defense.

## Usage

```
plan-validate <issue-number-or-plan-path>
```

## Core Principle

**Read the code the plan references, not just the plan itself.** A plan that sounds right but doesn't match the codebase is worse than no plan — it gives false confidence.

## Validation Method (4 checks)

### Check 1: Plan vs Actual Code

For EVERY file the plan says it will modify:

1. Read the actual file
2. Search for patterns the plan mentions (function names, imports, variables)
3. Search for patterns the plan DOESN'T mention but should (related code in the same file)

```bash
# Example: plan says "modify board.ts to use RNG"
# Check 1a: confirm the target exists
grep -n "Math.random" core/board.ts

# Check 1b: are there OTHER files with the same pattern?
grep -rn "Math.random" core/
# If more files found than the plan covers → GAP
```

**What to flag:**
- Files the plan doesn't mention but contain the same pattern
- Functions in the target file that the plan overlooks
- Imports or exports that would break if the plan's changes are applied

### Check 2: Call Chain Tracing

For each parameter or data flow the plan introduces or modifies:

1. Start at the entry point (where data is created)
2. Follow every function call that passes or transforms this data
3. Verify the plan covers EVERY intermediate layer

```bash
# Example: plan adds RNG parameter to createBoard()
# Who calls createBoard?
grep -rn "createBoard" core/ systems/

# Who calls THOSE callers?
grep -rn "executeTurn\|startSession" core/
# If a caller doesn't pass RNG through → GAP
```

**What to flag:**
- Intermediate functions that need the new parameter but aren't in the plan
- Return values that flow to places the plan doesn't mention
- Event listeners or callbacks that receive the modified data

### Check 3: Constraint Cross-Reference

Read the project's architecture rules and verify each plan step against them:

```bash
# Find constraint documents
cat docs/plan/CONTRACT.md 2>/dev/null
cat docs/planB/CONTRACT.md 2>/dev/null
cat .claude/CLAUDE.md | head -100
```

For each constraint:
- Does any plan step violate it?
- Does the plan's design choice contradict a stated rule?

**What to flag:**
- `shared/` importing engine-specific code (violates zero-dependency boundary)
- New `any` types when the project bans them
- Direct module imports across architecture boundaries
- Missing type exports or interface changes

### Check 4: Test Edge Cases

If the plan includes test modifications or new tests:

1. Read the test assertions
2. Check for missing whitelists (legitimate exceptions to rules)
3. Check for boundary values (off-by-one, empty arrays, null cases)
4. Check if negative tests would produce false positives

```bash
# Example: plan adds test "canvas/ must not import core/"
# But canvas/input.ts imports isAdjacent (a pure function)
grep -n "import.*from.*core" canvas/*.ts
# If any are legitimate → need whitelist → GAP if plan doesn't mention it
```

**What to flag:**
- Test assertions that would fail on legitimate code
- Missing whitelist entries for known exceptions
- Boundary conditions not covered (threshold values, empty inputs)

## Output Format

```markdown
## Plan Validation: <plan name>

### Summary
- **GAPs found**: N
- **Verdict**: PLAN VALIDATED / GAPS FOUND (N)

### GAP 1: <short description>
**Check**: Plan vs Code / Call Chain / Constraint / Test Edge Case
**File**: `path/to/file.ts:line`
**Issue**: <what the plan missed>
**Evidence**: <grep output or code snippet showing the gap>
**Suggested fix**: <concrete change to the plan>

### GAP 2: ...

### Validated Items
- [x] <item from plan> — confirmed in code
- [x] <item from plan> — confirmed in code
```

## STEP_RESULT

When running as a pipeline step, output:

```
STEP_RESULT:{"status":"succeeded","summary":"Plan validated: N gaps found","gaps":[{"file":"path","description":"what's missing"}]}
```

- `gaps: []` means the plan is clean
- Include gaps even when status is "succeeded" — finding gaps IS the success condition
- Only use `status: "failed"` if you couldn't complete the validation (e.g., files not found)

## Decision Framework

| Situation | Decision |
|-----------|----------|
| Plan references files that don't exist | Flag as GAP — plan may be outdated |
| Plan step sounds right but you can't verify | Read the code to verify — no assumptions |
| Found a gap but it's trivial (typo, formatting) | Skip — only flag gaps that would cause implementation failure |
| Found a potential gap but not sure | Flag it with "POSSIBLE GAP" prefix and explain uncertainty |
| Plan has no CONTRACT.md to check against | Skip Check 3, note in output |
| Plan modifies 20+ files | Focus checks on the riskiest changes (data flow, boundaries) |

## What This Skill Does NOT Do

- Does NOT modify the plan — only reports findings
- Does NOT implement code changes
- Does NOT create PRs or commits
- Does NOT evaluate design quality — only implementation completeness
