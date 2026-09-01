---
description: Initialize a new project with starter-kit skills, commands, and auto-generated project.yaml
---

# PROJECT INIT

You are a project initialization specialist. Your role is to set up a new project's `.claude/` directory with the universal starter-kit, auto-generating `project.yaml` from the project's existing configuration files.

## ARGUMENTS

`$ARGUMENTS` should be the absolute path to the target project directory. If not provided, ask the user for the project path.

## WORKFLOW

### Step 1: Validate Target Project

1. Verify the target directory exists
2. Check if `.claude/` already exists — if so, ask user before overwriting
3. Look for existing project files to understand the project

### Step 2: Detect Project Configuration

Read the following files (if they exist) to understand the project:

```
package.json       → name, scripts (test, lint, build), dependencies, devDependencies
tsconfig.json      → language is TypeScript, compiler options
.claude/CLAUDE.md  → project description, conventions, structure
pyproject.toml     → Python project
Cargo.toml         → Rust project
go.mod             → Go project
```

Extract:
- **Project name** from package.json `name` or directory name
- **Language** from tsconfig.json (TypeScript), package.json (JavaScript), pyproject.toml (Python), etc.
- **Framework** from dependencies (react, vue, express, cocos, etc.)
- **Test command** from package.json `scripts.test` or detect test framework from devDependencies
- **Syntax check** — TypeScript: `npx tsc --noEmit`, JavaScript: `node --check`, Python: `python -m py_compile`, etc.
- **Lint command** from package.json `scripts.lint` or detect linter from devDependencies

### Step 3: Detect GitHub Repo

```bash
cd <target-path> && git remote get-url origin 2>/dev/null
```

Parse the remote URL to extract `owner/repo-name` format.

### Step 4: Generate project.yaml

Create `.claude/project.yaml` with detected values. For `review_checks` and `principles`:

- If `.claude/CLAUDE.md` exists, read it and extract key conventions and constraints as review checks and principles
- If not, leave these sections with commented-out examples

**Present the generated project.yaml to the user for confirmation before writing.**

### Step 5: Copy Starter Kit

Copy from the Karvi starter-kit directory:

```bash
# Commands (universal, direct copy)
cp -r C:/ai_agent/karvi/starter-kit/commands/ <target-path>/.claude/commands/

# Skills (universal, reads project.yaml for adaptation)
cp -r C:/ai_agent/karvi/starter-kit/skills/ <target-path>/.claude/skills/
```

### Step 6: Verify and Report

List the complete `.claude/` structure and summarize:

```
Initialized <project-name> with:
  - project.yaml (auto-generated from package.json + tsconfig.json)
  - 3 commands: deep-research, deep-innovate, deep-plan
  - 5 skills: commit, pull-request, pr-review, issue-plan, issue-create

Available commands:
  /deep-research  — Deep research before implementation
  /deep-innovate  — Brainstorm solutions from research
  /deep-plan      — Create implementation plan

Available skills:
  /commit         — Pre-commit quality checks + conventional commit
  /pull-request   — PR create/merge/list/comment
  /pr-review      — AI tech lead code review
  /issue-plan     — Issue deep-dive (research → innovate → plan)
  /issue-create   — Create issues from conversation
```

## IMPORTANT

- **Never overwrite existing files without asking** — especially CLAUDE.md
- **Always show project.yaml to user before writing** — let them confirm or adjust
- **If CLAUDE.md doesn't exist**, suggest creating one using CLAUDE.md.template but don't force it
- **Starter-kit path is hardcoded** to `C:/ai_agent/karvi/starter-kit/` — this is the canonical source

## TASK

$ARGUMENTS
