---
name: code-review
description: "Review merged code, commit ranges, or codebase paths for correctness, security, architecture, and test risk; use pr-review for open PRs"
---

# Retrospective Code Review

You are the tech lead performing an evidence-driven review of code that already exists. Review the requested scope without modifying code, distinguish proven defects from questions, and never claim coverage you did not complete.

Use `pr-review` instead when the user wants a pre-merge review of an open pull request. Use `issue-scan` when the goal is broad work discovery rather than a bounded quality review.

## Usage

```text
code-review                                      # Review all tracked, reviewable files
code-review <commit-range>                       # Review a Git range, e.g. HEAD~5..HEAD
code-review --path <file-or-directory>           # Review one tracked path
code-review [<range> | --path <path>] --create-issues
```

`<commit-range>` and `--path` are mutually exclusive scope selectors. `--create-issues` is only a modifier: it does not authorize issue creation until the user approves the final issue draft table.

## Mode Routing

Parse `args` once at the start:

| Input | Mode | Scope source |
|-------|------|--------------|
| no scope selector | `full` | all tracked, reviewable files |
| one positional Git range | `range` | files changed by that range |
| `--path <path>` | `path` | tracked files below that path |
| any mode + `--create-issues` | same mode + issue draft | findings from this review only |

Reject ambiguous or incomplete input instead of guessing:

- range together with `--path`
- more than one positional range
- missing value after `--path`
- unknown flags
- a path outside the repository

---

## Workflow

### Step 0: Parse and Validate Scope

Establish repository state before reading code:

```bash
git rev-parse --show-toplevel
git status --short
```

Then validate the selected mode. For `--path`, validate existence and repository containment before enumerating files; a string that merely resembles a path is not sufficient.

#### Full mode

Use Git as the source of truth so ignored build output is not scanned:

```bash
git ls-files
```

#### Range mode

Validate that the range resolves, then list added, copied, modified, renamed, and deleted paths. Deletions are review evidence and must not disappear from scope:

```bash
git rev-list --count "<commit-range>"
git diff --name-status --find-renames --diff-filter=ACMRD "<commit-range>" --
```

Normalize rename rows into old and new paths, and retain the status in the scope manifest. Review deleted code from the range diff or its preimage, then inspect surviving callers for breakage. Do not expand range mode into a whole-codebase review. Read unchanged definitions and callers only as supporting context; findings must concern behavior affected by the range.

#### Path mode

Resolve `<path>` against the repository root and validate that the result remains inside it. Then enumerate tracked files:

```bash
git ls-files -- "<path>"
```

A directory existing on disk is not enough: ignored or untracked files are outside the default retrospective scope. If the user explicitly requests untracked files, disclose that exception in the report.

#### Reviewable files

Keep source code and behavior-bearing project configuration. Detect the stack from real manifests; do not assume TypeScript, Rust, or a `src/` layout.

- Source: `.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx`, `.go`, `.java`, `.kt`, `.c`, `.cc`, `.cpp`, `.h`, `.hpp`, `.cs`, `.rb`, `.php`, `.swift`, `.scala`, `.sh`
- Build/runtime configuration: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `build.gradle*`, `Dockerfile*`, and CI workflow YAML
- Exclude binary files, lock files, generated files, vendored code, and patterns from `.claude/review-config.md`
- Never exclude tests merely because they are tests

If zero reviewable files remain, stop with `NO REVIEWABLE FILES`, show the raw paths and exclusion reasons, and do not produce a score.

If relevant files have uncommitted changes, stop and ask whether to review the working tree or committed `HEAD`; do not silently mix both states.

Create an internal scope manifest containing:

```text
mode | scope expression | included files | excluded files with reason | repository state
```

For a large scope, partition by crate/package/module and maintain a coverage ledger. Never silently sample. If the available context cannot cover every file, return `INCOMPLETE` with the exact unreviewed paths and no overall grade.

### Step 1: Load Project Evidence

Read context in this order:

1. `AGENTS.md` and `CLAUDE.md` when present
2. `.claude/review-config.md` when present
3. detected manifests such as `Cargo.toml`, `package.json`, `pyproject.toml`, or `go.mod`
4. architecture, contract, and safety documents referenced by those files

Project-specific rules override generic checklist items. Verify external APIs and framework behavior from repository examples or primary documentation; mark anything unverified instead of guessing.

If `.claude/review-config.md` is absent, derive provisional criteria in memory and show the assumptions in the final report. Write a new config only after explicit user approval; a code review is read-only by default.

For Edda, the existing review config defines crate layering, module boundaries, safety invariants, Rust checks, and ignore patterns. Treat those rules as review evidence, not optional suggestions.

