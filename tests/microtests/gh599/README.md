# gh599 micro-test — fleet-epic-split rewrite (old vs new), dry run

Old vs new decomposition of the same input, prompts identical except the
embedded skill text: old arm embeds fleet-epic-split from `origin/main`
(the 38-line pre-absorption text), new arm embeds this branch's HEAD text
(`git show HEAD:.claude/skills/fleet-epic-split/SKILL.md`). Input is the
Stage 2 section of epic #560 (`input-stage2.md`, extracted verbatim via
`gh issue view 560 --repo fagemx/edda --json body -q .body`). Every run is a
**dry run**: the prompt forbids creating/editing/commenting on issues and
requires the confirmation table plus every would-be issue body.

The new skill points at `.claude/skills/issue-intake/templates.md` for the
single body contract; the run happens inside the repo worktree, so the
pointer must resolve for the new arm to emit bodies — the pointer wiring is
part of what is tested.

Model `z-ai/glm-5.3-flash`, read-only (`--exclude-tools edit,write`), **N=1
run per arm** (per the lane brief: one run each, no more). Session ids
`microtest-gh599-old-<ts>` / `microtest-gh599-new-<ts>`. Committed outputs
are from run `20260902-160921`.

Portable invocation of the `<rev>:<path>` form on this workstation (Git Bash)
needs `MSYS_NO_PATHCONV=1` — see ../gh594/README.md. `run.sh` does not invoke
`git show` at all; it reads the committed prompt files.

Replay: `sh tests/microtests/gh599/run.sh` — overwrites
`out-old-1.md` / `out-new-1.md`.

## Scoring rule (applied verbatim to every run)

- **Confirmation table**: the run passes if it prints a confirmation table
  (one row per proposed issue, plus the skip list of duplicates) BEFORE any
  issue creation, and stops there (dry run — no `gh issue create`).
- **Surface fields**: the run passes if every would-be issue body it prints
  contains both a `Suspected surface` section and a `Predicted surface`
  section. Zero would-be bodies makes this vacuously true — reported as
  such, never counted as a positive demonstration.
- **Old is the control**: the old text has no dedupe procedure over open
  issues and no operator-confirmation gate; its expected failure mode is
  proposing issues that duplicate already-open ones.

## Result (run 20260902-160921, quoted lines scored on)

| Run | Confirmation table (dry-run stop) | Surface fields in would-be bodies | Dedupe over open issues |
|---|---|---|---|
| old-1 | table printed, 4 proposed — "## ③ Propose — 確認表 … | A | feat(edda-notify): 新增 conductor plan 事件變體與三格式渲染 |…" | n/a — old text has no such step; its only dedupe note is "dedupe 來源：operator-runbook 缺口表全列（operator-runbook.md:94-99），無重複單" — a docs gap table, never `gh issue list` |
| new-1 | table printed, 0 proposed — "## ④ 確認表（dry run — 停在此步）… | — | **（無擬建單）** |…" plus skip list "Stage 2 phase terminal-state 通知 → **#564** … Stage 2 gate-entry routing（sibling）→ **#545**" | vacuous (0 bodies) — reported as such | four procedures run with named queries — "候選A phase terminal-state notify | queries: 文件内引用→…; edda ask "notify"→命中 epic560.stage1-slice…; edda search "terminal-state notification"→…; gh issue list 模糊比對→#564 標題逐字相符 | verdict: duplicate of #564" |

Provenance (new arm only — the old text has no provenance requirement):
"provenance：epic issue #560「Stage 2 — event-driven delivery」節；決策 key
`epic560.stage1-slice`".

**Old: 4 would-be issues proposed, duplicates of open #564/#545 among them
(overlap verified at scoring time via `gh issue view 564` / `gh issue view
545` — both OPEN, titles "feat(conductor): phase terminal-state notifications
through edda-notify channels" / "feat(conductor): route the gate-entry
notification through edda-notify channels instead of hardcoded stdout").
New: 0 would-be issues, both Stage 2 items caught by dedupe with a query
trail, confirmation table + skip list printed, provenance back-linked.**

## Honest reading

1. **Same input, opposite outcomes — and the difference is the absorbed
   capability.** The old arm proposed four issues, three of which
   substantially duplicate open #564/#545 (it never ran an open-issue
   check; the old text has no such step). The new arm ran all four dedupe
   procedures, matched both Stage 2 items to open issues, and proposed
   zero — exactly the "done things get no issue" behavior the rewrite
   requires. This is direct evidence for the dedupe + confirmation-table +
   provenance absorption, on an input whose items were already filed.
2. **The run does NOT demonstrate the emitted body contract.** The new arm
   printed zero bodies, so the `Suspected surface`/`Predicted surface`
   criterion is vacuously satisfied for it; no claim is made that the new
   skill's body output was observed, and the templates.md pointer was
   never exercised (with zero bodies the model had no reason to read it).
3. **Environment leak into the control.** The old arm's bodies contained
   `Predicted surface` and Wiring-audit sections even though the old skill
   text never mentions them — with read tools available, the model picked
   the convention up from repo files. So this run cannot attribute any
   body-format difference to the skill text.
4. **One run per arm.** No repeatability claim is made; a single cheap-model
   run is evidence of what happened, not of a stable behavioral
   differential.
