---
name: issue-intake
description: Use when the ready-issue queue is low or a delivery wave is closing, when a defect/finding surfaces mid-session (runtime failure, review discovery, repeated manual step), or when deciding whether an observation deserves a GitHub issue. Also use when authoring any issue body.
---

# Issue Intake

Keep the delivery queue fed with issues that are worth doing and machine-ready
— without the operator. Every issue this engine ships is born in the format
parallel-wave Layer 1 judges: **named surface + machine-checkable doneWhen**.
The body contract lives once in [templates.md](templates.md) — issue-create
and fleet-epic-split emit the same shape and only point here; no parallel
body formats. Composes with fleet-orchestrate (discovery rail) and
parallel-wave (delivery).

## Sources — check in this order

| Source | Trigger | Cost |
|---|---|---|
| Review exhaust | every review round's FOLLOW-UP ISSUE findings | free |
| Runtime scars | any failure the pipeline itself hits (stale transitions, misclassification, lost state, timeout patterns) | free |
| Self-improvement | any manual controller step repeated ≥2 times becomes a roadmap candidate | free |
| Scheduled recon | ready queue < 3 at wave close → run a read-only campaign (charter: fixed object, rotating lenses — correctness / concurrency / data-loss / false-green tests; one reviewer per cell) | paid |
| Epic split | operator-signed goal → fleet-epic-split into atomic issues | paid, operator-gated |

A finding observed mid-session is filed (or parked as a candidate) **in the
same session** — findings that stay in the transcript are lost.

## Promotion ladder

candidate → verified finding → draft → **ready**. Never skip a rung.

| To reach | Requires |
|---|---|
| verified finding | expected vs actual on a named full SHA, with evidence (repro, failing test, trace, or direct code proof); base comparison — regression or inherited? |
| draft | named surface (crate/path/symbols) + machine-checkable doneWhen list |
| ready | dedupe ran: `edda ask "<domain>"` AND `edda search query "<keyword>"` AND open-issue search — no duplicate, not already decided/fixed |

## Disposition table

| Observation | Disposition |
|---|---|
| Evidenced defect, surface derivable | File it — use the body template in [templates.md](templates.md) |
| Real hunch, evidence incomplete | Park as candidate WITH the attempts already made and the next bounded experiment. Do not file "investigate X" |
| Rule a linter/CI check could enforce | Automate it (or file ONE issue to add the check). Never file per-occurrence issues |
| Matches a ledger decision or an existing issue/fix | Drop, with a pointer to the duplicate |
| Product direction / high-risk change | File, label `needs-operator`, queue — it waits for the operator; everything else keeps moving |

## The improvement loop (what "gets better" means)

At each wave close, write one `edda note --tag intake` line covering:
- review findings by category — a category recurring across waves becomes a new
  campaign lens AND a landmine line in future plan prompts;
- Layer-1 predicted surface vs Layer-3 actual `gh pr diff --name-only` delta —
  chronic misprediction rewrites the surface-authoring guidance;
- doneWhen adequacy — a real bug caught only OUTSIDE the acceptance ceiling
  means the doneWhen template gains that class of item.

## Operator-absent authority

- Standing (once granted): file evidenced issues, label `fleet:pending`;
  low-risk classes (fix/test/docs in named crates) may enter delivery.
- Queued, never improvised: direction and high-risk issues wait under
  `needs-operator`.
- Anti-corruption is structural: the issue's author never reviews the PR that
  closes it; promotion to verified finding needs an independent check; flooding
  is blocked by the ladder, not by restraint.

## Red flags — stop, you are rationalizing

| Thought | Reality |
|---|---|
| "I'll write it up at the end of the session" | Findings die in transcripts. File or park it now. |
| "The queue is low, this hunch will do" | An unevidenced issue costs a whole delivery lane later. Park it as a candidate. |
| "doneWhen is obvious, the implementer will know" | #547 shipped without doneWhen and acceptance had to be invented at dispatch. Write it. |
| "Surface is obvious from the title" | Layer 1 cannot judge a title. Name crate/paths/symbols. |
| "Probably not a duplicate" | `edda ask` + `edda search` + issue search take a minute. Run them. |
| "I wrote the spec, so I can verify the fix fastest" | Author-verifier collusion checks compliance, not design. Hand it off. |
