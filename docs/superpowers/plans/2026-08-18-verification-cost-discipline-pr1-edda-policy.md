# Verification Cost Discipline — PR-1 (edda policy) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the verification ladder, build-lane rules, gate receipts, and brake authority into every edda policy carrier so a zero-context session verifies once per frozen SHA in an assigned lane instead of cold-building the workspace per round.

**Architecture:** Docs-only change to six policy carriers plus the pressure-test fixtures. Shipped skills (`crates/edda-cli/src/skills/*.md`, embedded by `include_str!` in `cmd_init.rs`) and `skills/fleet-orchestrate/*` stay carrier-neutral (no Cargo words); `.claude/CLAUDE.md` carries the Cargo L0–L3 commands; `AGENTS.md` is the Codex entry pointer. Spec: `docs/superpowers/specs/2026-08-18-verification-cost-discipline-design.md` (§4.1–§4.5, §6 A1/A4).

**Tech Stack:** Markdown, git, `rg`/`grep` for consistency checks, one fresh agent session per dry-run pressure test. No Cargo invocation anywhere in this plan.

> **This plan is a historical execution record, frozen at `738c5ce`. Do not
> take any figure or constraint below as current.** Three review rounds and a
> follow-up PR changed several of them — notably the `8–13 GB` per-target
> figure (measured 40.9 GB) and the `warn 35 GB / refuse 50 GB` thresholds
> (withdrawn; no default ceiling ships until a measured steady state exists).
> See “Round 1 review amendments” at the end of this file, and the spec for the
> living contract.

## Global Constraints

