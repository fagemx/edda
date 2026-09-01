# RED: baseline failures (observed, not synthetic)

Real behavior in the edda fleet WITHOUT this skill, 2026-08 ~ 09:

1. **Issues enter delivery without doneWhen.** #547 had no doneWhen list; the
   wave author had to synthesize acceptance inside the plan prompt ("the issue
   has no doneWhen list, so this is it"). Acceptance invented at dispatch time
   is acceptance nobody reviewed.
2. **Issues enter delivery without a named surface.** Parallel judgment
   (parallel-wave Layer 1) then cannot derive a write surface, forcing manual
   crate-map archaeology per issue at dispatch time.
3. **Findings die in the transcript.** Runtime defects observed mid-session get
   discussed and lost unless someone happens to file them. The GH-556 skip-loss
   defect was nearly lost the same way — it was filed only because the
   controller was in the middle of writing the intake design.
4. **Vague queue-fillers.** Pressure to "keep the queue full" produces
   "investigate X" / "improve ergonomics" issues with no expected-vs-actual and
   no evidence (e.g. the #605-style entries seen in planning exercises).
5. **No dedupe against the ledger.** Decisions and prior fixes live in edda's
   ledger; issues get filed for things already decided or already fixed unless
   someone remembers to run `edda ask` / `edda search` first.
6. **Author reviews own spec.** An agent that wrote the issue's doneWhen later
   verifies the closing PR against it — compliance-checking instead of
   design-questioning (memory: no-self-review-of-authored-specs).

GREEN criterion: given a mixed bag of raw observations (evidenced defect /
vague hunch / lint-automatable rule / duplicate of a ledger decision / runtime
failure in a receipt), a fresh agent must: disposition each correctly
(file / keep-as-candidate / automate-not-file / dedupe-drop), produce a
correctly-shaped issue body for the fileable one (surface + doneWhen +
evidence + failing-first regression requirement), and respect the
author-does-not-review rule.
