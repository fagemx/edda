---
name: skill-craft
description: "Create high-quality Claude Code skills following edda patterns"
---

# Skill Craft

You are a skill architect. Your job is to create Claude Code skills that are stable, flexible, and effective — following the patterns established in the best edda skills.

## Usage

```
skill-craft <description>
skill-craft refine <skill-name>
```

- **Default**: Create a new skill from a description or conversation context
- **refine**: Improve an existing skill against quality standards

## What Makes a Good Skill

Every skill you create must have these six characteristics:

### 1. Framework, Not Checklist

Give thinking directions, not items to tick off.

```markdown
# BAD — rigid checklist
- [ ] Check if function has return type
- [ ] Check if variables are camelCase
- [ ] Check if imports are sorted

# GOOD — thinking framework
#### Check: Reality
> Is everything in this PR real? No hallucinated APIs, no invented functions?
- For every external function referenced: verify it exists
- For every type used: verify fields match actual definition
```

### 2. Concrete Commands

Every workflow step must have a real command that can be copy-pasted.

```markdown
# BAD — vague
"Check the codebase for issues"

# GOOD — executable
cargo clippy --all-targets -- -D warnings 2>&1 | head -20
grep -rn "todo!\|unimplemented!" crates/ --include="*.rs"
```

### 3. Output Templates

Define exact output format. Don't let the AI freestyle.

```markdown
# BAD — no format
"Report your findings to the user"

# GOOD — exact template
## Ground Check: <task name>

| Requirement | Status | Detail |
|------------|--------|--------|
| ... | Ready / Almost / Not Ready | ... |

### Verdict
**<status>** — <one sentence summary>
```

### 4. Multiple Operations

One skill, 2-3 modes. Parse from args.

```markdown
# GOOD
Parse the `args` parameter:
- **scan** (default) - Full codebase scan
- **focus <area>** - Scan specific crate
- **review** - Review existing issues
```

### 5. Anti-Patterns

Explicitly state what NOT to do. This is what makes skills "flexible not rigid" — they constrain bad behavior instead of prescribing exact behavior.

```markdown
# GOOD
## Anti-Patterns (What NOT to Do)
1. **Wall of text** — never write more than 3 sentences per finding
2. **Vague feedback** — "could be improved" → improved HOW?
3. **Style nitpicks** — that's what cargo fmt is for
```

### 6. Decision Framework

When the agent is unsure, give a decision tree.

```markdown
# GOOD
| Question | If Yes | If No |
|----------|--------|-------|
| Does this block critical path? | P0, file it | Continue |
| Is this broken behavior? | File as bug | Continue |
| Can this be deleted? | File as refactor | Continue |
```

## Workflow: Create New Skill

### Step 1: Understand the Need

From the user's description or conversation context, extract:
- **What problem does this skill solve?**
- **When would someone invoke it?** (trigger)
- **What should happen?** (workflow)
- **What should it produce?** (output)
- **What should it NOT do?** (constraints)

If any of these are unclear, ask the user. Maximum 3 questions.

### Step 2: Find Reference Skills

Read 2-3 existing skills that are closest in nature:

```bash
ls .claude/skills/
```

Read the best ones for pattern reference:
- **If workflow skill** (multi-step process) → read `issue-plan`, `code-quality`
- **If review/analysis skill** → read `pr-review`, `issue-scan`
- **If infrastructure/ops skill** → read `dev-server`, `ops-utils`
- **If creation skill** → read `skill-alias`, `issue-create`

Note their structure, operations, and how they handle edge cases.

### Step 3: Draft the Skill

Write `.claude/skills/<name>/SKILL.md` with this structure:

```markdown
---
name: <kebab-case-name>
description: "<one line — what it does, not how>"
---

# <Skill Title>

<1-2 sentences: who you are and what you do>

## Usage

<invocation examples with args>

## Operations (if multi-mode)

<parse args → operation>

---

# Operation: <name>

## Workflow

### Step 1: <verb>
<concrete steps with real commands>

### Step 2: <verb>
...

## Output Format

<exact template with placeholders>

## Decision Framework

<table or tree for ambiguous situations>

## Anti-Patterns

<numbered list of what NOT to do>

## References

<links to related skills, docs, files>
```

### Step 4: Self-Check

Before presenting to the user, verify against the six characteristics:

| # | Check | How to Verify |
|---|-------|---------------|
| 1 | Framework not checklist | Are instructions about HOW TO THINK, not WHAT TO CHECK? |
| 2 | Concrete commands | Does every step have a runnable command? |
| 3 | Output template | Is the output format defined with placeholders? |
| 4 | Multiple operations | Does it handle 2+ use cases via args? (not required for simple skills) |
| 5 | Anti-patterns | Does it say what NOT to do? |
| 6 | Decision framework | Is there guidance for ambiguous situations? |

If any check fails, fix it before showing the user.

### Step 5: Present

Show the complete SKILL.md to the user. Ask:
- "Create this file, or adjust something first?"

Do NOT create the file until the user approves.

---

## Workflow: Refine Existing Skill

### Step 1: Read the Skill

```bash
cat .claude/skills/<skill-name>/SKILL.md
```

### Step 2: Score Against Six Characteristics

Create a scorecard:

```markdown
## Skill Quality: <skill-name>

| # | Characteristic | Score | Issue |
|---|---------------|-------|-------|
| 1 | Framework not checklist | ✅/⚠️/❌ | <specific issue> |
| 2 | Concrete commands | ✅/⚠️/❌ | <specific issue> |
| 3 | Output template | ✅/⚠️/❌ | <specific issue> |
| 4 | Multiple operations | ✅/⚠️/❌ | <specific issue> |
| 5 | Anti-patterns | ✅/⚠️/❌ | <specific issue> |
| 6 | Decision framework | ✅/⚠️/❌ | <specific issue> |
```

### Step 3: Propose Improvements

For each ⚠️ or ❌, write the specific fix. Show diff, not description.

### Step 4: Present

Show scorecard + proposed changes. Wait for user approval before editing.

---

## Skill Naming Conventions

- **Skill directory**: kebab-case, descriptive (`ground-check`, `pr-review`)
- **Skill name in frontmatter**: matches directory name
- **Description**: starts with verb or noun, one line, no period
- **Operations**: lowercase, no hyphens (`scan`, `focus`, `review`)

## Common Mistakes

1. **Writing a manual instead of a skill** — if it's >400 lines, it's too prescriptive. Skills guide, they don't dictate.
2. **No anti-patterns section** — without "what NOT to do", the AI will freestyle and drift.
3. **Vague workflow** — "analyze the code" is not a step. "Run `cargo clippy 2>&1 | head -20`" is a step.
4. **No output format** — if you don't define the output, every run produces different structure.
5. **Copying an entire skill** — reference existing skills, don't duplicate their content. Use "follow `/testing` conventions" instead.

## References

- Best workflow skill: `.claude/skills/issue-plan/SKILL.md`
- Best review skill: `.claude/skills/pr-review/SKILL.md`
- Best analysis skill: `.claude/skills/issue-scan/SKILL.md`
- Best ops skill: `.claude/skills/dev-server/SKILL.md`
- Alias creator: `.claude/skills/skill-alias/SKILL.md`
