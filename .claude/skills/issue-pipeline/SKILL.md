---
name: issue-pipeline
description: "End-to-end issue pipeline: plan → implement → review → merge. Dispatches parallel sub-agents with worktree isolation. Usage: /issue-pipeline 566 567 568 [--skip-plan] [--skip-review] [--no-merge]"
---

# Issue Pipeline Skill

You orchestrate the full lifecycle of GitHub issues through parallel sub-agents. Each phase runs all issues concurrently in isolated worktrees.

## Usage

```
/issue-pipeline <issue-numbers...> [flags]
```

**Flags:**
- `--skip-plan` — Skip plan phase, go straight to implement (issues already have plans)
- `--skip-review` — Skip review phase, merge after implement
- `--no-merge` — Stop after review, don't auto-merge

## Prerequisites

This skill depends on these sub-skills being installed in `.claude/skills/`:
- `issue-plan` — Deep-dive planning (research → innovate → plan)
- `issue-action` — Implementation from plan to PR
- `pr-review-loop` — Iterative review with auto-fix

If any are missing, inform the user and suggest installing them from the karvi starter-kit.

## Pipeline Phases

### Phase 1: Plan (parallel)

For each issue, launch a sub-agent with `isolation: "worktree"` and `run_in_background: true`:

```
prompt: |
  You are working on GitHub issue #{number} for this project at {project_root}.
  Load and execute the issue-plan skill by reading {project_root}/.claude/skills/issue-plan/SKILL.md
  and following its instructions for issue #{number}.
  IMPORTANT: You are in a worktree. Do NOT modify source code — planning only.
```

**Wait for all agents to complete.** Report results to user as a table.

### Phase 2: Implement (parallel)

For each issue, launch a sub-agent with `isolation: "worktree"` and `run_in_background: true`:

```
prompt: |
  You are working on GitHub issue #{number} for this project at {project_root}.
  Load and execute the issue-action skill by reading {project_root}/.claude/skills/issue-action/SKILL.md
  and following its instructions for issue #{number}.
  The plan has been posted as a comment on the issue — read it with `gh issue view {number} --comments`.
  IMPORTANT: You are in a worktree. Pull latest main first: `git pull origin main`.
```

**Wait for all agents to complete.** Collect PR URLs. Report results to user.

### Phase 3: Review (parallel)

For each PR created in Phase 2, launch a sub-agent with `isolation: "worktree"` and `run_in_background: true`:

```
prompt: |
  You are reviewing PR #{pr_number} for this project at {project_root}.
  Load and execute the pr-review-loop skill by reading {project_root}/.claude/skills/pr-review-loop/SKILL.md
  and following its instructions for PR #{pr_number}.
  IMPORTANT: You are in a worktree. If fixes are needed, make them on the PR's branch, commit, and push.
```

**Wait for all agents to complete.** Report verdicts.

### Phase 4: Merge

For each PR with LGTM verdict:

```bash
gh pr merge {pr_number} --squash --delete-branch
```

Report final status table.

## Execution Rules

1. **All sub-agents use `isolation: "worktree"`** — never work on main directly
2. **All sub-agents use `run_in_background: true`** — maximize parallelism
3. **Launch all agents for a phase in a SINGLE message** (one tool call per agent, all in parallel)
4. **Wait for ALL agents in a phase before starting the next phase**
5. **Report a status table after each phase** — issue number, status, key details
6. **If any agent fails, report it but continue with the rest** — don't block the pipeline
7. **Clean up worktrees after merge phase** — `git worktree remove` + `git worktree prune`

## Status Table Format

After each phase:

```
| Issue | Phase | Status | Details |
|-------|-------|--------|---------|
| #566  | plan  | ✅     | Plan posted to issue |
| #567  | plan  | ✅     | Plan posted to issue |
| #568  | plan  | ❌     | Agent error: ... |
```

## Error Handling

- **Agent fails**: Mark as ❌, continue with other issues. Report at end.
- **PR creation fails**: Try to identify branch, suggest manual fix.
- **Review finds issues**: Review loop handles fix + re-review automatically.
- **Merge conflict**: Report conflict, skip merge for that PR, suggest manual resolution.
