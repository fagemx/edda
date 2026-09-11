# C1 — One bounded Flash repair with observable handoff and cost

> Owner: strong planner, one Flash author, independent reviewer; these describe this trial,
> not new mandatory product roles. Depends on: A2 candidate guidance/product binary, not B2/A3/S5/node.
> Delivery: pilot evidence; no duplicate issue/PR for a historical fix.

## Concrete task, not an invented future issue

Use the real historical #1143 diagnostic bug on immutable input
`e6b3ba11b7f8462a39dad16ed3ea37ce445a9d6e`:
zero-byte diagnostic output was classified as absent instead of indeterminate.

Acceptance for the trial:

- plain successful `gh pr checks --required` remains the only green authority;
- diagnostic zero-byte stdout is indeterminate on every exit; decoded `[]` remains absence;
- malformed/unknown shapes and real failure never authorize squash;
- no stderr-text matching; existing argv/environment behavior is preserved.

Allowed repair paths: `crates/edda-cli/src/cmd_review/github.rs`, corresponding narrow
unit tests there, `crates/edda-cli/tests/review_merge_advisory.rs`, `docs/reference/cli.md`.
No merge authority/shell/CI/workflow modifications. No GitHub publication or real merge.
This experiment replays a closed issue; it does not count as new shipped product value.

## Preparation (controller)

1. Check exact historical object exists; obtain it from PR1143's existing refs/history if
   necessary. Use a fresh isolated experimental checkout, never an active peer checkout.
2. Bind actual installed runtime/provider/model/version, guidance/product source SHA,
   timeout, one funded author call and one separately funded independent review, available
   build lane and store isolation. No correction call is authorized by this card. Record
   missing cost reporting and budget enforcement capability. Never inspect or print secret values.
3. Give the worker issue behavior and source starting points, not the already merged fix
   diff or gold patch. Reviewer can use final `10865bdcf503d7296ed6749318f781b00001dc13`
   as an oracle but compares behavior, not identical implementation.
4. Strong planner writes concise outcome/scope plus Flash-specific ordered reads and a
   zero-output-versus-empty-array probe. Use ordinary controller-authored prompt-file;
   do not relabel imported issue/capsule prose as trusted commands or activate unresolved
   S5 controlled APIs. Existing direct low-level dispatch remains a separate path.
5. Offer source access and safe offline tests, no live forge writes. This is a functional
   workflow trial in an isolated environment, not proof of an adversarial sandbox.

## Run one guided attempt

Bind PI_MODEL to a real installed provider/model pattern. For this Pi example, validate
all named CLI options with the installed binary's help before launch; no new flags:

```bash
edda dispatch --agent pi --list-models
# Controller binds absolute PILOT_WT, PROMPT_FILE, receipt destinations and PI_MODEL.
# Set assigned CARGO_TARGET_DIR in the caller environment; do not create another target.
edda dispatch --agent pi --model "$PI_MODEL" --cwd "$PILOT_WT" \
  --prompt-file "$PROMPT_FILE" --timeout-sec "$TIMEOUT_SEC" \
  --json > "$DISPATCH_RECEIPT"
```

TIMEOUT_SEC and spend allowance must be explicit before paid execution. Budget flags
are not represented here as a hard guarantee: repository evidence shows some backends
report over-budget after spending. If a hard spend ceiling is required but the selected
provider cannot enforce it, do not run; return unavailable for this attempt.
Run exactly one author attempt and one independent review. Check the corresponding
funded allowance before each paid launch. No correction/resume, automatic stronger-model
fallback or repeated benchmark is authorized. Record a requested correction as follow-up,
not another call. Native resume remains covered by A2 fixtures, marked not exercised here.
For future separately authorized experiments, Pi dispatch repeats session ID; unlike
product review it does not accept --resume. Do not mix backend continuation semantics.

## Self-check / review / validation

Author performs the two A2 lenses and hands off one facts block. No author self-LGTM.
Read observed exit/outcome/session/model; `outcome=done` alone is not acceptance.

At the trial source, the existing process fixture
`required_check_diagnostics_refuse_every_observed_shape_without_squashing` is the starting
point. It asserted some old behavior, so the worker must correct expectations and add
failing-side coverage rather than merely run it unchanged:

```bash
cargo test -p edda --test review_merge_advisory
cargo test -p edda --bin edda cmd_review
```

Run focused edda lint/format under the current verification budget, no full workspace.
Author freezes the repair as a local experimental commit, no push/PR; capture that full
result SHA. A current A2-built review binary (not the historical candidate executable)
can review the input-to-result range with acceptance in --spec and author facts in the
new --context-file. Bind that review-binary SHA/version separately. If author produces no
candidate, record failure; any review of partial work is evidence, not a frozen-head LGTM.
Independent reviewer checks actual source and offline evidence in its one funded call.
No new PR, no live forge writes and no correction loop. This is local-only evidence under
existing local review rules.

## Report at docs/evidence/delivery-first/pilot.md

V5 defines fields: input/result/guidance SHA, requested/observed model/runtime/session,
attempts, actual commands/evidence, acceptance outcome, manual interventions/minutes,
elapsed breakdown, cost known/unknown (all retries/review included), unexpected waits,
resume/replacement outcome and at most three evidenced follow-ups.

If the trial fails, report it as failure, not feature blockage. One guided success proves
only this case; a speed/cost improvement claim requires a matched baseline or additional
repeatable runs. Optional later baseline uses the same task/model/allowance and a fresh
session; it is not a prerequisite for B delivery. Historical patch familiarity may bias
results; disclose it and never claim benchmark-level generalization.

## Stop / cleanup

Do not continue on ambiguous prior launch, unavailable funded allowance, scope violation
or real data/security risk. Preserve receipts and unmerged experimental sources; controller
cleanup follows existing merged-artifact policy, not unconditional rm/reset from this card.
No runtime/schema expansion is authorized just because a trial exposes friction.
