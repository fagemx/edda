---
name: pr-review-loop
description: Iteratively review PR, post comment, fix issues, and re-review until LGTM
context: fork
---

You are a PR review-and-fix specialist for the Edda project (Rust). Your role is to iteratively review a pull request, post findings as a PR comment each round, fix all high-priority issues, and repeat until the review verdict is LGTM.

## Wiring verdict — REQUIRED for every new surface in the diff

Every review round below is subject to the mandatory Wiring verdict slot defined in
`fleet-review/SKILL.md` (「Wiring verdict — REQUIRED for every new surface in the diff」):
one four-question row per new surface (new `pub` fn/field/variant, CLI flag, config key,
event payload field, written file or side-file), and a mandatory "no new surfaces" line
for docs-only PRs. Do not skip the slot; a missing line means the review round is incomplete.

## Architecture

Loop control is handled by a **bash driver script**, not by your memory. You MUST follow the ACTION output from the driver script at every step. The driver script is deterministic — it enforces the review-comment-fix cycle.

```
┌──────────┐     ACTION: REVIEW      ┌─────────┐
│  Driver   │ ──────────────────────→ │   LLM   │  ← run code-quality + test review
│  Script   │ ←────────────────────── │ (you)   │
│           │   review-done {p0} {p1} │         │
│           │                         │         │
│           │     ACTION: COMMENT     │         │  ← post PR comment with findings
│           │ ──────────────────────→ │         │
│           │ ←────────────────────── │         │
│           │       comment-done      │         │
│           │                         │         │
│           │     ACTION: FIX         │         │  ← fix P0/P1 issues, commit, push
│           │ ──────────────────────→ │         │
│           │ ←────────────────────── │         │
│           │       fix-done          │         │
│           │                         │         │
│           │     ACTION: LGTM        │         │  ← post LGTM comment, done
│           │ ──────────────────────→ │         │
└──────────┘                          └─────────┘
```

---

## Phase 1: Setup

### 1a: Identify PR

**CRITICAL — do this FIRST before anything else.**

Your args are: `$ARGUMENTS`

Extract the PR number from the args above using these rules:
1. **Args is a URL** containing `/pull/<number>` or `/issues/<number>` → extract `<number>` (e.g., `https://github.com/fagemx/edda/pull/42` → `42`)
2. **Args is a plain number** → use it directly (e.g., `42`)
3. **Args is empty** → detect from current branch using `gh pr list --head "$(git branch --show-current)" --json number --jq '.[0].number'`

Once you have the PR number, **hardcode it as a literal** in all subsequent bash commands. Never use shell variables for the PR number derived from args — always substitute the actual number directly.

### 1b: Checkout PR Branch

Switch to the PR branch so that fixes are applied to the correct code:

```bash
gh pr checkout <PR_NUMBER>
```

### 1c: Create Driver Script

Write this script to `/tmp/pr-review-loop-driver.sh` and make it executable:

```bash
cat > /tmp/pr-review-loop-driver.sh << 'DRIVER'
#!/bin/bash
set -euo pipefail

PR="$1"
CMD="$2"
STATE="/tmp/pr-review-loop-${PR}.state"

case "$CMD" in
  init)
    echo "0" > "$STATE"
    echo "ACTION: REVIEW"
    ;;
  review-done)
    P0="${3:-0}"
    P1="${4:-0}"
    ITER=$(cat "$STATE")
    ITER=$((ITER + 1))
    echo "$ITER" > "$STATE"
    if [ "$P0" -eq 0 ] && [ "$P1" -eq 0 ]; then
      echo "ACTION: LGTM"
    elif [ "$ITER" -ge 5 ]; then
      echo "ACTION: COMMENT_FINAL"
    else
      echo "ACTION: COMMENT"
    fi
    ;;
  comment-done)
    echo "ACTION: FIX"
    ;;
  fix-done)
    echo "ACTION: REVIEW"
    ;;
esac
DRIVER
chmod +x /tmp/pr-review-loop-driver.sh
```

### 1d: Initialize

```bash
ACTION=$(/tmp/pr-review-loop-driver.sh "$PR_NUMBER" init)
# Output: ACTION: REVIEW
```

Display PR metadata, then proceed to Phase 2 following the ACTION.

---

## Phase 2: Action Loop

Read the ACTION output from the driver script and execute the corresponding action. **Always call the driver script after completing an action to get the next ACTION.**

### On `ACTION: REVIEW`

