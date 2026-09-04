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
comment on that issue (skill-bearing strong-model sessions get principles
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
role: worker · lane: worker-1 · task id: 12 · issue: #792 ·
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

```text
role: worker · lane: lane-655 · task id: 12 · issue: #792 ·
base full SHA: fb6ab1b503c3abb8502b3964678977aa23d316c4 ·
scope paths: docs/guides/brief-template.md, docs/README.md, AGENTS.md ·
entry: none: procedure below · gate owner: not you; review queue ·
out-of-scope: skills/, crates/, CI config, REVIEW.md (owned by the #820 lane)

1. git status --porcelain --branch
   output schema: one line `## docs/gh792-brief-template` and nothing else;
   any other line → stop and report that line verbatim.
2. gh issue view 792 --json body --jq .body
   output schema: the issue body on stdout, exit 0. Acceptance is the
   doneWhen items of #792, read from that body; any command error → stop
   and report it verbatim.
3. git rev-parse HEAD
   output schema: exactly one 40-character hexadecimal string; that string
   is the base full SHA for the REPORT.
4. Write the three owned paths, in this order, with the session's
   file-write/edit tool. If the session has no file-write tool, stop and
   report `no file-write tool`:
   4a. docs/guides/brief-template.md
   4b. docs/README.md
   4c. AGENTS.md
   output schema: each write completes; no path outside these three is
   modified.
5. git diff --name-only
   output schema: exactly the three owned paths, one per line; any path
   outside them → stop and report those paths verbatim; do not restore,
   checkout, reset, or clean.
6. sh scripts/lint-markdown-content.sh
   output schema: empty stdout, exit 0; any printed line is a failure to fix
   before step 7.
7. git add docs/guides/brief-template.md docs/README.md AGENTS.md &&
   git commit -m "docs(fleet): brief template for role × runtime composition" \
     -m "Issue: #792"
   output schema: one `files changed` summary line; the commit message
   carries `Issue: #792` as its own line.
8. git push -u origin docs/gh792-brief-template
   output schema: one `To <url>` line plus one `branch ... set up to track`
   line; paste both into the REPORT.
9. Finish step — the REPORT carries these three sentences as its output
   schema (REPORT fields, one sentence each; they are not git commands):
   (a) the commit message carries `Issue: #792` as its own line;
   (b) the PR body carries `Issue: #792` as its own line;
   (c) pr.closing-keyword: `Closes #792` is allowed only because every
   doneWhen item of #792 is delivered; with any item undelivered the body
   links the issue without a closing keyword.
```
