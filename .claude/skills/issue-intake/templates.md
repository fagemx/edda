# Templates

## Issue body (the ready-bar format)

Shaped so parallel-wave Layer 1 can judge it without archaeology. GH-556 is a
real example of this template.

```markdown
## What happened
<expected vs actual, observed on <full SHA>, environment if relevant.
Verbatim evidence: command output, log excerpt, or failing test.>

## Why it matters
<consequence if unfixed: what breaks, who hits it, worst case.>

## Suspected surface
<crate/path(s), symbols if known. This is what makes the issue
machine-judgeable for parallel dispatch — do not skip it.>

## doneWhen
- <observable, machine-checkable condition>
- <...>
- A regression test reproduces the defect first and is verified to FAIL
  before the fix.

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
