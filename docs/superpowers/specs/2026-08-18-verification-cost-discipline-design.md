# Verification Cost Discipline — Design

Date: 2026-08-18
Status: draft for operator review
Scope: edda coordination policy (this repo) + fleet-workstation machine tooling
Basis: origin/main `7f5d1555b4c00e4fe6ac9a452a3c81c62289972c`

## 1. Problem

Fleet sessions in this repository over-verify, and every verification round
runs as a new cold build in a new Cargo target directory. Nothing in the
current rules bounds that cost or authorizes reclaiming it.

Evidence collected on 2026-08-18 (read-only; nothing was modified or deleted):

- 12 ad-hoc Cargo target directories exist next to the repository
  (`C:\ai_agent\edda-target-*`), named per issue × round × SHA × timestamp:
  `gh466-drill`, `gh466-drill-<ts>` ×3, `gh466-round3`, `gh466-round4`,
  `gh466-tests`, `gh466-final`, `gh466-quoted-command-fix`,
  `gh466-verifier-<sha>`, `gh465-verifier-<sha>`, `recovery-wave1`. One of them
  (`recovery-wave1`) is 12.79 GB (deps 7.50 GB, incremental 5.11 GB). The
  operator's audit counted 190 isolated-target Cargo invocations across 15
  distinct target directories (the other three lived elsewhere or are already
  gone), 63 workspace-level builds, 40 `--all-targets` runs, and zero cleanup
  calls, totalling roughly 194 GB.