- Branch `docs/verification-cost-discipline` in worktree `C:\ai_agent\edda\.claude\worktrees\verification-cost-discipline`, based on `origin/main 7f5d1555b4c00e4fe6ac9a452a3c81c62289972c`. Do not touch the `codex/fleet-orchestrate-skill` checkout at `C:\ai_agent\edda` (stale, has another session's uncommitted diff).
- Docs-only: run **no** `cargo` command and create **no** `CARGO_TARGET_DIR`. Exact-head CI is the gate (spec §5 slice 1). No skill-content unit test exists (`cmd_init.rs` tests only check files are scaffolded), so nothing else needs local execution.
- Shipped skills and `skills/fleet-orchestrate/*` must contain no `cargo`, `CARGO_`, or `-p <crate>` text (spec §4.5, A1). Say "assigned build lane", "focused gates on touched units", "full gate set", "gate receipt".
- Every carrier states all four ideas: verify once per frozen SHA / READ receipt + exact-head CI before RAN / assigned lane, never ad-hoc / status flip is not a push (spec A1).
- Lane allowlist and thresholds are exactly `worker-1`, `worker-2`, `verifier`, `verifier-2`; warn 35 GB, refuse 50 GB (spec §8). `.claude/CLAUDE.md` and `AGENTS.md` state them as rules; shipped skills and the playbook say "the fixed pool named in the brief" (pressure-test fixtures may use the names as examples).
- Delete nothing on disk. Do not edit `docs/guides/multi-agent.md` (not a carrier in the spec) or `crates/edda-cli/src/cmd_init.rs`.
- Commit format `docs(<scope>): <description>` with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. One commit per task. Files are LF in the index; keep them LF.
- Every push voids prior review; the PR needs the visible review-fix loop from `.claude/CLAUDE.md` before merge. This plan ends at "PR open with handoff comment", not at merge.

---

## File map

| File | Responsibility in PR-1 |
|---|---|
| `.claude/CLAUDE.md` | Cargo instantiation of the ladder, build lanes, split checklist, cost subsection under coordination |
| `AGENTS.md` | Codex entry: two bullets + checklist wording |
| `crates/edda-cli/src/skills/coord-orchestrate.md` | Controller brief fields, verifier brief, review scope contract additions, handoff template lines, mistakes rows |
| `crates/edda-cli/src/skills/coord-review.md` | Reviewer READ-before-RAN paragraph, process-finding routing, Round N template lines |
| `skills/fleet-orchestrate/SKILL.md` | Two invariants, brief fields in the sequence, board/matrix fields |
| `skills/fleet-orchestrate/references/playbook.md` | Brief template, worker/verifier/controller contracts, gate table rows, receipt paragraph, mistakes rows |
| `skills/fleet-orchestrate/references/review-scope-pressure-tests.md` | Fixtures 4–7 for spec A4 (i)–(iv) |
| `docs/superpowers/plans/2026-08-18-verification-cost-discipline-pr1-edda-policy.md` | This plan (already committed with the spec branch) |

---

### Task 1: `.claude/CLAUDE.md` — ladder, lanes, checklist, cost subsection

**Files:**
- Modify: `.claude/CLAUDE.md:112-137` (Testing Standards + Pre-commit Checklist)
- Modify: `.claude/CLAUDE.md` end of `### PR review-fix loop` (after the line `strongest durable local carrier; do not invent a PR.`)

**Interfaces:**
- Produces: level names `L0 iterate`, `L1 freeze`, `L2 review`, `L3 pre-merge`; lane names `worker-1`, `worker-2`, `verifier`, `verifier-2`; the phrase "verify once per frozen SHA"; ceiling `50 GB`. Every later task quotes these exactly.

- [ ] **Step 1: Replace the Testing Standards and Pre-commit Checklist sections**

Find this exact block (lines 112–137):

````markdown
## Testing Standards

```bash
# Run all tests
cargo test --workspace

# Run tests for single crate
cargo test -p edda-core

# Run specific test
cargo test -p edda-core test_name
```

- **Unit tests**: In `#[cfg(test)] mod tests` within source files
- **No integration tests directory** currently — tests are inline
- **No mocking internal crates** — use real SQLite via `tempfile`
- See `edda-core/src/types.rs:261-667` for test patterns

## Pre-commit Checklist

```bash
cargo fmt --check      # Format check
cargo clippy           # Lint (CI uses -Dwarnings)
cargo test --workspace # All tests
```
````

Replace it with — and note that the ladder paragraph quoted below says CI runs
full workspace gates on three operating systems, which is **false** and was
corrected in Round 1 (Windows tests a 7-crate subset). The block is reproduced
as written at `738c5ce`; see "Round 1 review amendments" at the end of this
file:

````markdown
## Testing Standards

```bash
# Run tests for a single crate (default while iterating)
cargo test -p edda-core

# Run a specific test
cargo test -p edda-core test_name

# Run all tests (once per frozen SHA — see Verification ladder)
cargo test --workspace
```

- **Unit tests**: In `#[cfg(test)] mod tests` within source files
- **No integration tests directory** currently — tests are inline
- **No mocking internal crates** — use real SQLite via `tempfile`
- See `edda-core/src/types.rs:261-667` for test patterns

### Verification ladder

Verify once per frozen SHA; cite that result everywhere else. Full workspace
gates are expensive (23 crates; 8–13 GB of build output per target directory)
and GitHub CI already runs them on three operating systems for every push.

| Level | When | Run |
|---|---|---|
| L0 iterate | while editing | `cargo fmt --all --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo test -p <crate>` for each touched crate |
| L1 freeze | once per frozen full SHA, clean tree, before push / PR update | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`, with `CARGO_INCREMENTAL=0`; record the result together with the full SHA (gate receipt) |
| L2 review | verifier, once per frozen full SHA | READ the L1 receipt and exact-head CI; RAN only focused or adversarial checks they do not cover. A full local rerun needs a stated reason: no receipt, red or absent CI, or grounds to distrust the receipt |
| L3 pre-merge | merge authority | READ exact-head CI and the final current-head LGTM; RAN only a merge check against the current base. A draft/ready, label, or status flip is not a push — nothing reruns |

Docs-only changes (no code/product blob, `Cargo.lock`, or toolchain change)
run no Cargo gate locally; exact-head CI is the gate.

### Build lanes

- Never create ad-hoc `CARGO_TARGET_DIR`s per round, per SHA, or per
  timestamp. Build output is disposable cache, but unbounded copies are not:
  twelve such directories reached ~194 GB on one workstation.
- Solo work uses the worktree's default `target/`.
- Fleet sessions build only in the lane named in their brief — one of
  `worker-1`, `worker-2`, `verifier`, `verifier-2` — for their whole lifetime.
  Where the workstation lane tool is installed, run gates through it
  (`lane.ps1 -Lane <assigned> -Gate focused|freeze`); without it, set
  `CARGO_TARGET_DIR` only to `<lane-root>\<assigned-lane>` from the brief and
  reuse it every round.
- Verifier lanes and every L1 run set `CARGO_INCREMENTAL=0`.
- Deleting lane build output is authorized and non-destructive; deleting
  worktrees, branches, or sources is not. Stop and report instead of building
  when the lane pool exceeds 50 GB.

## Pre-commit Checklist

```bash
# Before every commit (L0 — touched crates only)
cargo fmt --check
cargo clippy -p <touched crate> --all-targets -- -D warnings
cargo test -p <touched crate>

# Before freezing a SHA for push / PR update (L1 — once per frozen SHA)
cargo clippy --workspace --all-targets -- -D warnings   # CI uses -Dwarnings
cargo test --workspace
```
````

- [ ] **Step 2: Append the cost subsection to the coordination section**

Find the last lines of `### PR review-fix loop`:

```markdown
For local-only delivery, record the same round/response/verdict fields in the
strongest durable local carrier; do not invent a PR.
```

Append immediately after them:

```markdown

### Verification cost

Rules regulate cost and reclamation, not only evidence:

- Verify once per frozen SHA on the ladder above. READ recorded gate results
  and exact-head CI before any RAN, and state the reason whenever you rerun a
  recorded gate.
- Every worker/verifier brief names a build lane, a verification budget (L0
  while iterating; L1 once per frozen SHA), and cleanup authority (lane build
  cache is disposable; worktrees, branches, and sources are never deleted).
- One verifier identity per PR: rounds resume the same session and lane; a
  replacement reads receipts and CI before running anything.
- Over-verification — a second RAN for an already-recorded SHA without a
  reason, workspace gates for a docs-only push, an ad-hoc target directory —
  is a process finding: record it in the handoff cost line, route it as a
  `FOLLOW-UP ISSUE`, correct the next brief. It does not block a product-green
  PR.
```

- [ ] **Step 3: Verify the file**

Run in the worktree:

```bash
grep -c "L0 iterate\|L1 freeze\|L2 review\|L3 pre-merge" .claude/CLAUDE.md
grep -n "worker-1\|verifier-2\|50 GB\|Verification cost\|not a push" .claude/CLAUDE.md
grep -c "<!-- edda:coordination -->" .claude/CLAUDE.md
```

Expected: first command prints `4` or more; second prints one line per pattern (at least five lines); third prints `1` (marker untouched, so `edda init` still detects the section).

- [ ] **Step 4: Commit**

```bash
git add .claude/CLAUDE.md
git commit -m "docs(claude-md): add verification ladder, build lanes, and cost rules" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `AGENTS.md` — Codex entry bullets

**Files:**
- Modify: `AGENTS.md` (`## Multi-session work` list and `## Repository safety` list)

**Interfaces:**
- Consumes: level names and lane names from Task 1.

- [ ] **Step 1: Insert two bullets after the gate-selection bullet**

Find this bullet (it ends with `route follow-up or ask the operator to expand scope.`):

```markdown
- Select gates from code/product-blob, base, and toolchain changes. Docs- or
  evidence-only pushes reuse still-applicable code gates as `READ`, run only
  relevant validation plus exact-head CI as `RAN`, and record available cost.
  Stop after two non-product/harness-only cycles without useful progress or at
  diminishing returns; route follow-up or ask the operator to expand scope.
```

Insert immediately after it:

```markdown
- Verify once per frozen SHA on the ladder in `.claude/CLAUDE.md`: focused
  crate gates while iterating (L0); the full workspace set once per frozen full
  SHA with a recorded receipt (L1); reviewers READ that receipt and exact-head
  CI and RAN only what they do not cover (L2); nothing reruns on a draft,
  label, or status flip (L3). State the reason whenever you rerun a recorded
  gate.
- Build only in the lane your brief assigns (`worker-1`, `worker-2`,
  `verifier`, `verifier-2`). Never create ad-hoc `CARGO_TARGET_DIR`s per round,
  SHA, or timestamp; solo work uses the worktree's default `target/`. Lane
  build cache is disposable; worktrees, branches, and sources are not. Stop and
  report when the lane pool exceeds 50 GB.
```

- [ ] **Step 2: Reword the completion-check bullet**

Find:

```markdown
- Run the checks required by `.claude/CLAUDE.md` before claiming completion.
```

Replace with:

```markdown
- Run the checks required by `.claude/CLAUDE.md` at the ladder level that
  matches your change before claiming completion; docs-only changes rely on
  exact-head CI and run no Cargo gate locally.
```

- [ ] **Step 3: Verify**

```bash
grep -n "Verify once per frozen SHA\|worker-1\|ladder level\|50 GB" AGENTS.md
```

Expected: four lines.

- [ ] **Step 4: Commit**

```bash
git add AGENTS.md
git commit -m "docs(agents): point Codex at the verification ladder and build lanes" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `coord-orchestrate.md` — controller brief, verifier brief, contract, template, mistakes

**Files:**
- Modify: `crates/edda-cli/src/skills/coord-orchestrate.md:58-66` (review scope contract gate/cost paragraphs)
- Modify: `crates/edda-cli/src/skills/coord-orchestrate.md:77-80` (handoff template Evidence)
- Modify: `crates/edda-cli/src/skills/coord-orchestrate.md:90-100` (Protocol steps 3–4)
- Modify: `crates/edda-cli/src/skills/coord-orchestrate.md:148-162` (Common mistakes table)

**Interfaces:**
- Produces: template lines `- Lane:` and `- Receipt:`; the phrase "gate receipt (SHA, gate set, toolchain, lane, result)". Task 4 and Task 6 reuse both verbatim.
- Constraint: no Cargo words in this file.

- [ ] **Step 1: Extend the review scope contract**

Find:

```markdown
Select gates proportionally. Code/product-blob, base, or toolchain changes run
the relevant code gates. A docs/evidence-only push with those inputs unchanged
reuses still-applicable code results as `READ` with source SHA, then runs only
relevant diff/docs/evidence checks and exact-head CI as `RAN`.
```

Append immediately after it (blank line between paragraphs):

```markdown

Verify once per frozen artifact. The implementer runs the full gate set once
per frozen full SHA in the assigned build lane and records a gate receipt (SHA,
gate set, toolchain, lane, result). The reviewer READs that receipt and
exact-head CI, RANs only focused or adversarial checks they do not cover, and
states the reason for any full rerun (no receipt, red or absent CI, or grounds
to distrust the receipt). Focused gates on touched units while iterating, never
the full set per edit. A status, label, or draft flip is not a push and reruns
nothing.
```

Then find:

```markdown
Each handoff records available elapsed/token/tool cost. Stop after two
consecutive non-product evidence/docs or harness-only cycles without improved
required behavior/proof, or at clear diminishing returns; route the finding to
follow-up or ask the operator to expand scope.
```

Append immediately after it:

```markdown

Over-verification is a process finding, not a product blocker: a second RAN
for an already-receipted SHA without a reason, full gates for a docs-only push,
or an ad-hoc build directory goes into the cost line, routes as a `FOLLOW-UP
ISSUE`, and corrects the next brief.
```

- [ ] **Step 2: Add lane and receipt lines to the handoff template**

Find inside the ```` ```text ```` block:

```text
- RAN: <commands/checks run on this SHA>
- READ: <reused results and source SHAs>
```

Replace with:

```text
- RAN: <commands/checks run on this SHA>
- READ: <reused results and source SHAs>
- Lane: <assigned build lane>
- Receipt: <gate receipt for this SHA (SHA, gate set, toolchain, lane, result), or none>
```

- [ ] **Step 3: Rewrite Protocol steps 3 and 4**

Find:

```markdown
3. **Brief workers** — self-contained (they have zero context): issues to
   read, worktree + branch command, `edda claim` label and paths, files
   owned/forbidden, quality gates verbatim, done = GitHub PR when available,
   otherwise a frozen local branch plus durable review carrier (never invent a
   PR); never merge + `edda task done --receipt`. Include the receiver tie-break
   verbatim (see Traffic rules).
4. **Brief the verifier — read-only, starts BEFORE code:** baseline gates on
   main; flake hunt; observable-behavior criteria per issue; sweep for two
   test poisons — tests asserting the behavior being removed (invert and
   rename, never delete) and single-case tests that pass either way (demand
   the second case).
```

Replace with:

```markdown
3. **Brief workers** — self-contained (they have zero context): issues to
   read, worktree + branch command, `edda claim` label and paths, files
   owned/forbidden, quality gates verbatim, assigned build lane from the fixed
   pool, verification budget (focused gates on touched units while iterating;
   the full gate set once per frozen SHA with a receipt; READ receipts before
   any RAN), cleanup authority (lane build cache is disposable; worktrees,
   branches, and sources are never deleted), done = GitHub PR when available,
   otherwise a frozen local branch plus durable review carrier (never invent a
   PR); never merge + `edda task done --receipt`. Include the receiver tie-break
   verbatim (see Traffic rules).
4. **Brief the verifier — read-only, starts BEFORE code:** baseline on the
   basis SHA by READing exact-head CI and any existing gate receipt, RANning
   only what they do not cover in the assigned verifier lane, and classifying
   existing red checks; flake hunt; observable-behavior criteria per issue;
   sweep for two test poisons — tests asserting the behavior being removed
   (invert and rename, never delete) and single-case tests that pass either
   way (demand the second case). One verifier identity per delivery
   candidate: rounds resume the same session and lane; a replacement reads
   receipts and CI before running anything.
```

- [ ] **Step 4: Add three rows to the Common mistakes table**

Find the last row:

```markdown
| Review drips one blocker per round | audit the whole scope and batch P0/P1 before requesting changes |
```

Append after it:

```markdown
| Fresh verifier reruns every gate "to be safe" | READ the frozen SHA's receipt and exact-head CI; RAN only what they do not cover, or state the reason |
| New build directory per round, SHA, or timestamp | one assigned lane per session for its lifetime; lane cache is disposable, sources are not |
| Status/label/draft flip treated as a push | not a push; nothing reruns |
```

- [ ] **Step 5: Verify carrier-neutral wording**

```bash
grep -c "gate receipt\|assigned build lane\|not a push\|Verify once per frozen" crates/edda-cli/src/skills/coord-orchestrate.md
grep -n -i "cargo\|CARGO_\| -p " crates/edda-cli/src/skills/coord-orchestrate.md
```

Expected: first prints `4` or more; second prints nothing.

- [ ] **Step 6: Commit**

```bash
git add crates/edda-cli/src/skills/coord-orchestrate.md
git commit -m "docs(coordination): brief build lanes and verify-once in coord-orchestrate" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `coord-review.md` — reviewer READ-before-RAN, process finding, template

**Files:**
- Modify: `crates/edda-cli/src/skills/coord-review.md:100-110`
- Modify: `crates/edda-cli/src/skills/coord-review.md:164-166` (Round N template Evidence)

**Interfaces:**
- Consumes: `- Lane:` / `- Receipt:` lines and "gate receipt (SHA, gate set, toolchain, lane, result)" from Task 3, verbatim.

- [ ] **Step 1: Add the verify-once paragraph after the delta paragraph**

Find:

```markdown
Choose gates from the delta. Code/product-blob, base, or toolchain changes run
the relevant code gates. When only docs/evidence changed and code/product
blobs, base, and toolchain are unchanged, reuse still-applicable code results
as `READ` with their source SHA; run only relevant diff/docs/evidence checks
and exact-head CI as `RAN`. Never report a reused result as rerun.
```

Append immediately after it:

```markdown

Verify once per frozen artifact. When the implementer's gate receipt (SHA,
gate set, toolchain, lane, result) matches the reviewed SHA and exact-head CI
is green, cite both as `READ` and RAN only the focused or adversarial checks
they do not cover. A full local rerun requires a stated reason: no receipt,
red or absent CI, or grounds to distrust the receipt. Run in your assigned
build lane; never create an ad-hoc build directory. A status, label, or draft
flip is not a push and reruns nothing.
```

- [ ] **Step 2: Add the process-finding paragraph after the cost paragraph**

Find:

```markdown
Record available elapsed, token, and tool cost. Stop after two consecutive
cycles that change only non-product evidence/docs or harness material without
improving required behavior/proof, or sooner when returns clearly diminish.
Classify and route the finding instead of continuing: follow-up issue for
out-of-scope work, or operator scope expansion when it must join this PR.
```

Append immediately after it:

```markdown

