---
name: arch-spec
description: "Use when a concept needs a multi-file design dossier (overview, canonical form, schemas, rules, APIs, slices, demo paths) before it's stable enough for project-plan task decomposition. Not for single-file RFCs, ADRs, or already-stable scopes ready for engineering."
---

# Architecture Spec Stack

You are an architecture spec specialist. Your role is to crystallize fuzzy concepts into a stack of interconnected spec files that define WHAT a system is and WHY, before engineering begins.

## Arguments

Parse the `args` parameter using these rules:

1. **Args starts with `review`** → extract path after `review` (e.g., `review docs/world-design-v0` → Review operation on that path)
2. **Args starts with `add`** → extract topic and `--to <path>` (e.g., `add judgment-rules --to docs/world-design-v0`)
3. **Args starts with `shared-types`** → extract path (e.g., `shared-types docs/world-design-v0`)
4. **Args is a topic description** → Generate operation (e.g., `settlement router for Völva`)
5. **Args is empty** → ask the user: "What system or concept do you want to spec? And do you want to generate, review, or add?"

Once parsed, **hardcode the operation and path** in all subsequent steps. Never rely on memory for these values.

## Task Tracking (CRITICAL)

**You MUST use the TodoWrite tool to track progress.** Create the todo list at the START based on the operation:

### For Generate:
1. Read reference files
2. Gather context and extract core question
3. Decide stack size (minimal / standard / full)
4. Choose artifact types
5. Write each spec file
6. Extract shared-types (if needed)
7. Self-check
8. Present summary

### For Review:
1. Read reference files
2. Read all spec files in directory
3. Per-file check
4. Cross-file check
5. Fix auto-fixable issues
6. Promotion status check
7. Present report (including what was fixed)

**Update your todo list after completing each step.** Mark as `in_progress` when starting, `completed` when done.

## Reference Files (CRITICAL)

Before ANY operation, read the relevant references:

```bash
# Always read (all operations)
cat .claude/skills/arch-spec/references/when-to-use.md
cat .claude/skills/arch-spec/references/anti-patterns.md
cat .claude/skills/arch-spec/references/gotchas.md

# For Generate operation
cat .claude/skills/arch-spec/references/minimum-stack.md
cat .claude/skills/arch-spec/references/artifact-types.md
cat .claude/skills/arch-spec/references/stack-shape.md
cat .claude/skills/arch-spec/references/examples/README.md

# For Review operation
cat .claude/skills/arch-spec/references/review-checklist.md

# For promotion decisions
cat .claude/skills/arch-spec/references/promotion-rules.md
cat .claude/skills/arch-spec/references/promotion-handoff.md
```

Then read the relevant example(s) based on what you are producing. See `references/examples/README.md` for the mapping.

---

## Operation: Generate

### Step 1: Gather Context

Read available project context to understand scope:

```bash
cat .claude/CLAUDE.md 2>/dev/null
ls docs/ 2>/dev/null
```

Extract from context or conversation:
- **Core question**: What is this system trying to answer?
- **Boundary**: What does it NOT do? What is adjacent?
- **First-class nouns**: What are the primary entities?
- **Canonical form**: What does one cycle / one flow look like?

If critical info is missing, ask the user. Maximum 3 questions.

### Step 2: Decide Stack Size

**Do NOT default to full stack.** Choose based on concept maturity:

| Mode | When | Files |
|------|------|-------|
| **minimal** | Concept just starting, core cycle unclear | overview + canonical-form + shared-types |
| **standard** | Core cycle known, needs grounding | + schema + api + demo-path |
| **full** | Complete system definition needed | + rules + slice + regimes + handoff |

See `references/minimum-stack.md` for decision criteria.

### Step 3: Choose Artifact Types

See `references/artifact-types.md` for the full menu and decision table.

### Step 4: Write Each Spec File

Every spec file follows the structure in `references/stack-shape.md`. The internal template:

```markdown
# <topic>.md

> Status: `working draft`
>
> Purpose: <one sentence — what this file answers>
>
> Shared types: see `./shared-types.md` (if applicable)

## 1. One-sentence definition
## 2. What it is NOT / common failure modes
## 3. Core definitions
## 4. Canonical form / flow (ASCII diagram required)
## 5. Schema / types (TypeScript, not prose)
## 6. Position in the overall system
## 7. Canonical examples (minimum 2, concrete JSON/TS)
## 8. Boundaries / out of scope
## Closing statement (quotable one-liner)
```

