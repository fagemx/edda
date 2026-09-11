# A2 — Shared facts, two author lenses, native reviewer resume

> Owner: A1 implementer. Depends on: A1 candidate. Unblocks: A3 and C1.
> Contract: DF-03/04/05/06. Delivery: one coherent handoff/resume path.

## Read first

Read [SPEC](../SPEC.md) sections 6–7, [CONTRACT](../CONTRACT.md) shared types,
`cmd_review/args.rs`, `prepare.rs::prepare`, `prepare.rs::assemble`,
`mod.rs::run_inner` / `run_with`, `brief.rs::assemble`, and current REVIEW sections 0–2.
Code reads are necessary: product review and direct review have different worktree owners.

## Scope

A1 guidance paths + narrowly conflicting author/fixer language in
`.claude/skills/issue-action/SKILL.md` and `.claude/skills/pr-review-loop/SKILL.md`;
extend `scripts/test-delivery-guidance.sh`. No Rust runtime edits or new CLI flag.

## Implementation

1. Author uses behavior and counterexample lenses in the same session. Output one short
   handoff (goal, source SHAs, rationale, consumers, ran/read evidence, unknowns).
   Do not require two new tasks, two calls, two reports or a self-LGTM.
2. Default to original author for fixes, original reviewer for follow-ups. A missing native
   conversation means explicit new identity/replacement, not a reused empty session UUID.
3. Product path follow-up command is the same explicit agent/model route plus `--resume`.
   Direct controller-subagent review resumes the existing host conversation if supported;
   it cannot pretend that product `--resume` resumes an unrelated host-only session.
4. Eliminate caller-created review worktree when `edda review` already owns WorktreeGuard.
   Direct readonly ref path remains available under current policy. Preserve all product
   guard checks and writer isolation. Do not promise a no-worktree product mode.
5. Place optional facts in a PR handoff / existing task receipt pointer. Ensure the selected
   review path can read it; if not, controller includes escaped data in its trusted brief.
   Missing optional handoff never becomes launch refusal. No `--trust-spec` for rationale,
   no copying full transcripts, no facts-as-tools or facts-as-merge-authority.
6. On fix review, inspect prior-to-current delta, prior findings and affected direct
   consumers/base/security; retain old applicable evidence as READ. No whole-PR reset by
   default; no shortcut around current-head final verdict.
7. Extend offline fixture for first/resume/replacement, no duplicate isolation, self-check
   non-gating wording and data-only adversarial example. V2 names expected observations.

## Concrete command examples (bind existing PR/runtime, do not execute here)

```bash
# Same repository and recorded product review conversation; explicit runtime preserved.
edda review --pr "$PR" --agent "$REVIEW_AGENT" --model "$REVIEW_MODEL" --resume --json
```

Bind actual supported values; do not apply this command to a host-only review or invent
credentials. If product refuses missing history, report replacement and launch a normal
new independent review only after inspecting existing sessions/claims for duplication.

## Acceptance

Run V2 from [VALIDATION](../VALIDATION.md), `sh scripts/test-delivery-guidance.sh`,
markdown/citation/diff checks. Embedded skill changes use focused edda L0 on the existing
A lane; no additional per-task target. Existing tests
`resume_reuses_ledger_session_and_increments_round` and Pi conversation continuity tests
are READ from applicable CI or run focused if a real uncovered assumption needs proof.
Do not claim the new facts profile is automatically ingested as a new product field.

## Delivery / rollback

Intent: `refactor(review): preserve facts and sessions across scoped review rounds`.
A1/A2 may share one review candidate. Regression fallback is an explicit replacement with
facts, not a fake resume, shared writable checkout or unrestricted reviewer.
