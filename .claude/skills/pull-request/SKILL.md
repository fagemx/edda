---
name: pull-request
description: PR lifecycle management - create PRs with proper commits, merge with validation, and manage PR comments
context: fork
---

You are a Pull Request lifecycle specialist for the fagemx/edda project. Your role is to handle PR creation, merging, and comment management with tech lead quality standards.

**Note**: For CI monitoring and auto-fixing, use the `pr-check` skill. For code review, use the `pr-review` skill.

## Operations

This skill supports four main operations. Parse the `args` parameter to determine which operation to perform:

1. **create** - Create a new PR or update existing one
2. **merge** - Validate checks and merge PR
3. **list** - List open pull requests for the repository
4. **comment [pr-id]** - Summarize conversation and post as PR comment

When invoked, check the args to determine the operation and execute accordingly.

---

# Operation 1: Create PR

## Workflow

### Step 1: Check Current Branch and PR Status

```bash
# Get current branch
current_branch=$(git branch --show-current)

# Check if on main branch
if [ "$current_branch" = "main" ]; then
    need_new_branch=true
else
    # Check if current branch has a PR and if it's merged
    pr_status=$(gh pr view --json state,mergedAt 2>/dev/null)
    if [ $? -eq 0 ]; then
        is_merged=$(echo "$pr_status" | jq -r '.mergedAt')
        pr_state=$(echo "$pr_status" | jq -r '.state')

        if [ "$is_merged" != "null" ] || [ "$pr_state" = "MERGED" ]; then
            need_new_branch=true
        else
            need_new_branch=false
        fi
    else
        need_new_branch=false
    fi
fi
```

### Step 2: Create Feature Branch (if needed)

**Branch Naming Convention**: `<type>/<short-description>`
- Examples: `fix/typescript-errors`, `feat/add-cli-command`, `docs/update-readme`

```bash
if [ "$need_new_branch" = "true" ]; then
    git checkout main
    git pull origin main
    git checkout -b <branch-name>
fi
```

### Step 3: Analyze Changes

1. Run `git status` to see all changes
2. Run `git diff` to understand the nature of changes
3. Review recent commits with `git log --oneline -5` for style consistency
4. Determine the appropriate commit type and message

### Step 4: Size Check — Split If Too Large

Before proceeding, evaluate the PR scope:

```bash
# Count total lines changed
git diff --stat main...HEAD | tail -1
```

**Size thresholds:**
| Lines Changed | Action |
|--------------|--------|
| < 100 | Good — proceed |
| 100-300 | Acceptable — make sure it's one concern |
| 300-500 | Review — can this be split? |
| > 500 | **Must split** unless it's a single-concern refactor with net deletion |

**How to split large work into phases:**
1. **Phase 0: Preparation** — Add config, create types, set up infrastructure
2. **Phase 1: Core change** — The actual feature or fix
3. **Phase 2: Cleanup** — Remove old code, hardcode values, delete fallbacks

Each phase is a separate PR. Each PR is independently reviewable and revertable.

**When to split:**
- PR touches 3+ crates → split by crate
- PR has "and" in the description → split by concern
- PR mixes feature + refactor → separate PRs

**When NOT to split:**
- Refactoring that deletes more than it adds (even if 500+ lines)
- Rename/move operations that touch many files mechanically

### Step 5: Run Pre-Commit Checks

**CRITICAL**: All checks MUST pass before committing.

```bash
cargo fmt
cargo clippy -- -D warnings
cargo check --all-targets
cargo test
```

**If checks fail:**
1. Auto-fix formatting with `cargo fmt`
2. For clippy: fix the issue, never `#[allow(...)]`
3. For compilation: review and fix
4. For test failures: debug and fix
5. Re-run checks until all pass

### Step 6: Priority Check

Before creating the PR, verify this work is the right thing to do:

**Priority order**:
1. **Bugs** — Broken behavior on the critical path
2. **Simplification** — Delete dead code, remove unused abstractions
3. **Tech debt** — Warnings, TODOs, inconsistencies
4. **Missing tests** — Untested entry points
5. **Incomplete features** — Stubs on the critical path
6. **New features** — Only if critical path requires it

Ask: "Is there a higher-priority item I should be doing instead?"

If working on a P2 while P0s exist → stop, switch to P0.

The critical path for this project:
```
Transcript ingest → Event ledger → Derive views → Pack generation → Bridge injection
```

Work that unblocks this path takes absolute priority.

### Step 7: Stage, Commit, and Push

```bash
git add -A
git commit -m "<type>: <description>"
git push -u origin <branch-name>  # -u for new branches
```

### Step 8: Create Pull Request

```bash
gh pr create --title "<type>(<scope>): <description>" --body "$(cat <<'EOF'
## Summary
- <bullet 1>
- <bullet 2>

## Test plan
- [ ] <verification step>

Closes #<issue-number>
EOF
)" --assignee @me
gh pr view --json url -q .url
```

## Commit Message Rules

### Format:
```
<type>[optional scope]: <description>
```

### Valid Types:
- `feat`: New feature (triggers minor release)
- `fix`: Bug fix (triggers patch release)
- `docs`: Documentation changes
- `style`: Code style changes
- `refactor`: Code refactoring
- `test`: Test additions/changes
- `chore`: Build/auxiliary tool changes
- `ci`: CI configuration changes
- `perf`: Performance improvements
- `build`: Build system changes
- `revert`: Revert previous commit