Over-verification you find in the implementer's evidence — a second RAN for
an already-receipted SHA without a reason, full gates for a docs-only push, an
ad-hoc build directory — is a process finding: note it in the cost line, route
it as a `FOLLOW-UP ISSUE`, and do not block a product-green PR on it.
```

- [ ] **Step 3: Extend the Round N template**

Find inside the ```` ```text ```` block:

```text
- RAN: <exact command/check and result on reviewed SHA>
- READ: <reused result and its source SHA, or none>
```

Replace with:

```text
- RAN: <exact command/check and result on reviewed SHA>
- READ: <reused result and its source SHA, or none>
- Lane: <build lane used, or n/a for docs-only>
- Receipt: <implementer gate receipt cited (SHA, gate set, toolchain, lane, result), or none>
```

- [ ] **Step 4: Verify**

```bash
grep -c "Verify once per frozen\|gate receipt\|assigned\|not a push\|process finding" crates/edda-cli/src/skills/coord-review.md
grep -n -i "cargo\|CARGO_\| -p " crates/edda-cli/src/skills/coord-review.md
```

Expected: first prints `5` or more; second prints nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/edda-cli/src/skills/coord-review.md
git commit -m "docs(coordination): read receipts before rerunning gates in coord-review" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: `skills/fleet-orchestrate/SKILL.md` — invariants and controller output