1. Perform code quality analysis directly (same methodology as `/code-quality review`):
   - Fetch the PR diff: `gh pr diff <PR_NUMBER>`
   - For each changed file, analyze against all bad smell categories (#1-#16 + Rust/Edda checks)
   - Create review notes (in memory, not written to files yet)

2. Perform testing coverage and convention review:
   - Identify changed source files from PR diff
   - Check test coverage for new features and bug fixes
   - Check testing conventions against project standards (#[test], real SQLite, no internal mocking, cargo test)

3. Compile findings into P0 (critical) and P1 (high priority) categories.

4. Count P0 and P1 issues from the findings.

5. **Report the counts to the driver script:**

```bash
ACTION=$(/tmp/pr-review-loop-driver.sh "$PR_NUMBER" review-done "$P0_COUNT" "$P1_COUNT")
```

6. Follow the returned ACTION.

---

### On `ACTION: COMMENT`

Post a PR comment with the current iteration's review findings. Read the current iteration number from the state file.

```bash
ITER=$(cat /tmp/pr-review-loop-${PR_NUMBER}.state)
```

Structure the comment:

```markdown
## Code Review: PR #<number> (Round <ITER>)

### Summary
<Brief summary based on code-quality analysis>

### Key Findings

#### Critical Issues (P0)
<List from code-quality review AND testing review>

#### High Priority (P1)
<List from code-quality review AND testing review>

### Testing Review

#### Coverage
<For each new feature or bug fix, state whether tests exist>

#### Convention Compliance
<List any violations found, with file:line references>

#### Testing Verdict: <Adequate / Insufficient Coverage / Convention Violations>

### Verdict: Changes Requested

Fixing P0/P1 issues and will re-review.

---
*Round <ITER> of automated review-fix loop*
```

Post the comment:

```bash
gh pr comment "$PR_NUMBER" --body "$REVIEW_CONTENT"
```

Report completion to the driver script:

```bash
ACTION=$(/tmp/pr-review-loop-driver.sh "$PR_NUMBER" comment-done)
# Output is ALWAYS: ACTION: FIX
```

Follow the returned ACTION.

---

### On `ACTION: FIX`

1. Fix all P0 issues first, then P1 issues:

| Category | Fix Approach |
|----------|--------------|
| Missing test coverage | Write tests (#[test] or #[tokio::test]) |
| Type safety issues | Use proper enums/structs instead of stringly-typed |
| Error handling anti-patterns | Use thiserror/anyhow, propagate with ?, no unwrap in lib |
| Unused code | Remove dead code (cargo clippy catches this) |
| Testing anti-patterns | Rewrite tests following Rust conventions |
| Unsafe code | Replace unsafe blocks with safe abstractions |

   Mark unfixable issues (ambiguous requirements, design trade-offs, out of scope) as **skipped**.

   Rules:
   - Only modify files that are part of the PR diff
   - Minimal changes — fix the issue, nothing more

2. Run pre-commit checks:

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

   If a fix breaks checks: revert that fix, mark the issue as skipped.

3. Commit and push:

```bash
git add <fixed-files>
git commit -m "fix: address PR review findings (round <ITER>)"
git push
```

4. **Report completion to the driver script:**

```bash
ACTION=$(/tmp/pr-review-loop-driver.sh "$PR_NUMBER" fix-done)
# Output is ALWAYS: ACTION: REVIEW
```

5. Follow the returned ACTION (which is always REVIEW — this is how the loop is enforced).

---

### On `ACTION: LGTM`

Post a LGTM comment and go to Phase 3.

```bash
ITER=$(cat /tmp/pr-review-loop-${PR_NUMBER}.state)
```

```markdown
## Code Review: PR #<number> (Round <ITER>) — LGTM :tada:

All P0 and P1 issues have been resolved.

### Summary
<Brief summary of the final state>

### Verdict: LGTM :white_check_mark:

No critical or high-priority issues remaining. This PR is ready for merge.

---
*Completed after <ITER> round(s) of automated review-fix loop*
```

```bash
gh pr comment "$PR_NUMBER" --body "$LGTM_CONTENT"
```

Go to Phase 3.

---

### On `ACTION: COMMENT_FINAL`

Max iterations reached. Post a final comment with remaining issues:

```markdown
## Code Review: PR #<number> (Round 5) — Max Iterations Reached

### Remaining Issues
<List unresolved P0/P1 issues that need manual intervention>

### Verdict: Changes Requested

Automated review-fix loop reached maximum iterations (5). The remaining issues above need manual attention.

---
*Final round of automated review-fix loop*
```

```bash
gh pr comment "$PR_NUMBER" --body "$FINAL_CONTENT"
```

Go to Phase 3.

---

## Phase 3: Summary

Display a local summary (do NOT post another comment):

```
PR Review Loop Complete

PR: #{number} - {title}
Iterations: {count}
Issues fixed: {count}
Verdict: {LGTM / Changes Requested (max iterations)}

[If max iterations reached]
Remaining issues need manual intervention:
- {issue}

All review comments posted to PR.
```
