# Reference: VALIDATION.md

> This is a real validation plan from a production project.
> Use this as the quality bar when generating VALIDATION.md.

---

# Validation Plan

## Track Acceptance Criteria

### Track A: Ledger + Blob Store

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| Build | `cargo build` zero errors | `cargo build 2>&1` |
| Clippy | Zero warnings | `cargo clippy -- -D warnings` |
| init | `.gctx/` complete directory structure | `ls .gctx/ledger/ .gctx/branches/ .gctx/refs/` |
| note | Writes to events.jsonl with correct hash chain | `gctx note "test" && cat .gctx/ledger/events.jsonl` |
| run | Writes cmd event + blob | `gctx run -- echo hi && ls .gctx/ledger/blobs/` |
| hash chain | Each parent_hash = previous event's hash | Line-by-line verification script |
| blob addressing | sha256(bytes) = filename | Verify blob filenames |

### Track B: Commit + Derive Views

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| commit | Writes commit event, auto label=claim when no evidence | `gctx commit -m "test" && cat events.jsonl` |
| status | Shows HEAD/last commit/uncommitted events | `gctx status` |
| context | Outputs Markdown snapshot | `gctx context --depth 5` |
| rebuild | Delete views then rebuild | `rm -rf .gctx/branches && gctx rebuild --all` |
| deterministic | Repeat rebuild = identical output | diff two rebuild outputs |

---

## Golden Path Scenarios

### GP-1: Minimum Closed Loop (Track A-B)

**Description**: From zero to outputting a context snapshot for an LLM agent.

**Steps**:
1. `gctx init` → create workspace
2. `gctx note "todo: implement auth" --tag todo` → record todo
3. `gctx run -- echo hello` → record command execution
4. `gctx commit -m "c1" --evidence evt_xxx` → create milestone
5. `gctx context --depth 5` → output context snapshot
6. `rm -rf .gctx/branches && gctx rebuild --all` → full view rebuild

**Verification**: events.jsonl event count is correct, hash chain is continuous, context includes commits + evidence, views are fully rebuilt.

---

### GP-2: Explore Branch and Merge Back (Track C)

**Description**: Create a branch for experimentation, merge back to main on success.

**Steps**:
1. `gctx branch create feat/x -m "try alt approach"`
2. `gctx switch feat/x`
3. `gctx run -- echo "experiment"`
4. `gctx commit -m "feat: new approach"`
5. `gctx switch main`
6. `gctx merge feat/x main -m "accept feature x"`
7. `gctx context --depth 10`

**Verification**: main/log.md has MERGE record, branches.json is correct, context includes adopted commits.

---

### GP-3: Governance Closed Loop (Track E-F)

**Description**: Propose draft → policy requires approval → approve → apply succeeds.

**Steps**:
1. `gctx run -- bash -c "exit 2"` → record failed command
2. `gctx draft propose -m "risky release" --label risk`
3. `gctx draft apply drf_xxx` → rejected by policy (needs approval)
4. `gctx draft approve drf_xxx --by alice --note "LGTM"`
5. `gctx draft apply drf_xxx` → succeeds, writes commit
6. `gctx context --depth 5` → includes new commit + approval evidence

**Verification**: ledger has approval + commit events, draft-to-commit evidence is preserved, hash chain is continuous before and after apply.

---

## Quality Benchmarks

| CONTRACT Rule | Metric | Baseline | Verification |
|--------------|--------|----------|-------------|
| LEDGER-01 | rebuild restores all views | 100% views correct | `rm -rf .gctx/branches && gctx rebuild --all && diff` |
| HASH-01 | hash chain integrity | 100% continuous | Line-by-line verification script |
| VIEW-01 | deterministic rebuild | diff = 0 | Two rebuilds, diff output |
| RUST-01 | clippy warning count | 0 | `cargo clippy -- -D warnings` |

---

> **Pattern notes for generation:**
> - **Track Acceptance Criteria**: one table per track, columns are Item / Pass Criteria / Verification
> - **Golden Path Scenarios**: end-to-end user stories that prove multiple tracks work together
>   - Each GP has: Description (one sentence), Steps (numbered bash commands), Verification (what to check)
>   - GP scenarios are ordered from simplest (minimum loop) to most complex (full system)
>   - Each GP references which tracks it validates
> - **Quality Benchmarks**: maps CONTRACT rules to measurable metrics with baseline values
> - Golden paths are NOT unit tests — they are integration scenarios that a human or agent can run manually
> - Every verification is a concrete command or observable result, not "verify it works"