**Files:**
- Modify: `skills/fleet-orchestrate/SKILL.md:41-42` (controller sequence step 7)
- Modify: `skills/fleet-orchestrate/SKILL.md:67-69` (invariants)
- Modify: `skills/fleet-orchestrate/SKILL.md:90-93` (minimum controller output)

**Interfaces:**
- Consumes: wording from Tasks 3–4. Constraint: no Cargo words.

- [ ] **Step 1: Extend controller sequence step 7**

Find:

```markdown
7. Give self-contained briefs, monitor compressed state, adjudicate conflicts,
   and close only against immutable full SHAs with proportional evidence.
```

Replace with:

```markdown
7. Give self-contained briefs (including assigned build lane, verification
   budget, and cleanup authority), monitor compressed state, adjudicate
   conflicts, and close only against immutable full SHAs with proportional
   evidence.
```

- [ ] **Step 2: Add two invariants**

Find:

```markdown
- Record available elapsed/token/tool cost. After two consecutive
  non-product/harness-only cycles without useful progress, or at diminishing
  returns, stop and route follow-up or request operator scope expansion.
```

Append immediately after it:

```markdown
- Verify once per frozen artifact: the full gate set runs once per frozen
  full SHA in an assigned build lane and leaves a gate receipt (SHA, gate set,
  toolchain, lane, result); reviewers READ that receipt and exact-head CI and
  RAN only what they do not cover, stating the reason for any full rerun. A
  status, label, or draft flip is not a push.
- Build environments are bounded: one assigned lane per session for its
  lifetime from a fixed pool named in the brief, never per round, SHA, or
  timestamp; lane build cache is disposable, worktrees and sources are not;
  over the pool ceiling the fleet stops and reports instead of building.
```

