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
