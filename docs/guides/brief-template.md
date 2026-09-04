---
title: Lane Brief Template
---

# Lane Brief Template

Every brief a controller hands to a worker, verifier, or lane is composed from
this template — by hand until the rail renders it (#793). It exists because the
failure classes repeat: facts rediscovered under pressure, procedure that
balloons a strong model does not need, and a flash lane improvising around
phrasing that delegates choices back to it. It implements the rulings
recorded on [Issue #792](https://github.com/fagemx/edda/issues/792) — the
issuer welds context into the brief at dispatch, and flash-tier briefs are
exhaustive, enumerated, and fixed-schema — plus the 2026-09-03 operator
ruling in that issue's body (skill-bearing strong-model sessions get principles
with reasons, not procedures).

## The two axes

### Role

- **worker** — implements a scoped bundle; owns no gate.
- **verifier** — audits a delivery candidate; runs the review ladder.
- **controller** — assigns bundles, adjudicates mid-flight, holds the merge
  decision pending explicit operator authority.

### Runtime (three values)

1. **Skill-bearing session with edda hooks** (Claude Code, Codex) — the pack is
   injected by a hook, so the brief stays short.
2. **Skill-less flash-tier lane launched by a controller** (`pi` on
   glm-5.3-flash, brief pasted by the launcher) — the brief is the lane's only
   context, so it is exhaustive, enumerated, and fixed-schema.
3. **Hook-less interactive session** (a pi TUI, or any session edda did not
   launch, on a strong model) — no hook injected the pack, so the brief's
   `entry` field is not a skill name but the three start commands in
   [AGENTS.md](../../AGENTS.md), section "Session start without edda hooks".

## Facts block — all roles, all runtimes

Fixed field order, every field mandatory. Write `none`; never omit a field.

| # | Field | Content |
| - | - | - |
| 1 | role | worker / verifier / controller |
| 2 | lane | the build lane, or `none` when the session compiles nothing locally |
| 3 | task id | the rail task, or `none` |
| 4 | issue | the GitHub issue number (`#N`) |
| 5 | base full SHA | the full 40-character SHA the work is based on |
| 6 | scope paths | the paths this session may touch |
| 7 | entry | runtime 1: the skill name · runtime 2: `none: procedure below` · runtime 3: the three `edda` start commands from AGENTS.md |
| 8 | gate owner | always `not you; review queue` |
| 9 | out-of-scope list | the adjacent paths and concerns explicitly excluded |

Acceptance is never restated in the brief. The template forbids copying
doneWhen from the issue: the brief carries the issue number, and the reader
pulls doneWhen from the issue at start.

## Principles block — skill-bearing runtime only

At most 6 lines. Each line is one principle plus the reason it exists:

- Confirm the target is still live before any expensive read — a reviewer once
  read all 727 lines of REVIEW.md for a PR that merged mid-read.
- Verify once per frozen SHA and cite that recorded result elsewhere — a rerun
  without a stated reason is a process finding, not diligence.
- Only result lines of build/test output enter context — unfiltered tool
  output once flooded a lane's entire budget.
- The PR body always carries `Issue: #N` as its own line, and a closing
  keyword (`Closes #N`) is allowed only when every doneWhen item of that issue
  is delivered (`pr.closing-keyword=only-when-all-donewhen-delivered`) — the
  earlier blanket ban on closing keywords was narrowed, and closing early
  hides undelivered acceptance items.

## Procedure block — flash-tier runtime only

Every step is enumerated, names its exact command, and states the fixed output
schema the step must produce. Phrasing that delegates a choice back to the
lane is banned — the template's acceptance grep over this file must return
zero hits in the procedure block and the examples.

Bind the launcher, shell, and native editing tools in the brief. A native tool
call must name the tool and its argument schema; an editor variable or a bare
instruction to edit a path is not an executable step. File contents are the
worker's implementation output, supplied as strings to that bound tool. The
issuer fixes paths, command order, output checks, and failure handling.

The finish step is mandatory and carries three one-sentence statements; the
lane will not infer them:

1. The commit message carries `Issue: #N` as its own line.
2. The PR body carries `Issue: #N` as its own line — REVIEW.md U3 checks the
   body, and the GH-742 lane shipped a commit trailer without it.
3. `pr.closing-keyword`: state whether a closing keyword is allowed for that
   PR — allowed only when every doneWhen item of the issue is delivered.

## PR-body evidence standard

The worked example of the evidence a PR body must carry is the description of
PR #790: a control-group table (base vs branch, repeated runs), timestamps
proving binary freshness, and the root cause stated before the diff. Evidence,
not assertion.

## Worked examples

Both briefs dispatch work on the same issue (#792). The only difference between
them is the runtime axis.

### Example A — Claude Code worker (runtime 1, skill-bearing)

```text
role: worker · lane: none · task id: 12 · issue: #792 ·
base full SHA: fb6ab1b503c3abb8502b3964678977aa23d316c4 ·
scope paths: docs/guides/brief-template.md, docs/README.md, AGENTS.md ·
entry: $coord-sync · gate owner: not you; review queue ·
out-of-scope: skills/, crates/, CI config, REVIEW.md (owned by the #820 lane)
- Confirm the target is still live before any expensive read — a reviewer once read 727 lines of a spec whose PR had already merged.
- Verify once per frozen SHA and cite that recorded result elsewhere — a rerun without a stated reason is a finding, not diligence.
- Only result lines of build/test output enter context — unfiltered tool output once consumed a whole lane budget.
- The PR body carries `Issue: #792` as its own line; `Closes #792` is allowed only when every doneWhen item of #792 is delivered.
- sh scripts/lint-markdown-content.sh → exit 0, no output.
```

### Example B — pi flash-tier lane (runtime 2, skill-less)

Launcher contract for this example: the controller prepares an exclusively
owned, clean worktree on `codex/gh792-brief-template` at the facts-block SHA,
with task 12 assigned and no existing PR for that branch. It launches pi
0.84.4 with `--tools bash,write,edit`, Git Bash as the shell, and authenticated
`git`, `gh`, and `edda` on PATH. No local Cargo build is assigned. Both examples
have these same task, branch, scope, and gate assignments.

The pi 0.84.4 built-in tool schemas are `bash({"command": string})`,
`write({"path": string, "content": string})`, and
`edit({"path": string, "edits": [{"oldText": string, "newText": string}]})`.
The launcher must expose these schemas; a mismatch stops dispatch. In the
procedure, shell commands go in `bash.command`, and `write` / `edit` lines
are native tool calls, not shell programs. `GUIDE_TEXT` below denotes the
worker-authored Markdown string implementing #792, JSON-encoded as `content`;
it is the only authored payload slot, not a command or an environment variable.

```text
role: worker · lane: none · task id: 12 · issue: #792 ·
base full SHA: fb6ab1b503c3abb8502b3964678977aa23d316c4 ·
scope paths: docs/guides/brief-template.md, docs/README.md, AGENTS.md ·
entry: none: procedure below · gate owner: not you; review queue ·
out-of-scope: skills/, crates/, CI config, REVIEW.md (owned by the #820 lane)

Failure rule for every numbered step: a nonzero command exit, tool error,
or output-schema mismatch stops execution. Report exactly
STOP step=<number> output=<verbatim unexpected output>. Preserve all files;
do not restore, checkout, reset, clean, retry, review, or merge. The controller
issues the next brief. Success advances to the next numbered step.

1. gh issue view 792 --repo fagemx/edda --json state --jq .state
   output: `OPEN`.
2. git status --porcelain=v1 --untracked-files=all --branch
   output: exactly ## codex/gh792-brief-template, no other lines.
3. git rev-parse HEAD
   output: fb6ab1b503c3abb8502b3964678977aa23d316c4 (the base full SHA).
4. edda context
   output: context text, exit 0; an off-limits overlap with a scope path
   is a STOP under the failure rule.
5. edda task list
   output: task-list text, exit 0, including assigned task 12.
6. edda task show 12
   output: task 12 and this brief, exit 0. This is the #793 interim command.
7. edda claim gh792-worker --paths docs/guides/brief-template.md --paths docs/README.md --paths AGENTS.md
   output: successful claim for gh792-worker and those three paths, exit 0.
8. cat .claude/CLAUDE.md AGENTS.md docs/README.md
   output: the three complete source documents, exit 0.
9. gh issue view 792 --repo fagemx/edda --json body --jq .body
   output: issue body, exit 0; read acceptance there, do not copy doneWhen
   into the guide. Author GUIDE_TEXT as the complete new guide.
10. write({"path":"docs/guides/brief-template.md","content":GUIDE_TEXT})
    output: `Successfully wrote <integer> bytes to docs/guides/brief-template.md`.
11. edit({"path":"docs/README.md","edits":[{"oldText":"## For contributors and the curious","newText":"For lane issuers: [Lane Brief Template](./guides/brief-template.md).\n\n## For contributors and the curious"}]})
    output: Successfully replaced 1 block(s) in docs/README.md.
12. edit({"path":"AGENTS.md","edits":[{"oldText":"## Multi-session work","newText":"## Session start without edda hooks (pi, or any session edda did not launch)\n\nClaude Code and Codex sessions get the edda pack injected by a hook. If you are\nnot one of those, run these three before touching any file, and treat their\noutput as the pack:\n\n1. `edda context` — decisions, peers, off-limits paths.\n2. `edda task list` — the task rail; pick the task assigned to you.\n3. `edda task show <id>` — reads your brief. Follow it: it names your scope,\n   who owns the gate, and what is out of scope.\n\nUntil #793 lands, step 3 uses `show`; then it becomes `edda task start <id>`.\n\n## Multi-session work"}]})
    output: Successfully replaced 1 block(s) in AGENTS.md.
13. git status --porcelain=v1 --untracked-files=all
    output: exactly these three lines (strip the four-space presentation
    indent; the two M lines each retain one leading space):
     M AGENTS.md
     M docs/README.md
    ?? docs/guides/brief-template.md
14. git add -- docs/guides/brief-template.md docs/README.md AGENTS.md
    output: empty stdout, exit 0.
15. git diff --cached --name-only
    output: exactly AGENTS.md, docs/README.md, docs/guides/brief-template.md,
    in that order, one path per line. Staging makes the new guide visible.
16. git diff --exit-code
    output: empty stdout, exit 0 (nothing changed after staging).
17. git diff --cached --check
    output: empty stdout, exit 0.
18. sh scripts/lint-markdown-content.sh
    output: empty stdout, exit 0. Cargo budget: zero; exact-head CI is the gate.
19. git -c core.quotePath=false diff --cached --stat
    output: three path-stat lines and one summary for exactly 3 files.
20. git commit -m "docs(fleet): brief template for role and runtime composition" -m "Issue: #792"
    output: git commit summary, exit 0. Do not infer a PR-body link from it.
21. git log -1 --format=%B | grep -Fx 'Issue: #792'
    output: Issue: #792.
22. git status --porcelain=v1 --untracked-files=all
    output: empty stdout, exit 0.
23. git rev-parse HEAD
    output: one 40-character hexadecimal SHA; retain as delivery_sha.
24. git push --porcelain -u origin codex/gh792-brief-template
    output: Git porcelain push status, exit 0, no rejected ref. This is a
    normal push; no force option is authorized.
25. cat >"$(git rev-parse --git-path gh792-pr-body.md)" <<'PR_BODY'
## Problem and change
Lane briefs lacked a shared contract for role and runtime. This adds the
guide, docs-map link, and hook-less AGENTS.md entry commands for Issue #792.
Until #793 lands, the third entry command is edda task show <id>.

Issue: #792

## Validation
RAN: sh scripts/lint-markdown-content.sh, exit 0; git diff --cached --check,
exit 0 before commit. No Cargo gate; build lane none.
Pending: exact-head CI and independent review owned by the review queue.
Acceptance and any closing keyword await the gate owner's confirmation.
PR_BODY
    output: empty stdout, exit 0. This file is Git metadata scratch, not a
    source path or an additional staged file.
26. git rev-parse HEAD >>"$(git rev-parse --git-path gh792-pr-body.md)"
    output: empty stdout, exit 0; appends the delivery full SHA to the body.
27. gh pr create --repo fagemx/edda --base main --head codex/gh792-brief-template --draft --title "docs(fleet): brief template for role and runtime composition" --body-file "$(git rev-parse --git-path gh792-pr-body.md)"
    output: one https://github.com/fagemx/edda/pull/<integer> URL, exit 0.
28. gh pr view codex/gh792-brief-template --repo fagemx/edda --json body --jq .body
    output: the body from steps 25–26, including its own Issue: #792 line
    and delivery_sha; no closing keyword. Missing/different content is STOP.
29. edda note 'GH792 worker committed and opened draft PR; docs lint passed; exact-head CI and independent acceptance pending with review queue; no Cargo build.' --tag session
    output: Wrote NOTE <event-id>, exit 0.
30. printf '%s\n' 'The commit message carries Issue: #792 as its own line.' 'The PR body carries Issue: #792 as its own line.' 'pr.closing-keyword: none in this draft; the gate owner must confirm every doneWhen item before adding one.'
    output: exactly those three sentences, one per line. Return these lines
    with delivery_sha and the PR URL from steps 23 and 27 to the controller.
```