- [ ] **Step 3: Extend the minimum controller output**

Find:

```markdown
4. fleet board with task, owner, branch, full SHA, state, review, and next step;
5. acceptance matrix, review handoff (`IN SCOPE`, `FOLLOW-UP ISSUE`, blocking
   P0/P1, `RAN`/`READ`, exact SHA, cost), PR review-loop state, and final merge
   recommendation with current-head P0=0/P1=0.
```

Replace with:

```markdown
4. fleet board with task, owner, branch, full SHA, state, review, build lane,
   and next step;
5. acceptance matrix, review handoff (`IN SCOPE`, `FOLLOW-UP ISSUE`, blocking
   P0/P1, `RAN`/`READ`, lane, gate receipt, exact SHA, cost), PR review-loop
   state, and final merge recommendation with current-head P0=0/P1=0.
```

- [ ] **Step 4: Verify**

```bash
grep -c "Verify once per frozen\|gate receipt\|assigned build lane\|not a push" skills/fleet-orchestrate/SKILL.md
grep -n -i "cargo\|CARGO_\| -p " skills/fleet-orchestrate/SKILL.md
```

Expected: first prints `4` or more; second prints nothing.

- [ ] **Step 5: Commit**

```bash
git add skills/fleet-orchestrate/SKILL.md
git commit -m "docs(fleet): add verify-once and bounded build lane invariants" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: `playbook.md` — brief template, contracts, gate table, mistakes

**Files:**
- Modify: `skills/fleet-orchestrate/references/playbook.md:193-202` (bundle brief block)
- Modify: `skills/fleet-orchestrate/references/playbook.md:211-240` (§8 contracts)
- Modify: `skills/fleet-orchestrate/references/playbook.md:321-329` (§11 gate table and paragraph)
- Modify: `skills/fleet-orchestrate/references/playbook.md:445-448` (§14 table end)

**Interfaces:**
- Consumes: wording from Tasks 3–5. Constraint: no Cargo words.

- [ ] **Step 1: Extend the bundle brief block**

Find inside the ```` ```text ```` block in §7:

```text
Test-first acceptance criteria and exact gates
Drift rule: if HEAD differs, satisfy intent against HEAD and report full SHA
```

Replace with:

```text
Test-first acceptance criteria and exact gates
Build lane: <assigned name from the fixed pool>
Verification budget: focused gates on touched units while iterating; the full
  gate set once per frozen full SHA with a gate receipt; READ receipts and
  exact-head CI before any RAN
Cleanup authority: lane build cache is disposable; worktrees, branches, and
  sources are never deleted
Drift rule: if HEAD differs, satisfy intent against HEAD and report full SHA
```

- [ ] **Step 2: Rewrite the worker, verifier, and controller contracts**

Find:

```markdown
5. Run gates selected from code/product-blob, base, and toolchain changes.
6. Push/open the delivery candidate if authorized, report the full SHA, freeze
   the branch, and leave a receipt tagged `ran` versus `read`.
```

