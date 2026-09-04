# Templates

Both are distilled from the wave-b parallel lanes that ran successfully on
2026-09-01 (plans wave-b-par-b4 / wave-b-par-b5).

## Single-phase parallel plan (per issue)

Save outside the repo; run with cwd = the worktree:

```bash
cd <worktree> && CARGO_TARGET_DIR="$LOCALAPPDATA/fleet-workstation/lanes/<lane>" \
  edda conduct run <abs-path-to-yaml> --agent pi
```

```yaml
name: wave-<wave>-par-<issue>
purpose: >
  Parallel lane for issue #NNN, split out of <origin plan> (its depends_on was
  declarative, not real). Worktree <path>, branch <branch>, lane <lane>.
  Symbol ownership: <owned paths/symbols>. FORBIDDEN: <peer-owned symbols> —
  those belong to the concurrent #MMM lane.
tags: [<wave>, parallel]

phases:
  - id: par-NNN-<slug>
    context: >
      Issue #NNN. Branch <branch>, already checked out in this worktree.
      Scope: <one line>. Open the PR; never merge.
    prompt: |
      Work ONLY on GitHub issue #NNN (<title gist>).
      Read it first: gh issue view NNN --comments

      Setup: you are ALREADY in a dedicated git worktree on branch <branch>
      based on origin/main (<basis short SHA>). Do NOT run git checkout main,
      do NOT pull, do NOT create another branch. Work here.

      First run `sh scripts/githooks/install.sh` to enable the git-native
      pre-commit / commit-msg hooks — they enforce the commit-hook subset of
      the L0 gates (fmt, touched-crate clippy, markdown lint, 1 MB cap,
      conventional commits) at every commit, so you do not have to remember
      them. `cargo test -p <crate>` stays a manual L0 step; CI runs it as
      well.

      Its doneWhen is the ceiling — do not exceed it:
        - <doneWhen items verbatim>

      <issue-specific landmines: reuse-what-exists pointers, out-of-scope lines>

      OWNERSHIP BOUNDARY (concurrent agents are working in <area> right now):
      do NOT touch <FORBIDDEN symbols/files>. If your change seems to need
      them, stop and say so in the PR body instead.

      Any regression test must be verified to FAIL without the fix
      (stash it, run, restore).
      Run until green:
        cargo fmt --all --check
        cargo clippy -p <crate> --all-targets -- -D warnings
        cargo test -p <crate>
      Commit with a conventional message, push with:
        git push -u origin <branch>
      then open the PR with "Closes #NNN", a short summary, and the gates that
      ran green. Do NOT merge and do NOT enable auto-merge. Print the PR URL
      as your final output.
    timeout_sec: 3600
    check:
      - type: cmd_succeeds
        cmd: "cargo fmt --all --check"
        timeout_sec: 300
      - type: cmd_succeeds
        cmd: "cargo clippy -p <crate> --all-targets -- -D warnings"
        timeout_sec: 2400
      - type: cmd_succeeds
        cmd: "cargo test -p <crate>"
        timeout_sec: 3000
```

No verdict gates. Checks inherit the lane env because they run in the plan's
process environment.

## Independent reviewer brief (per PR, dispatched on PR-open)

```text
You are an INDEPENDENT reviewer for PR #P in <repo>. The implementer was a
different agent; the controller cannot self-review. Review only — no fixes,
no merge.

1. gh pr view P --json headRefOid — the verdict binds that exact SHA.
2. gh issue view NNN --comments — its doneWhen is the acceptance ceiling:
   <doneWhen verbatim, plus any comment that changes the fix>
3. Read code WITHOUT touching the working tree (agents occupy it):
   gh pr diff P, plus git fetch + git show origin/<branch>:<path>.
4. Evidence: required check "CI Gate" green on the head SHA — READ it
   (ci.merge-gate). Docs-only PRs may show skipped clippy/test jobs — that is
   ci.path-filter, not a missed run. Know the coverage gap:
   Windows CI tests only the 7-crate subset; a changed crate outside it earns
   a focused local check, otherwise do NOT build (lanes occupied).
5. IN SCOPE (bounded complete review — audit ALL of it): changed
   behavior/paths, direct callers/consumers, issue doneWhen, introduced
   security/data-loss risk, current-base integration. Regression tests must
   genuinely fail without the fix — reason about it. Adjacent/pre-existing
   defects → FOLLOW-UP ISSUE, not blockers.
6. Batch ALL blocking P0/P1 first, then post ONE comment via
   gh pr comment P --body-file <tempfile>:

   ## Code Review: Round <N>
   Reviewed full SHA: <sha>
   IN SCOPE: <one line>
   Blocking: P0=<n>, P1=<n>
   <findings with path/symbol + failure scenario, or "none">
   FOLLOW-UP ISSUE: <links or none>
   Evidence:
   - READ: <CI + receipts>
   - RAN: <what, and why CI did not cover it>
   Verdict: LGTM | Changes Requested
```

## Merge-tail check (per PR, after LGTM)

1. Current-head LGTM comment, P0=0/P1=0.
2. Required check "CI Gate" green on that head (ci.merge-gate; docs-only
   PRs may show skipped clippy/test jobs — ci.path-filter, not a missed run).
3. SHA window: verdict SHA == headRefOid right now.
4. Authorization window covers this PR.
5. After merging: for each remaining open PR, intersect its
   `gh pr diff --name-only` with the merged diff. Disjoint ⇒ leave base alone
   (no rebase, no re-review). Intersecting ⇒ rebase ⇒ push voids verdict ⇒
   delta review.
