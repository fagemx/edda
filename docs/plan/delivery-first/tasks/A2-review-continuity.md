# A2 — Reachable shared facts and native reviewer resume

> Owner: A1 implementer. Depends on: A1 candidate. Unblocks: C1 only.
> Contract: DF-03/04/05/06. Delivery: optional context input + coherent handoff/resume path.

## Read first

Read [SPEC](../SPEC.md) sections 6–7, [CONTRACT](../CONTRACT.md) shared types/input contract,
`cmd_review/args.rs`, `prepare.rs::prepare` / `assemble`, `mod.rs::run_inner` / `run_with`,
`brief.rs::assemble`, and current REVIEW sections 0–2. Pi/Claude product reviewers lack
shell/gh access; an arbitrary PR comment link is not an accessible handoff carrier.

## Scope

A1 canonical operating section and its projections; narrowly conflicting author/fixer
language in `.claude/skills/issue-action/SKILL.md` and `.claude/skills/pr-review-loop/SKILL.md`.
Preserve A1's thin routes and task lifecycle; do not recreate a second delivery loop.
Extend `scripts/test-delivery-guidance.sh`. Minimal product wiring is also in scope:

- `crates/edda-cli/src/cmd_review/args.rs`: optional `context_file: Option<PathBuf>`.
- `prepare.rs`: load one bounded context buffer before dispatch, warnings/provenance.
- `brief.rs`: optional JSON-escaped data section; no authority or gate additions.
- `mod.rs`: only necessary field/notes plumbing; preserve WorktreeGuard/session behavior.
- `cmd_review/tests.rs`, inline args/brief tests and direct struct constructors: fixtures.
- `docs/reference/cli.md`: new optional flag, bounds, warning behavior and trust meaning.

No control API, public event/SDK schema, new tool permission or mandatory context profile.
Use the existing file/read utilities where suitable; no new universal context subsystem.

## Implementation

1. Author uses behavior and counterexample lenses in the same session. Produce one small
   handoff, not two jobs/calls/reports/sign-offs. File is explicitly selected by controller;
   no automatic transcript, environment, home-directory or arbitrary PR-body collection.
2. Add proposed `edda review --context-file <path>`. CONTRACT section 5 defines the complete
   bounded UTF-8 data-only behavior. Read once before launch and hash/render the same buffer.
   The option works on first, resumed and replacement product reviews. Existing calls with
   no flag remain behaviorally unchanged. Missing/invalid optional file yields a visible
   warning and continues without that context, never fake evidence or new refusal.
3. Inject file content as JSON-escaped data in the brief, after trusted rules and separate
   from SPEC/LEDGER/EVIDENCE. Actual current subject SHA comes from prepared subject, not
   from author text. Put context digest/omission reason in existing notes and brief output;
   no new event field. Final outcome must never become green solely because context says so.
4. Prefer original author for fixes and original reviewer for follow-up. Missing native
   conversation requires a new identity/replacement with explicit context file; never
   reuse an empty UUID. No-context replacement remains allowed with visible limitation.
5. Product follow-up adds `--resume` with same agent and real persisted conversation.
   Direct host reviewer uses its host session or explicit replacement. Product --resume
   cannot resume a host-only session. Direct reviewer receives quoted data from controller.
6. Product WorktreeGuard remains; caller creates no second review worktree. Direct readonly
   ref path stays available under current policy. No shared writable author checkout.
7. Fix review checks delta, prior findings and affected direct consumers/base/security,
   reusing applicable evidence as READ; final verdict still binds current head.
8. Update callers to pass context only after capability detection. Old binary: omit the
   unsupported flag and disclose missing product context, or use the existing permitted
   direct-review route. Do not install/upgrade automatically or add a fallback wrapper.

## Proposed command (available only after this card lands)

```bash
edda review --pr "$PR" --agent "$REVIEW_AGENT" --model "$REVIEW_MODEL" \
  --resume --context-file "$FACTS_FILE" --json
```

For replacement omit --resume and allocate a new reviewer identity after checking existing
claims/session state; pass facts containing prior findings. Author facts do not replace
required acceptance. Never use `--trust-spec` for rationale or handoff material.

## Acceptance

V2 in [VALIDATION](../VALIDATION.md): fake launcher sees exact escaped context bytes for
first/resume/replacement; actual subject unchanged; no extra tool/shell permission;
malicious delimiters remain data; missing/non-UTF8/oversized/nonregular input is visibly
omitted; no-file legacy path unchanged; digest matches rendered buffer and persists in
existing notes. Tests prove the carrier, not only presence of a CLI field.

```bash
cargo test -p edda
cargo clippy -p edda --all-targets -- -D warnings
sh scripts/test-delivery-guidance.sh
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
git diff --check
```

Use existing A lane, focused format/file-length checks, then exact-head CI. Same-reviewer
resume fixtures and Pi native-context checks stay covered. No full local workspace gate.

## Delivery / rollback

Intent: `feat(review): accept optional data-only handoff context`.
A1/A2 may share one candidate if coherent; A3 is independent, not a prerequisite.
Fallback is explicit replacement/limited context, not fake resume, hidden missing evidence,
shared writable checkout or unrestricted reviewer. Existing no-flag invocation remains usable.