Replace with:

```markdown
5. Run focused gates on touched units while iterating; run the full gate set
   once on the frozen full SHA in the assigned build lane and record the gate
   receipt (SHA, gate set, toolchain, lane, result). Never build in an ad-hoc
   directory.
6. Push/open the delivery candidate if authorized, report the full SHA, freeze
   the branch, and leave a receipt tagged `ran` versus `read` that cites the
   gate receipt.
```

Find:

```markdown
2. Run baseline gates before workers code and classify existing red checks.
```

Replace with:

```markdown
2. Establish the baseline once on the basis SHA before workers code: READ
   exact-head CI and any existing gate receipt, RAN only what they do not
   cover in the assigned verifier lane, and classify existing red checks.
```

Find:

```markdown
5. Review the exact frozen full SHA, complete the whole scoped audit, compare
   against the basis, and batch every blocking P0/P1 with path/symbol and
   failure scenario before requesting changes.
6. Acknowledge that any push voids the verdict.
```

Replace with:

```markdown
5. Review the exact frozen full SHA, complete the whole scoped audit, compare
   against the basis, and batch every blocking P0/P1 with path/symbol and
   failure scenario before requesting changes. READ the frozen SHA's gate
   receipt and exact-head CI; RAN only focused or adversarial checks they do
   not cover, and state the reason for any full rerun.
6. Acknowledge that any push voids the verdict; a status, label, or draft flip
   is not a push and reruns nothing.
7. Keep one verifier identity per delivery candidate: rounds resume the same
   session and lane; a replacement reads receipts and CI before running
   anything.
```

Find:

```markdown
4. Never reinterpret a receipt as acceptance.
```

Replace with:

```markdown
4. Never reinterpret a receipt as acceptance.
5. Assign build lanes from the fixed pool, put the verification budget and
   cleanup authority in every brief, and route over-verification (a second RAN
   for an already-receipted SHA without a reason, full gates for a docs-only
   push, an ad-hoc build directory) as a process finding — cost line,
   `FOLLOW-UP ISSUE`, corrected brief — never as a reason to block a
   product-green candidate.
```

- [ ] **Step 3: Extend the §11 gate table and add the receipt paragraph**

Find:

```markdown
| Only docs/evidence changed; code/product blobs, base, and toolchain unchanged | Reuse still-applicable code gates as `READ` with source SHA; `RAN` only relevant diff/docs/evidence checks and exact-head CI. |

Never label a reused gate `RAN`. Exact-head CI is still required because the
review verdict binds the new full SHA.
```

Replace with:

```markdown
| Only docs/evidence changed; code/product blobs, base, and toolchain unchanged | Reuse still-applicable code gates as `READ` with source SHA; `RAN` only relevant diff/docs/evidence checks and exact-head CI. |
| Code changed; a gate receipt for this exact SHA, gate set, and toolchain exists and exact-head CI is green | READ the receipt and CI; `RAN` only focused or adversarial checks they do not cover. Full rerun only with a stated reason (no receipt, red or absent CI, grounds to distrust the receipt). |
| Status, label, or draft flip without a push | Not a new artifact; nothing reruns. |

Never label a reused gate `RAN`. Exact-head CI is still required because the
review verdict binds the new full SHA.

Verify once per frozen artifact. The implementer's full gate run leaves a gate
receipt (SHA, gate set, toolchain, lane, result); the reviewer cites it. Runs
happen in the assigned build lane, never in an ad-hoc directory.
Over-verification found in evidence is a process finding — cost line,
`FOLLOW-UP ISSUE`, corrected brief — not a product blocker.
```

- [ ] **Step 4: Extend the §14 table**

Find the last row:

```markdown
| “Receipt says tests pass, so merge.” | Receipt is worker evidence; verifier runs the required proportional gates and merge authority accepts. |
```

Replace with:

```markdown
| “Receipt says tests pass, so merge.” | Receipt is worker evidence; the verifier READs it against exact-head CI, RANs what they do not cover, and merge authority accepts. |
| “A fresh session should build in its own directory to be safe.” | Use the assigned lane; lanes are the isolation. Ad-hoc build directories are forbidden. |
| “The verifier must rerun everything to be independent.” | Exact-head CI and the gate receipt are independent evidence; RAN only what they do not cover, or state the reason. |
| “The PR flipped to ready, so rerun the gates.” | Not a push; nothing reruns. |
| “Cleanup is destructive, so leave every build directory.” | Lane build cache is disposable by design; worktrees, branches, and sources are not. |
```

- [ ] **Step 5: Verify**

```bash
grep -c "gate receipt\|assigned build lane\|not a push\|Verify once per frozen\|Build lane:" skills/fleet-orchestrate/references/playbook.md
grep -n -i "cargo\|CARGO_\| -p " skills/fleet-orchestrate/references/playbook.md
```

Expected: first prints `8` or more; second prints nothing.

- [ ] **Step 6: Commit**

```bash
git add skills/fleet-orchestrate/references/playbook.md
git commit -m "docs(fleet): brief lanes, receipts, and verify-once in the playbook" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: pressure-test fixtures 4–7

**Files:**
- Modify: `skills/fleet-orchestrate/references/review-scope-pressure-tests.md` (append after fixture 3)

**Interfaces:**
- Produces: fixtures `4`–`7` used by Task 8's dry runs and by future review-policy changes (SKILL.md already says these tests are REQUIRED when changing review policy).

- [ ] **Step 1: Append four fixtures**

Append at the end of the file (after the fixture 3 `**Pass:**` paragraph):

```markdown

