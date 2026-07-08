# edda decision recall eval

Deterministic eval harness for `edda ask` — measures whether a natural-language
question about a past decision returns that decision in the top-K results.

Baseline numbers are workspace-specific (each ledger has its own decisions).
Ship the harness portable; ship the corpus + results per workspace.

## Run

```bash
node evals/recall/run.mjs \
  --corpus evals/recall/corpus.foundry.jsonl \
  --out /tmp/eval-out \
  --workspace "C:/path/to/some-repo" \
  --edda "C:/path/to/edda.exe"     # optional; defaults to `edda` on PATH
```

Outputs `results.json` (machine) and `results.md` (human) under `--out`.
The console prints one line: `R@1=... R@5=... n=... skipped=... misses=...`.

## Metric definitions

- **R@1**: fraction of scored queries where the expected decision is the top
  result from `edda ask <question> --json`.
- **R@5**: fraction where the expected decision is in top 5.
- **Skipped**: expected key not present in the current ledger. Skipped rows are
  excluded from denominators — a shrinking or different ledger cannot inflate
  the score.
- **Misses**: expected key was in the ledger but not in the returned top-`--limit`.
- **Baseline is deterministic BM25-style `ask`** (structured domain lookup +
  keyword timeline). No LLM in the loop. LLM assist would be an opt-in variant
  that adds a re-rank pass, tracked separately.

## Corpus authoring rules (anti-overfit)

1. **Do not read the target decision's `value` or `reason` before writing the
   question.** Only look at the `key` and general domain knowledge.
2. Write the question the way a future agent would naturally ask it — not
   verbatim from the decision text.
3. Mix styles: some questions phrase the topic (natural), some resemble the
   exact key (control). Both are legitimate future queries.
4. Aim for coverage across domains, not just the largest one.
5. When the target is superseded or removed, the query becomes `skipped`.
   Do not backfill by rewriting — regenerate the corpus row with a new target.

The corpus is workspace-specific by design. A different workspace should ship
its own corpus alongside its baseline; do not reuse another workspace's corpus.

## Files

| File | Purpose |
|---|---|
| `scoring.mjs` | Pure scoring functions (rankOf/scoreOne/aggregate) |
| `scoring.test.mjs` | `node --test` unit tests for scoring |
| `run.mjs` | Runner: corpus → edda ask → rank → aggregate → report |
| `corpus.foundry.jsonl` | Corpus for the AI Delivery Foundry workspace |
| `README.md` | This file |

## Reproducibility

Same corpus + same ledger events + same edda binary version ⇒ identical
`results.json` (no timestamps in report body). Ledger drift or edda changes
that move ranks are expected to change the numbers; that is the point.