### Step 2: Build Three Review Tracks

The tracks are review lenses, not mutually exclusive buckets. A file may appear in more than one track when it contains both production behavior and tests.

| Track | Include | Primary questions |
|-------|---------|-------------------|
| **Core** | domain logic, persistence, state, engines, manifests, wiring | Can behavior violate architecture, invariants, data integrity, or concurrency guarantees? |
| **Interface** | API/HTTP handlers, CLI, MCP, bridges, schemas, parsers, external integrations | Are trust boundaries validated, contracts stable, errors safe, and dependency failures handled? |
| **Tests** | test directories/files plus inline test modules in production files | Do tests prove important behavior, failure paths, boundaries, and isolation without false positives? |

Use project structure and imports first, then naming heuristics. For Rust, find inline tests explicitly:

```bash
rg -l '#\[cfg\(test\)\]|#\[(tokio::)?test\]' --glob '*.rs' .
```

Intersect these results with the scope manifest. Do not pass hundreds of file paths on the command line or allow this repository-wide discovery command to broaden range/path findings.

Typical Interface hints include `routes/`, `handlers/`, `schemas/`, `api/`, `cli/`, `mcp/`, `bridge/`, `integrations/`, and project-specific boundary crates. A public `lib.rs`, `main.rs`, or manifest can also belong to Core because it defines wiring or dependencies.

If a track has no files, mark it `N/A` with the reason. Do not launch an empty review agent and do not penalize the score.

### Step 3: Run Non-Mutating Validation

Use commands documented by the project. Do not invent scripts or change files to make checks pass.

For a Cargo workspace whose project rules match Edda:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For a focused crate, first read its `Cargo.toml` to obtain the actual package name, then use `-p <package-name>`. For JavaScript/TypeScript, read `package.json` and run only scripts that actually exist. Apply the same evidence-first rule to other stacks.

Record each command, exit status, and relevant failure excerpt. A failed check is evidence to investigate, not automatically a P0. If a tool is unavailable, a command times out, or the project documents no suitable command, report `NOT RUN` with the reason.

### Step 4: Execute Track Reviews

Run the non-empty tracks in parallel when agent delegation is available; otherwise run three separate passes with the same contracts. Give each track:

1. its exact file list
2. the scope manifest and project evidence
3. relevant validation output
4. the track questions below
5. the finding contract

Every reviewer must read the relevant code before judging it, trace definitions and call sites far enough to prove reachability, and remain inside the assigned review scope.

#### Core lens

- Check documented layer and module boundaries against actual imports and manifests.
- Trace state transitions, transactions, serialization, concurrency, and error paths for invariant violations.
- Treat `unsafe`, panic paths, suppression attributes, unchecked conversions, and swallowed errors according to project rules and reachability.
- Distinguish dead code from public extension points using callers and tests, not naming alone.

#### Interface lens

- Identify every trust boundary and verify input shape, size, encoding, authentication/authorization when applicable, and command/SQL construction.
- Compare request, response, CLI, event, and schema behavior with existing contracts and callers.
- Trace external dependency failures, timeouts, retries, cancellation, error exposure, and partial success.
- Check bounds and resource controls where user input can amplify work.

#### Tests lens

- Map critical production behavior to happy path, error path, and boundary assertions.
- Include Rust inline `#[cfg(test)]` modules; do not rely only on filenames or `tests/` directories.
- Detect assertions that can pass without proving the result, order-dependent state, over-mocking, leaked resources, and ignored failures.
- Report missing tests only when tied to a concrete risk or changed behavior.

### Finding Contract

Only report a finding when all required fields are supported:

```markdown
#### [P0|P1] <short problem statement>
- Evidence: `<file>:<line>` - <specific code behavior>
- Impact: <reachable failure or violated invariant>
- Required outcome: <observable condition that must become true>
- Confidence: High | Medium
```

Severity rules:

| Severity | Use when |
|----------|----------|
| **P0** | Proven exploitable security issue, data loss/corruption, broken build/runtime critical path, or documented safety invariant violation |
| **P1** | Proven material correctness, reliability, maintainability, or test risk that is not immediately critical |
| omit | Style preference, speculative concern, or behavior already enforced by tooling with no residual risk |

Low-confidence concerns belong in `Open Questions`, not findings. Each finding needs a real file and line, a reachable impact, and no duplicate from another track.

### Step 5: Reconcile and Score

Merge duplicates by root cause. When tracks disagree, re-read the evidence; do not average confidence or keep contradictory findings.

Assign each non-empty track a deterministic grade:

| Grade | Rule |
|-------|------|
| **D** | A deployment-unsafe P0: exploitable security, plausible data loss/corruption, or systemic architecture/safety failure |
| **C** | Any other P0 |
| **B** | Three or more P1s, or one cross-cutting P1 affecting multiple modules |
| **B+** | One or two localized P1s |
| **A** | No P0/P1, complete coverage, and required validation passed |

If required validation was not run, the highest possible track grade is `B+`. If coverage is incomplete, use `INCOMPLETE` instead of a grade. The overall score is the worst non-`N/A` track grade (risk-first), not a subjective or file-count weighted average. Any explicit auto-fail rule in `.claude/review-config.md` overrides this table.

### Step 6: Produce the Report

Use this exact structure:

```markdown
# Code Review Report - <project>

## Verdict: <A | B+ | B | C | D | INCOMPLETE>
<one sentence tied to the highest-risk evidence>

## Scope and Coverage
- Mode: <full | range | path>
- Scope: `<expression>`
- Repository state: <HEAD/working tree and dirty status>
- Reviewed: <count>/<count> files
- Excluded: <count with reasons>
- Unreviewed: <none or exact paths>

## Validation
| Command | Result | Evidence |
|---------|--------|----------|
| `<command>` | PASS / FAIL / NOT RUN | <short excerpt or reason> |

## Track Grades
| Track | Grade | Files | Reason |
|-------|-------|-------|--------|
| Core | <grade/N/A> | <count> | <evidence-based sentence> |
| Interface | <grade/N/A> | <count> | <evidence-based sentence> |
| Tests | <grade/N/A> | <count> | <evidence-based sentence> |

## P0 - Must Fix
<findings or "None">

## P1 - Should Fix
<findings or "None">

## Test Gaps
<risk-linked gaps or "None">

## Systemic Issues
<cross-cutting root causes or "None">

## Open Questions
<unverified concerns or "None">
```

Do not show an `A` verdict when checks were skipped or coverage is incomplete. Positive observations are optional and must cite evidence; they never cancel a finding.

### Step 7: Draft or Create GitHub Issues

Skip this step unless `--create-issues` was supplied. The flag authorizes drafting, not immediate creation.

1. Exclude low-confidence questions and deduplicate against open issues.
2. Create one draft per P0. Group P1s only when they share a root cause, module, and acceptance boundary.
3. Keep each issue to one independently verifiable PR.
4. Check repository and existing labels before proposing them:

   ```bash
   gh auth status
   gh repo view --json nameWithOwner,url
   gh label list --limit 200
   gh issue list --state open --limit 200
   ```

5. Present a confirmation table with title, severity, evidence, scope, and labels.
6. **Explicit confirmation required.** STOP and create nothing until the user explicitly approves that table.
7. After approval, run `gh issue create` for approved rows only and report each returned URL.

Never create missing labels, modify code, or broaden issue scope as part of this skill unless the user separately asks.

---

## Recovery Rules

| Failure | Required response |
|---------|-------------------|
| invalid range or outside-repo path | stop and show the failing input |
| zero reviewable files | return `NO REVIEWABLE FILES`; no score |
| relevant dirty worktree | ask which repository state to review |
| validation command fails | investigate and include evidence; continue the review |
| one track fails | retry once; if still unavailable, mark coverage `INCOMPLETE` |
| GitHub CLI/auth failure | keep issue drafts in the report and create nothing |

## Anti-Patterns

1. **Hardcoded layout or language** - never assume `src/`, TypeScript, or Rust without manifest evidence.
2. **Silent sampling** - never review the first N files and imply full coverage.
3. **Exclusive test classification** - inline tests may share files with production code.
4. **Review without reading** - searches identify candidates; they do not prove findings.
5. **Vague findings** - every P0/P1 needs `file:line`, reachable impact, and confidence.
6. **Severity inflation** - missing polish or speculative risk is not P0.
7. **Style review** - leave formatting and lint-only concerns to configured tooling.
8. **Range leakage** - supporting context is not permission to report unrelated legacy issues.
9. **Score inflation** - skipped checks or incomplete coverage cannot receive `A`.
10. **Unconfirmed writes** - reviews are read-only; issue creation requires explicit approval.

## Related Skills

| Skill | Use when |
|-------|----------|
| `pr-review` | Review an open PR before merge |
| `code-review` | Review merged code, a commit range, or a bounded codebase path |
| `issue-scan` | Discover and prioritize new work across the codebase |
| `plan-validate` | Validate a proposed plan against actual code |

## Project References

- `AGENTS.md`
- `CLAUDE.md`
- `.claude/review-config.md`