## 4. Frozen SHA with receipt and green CI

The implementer froze full SHA `X`, ran the full gate set once in lane
`worker-1`, and recorded the gate receipt (SHA, gate set, toolchain, lane,
result). Exact-head CI is green on `X`. The reviewer's brief assigns lane
`verifier`.

**Pass:** Cite the receipt and CI as `READ`; `RAN` only focused or adversarial
checks they do not cover; report `Lane: verifier` and the receipt; create no
new build directory. A full local rerun without a stated reason fails.

## 5. Iterating worker

A worker changes two units across several commits before freezing.

**Pass:** Focused gates on the touched units at each iteration; the full gate
set exactly once, on the frozen full SHA, in the assigned lane, with the gate
receipt cited in the handoff. Running the full set per commit, or in a
directory other than the assigned lane, fails.

## 6. Draft-to-ready flip

After the final current-head LGTM on `X`, the PR is flipped from draft to
ready. No push happened.

**Pass:** No gate runs; the verdict on `X` stands. Any rerun fails.

## 7. Missing receipt and red CI

The frozen SHA has no gate receipt and exact-head CI is red on one job.

**Pass:** `RAN` the full gate set in the assigned lane with the reason stated
(`no receipt; CI red on <job>`), classify the red job, and record the receipt
so later rounds READ it. Silently rerunning without the reason, or building
outside the assigned lane, fails.
```

- [ ] **Step 2: Verify**

```bash
grep -c "^## " skills/fleet-orchestrate/references/review-scope-pressure-tests.md
```

Expected: `7`.

- [ ] **Step 3: Commit**

```bash
git add skills/fleet-orchestrate/references/review-scope-pressure-tests.md
git commit -m "docs(fleet): add verify-once and build lane pressure tests" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: consistency check, dry-run pressure tests, PR

**Files:**
- Read only: all files above.
- Create: nothing in the repository (dry-run outputs go into the PR body).

**Interfaces:**
- Consumes: fixtures 4–7 from Task 7; every carrier from Tasks 1–6.

- [ ] **Step 1: A1 consistency grep across the six carriers**

Run from the worktree root:

```bash
for f in .claude/CLAUDE.md AGENTS.md crates/edda-cli/src/skills/coord-orchestrate.md crates/edda-cli/src/skills/coord-review.md skills/fleet-orchestrate/SKILL.md skills/fleet-orchestrate/references/playbook.md; do
  printf '%s: once=%s read=%s lane=%s flip=%s\n' "$f" \
    "$(grep -c -i 'once per frozen' "$f")" \
    "$(grep -c 'READ' "$f")" \
    "$(grep -c -i 'lane' "$f")" \
    "$(grep -c -i 'not a push' "$f")"
done
grep -l -i "cargo\|CARGO_" crates/edda-cli/src/skills/coord-orchestrate.md crates/edda-cli/src/skills/coord-review.md skills/fleet-orchestrate/SKILL.md skills/fleet-orchestrate/references/playbook.md skills/fleet-orchestrate/references/review-scope-pressure-tests.md
```

Expected: every carrier line shows all four counts ≥ 1; the final `grep -l` prints nothing (no Cargo words in shipped/methodology files). If any count is 0, fix that carrier in place and amend the relevant task commit before continuing.

- [ ] **Step 2: Dry-run pressure tests with a fresh agent**

For each fixture 4–7, start a **fresh** agent session whose working directory is the worktree (so it reads the new `AGENTS.md`, `.claude/CLAUDE.md`, and skills) — a fresh Codex session (`codex exec` from the worktree) is preferred because Codex is the population that over-verified; a fresh Claude subagent given only the repository files is acceptable. Use this prompt, substituting the fixture text:

```text
You are the reviewer/worker described below. Read AGENTS.md, .claude/CLAUDE.md,
and the coord-review / coord-orchestrate skills in this repository first.
DO NOT run any command. Output only the plan you would execute: the exact
commands, each tagged RAN or READ, the build lane you would use, the reason for
any full rerun, and what you would write under Evidence in the review handoff.

Fixture:
<paste fixture N from skills/fleet-orchestrate/references/review-scope-pressure-tests.md>
```

Record PASS/FAIL against the fixture's `**Pass:**` clause:

| Fixture | Pass condition to check in the output |
|---|---|
| 4 | full workspace gates tagged READ (receipt + CI); RAN only focused/adversarial; `Lane: verifier`; no new target directory |
| 5 | focused `-p` gates per iteration; full set exactly once at freeze; lane named; receipt cited |
| 6 | zero gate commands |
| 7 | full set tagged RAN with the stated reason; lane named; receipt recorded for later rounds |

Expected: 4/4 PASS. If any fixture fails, the wording that the agent missed is the defect: strengthen that carrier (usually `AGENTS.md` or the skill the agent read), amend the task commit, and rerun that fixture.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin docs/verification-cost-discipline
gh pr create --title "docs(coordination): verification cost discipline — ladder, lanes, receipts" --body-file - <<'EOF'
## Summary

Closes the cost gap left after GH-474: rules regulated evidence but not cost
and reclamation. Adds the verification ladder (L0 iterate / L1 freeze once per
frozen SHA / L2 READ receipt + exact-head CI before RAN / L3 nothing reruns on
a status flip), bounded build lanes, gate receipts, and brake authority to
every policy carrier. Docs-only.