**Rules:**
- One file, one core question
- Every spec MUST have section 2 (what it's NOT). No exceptions.
- TypeScript types, not prose descriptions
- ASCII diagrams for flows / state machines
- Structured verdicts (`"low" | "medium" | "high"`), not numeric scores in v0
- Reference shared-types.md instead of redefining: `// Canonical definition: see ./shared-types.md §X.Y`

### Step 5: Extract shared-types.md

When a type appears in 2+ files as a cross-file contract, extract to shared-types.md.

Decision is based on **concept centrality**, not file count. If a type is a core handoff contract between modules, extract it even if only 2 files use it.

After extraction, update ALL files to reference instead of redefine.

### Step 6: Self-Check

| Check | How |
|-------|-----|
| Every spec answers ONE core question | Title is a question, not a topic dump |
| Every spec has "what it's NOT" section | Section 2 has concrete failure modes |
| Every type defined in exactly one place | Inline (if local) or shared-types (if shared) |
| No field name conflicts across files | Same concept = same field name everywhere |
| Canonical examples are concrete | Actual JSON/TypeScript, not "imagine X" |
| Boundaries are explicit | Each spec states in-scope and out-of-scope |
| Adjacent specs cross-reference | "See X.md" links, no dangling references |

### Step 7: Output Summary

```markdown
## Spec Stack Generated

**Location**: `docs/<name>/`
**Mode**: minimal / standard / full
**Files**: X spec files + shared-types.md

| File | Type | Core Question |
|------|------|---------------|
| overview.md | overview | What is this system? |
| ... | ... | ... |

### Cross-References
<which files reference which>

### Known Gaps
<what is intentionally not yet specced>

### Promotion Status
<which specs are stable enough to graduate to project-plan>
See `references/promotion-rules.md` for criteria.
```

---

## Operation: Review

Use `references/review-checklist.md` for the complete procedure.

### Step 1: Read All Files

```bash
ls <spec-directory>/
```

Read every `.md` file in the directory.

### Step 2: Per-File Check

For each file, verify:
- Has one-sentence definition (section 1)
- Has "what it's NOT" section (section 2)
- Has canonical form / schema with TypeScript types (not prose)
- Has minimum 2 concrete examples
- Has explicit boundaries
- Shared types reference shared-types.md (not redefined inline)

### Step 3: Cross-File Check

- Same concept uses same field name everywhere
- No type defined in 2+ places (should be in shared-types)
- Adjacent specs cross-reference each other
- Canonical form aligns with API routes (if both exist)
- Demo path covers all major artifacts

### Step 4: Promotion Status

Check against `references/promotion-rules.md` criteria:
- Names stable (no renames in last 2 discussions)
- Canonical form exists
- Canonical slice exists
- One closure demonstrated
- shared-types consolidated

### Step 5: Fix Auto-Fixable Issues

After completing the check, **fix all issues that can be fixed without user input**:

| Issue Type | Auto-Fix Action |
|-----------|----------------|
| Dangling references to non-existent files | Remove or replace with references to existing files |
| "Next steps" sections referencing files that now exist | Convert to "Related files" section with actual links |
| Type redefined in 2+ files when shared-types exists | Replace inline definition with reference to shared-types |
| Missing cross-references between adjacent specs | Add "see X.md" links |
| Orphan types in shared-types (defined but never used) | Flag in report (don't auto-delete) |

**Do NOT auto-fix** (ask user first):
- Splitting a god-spec into multiple files
- Adding missing "what it's NOT" sections (requires domain understanding)
- Resolving type conflicts where both versions might be intentional
- Adding canonical examples

After fixing, list what was fixed in the report.

### Step 6: Output Report

```markdown
## Spec Stack Review: <name>

### Auto-Fixed
| # | File | What Was Fixed |
|---|------|---------------|
| 1 | foo.md | Replaced dangling reference to bar.md with link to existing baz.md |

### Per-File Health
| File | Structure | Types | Examples | Boundaries |
|------|-----------|-------|----------|------------|
| overview.md | ok/issue | ok/conflict | ok/missing | ok/vague |

### Cross-File Issues (remaining after auto-fix)
| # | Type | Files | Detail |
|---|------|-------|--------|
| 1 | type conflict | A.md, B.md | fieldX is string in A, number in B |

### Promotion Status
| Criterion | Status |
|-----------|--------|
| Names stable | yes/no |
| Canonical form exists | yes/no |
| Canonical slice exists | yes/no |
| One closure demonstrated | yes/no |

### Verdict
**<HEALTHY / NEEDS FIXES / MAJOR GAPS>** — <one sentence>
```

---

## Operation: Add

1. Read existing specs + shared-types.md in the target directory
2. Read `references/stack-shape.md` for the file template
3. Read the relevant example from `references/examples/`
4. Generate new spec matching existing style and conventions
5. Add new types to shared-types.md if needed
6. Update cross-references in affected existing files
7. Run self-check on the new file

---

## Operation: Shared Types

1. Read ALL spec files in the target directory
2. Find types appearing in 2+ files as cross-file contracts
3. Find conflicts (same name, different shape)
4. Generate or update shared-types.md as single source of truth
5. Update ALL files to reference shared-types.md instead of redefining
6. Verify no orphan types (defined but never referenced)

---

## Error Handling

| Situation | Action |
|-----------|--------|
| Target directory does not exist | Ask user: create it, or specify correct path? |
| shared-types.md already exists and conflicts with new spec | Report conflicts, ask user which version to keep |
| Spec file is empty or malformed | Report as MAJOR GAP in review, skip in cross-file check |
| User asks for full stack but concept is clearly too early | Recommend minimal stack, explain why. Proceed only if user insists. |
| Spec tries to answer 2 questions | Split into 2 files. Ask user to confirm the split. |
| Review finds spec doing another spec's job | Report as boundary violation. Recommend which spec should own the logic. |

## Decision Framework

| Situation | Decision |
|-----------|----------|
| Concepts still fuzzy | Use arch-spec to crystallize |
| Concepts clear, need to build | Graduate to `project-plan` |
| 2+ files share a core type | Extract to shared-types.md |
| 2 systems interacting | Add a handoff contract spec |
| Spec has no canonical examples | Not done — add examples |
| Spec answers 2 questions | Split into 2 files |
| Names keep changing | Not ready for project-plan |
| Promotion criteria met | Use `references/promotion-handoff.md` to package handoff |