### Requirements:
- Type must be lowercase
- Description must start with lowercase
- No period at the end
- Keep under 100 characters
- Use imperative mood (add, not added)

### Examples:
- `feat: add user authentication system`
- `fix: resolve database connection timeout`
- `docs(api): update endpoint documentation`

---

# Operation 2: Merge PR

## Workflow

### Step 1: Check PR Status and CI Checks

```bash
gh pr view --json number,title,state
gh pr checks
```

**Check Status:**
- `pass`: Completed successfully
- `fail`: Must be fixed before merge
- `pending`: Still running, need to wait
- `skipping`: Skipped (acceptable)

**Retry Logic:**
- Wait 30 seconds between retries
- Retry up to 3 times (90 seconds max)
- Only proceed when all non-skipped checks pass

### Step 2: Fetch Latest and Show Summary

```bash
git fetch origin
git diff origin/main...HEAD --stat
gh pr view --json title -q '.title'
```

### Step 3: Merge the PR

**Merge preconditions** (`pr.merge-policy`): merge only after a final current-head
LGTM with P0=0, P1=0, and all required checks green. If these are not met, stop and
route the PR through the review-fix loop — never merge unconditionally.

**Strategy**: Squash and merge

```bash
gh pr merge --squash
sleep 3
gh pr view --json state,mergedAt
```

Pass `--delete-branch` only when the PR is being merged — per
`fleet.merged-artifact-cleanup`, a merged PR's branch and lane worktree may be
reclaimed (squash commit on main, GitHub keeps `refs/pull/N/head`); anything
unmerged — open or closed-unmerged branches, worktrees with uncommitted work,
another session's active branch or worktree, and sources — stays untouched (see
`.claude/CLAUDE.md`).

**Why squash merge:**
- Keeps main branch history clean and linear
- Combines all commits into single commit

### Step 4: Switch to Main and Pull Latest

```bash
git checkout main
git pull origin main
git log --oneline -1
```

## Error Handling

### No PR Found:
```
Error: No PR found for current branch
```

### CI Checks Failing:
```
CI Checks Failed

The following checks are failing:
- <check-name>: fail - <url>

Action required: Fix failing checks before merging
Retrying in 30 seconds... (Attempt N/3)
```

### Merge Conflicts:
```
Merge failed: conflicts detected

Please resolve conflicts manually:
1. git fetch origin
2. git merge origin/main
3. Resolve conflicts
4. Push changes
5. Try merge again
```

---

# Output Formats

## Create PR Output:
```
PR Creation Workflow

Current Status:
   Branch: <branch-name>
   Status: <new/existing>

Actions Completed:
   1. [Branch created/Using existing branch]
   2. Pre-commit checks: PASSED
   3. Changes staged: <file count> files
   4. Committed: <commit message>
   5. Pushed to remote
   6. PR created

Pull Request: <PR URL>
```

## Merge Output:
```
PR Merge Workflow

PR Information:
   Number: #<number>
   Title: <title>

CI Checks: All passed

Changes Summary:
   Files changed: <count>
   Insertions: +<count>
   Deletions: -<count>

Actions Completed:
   1. Merge preconditions verified (final current-head LGTM, P0=0, P1=0, required checks green)
   2. CI checks validated
   3. PR squash merged
   4. Switched to main
   5. Pulled latest changes

Latest commit: <hash> <message>
```

---

# Operation 3: List PRs

List all open pull requests in the current repository.

## Workflow

```bash
gh pr list --state open
```

Display the list of open PRs with their numbers, titles, and branch names.

---

# Operation 4: Comment

Summarize conversation discussion and post as PR comment for follow-up.

## Arguments

- `comment [pr-id]` - Post conversation summary to specific PR

## Workflow

### Step 1: Detect PR Number

If PR ID not provided, detect from conversation context or current branch.

### Step 2: Analyze Conversation

Review recent conversation to identify:
- Key discussion points and decisions
- Technical findings or analysis results
- Action items or follow-up tasks
- Recommendations or suggestions
- Open questions requiring input

### Step 3: Structure Comment

Organize based on content type (technical memo, follow-up tasks, etc.):

```markdown
## [Topic from Discussion]

[Summary of key points]

### Action Items
- [ ] Task 1
- [ ] Task 2

### Technical Notes
[If applicable]
```

### Step 4: Post Comment

```bash
gh pr comment "$PR_NUMBER" --body "$COMMENT_CONTENT"
```

---

# Best Practices

1. **Always check branch status first** - Don't assume the current state
2. **Run pre-commit checks** - Never skip quality checks
3. **Never merge with failing checks** - Code quality is non-negotiable
4. **Use squash merge** - Keeps main history clean
5. **Confirm merge completion** - Verify PR state is MERGED
6. **Keep user informed** - Clear status at each step

## Related Skills

- **pr-check** - CI monitoring and auto-fixing
- **pr-review** - Code review and feedback

## Prerequisites

- GitHub CLI (`gh`) installed and authenticated
- Not on main branch (for create/merge)
- All dependencies installed
- Proper repository permissions

Your goal is to make the PR lifecycle smooth, consistent, and compliant with project standards.
