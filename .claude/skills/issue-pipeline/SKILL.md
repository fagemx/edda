---
name: issue-pipeline
description: "Route one or more issue inputs into the canonical delivery flow. Usage: /issue-pipeline 566 567 [--skip-plan] [--no-merge]"
---

# Issue Pipeline

This is a bounded compatibility entry, not a separate plan/implement/review/
merge pipeline. Parse the issue inputs and flags, then follow the
`delivery-flow/1` section in `coord-orchestrate`.

## Inputs that must survive routing

```text
/issue-pipeline <issue-numbers...> [--skip-plan] [--no-merge]
```

- `--skip-plan` means the caller says usable acceptance already exists. Read and
  reuse that acceptance; if it is missing or materially unclear, do bounded
  discovery. Never interpret the flag as permission to work without acceptance.
- `--no-merge` stops this invocation before merge. It never grants merge
  authority to this controller, a worker, fixer or reviewer.
- Issue numbers remain acceptance, ownership and traceability inputs. They do
  not force one task, agent, worktree or PR per issue.
- There is no review-bypass meaning or flag.

## Route

1. Read repository/host instructions. If this session is already assigned an
   Edda task, run `edda task show <id>`, read its reachable brief and results,
   and perform that role; do not create another pipeline.
2. Read each issue's current acceptance, ownership/claim state, existing plan,
   branch/PR and prior attempts. Preserve any repository-required claim before
   a forge write or dispatch.
3. Classify the work with the canonical entry table: ordinary single-owner,
   one cohesive writer, or a real parallel formation. Small work does not form
   a fleet merely because this legacy entry was used.
4. For a formation, select one repository-wide rail owner before task creation,
   materialize only concrete bundles and actual dependencies, and use the
   backend-specific adapter. Independent ready bundles proceed without an
   all-issue phase barrier.
5. Return each issue's observed result and durable evidence. Task done may mean
   a local candidate; it does not require a PR or merge. Review and merge use
   repository policy and the canonical product path only.

Do not recreate the retired all-agent phase waits, fresh-fixer sub-pipeline,
author-as-independent-reviewer path or direct forge merge loop here.
