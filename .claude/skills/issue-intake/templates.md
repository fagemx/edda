# Templates

## Issue body (the ready-bar format)

The **single body contract**: issue-intake, issue-create, and fleet-epic-split
all emit this shape; no parallel body formats exist (retired formats stay
retired). Shaped so parallel-wave Layer 1 can judge it without archaeology.
GH-556 is a real example of this template.

```markdown
## What happened
<expected vs actual, observed on <full SHA>, environment if relevant.
Verbatim evidence: command output, log excerpt, or failing test.>

## Why it matters
<consequence if unfixed: what breaks, who hits it, worst case.>

## Suspected surface
<crate/path(s), symbols if known. This is what makes the issue
machine-judgeable for parallel dispatch — do not skip it.>

## Predicted surface
<paths + symbols this work will create or modify — the write surface
parallel-wave Layer 1 intersects to judge parallelism. Cannot list one?
The scope is too vague: NOT READY, back to scope sharpening.>

## doneWhen
- <observable, machine-checkable condition>
- <...>
- A regression test reproduces the defect first and is verified to FAIL
  before the fix.

## Wiring audit — REQUIRED whenever the issue cites or adds code
"Exists" ≠ "wired". For EACH component the issue cites or adds, one line
answering four questions, each backed by a `file:line` verified this session
(canonical wording — issue-create and fleet-epic-split defer to this
definition):

| Component | 1. Writer & shape — who writes it, structured field or prose string? | 2. Reader — name one actual consumer, or state "no consumer". | 3. Failure signal — what happens when it fails: swallowed errors, success-only logs, best-effort writes? | 4. Layer reach — does the capability arrive at the layer the issue claims (CLI flag ↔ builder ↔ store)? |
|---|---|---|---|---|
| <component> | | | | |

If the proposal adds a write-end, doneWhen MUST include a consumption proof
(one test walking write → read end-to-end) and a death-visibility line
(freshness/coverage surfaced in output).

## Relation to existing issues
<adjacent issues and why this is distinct; or "none found" +
which dedupe queries ran (edda ask / edda search / issue search).>
```

Rules: closing keywords ("closes #N") only if auto-close is intended — GitHub
links them even inside negations. Label `fleet:pending` when standing authority
covers it; `needs-operator` for direction/high-risk.

## Parked candidate (not yet fileable)

Record durably (`edda note --tag candidate` or a draft), never file:

```text
CANDIDATE: <one-line suspicion>
Basis: <SHA> · Attempted: <what was tried, what it showed>
Next bounded experiment: <the one check that would promote or kill this>
```

## Campaign charter (scheduled recon)

```text
Object: <repo/subsystem/diff under review>   Basis: <branch @ full SHA>
Lenses: <rotating pick: correctness | concurrency | data-loss | false-green tests | UX>
Exclusions: <what this campaign will not inspect>
Evidence bar: repro, failing test, trace, or direct code proof
Coverage: <scope cells × lenses; one reviewer per cell>
Stop: coverage complete | budget <N> | operator decision
```

## Wave-close intake note

```bash
edda note "intake: findings <cat:count,...>; surface-delta <files predicted-vs-actual>; ceiling-misses <n>; queue-ready <n>" --tag intake
```