- One issue (GH-466, PR #473) went through nine review rounds; each round was
  assigned to a new zero-context session label (`round6-verifier`,
  `gh466-r7-local-review`, `gh466-r7-local-rereview`, `gh466-round7-verifier`,
  `gh466-post-r7-local-review`, `gh466-round8-pr-verifier`,
  `gh466-round9-merge-verifier`). Wave 1 had five "final verifier" passes
  (task rail #16, #20, #22, #25, #32).
- Ledger decision `d-032` records a full workspace rerun (fmt, clippy
  `--workspace --all-targets`, `test --workspace`, isolated target) after PRs
  were flipped from draft to ready with zero code change.
- GitHub CI already runs fmt, plus clippy `--workspace --all-targets` on Linux,
  macOS and Windows, plus `cargo test --workspace` on Linux and macOS, for every
  push (the "7/7 green" recorded throughout the ledger). Local full reruns by
  verifiers and controllers largely duplicate that independent run. **Windows
  tests only a 7-crate subset** (`edda-store`, `edda-ledger`, `edda-search-fts`,
  `edda-transcript`, `edda-bridge-claude`, `edda-conductor`, `edda`; GH-433), so
  Windows behavior in the other 16 crates is genuinely uncovered — the ladder
  must name that gap rather than let a reviewer treat green CI as total
  coverage.

## 2. Root causes

GH-474 (PR #476, merged) already bounded review *scope*: `IN SCOPE` /
`FOLLOW-UP ISSUE`, proportional gates for docs-only pushes, cost recording,
and "stop after two non-product cycles". Three causes remain open:

1. **The canonical checklist is workspace-wide.** `.claude/CLAUDE.md`
   "Pre-commit Checklist" says `cargo test --workspace`; `AGENTS.md` tells
   Codex to run the checks required by `.claude/CLAUDE.md` before claiming
   completion. A fresh session following the rules therefore runs the 23-crate
   workspace for every change, and the code-changed row of the proportional
   gate table ("run the relevant code gates") gives it no smaller default.
2. **Nothing bounds the build environment.** No rule names where a session may
   build, how many build directories may exist, who assigns them, or whether
   deleting build output is destructive. Sessions that met Windows `LNK1104`
   file locks on a shared target correctly isolated — and then isolated per
   round, per SHA, and per timestamp because nothing said "reuse a lane".
   Task briefs forbade destructive cleanup without distinguishing source
   worktrees from build cache, so nothing was ever reclaimed.
3. **Nobody has a brake.** The only stop is prose ("stop after two
   non-product cycles") plus controller discipline. A fresh verifier session
   has no way to see that the frozen SHA was already gated, no receipt to
   cite instead of rerunning, and no tool that refuses to start when the
   machine is over budget. Verifier identity changes every round, so nothing
   accumulates.

The gap is methodological, not Cargo-specific: rules regulate evidence but not
cost and reclamation. Cargo on Windows only makes the cost visible in
gigabytes; the same shape appears with any toolchain cache.

## 3. Goals and non-goals

Goals:

- One full workspace verification per frozen artifact, cited everywhere else.
- A bounded, assigned, reusable set of build lanes; no ad-hoc build dirs.
- A brake that fails closed without relying on a fresh session's memory.
- Rules stated once per layer, distributed through existing pipelines
  (edda skills → fleet-workstation pin → installed skills; fleet-orchestrate
  → fleet-playbook via `scripts/skills/sync-fleet-orchestrate.ps1`).

Non-goals:

- Deleting any existing target directory (operator action, out of scope).
- Lowering review independence or the PR-visible review-fix loop.
- Changing CI, adding an `edda gate` subcommand, or a Bash twin of the tool.
- Editing the fleet-playbook `fleet-review` / `fleet-pr-loop` skills; that is
  a fleet-playbook ruling and is listed as follow-up.

## 4. Design

### 4.1 Verification ladder (carrier-neutral)

| Level | Who / when | Required | Forbidden |
|---|---|---|---|
| L0 iterate | worker while editing | focused gates on touched units, in the session's own lane, incremental cache on | workspace gates per edit |
| L1 freeze | worker, once per frozen full SHA before push/PR update | full gate set on the committed SHA in the session's lane; receipt recorded | freezing a dirty tree; a second L1 run for the same key |
| L2 review | verifier, once per frozen full SHA | READ the L1 receipt and exact-head CI; RAN adversarial/focused checks and anything neither covers, including behavior on a platform the CI matrix only partially covers | full local rerun without a stated reason (missing receipt, red/absent CI, receipt suspected invalid, real CI coverage gap); a full run against deterministically red CI, which cannot change the verdict |
| L3 pre-merge | controller / merge authority | READ exact-head CI + final current-head LGTM; RAN only a merge-conflict check against current base | any rerun triggered by a status/label/draft flip — that is not a push |

Independence is preserved **only as far as the project's CI actually reaches**,
and receipts are keyed to the immutable SHA. This is a precondition of the
ladder, not a given: before a reviewer may cite CI as independent evidence, the
project must state what its CI covers and what it does not, and the uncovered
surface is a standing reason to RAN. In this repository the gap is Windows
tests outside the 7-crate subset (§1). A project whose CI covers less gets less
independence from this ladder, and its L2 row must say so.

Carrying this policy to another project (§4.5, §5 slice 2) therefore carries the
obligation with it: the adapter bullet states the rule, and the project supplies
its own coverage statement. A verifier who cannot find one treats CI as
unproven coverage and RANs what the receipt alone would otherwise carry.

A verifier keeps the right to RAN anything, but must state why the READ evidence
is insufficient.

Cargo instantiation for this repository (goes into `.claude/CLAUDE.md`):

```bash
# L0 focused (touched crates)
cargo fmt --all --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>

# L1 freeze (once per frozen SHA, clean tree, incremental off)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 4.2 Receipts and READ-before-RAN

A gate receipt is one append-only JSON line:

```json
{"ts":"2026-08-18T05:12:03Z","repo":"edda","sha":"<full sha>","dirty":false,
 "gate":"freeze","crates":[],"toolchain":"<rustc -vV sha256>",
 "lockfile":"<Cargo.lock sha256>","lane":"verifier","result":"PASS",
 "duration_s":812,"by":"<session or label>"}
```

Key = (`repo`, `sha`, `gate`, `crates`, `toolchain`, `lockfile`). Rules:

- A receipt with `dirty:true` is never reusable.
- Same key + `PASS` → the run is satisfied by READ; the tool prints the exact
  citation line to paste into a handoff:
  `READ: freeze PASS on <sha> (lane verifier, <ts>, receipt <n>)`.
- `--force` performs a fresh RAN and appends a new receipt; it never deletes
  the old one.
- When the repository has `.edda/`, the tool also records
  `edda note "gate <gate> <result> sha=<sha> lane=<lane> key=<key-hash>" --tag gate`
  so `edda search` / `edda ask` can find it from any session.
- Handoffs and receipts already distinguish `RAN` from `READ` (GH-474); this
  design only supplies the durable thing to READ.

### 4.3 Build lanes (machine level)

**Scope of this section — narrower than the rest of the design.** Lanes govern
*local compilation cache for a large Cargo workspace on one Windows
workstation*. They are not fleet-wide infrastructure and not a cost of adopting
edda: the installed binary is ~26 MB and a project ledger is 0.4–9 MB, while a
long-lived `target/debug` for this 23-crate workspace measured 40.9 GB. Only
the second number motivates anything here. A project that is not compiled on
this machine needs no lane, and `lane.ps1` must be presented as a tool for
local heavy builds rather than as a standard every fleet session adopts.

**Footprint before ceilings.** That 40.9 GB is mostly reclaimable, not
intrinsic: 21.6 GB sat in 509 `incremental` session directories, every one older
than 24 hours — orphans from interrupted or killed builds that Cargo never
collects — and 8.1 GB was Windows debug symbols (`edda.pdb` 310 MB against a
51 MB `edda.exe`) because the workspace tunes `[profile.release]` only. Any
fixed pool ceiling is therefore provisional until that footprint work lands;
sizing a limit against the 194 GB pathology would refuse during normal
operation, since one warm lane alone approaches 40 GB. Reclaim by age first,
measure second, set a number third.

- One lane root per machine, configurable (`FLEET_LANE_ROOT`, default
  the `LOCALAPPDATA` directory plus `\fleet-workstation\lanes`, written
  shell-appropriately — `$env:LOCALAPPDATA` in PowerShell, `$LOCALAPPDATA` in
  Git Bash; `%LOCALAPPDATA%` expands only in cmd.exe); every lane is a
  subdirectory, so
  one scan measures the whole pool.
- Fixed allowlist: `worker-1`, `worker-2`, `verifier`, `verifier-2`. Unknown
  names are refused. The controller's primary checkout keeps its normal
  `target/`; it is not a lane.
- The controller assigns a lane in every brief **for a session that builds or
  caches locally**; a session that compiles nothing gets no lane and reports
  `n/a`. A session that has one keeps it for its lifetime; the same PR's verifier rounds reuse `verifier`; a lane changes
  hands only when the previous holder is finished or dead. Lanes are per
  concurrent session, never per round, SHA, or timestamp.
- Verifier lanes and every `freeze` run set `CARGO_INCREMENTAL=0` (one-shot
  builds gain nothing from incremental caches; that was 5.11 GB of 12.79 GB in
  the sampled target). Worker lanes iterating at L0 keep incremental on.
- Cargo's build-directory lock serializes *builds* inside a lane, but a test
  binary still running in one session can block relinking in another
  (`LNK1104`). A lane therefore has exactly one holder at a time. The tool
  records the holder label and last-use time per lane (`-As <label>`, default
  `$env:EDDA_SESSION_LABEL`, else the user name), shows them in `-Status`, and
  warns when the caller differs from the recorded holder. Assignment itself
  stays with the controller's brief. This addresses the original `LNK1104`
  reason for isolation without unbounded directories.
- Reclamation is authorized by design: lane contents are disposable build
  cache. `-Reclaim` removes build output of idle lanes (last use older than
  `FLEET_LANE_IDLE_HOURS`, default 6); `-Reclaim -All` removes every lane's
  build output. The tool never removes anything outside the lane root, never
  removes a path containing `.git`, and never touches worktrees, branches, or
  sources. Briefs say so verbatim.
- Capacity: **the tool ships with no default ceiling** (see the footprint note
  above — a limit sized against the 194 GB pathology would refuse during normal
  operation). `-Status` and `doctor.ps1` report the pool size; `FLEET_LANE_WARN_GB`
  and `FLEET_LANE_CEILING_GB` are unset until an operator sets them from a
  measured steady state. Once a ceiling exists, exceeding it prints the pool
  table and the reclaim command and exits non-zero, with no bypass flag.

### 4.4 Brake authority — who stops it

1. **Brief**: every worker/verifier brief carries `Build lane`, `Verification
   budget` (L0 while iterating; L1 once per frozen SHA; READ before RAN), and
   `Cleanup authority` (lane cache disposable; worktrees never). A brief
   without these fields is incomplete.
2. **Tool**: the lane tool refuses unknown lanes, refuses `freeze` on a dirty
   tree, satisfies repeated keys by READ, and refuses to start over the
   ceiling. A fresh session that follows AGENTS.md reaches the tool before it
   reaches Cargo.
3. **Reviewer / controller**: a handoff or receipt showing RAN workspace
   gates for a docs-only push, a second RAN for an already-receipted key
   without a stated reason, or an ad-hoc build directory is a *process
   finding*: record it in the cost line, route it as a `FOLLOW-UP ISSUE` on
   process, and correct the next brief. It does not block a product-green PR;
   the waste already happened, and blocking would add more.
4. **Continuity**: one verifier identity per PR; rounds resume the same
   session and lane; a replacement reads receipts and CI before running
   anything.
5. **Doctor**: `doctor.ps1` shows lane pool size and status; over-ceiling is a
   visible FAIL that any operator or session sees before starting work.

### 4.5 Where each rule lives

| Layer | Carrier | Wording |
|---|---|---|
| Methodology (source) | `skills/fleet-orchestrate/SKILL.md` invariants; `references/playbook.md` §7 brief template, §8 worker/verifier contracts, §11 gate table, §14 common mistakes | carrier-neutral: ladder, receipts, lanes as "assigned build lanes", brake authority, verifier continuity |
| Shipped skills (all projects) | `crates/edda-cli/src/skills/coord-orchestrate.md` (brief step, review scope contract, traffic table); `coord-review.md` (gate paragraph, Round N template gains `Lane:` and `Receipt:` under Evidence) | carrier-neutral, no Cargo words |
| Codex entry | `AGENTS.md` | two bullets: ladder + assigned lane, READ before RAN; never create ad-hoc build directories; "run the checks required by CLAUDE.md at the ladder level that matches the change" |
| Project canonical | `.claude/CLAUDE.md` Testing Standards + Pre-commit Checklist | Cargo L0/L1 commands above; "Build lanes" subsection; checklist split into before-commit (L0) and before-freeze (L1) |
| Machine tooling (local heavy builds only) | fleet-workstation `scripts/lane.ps1` + tests; `doctor.ps1` lanes check; `adapters/codex/AGENTS.global.md` and `adapters/claude/CLAUDE.global.md` one bullet each, worded so a session that runs no local build knows the rule does not apply to it; `fleet-workstation.json` / `source-lock.json` pin bump | tool contract in §4.6 |
| Operator's generic doc | `C:\ai_agent\EXECUTION-COORDINATION.md` | one axiom, only if the operator approves: regulate cost and reclamation, not only evidence |

Existing distribution stays the single path: fleet-workstation pins an edda
revision, so the shipped-skill wording is edited only in edda; fleet-playbook
receives `fleet-orchestrate` through the GH-478 sync tool.

### 4.6 Lane tool contract (fleet-workstation `scripts/lane.ps1`)

```
lane.ps1 -Lane <name> -Gate focused|freeze|premerge
         [-Crate a,b] [-Base <ref>] [-Force] [-Repo <path>] [-As <label>]
lane.ps1 -Status
lane.ps1 -Reclaim [-All] [-WhatIf]
```

- `-Repo` defaults to the current directory's Git top level; the receipt
  `repo` field is the `origin` URL when present, else the top-level directory
  name. `-Base` defaults to `origin/main`.
- `-Gate focused` requires `-Crate` or derives crates from
  `git diff --name-only <Base>` mapped to `crates/<name>/`; runs the L0 set.
- `-Gate freeze` refuses a dirty tree, sets `CARGO_INCREMENTAL=0`, runs the L1
  set, records the receipt.
- `-Gate premerge` runs no Cargo: it reads exact-head CI for the SHA
  (`gh run list --commit`), finds a `freeze` receipt for the key, runs
  `git merge-tree` against `-Base`, and prints READ lines plus a summary
  (CI per job, receipt found or missing, merge clean or conflicting).
- Toolchain profile `cargo` is built in and selected when `Cargo.toml` exists
  at the repo root; other profiles are follow-up.
- Exit codes: 0 pass or READ-satisfied; 1 gate failed; 2 refused (unknown
  lane, dirty freeze, over ceiling, missing tool); 3 internal error.
- Never: delete outside the lane root, touch `.git`, follow reparse points,
  modify the repository, or bypass capacity.
- Tests use a stub `cargo`/`gh` on `PATH`; no real compilation.

## 5. Delivery slices

Each slice gets its own implementation plan. **Order matters and changed after
measurement:** shrink the footprint before building a tool that polices it, or
the tool enforces a limit against an artificially inflated number.

1. **PR-1 (edda)** — policy only: `.claude/CLAUDE.md`, `AGENTS.md`,
   `crates/edda-cli/src/skills/coord-orchestrate.md`, `coord-review.md`,
   `skills/fleet-orchestrate/SKILL.md`, `references/playbook.md`, plus this
   spec and its plan. Docs-only: gates are exact-head CI plus the docs/skill
   consistency checks in §6; no local Cargo run. *(Merged as `91534f9`;
   scope-correction follow-up in progress.)*
2. **PR-1b (edda) — footprint, new and now ahead of the tool.** Reclaim stale
   `incremental` sessions by age, and evaluate a debug-profile setting
   (`debug = "line-tables-only"` or `debug = 1`) against a measured before/after
   in an isolated directory. Both are the difference between a ~40 GB and a
   ~10 GB steady state, and neither needs a tool. Only after this does a pool
   ceiling mean anything.
3. **PR-2 (fleet-workstation)** — `scripts/lane.ps1`, tests, `doctor.ps1`
   lanes check, adapter bullets, docs page, pin bump to the edda revision that
   contains PR-1, `source-lock.json` digests, `update.ps1` verified. Positioned
   as a tool for machines that compile a large workspace locally, not as
   default workstation infrastructure: `bootstrap.ps1` installs it without
   implying every session uses it, and its docs page opens with the scope
   statement from §4.3. Thresholds come from PR-1b's measurement.
3. Follow-ups (issues, not this work): fleet-playbook `fleet-review` /
   `fleet-pr-loop` "re-run gates" wording; `edda gate` native receipts;
   Bash twin of `lane.ps1`; repo-level gate profile override; operator
   decision on the `EXECUTION-COORDINATION.md` axiom; the pre-existing
   ubuntu-only flake in `codex_app_server` observed red on `7f5d155`
   (classified baseline; unrelated to PR #479). The test was
   `dropping_concrete_client_makes_child_pid_disappear`, renamed to
   `..._stops_the_child_process` in GH-482 because a zombie has already
   terminated, so the assertion was never about the PID disappearing.

## 6. Acceptance criteria

- A1 Consistency: the six edda carriers state the ladder, receipts, lanes, and
  brake without contradiction; a grep for `READ` before `RAN`, `lane`, and
  `once per frozen` hits each of `.claude/CLAUDE.md`, `AGENTS.md`,
  `coord-orchestrate.md`, `coord-review.md`, `SKILL.md`, `playbook.md`.
  Shipped skills contain no Cargo-specific commands.
- A2 Tool self-test (fleet-workstation, stubbed toolchain): unknown lane →
  exit 2; `freeze` on dirty tree → exit 2; `focused`/`freeze` set
  `CARGO_TARGET_DIR` under the lane root and `CARGO_INCREMENTAL=0` where
  required; receipt hit → READ output and zero toolchain invocations;
  `-Force` → RAN and a new receipt, old receipt intact; total ≥ ceiling →
  exit 2 with reclaim hint; `-Reclaim` removes only lane build output and
  refuses any path outside the root or containing `.git`; receipts file is
  append-only.
- A3 Doctor: `doctor.ps1` prints lane pool total, per-lane size, and
  OK/WARN/FAIL against the thresholds.
- A4 Fresh-agent pressure tests (same method as GH-474), fixtures 4–9 in
  `skills/fleet-orchestrate/references/review-scope-pressure-tests.md`:
  (i) frozen SHA with a `freeze` receipt and green exact-head CI → READ for the
  full gates, RAN only focused/adversarial checks, assigned lane named, no new
  build directory; (ii) worker iterating → focused gates on touched crates,
  full set exactly once at freeze; (iii) draft→ready flip → no gate run;
  (iv) missing receipt plus *deterministically* red CI → classify the red job,
  request changes, and do **not** spend a full run; (v) missing receipt plus
  *environmentally* red CI → re-run the failed job only, then the full set once
  with the reason stated; (vi) green receipt and green CI but behavior on a
  platform CI only partially covers → name the gap and RAN a focused check for
  it.
- A5 Distribution: fleet-workstation pin and lock updated to the merged edda
  revision; `update.ps1` installs the new wording for both runtimes.

## 7. Risks and trade-offs

- Independence versus reuse: mitigated by CI-as-independent-run, immutable
  SHA keys, and the verifier's retained right to RAN with a stated reason.
- Lane contention: four lanes cap concurrency at four building sessions;
  the controller already caps worker WIP at verifier capacity, so this is a
  formalization, not a new limit.
- Windows-only tool: acceptable for the workstation's declared target
  (Windows 11 x64); Bash twin is follow-up.
- Wording drift across carriers: single source in edda; PR-2 pins it.

## 8. Decisions taken in this design (defaults, revisable in the plan)

- Lane allowlist: `worker-1`, `worker-2`, `verifier`, `verifier-2`.
- Thresholds: **none by default.** `FLEET_LANE_WARN_GB` and
  `FLEET_LANE_CEILING_GB` are operator-set from a measured steady state; the
  earlier 35/50 GB pair was drawn from the pathology, not from a healthy
  lane, and is superseded (§4.3).
- Receipt store: `<lane-root>\receipts.jsonl` plus `edda note --tag gate`
  when `.edda/` exists.
- Verifier lanes always `CARGO_INCREMENTAL=0`; worker lanes only for `freeze`.
- Controller checkout keeps its normal `target/`; only fleet sessions use
  lanes.
