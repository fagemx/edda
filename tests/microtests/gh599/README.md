# gh599 micro-test — fleet-epic-split rewrite (old vs new), attributable text-only run

Old vs new decomposition of the same input, prompts identical except the
embedded skill text. Redesigned after PR #653 review round 1 so the
differential can only come from the skill text:

- **(a) Isolated cwd**: `run.sh` runs both arms from an empty `mktemp -d`
  directory, so no tool can read this repo's files — the old arm cannot
  import the new conventions from the worktree (round-1 finding).
- **(b) Byte-identical shared frame, no added requirements**: the frame is
  the first line before `=== SKILL BEGIN ===`, everything from
  `=== SKILL END ===` on, and nothing else. It adds no confirmation-table
  demand and no body-shape demand; its only rule is the dry-run/isolation
  sentence quoted below.
- **(c) Embedded input and skill text**: input is the Stage 2 section of
  epic #560 (`input-stage2.md`, extracted verbatim from
  `gh issue view 560 --repo fagemx/edda --json body -q .body`); old arm
  embeds the `origin/main` blob of `.claude/skills/fleet-epic-split/SKILL.md`
  (38 lines), new arm embeds the HEAD blob.
- **(d) N=1 per arm** (per issue #599), dry run only — no issue is created.

The shared frame in full (identical in both prompts):

> Dry run: do not create issues. Recon is unavailable here — treat every
> item in the input as not yet done. Print exactly what this skill would
> output at each step.

Model `z-ai/glm-5.3-flash`, read-only (`--exclude-tools edit,write`), session
ids `microtest-gh599-old-<ts>` / `microtest-gh599-new-<ts>`. Committed
outputs are from run `20260902-164626`.

## Workstation command probes (Git Bash, `MSYS_NO_PATHCONV` unset/empty)

Observed on this workstation while writing this README — exit codes only:

```text
$ git show HEAD:.claude/skills/fleet-epic-split/SKILL.md >/dev/null 2>&1; echo $?
0
$ git show origin/main:.claude/skills/fleet-epic-split/SKILL.md >/dev/null 2>&1; echo $?
128
```

The `origin/main:` invocation fails with `fatal: ambiguous argument
'origin\main;.claude\skills\fleet-epic-split\SKILL.md'` — the argument is
mangled. An earlier version of this README generalized this into a
"prefix with `MSYS_NO_PATHCONV=1`" claim; that generalization is deleted.
`run.sh` does not invoke `git show` at all — it reads the committed prompt
files — so replay is unaffected either way.

Replay: `sh tests/microtests/gh599/run.sh` — overwrites
`out-old-1.md` / `out-new-1.md`.

## Scoring rule (applied verbatim to every run)

- A run **catches** if it prints a confirmation table AND every proposed
  issue body has a `Suspected surface` field AND a `Predicted surface`
  field. Zero proposed bodies is a miss for the arm that proposes them
  (a finding about that arm's skill text, not the prompt).
- Both arms are scored the same way; no criterion favors either arm.

## Result (run 20260902-164626)

| Run | Confirmation table | `Suspected surface` in proposed bodies | `Predicted surface` in proposed bodies | Verdict |
|---|---|---|---|---|
| old-1 | none — the old text has no pre-create operator gate; it flows ①recon → ②decompose → ③propose straight to would-be `gh issue create` commands ("以下為兩張 issue body 草稿與**將會執行**（dry run 不執行）的建單指令") | absent — both bodies use the old format `## 背景 / ## 改哪裡 / ## doneWhen / ## verify / ## 獨立性 / ## 尺寸`; no surface section exists in either | absent — same | **miss** (0 of 2 criteria; no dedupe step exists in the old text) |
| new-1 | printed — "## ④ 確認表（dry run → 停在本步，不建任何 issue）" with 2 rows (擬建-1, 擬建-2) + skip list ("gate-entry routing → skip… #545") | present — "## Suspected surface / Phase 生命週期／終態判定所在的元件…" (擬建-1) and "## Suspected surface / Controller 的事件處理／訂閱端…" (擬建-2) | present — "## Predicted surface / phase 終態發射點…" (擬建-1) and "## Predicted surface / controller 訂閱端…" (擬建-2) | **catch** (3 of 3) |

**Old 0/3 · New 3/3 (N=1 per arm).**

Additional observed behavior, quoted:

- New arm ran the four dedupe procedures and, finding no repo/ledger in the
  isolated cwd, recorded them per the skill's isolation clause: "2. `edda
  ask`…：`unavailable（無 repo/ledger 環境）` … 4. open-issue 模糊比對…：
  `unavailable`" — and marked both rows "new（**unverified**）" for the
  operator to re-check. It also honored the in-document `#N` rule: "#545 …
  gate-entry routing 本身**不生單**（已存在 #545）" (skip list).
- New arm could not read `issue-intake/templates.md` in isolation and
  emitted all sections from the skill's fixed section order instead:
  "templates.md 讀不到（啟用隔離環境的固定節順序 fallback）".
- New arm emitted provenance footers ("Provenance: operator 對話 2026-09-02
  — …逐字引述"); the old text has no provenance requirement and emitted none.

## Honest reading

- The round-1 confounds are removed by construction: both arms ran in the
  same empty temp dir (no repo files to leak conventions), and the frame
  demanded no table and no body shape. What differs is the skill text, and
  the outputs differ exactly on the dimensions the rewrite owns (operator
  gate, dedupe, surface fields, provenance) while matching on what both
  texts share (recon-first posture, independence three-question, 30-cap,
  two-candidate split, dry-run obedience).
- `N=1 per arm`. No repeatability or generalization claim is made: one
  cheap-model run shows what happened, not a stable differential. The new
  arm's isolation fallbacks (`unverified` dedupe, fixed-section bodies) are
  exercised here for the first time and were scored as observed.
- The proposed bodies' `Predicted surface` values are necessarily
  placeholder-shaped ("paths 待 recon 後補") — recon was unavailable by
  design; no quality claim is made about surface prediction.
