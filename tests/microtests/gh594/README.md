# gh594 micro-test — wiring verdict slot (issue #594, PR #629)

Control vs variant review of the same fake diff (`fake.diff`), prompts
identical except the review-skill text: control embeds fleet-review from
`origin/main`, variant embeds this branch's fleet-review (with the Wiring
verdict slot). Model `z-ai/glm-5.3-flash`, read-only (`--exclude-tools
edit,write`), distinct timestamped session ids.

Replay: `sh tests/microtests/gh594/run.sh` — overwrites `out-control.md` /
`out-variant.md` with a fresh run. The committed outputs are from run
`20260902-142248` (sessions `microtest-control-20260902-142248` /
`microtest-variant-20260902-142248`).

## Result (run 20260902-142248)

| Defect in fake.diff | Control (origin/main) | Variant (with slot) |
|---|---|---|
| New `pub fn with_cost_weighting` with zero callers | caught (P1: no-op API, no reader) | caught (P1: dead on arrival, no consumer) |
| Swallowed `let _ = fs::write(...) // best-effort` on the digest write path | caught (P1: silent write-failure behavior change) | caught (P1: cost/report-path swallow + missing freshness signal) |

**Honest reading: control 2/2, variant 2/2.** This run does NOT demonstrate
that detection comes from the slot — the cheap model found both defects
without it. What the slot adds is structure and consistency, observable in
the outputs:

- the variant derives its findings **from the fixed severity rules** (P1
  no-consumer, P1 report-path swallow, P1 death-visibility) instead of
  free-form reasoning, so the verdict is repeatable rather than
  model-dependent;
- the variant **must emit the four-question Wiring table** and the mandatory
  "no new surfaces" line for docs-only diffs — mechanical checks a reviewer
  or CI can verify;
- the control's catch was incidental: without the slot its severity and
  framing vary run to run (in an earlier exploratory run with tools enabled,
  the control missed the dead-setter finding entirely).

Earlier runs (tools-enabled, aborted framing; and the same text-only
framing) are described in the PR #629 conversation; this directory is the
canonical replayable form.
