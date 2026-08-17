# Edda Codex Instructions

Before doing repository work, read `.claude/CLAUDE.md` completely. It is the
canonical project guide for architecture, Rust conventions, tests, decisions,
and multi-agent coordination.

## Multi-session work

- When two or more sessions are implementing in parallel, use
  `$coord-orchestrate` from `.agents/skills/coord-orchestrate/`.
- Before editing with active peers, use `$coord-sync`, inspect off-limits paths,
  and claim the smallest accurate scope.
- Record load-bearing facts in `edda task` or `edda decide` before using chat or
  host messaging as a doorbell.
- Request permission before crossing another session's scope.
- Every review handoff freezes `IN SCOPE`: changed behavior/paths, direct
  callers/consumers, issue/spec acceptance, introduced or exposed security or
  data-loss regressions, and current-base integration. Adjacent, pre-existing,
  or speculative findings become evidenced `FOLLOW-UP ISSUE`s and do not
  extend the current PR.
- Reviewers complete the whole scoped audit and batch blocking P0/P1 before
  requesting changes. Later-round blockers must be fix-caused or previously
  unobservable. The issue/spec is the acceptance ceiling; extra evidence is
  mandatory only to prove a required fact or safety boundary.
- Select gates from code/product-blob, base, and toolchain changes. Docs- or
  evidence-only pushes reuse still-applicable code gates as `READ`, run only
  relevant validation plus exact-head CI as `RAN`, and record available cost.
  Stop after two non-product/harness-only cycles without useful progress or at
  diminishing returns; route follow-up or ask the operator to expand scope.
- Review immutable full SHAs; any push invalidates the prior verdict.
- On GitHub PRs, publish numbered SHA-pinned review rounds. Requested changes
  require an implementer point-by-point response and a new review round; merge
  requires a final current-head LGTM comment with P0=0, P1=0 and ran gates.
  Internal verifier reports do not replace the PR-visible loop. For local-only
  delivery, record the same fields in the strongest durable local carrier; do
  not invent a PR.
- Worker receipts are execution evidence, not acceptance. Merge only with
  explicit operator authority.

## Repository safety

- Preserve unrelated user changes.
- Use `codex/` branch names for Codex-created branches unless directed otherwise.
- Run the checks required by `.claude/CLAUDE.md` before claiming completion.