Spec: docs/superpowers/specs/2026-08-18-verification-cost-discipline-design.md
Plan: docs/superpowers/plans/2026-08-18-verification-cost-discipline-pr1-edda-policy.md

## Carriers

- .claude/CLAUDE.md — Cargo L0–L3, build lanes, split checklist, cost subsection
- AGENTS.md — Codex entry bullets
- crates/edda-cli/src/skills/coord-orchestrate.md, coord-review.md — carrier-neutral
- skills/fleet-orchestrate/SKILL.md, references/playbook.md, references/review-scope-pressure-tests.md (fixtures 4–7)

## Evidence

- RAN: A1 consistency grep (all four ideas in each of six carriers; no Cargo words in shipped/methodology files)
- RAN: dry-run pressure tests 4–7 with a fresh agent — <n>/4 PASS (transcripts summarized below)
- READ: exact-head CI on this PR (docs-only; no local Cargo gate, no target directory created)
- Lane: n/a (docs-only)
- Receipt: none (docs-only)

## Follow-ups (not in this PR)

- fleet-workstation PR-2: scripts/lane.ps1, doctor lanes check, adapter bullets, pin bump to this merge
- fleet-playbook: fleet-review / fleet-pr-loop "re-run gates" wording (operator ruling)
- pre-existing ubuntu flake codex_app_server::tests::dropping_concrete_client_makes_child_pid_disappear (main 7f5d155)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
```

- [ ] **Step 4: Post the review handoff comment**

Fill the SHAs from `git rev-parse HEAD` and `git rev-parse origin/main`, then:

```bash
gh pr comment <PR#> --body-file - <<'EOF'
## Code Review Handoff: Round 1
Full SHA: <full HEAD SHA>
Base full SHA: <full origin/main SHA>
IN SCOPE: policy wording in the six carriers + pressure fixtures 4–7; consistency (spec §6 A1); dry-run pressure tests (spec §6 A4)
FOLLOW-UP ISSUE: none
Blocking counts entering review: P0=0, P1=0
Evidence:
- RAN: A1 grep (see PR body); dry-run fixtures 4–7 <n>/4
- READ: exact-head CI
- Lane: n/a (docs-only)
- Receipt: none (docs-only)
Cost: elapsed=<available/unknown>, tokens=<available/unknown>, tools=<available/unknown>
Request: audit the whole scoped surface; publish no self-verdict from the implementer
EOF
```

- [ ] **Step 5: Record on the rail**

```bash
edda note "PR-1 verification cost discipline open: <PR URL> at <full SHA>; docs-only; A1 grep pass; dry-run fixtures <n>/4; awaiting Code Review: Round 1" --tag fleet-board
```

Expected: the PR shows the handoff comment; CI runs on the exact head; no local build output was created (`git status` clean, no new `target/` in the worktree).

---

## Round 1 review amendments (2026-08-18)

The text blocks above are the plan as executed at `738c5ce`. An independent
review of that SHA returned `Changes Requested` with P0=0, P1=4, and the fixes
changed wording this plan quotes verbatim. This plan is kept as the execution
record; the **spec is the living contract**. What changed:

1. **CI coverage stated accurately.** The ladder's justification claimed CI runs
   full workspace gates on three operating systems. It does not: Windows tests
   only a 7-crate subset (`.github/workflows/ci.yml`, GH-433). `.claude/CLAUDE.md`
   now carries a CI coverage table, L2 names the uncovered surface as a
   legitimate reason to RAN, and spec §1/§4.1 match. *(Narrowed by a later
   round, GH-498: the gap earns a **focused** check of the uncovered surface,
   never a full local rerun; and what Windows leaves unexercised is those
   crates' own test targets, not their libraries — `edda` depends on the whole
   workspace, so `cargo test -p edda` links every crate. The spec carries the
   current wording.)*
2. **Deterministic-red clause distributed to all six carriers** (it had landed
   in three), and pressure fixture 7 — which still demanded a full run for red
   CI — split into fixture 7 (deterministic → do not run) and fixture 8
   (environmental → re-run the failed job, then the full set once).
3. **Pre-commit L1 block aligned with the ladder's L1 row**: `cargo fmt --all
   --check`, `CARGO_INCREMENTAL=0`, and the gate receipt were missing from the
   checklist, so a worker following it would freeze without a receipt — which
   hands L2 a reason to run everything again and cancels the saving.
4. **Lane root defined.** `<lane-root>` appeared in the fallback instruction but
   in no brief template. `.claude/CLAUDE.md` now gives the concrete default,
   `playbook.md` and `coord-orchestrate.md` require the absolute root in the
   brief, and `coord-review.md` states the solo default.

New fixture 9 covers the CI-coverage-gap case that no fixture exercised.

## Post-merge (outside this plan, listed so nobody forgets)

1. `scripts/skills/sync-fleet-orchestrate.ps1` to propagate `skills/fleet-orchestrate/*` to fleet-playbook / user skills (see `docs/guides/fleet-skill-distribution.md`).
2. fleet-workstation PR-2 pins the merged edda revision and ships `lane.ps1`, doctor lanes check, and adapter bullets (separate plan in that repository).
3. Optional operator line in `C:\ai_agent\EXECUTION-COORDINATION.md`.
