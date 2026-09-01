---
name: skill-test
description: "Test a Claude Code skill — verify it runs, references exist, output is sane"
---

# Skill Test

You are a skill QA engineer. Your job is to take a skill and verify it actually works — not in theory, but against the real codebase.

## Usage

```
skill-test <skill-name>                  # Full test (static + simulation)
skill-test <skill-name> static           # Static checks only (fast)
skill-test <skill-name> simulate <args>  # Simulate with specific input
skill-test all                           # Batch static check on all skills
```

## Operations

### Operation: Full Test (default)

Run static checks + simulate with a reasonable sample input.

---

### Operation: static

Fast structural validation. No execution, just checks.

#### Check 1: Format

Read the SKILL.md and verify:

```bash
cat .claude/skills/<skill-name>/SKILL.md
```

| Item | Check | How |
|------|-------|-----|
| Frontmatter | Has `name` and `description` | grep for `---` block at top |
| Name match | `name:` matches directory name | compare values |
| Description | One line, starts with verb/noun | read it |
| Role statement | First paragraph defines who the agent is | read first 5 lines after frontmatter |
| Operations | Has `args` parsing if multi-mode | grep for `args` or `## Operations` |
| Workflow | Has `### Step` sections | grep for `### Step` |

#### Check 2: References Exist

Every file path, command, and skill reference in the SKILL.md must be real.

```bash
# Extract file paths referenced in the skill
grep -oE '`[^`]*\.(rs|toml|md|yml|json)`' .claude/skills/<skill-name>/SKILL.md

# For each path, check if it exists
ls <path> 2>/dev/null || echo "MISSING: <path>"

# Extract skill references
grep -oE '/[a-z-]+`|`/[a-z-]+' .claude/skills/<skill-name>/SKILL.md

# For each referenced skill, check if it exists
ls .claude/skills/<referenced-skill>/SKILL.md 2>/dev/null || echo "MISSING: <skill>"

# Extract bash commands and check if tools exist
grep -oE '(cargo|gh|git|docker|flyctl|npm|pnpm) [a-z]+' .claude/skills/<skill-name>/SKILL.md | sort -u
```

#### Check 3: Quality Score

Score against the six characteristics from `/skill-craft`:

| # | Characteristic | How to Check |
|---|---------------|-------------|
| 1 | Framework not checklist | Are instructions thinking-oriented or tick-box? |
| 2 | Concrete commands | Count `bash` code blocks with real commands |
| 3 | Output template | Is there a `## Output` or template section with placeholders? |
| 4 | Multiple operations | Does it parse args for different modes? |
| 5 | Anti-patterns | Is there a "NOT to do" or "Anti-Patterns" section? |
| 6 | Decision framework | Is there a decision table or tree? |

#### Static Report

```markdown
## Static Test: <skill-name>

### Format
- Frontmatter: ✅/❌
- Role statement: ✅/❌
- Operations: ✅/❌ (or N/A for single-mode)
- Workflow steps: ✅/❌

### References
| Reference | Type | Exists |
|-----------|------|--------|
| `docs/DECISIONS.md` | file | ✅ |
| `/testing` | skill | ✅ |
| `cargo clippy` | command | ✅ |
| `docs/MISSING.md` | file | ❌ |

### Quality Score
| # | Characteristic | Score |
|---|---------------|-------|
| 1 | Framework | ✅/⚠️/❌ |
| 2 | Commands | ✅/⚠️/❌ |
| 3 | Output template | ✅/⚠️/❌ |
| 4 | Operations | ✅/⚠️/❌ |
| 5 | Anti-patterns | ✅/⚠️/❌ |
| 6 | Decision framework | ✅/⚠️/❌ |

### Verdict
**PASS / WARN / FAIL** — <summary>
```

---

### Operation: simulate

Dry-run the skill's workflow with real or sample input.

#### Step 1: Determine Input

If user provided args: use those.
If not: construct a reasonable sample input from the skill's usage examples.

#### Step 2: Walk Through Workflow

For each `### Step` in the skill:

1. **Read the step instructions**
2. **Execute the commands** (read-only — don't create files, PRs, or issues)
   - For `grep`/`find`/`ls` commands: run them and check output
   - For `gh`/`git` commands: run read-only variants (e.g., `gh issue list`, `git log`)
   - For write commands (`Write`, `Edit`, `gh issue create`): **simulate only** — describe what would be created
3. **Check output format** — does the actual output match the skill's template?
4. **Record any failures** — command errors, empty results, missing files

#### Step 3: Evaluate Output

Does the simulated output:
- Follow the skill's output template? (structure match)
- Contain real data from the codebase? (not hallucinated)
- Answer the question the skill claims to answer? (purpose match)

#### Simulation Report

```markdown
## Simulation Test: <skill-name>

### Input
`<args used>`

### Step-by-Step Results

#### Step 1: <step name>
- Commands executed: <count>
- Results: ✅ produced expected output / ❌ <error>
- Sample output:
  ```
  <first 10 lines of actual output>
  ```

#### Step 2: <step name>
...

### Output Evaluation
- Template match: ✅/❌ — <does output follow the defined format?>
- Real data: ✅/❌ — <does output reference actual files/code?>
- Purpose match: ✅/❌ — <does output answer the skill's stated question?>

### Verdict
**PASS / WARN / FAIL** — <summary>

### Issues Found
1. <specific issue with file:line reference in SKILL.md>
2. ...
```

---

### Operation: all

Batch static check on every skill.

#### Workflow

```bash
ls .claude/skills/
```

For each skill directory, run static checks. Produce summary:

```markdown
## Batch Static Test

| Skill | Format | References | Quality | Verdict |
|-------|--------|-----------|---------|---------|
| ground-check | ✅ | ✅ | 5/6 | PASS |
| path-forward | ✅ | ✅ | 5/6 | PASS |
| pr-review | ✅ | ⚠️ 1 missing | 6/6 | WARN |
| skill-alias | ✅ | ✅ | 3/6 | WARN |
| ... | ... | ... | ... | ... |

### Summary
- Total: <count>
- PASS: <count>
- WARN: <count>
- FAIL: <count>

### Top Issues
1. <most common issue across skills>
2. ...
```

---

## Anti-Patterns

1. **Testing format only** — A skill can have perfect YAML and still be useless. Always simulate.
2. **Running destructive commands** — Never execute write operations during testing. Simulate them.
3. **Passing everything** — If a skill has no anti-patterns section and no decision framework, it's a WARN at best, not a PASS.
4. **Testing in isolation** — Check cross-skill references. If skill A references `/testing`, verify that `/testing` skill exists and is compatible.
5. **Ignoring staleness** — A skill that references a crate that doesn't exist is broken, even if the format is perfect.

## What Triggers FAIL vs WARN vs PASS

| Verdict | Condition |
|---------|-----------|
| **FAIL** | Frontmatter missing/broken, OR referenced files don't exist, OR simulation produces errors |
| **WARN** | Quality score < 4/6, OR some references stale, OR output doesn't match template |
| **PASS** | Format correct, all references exist, quality ≥ 4/6, simulation produces expected output |

## References

- Quality standards: `.claude/skills/skill-craft/SKILL.md`
- Best reference skills: `pr-review`, `issue-scan`, `code-quality`
