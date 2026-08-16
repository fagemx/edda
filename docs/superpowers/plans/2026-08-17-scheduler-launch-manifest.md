# Scheduler Launch Manifest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace GH-466's overlong Windows Task Scheduler `/TR` command with a validated, immutable, machine-local launch manifest while preserving the exact-task and direct-execution safety model.

**Architecture:** Keep all production work in `cmd_reconcile.rs`. Install serializes one versioned launch payload, addresses it by SHA-256 under the existing Edda user store, and schedules `edda reconcile --scheduler-manifest <absolute-path>`. Scheduled re-entry validates the artifact before ledger access; lifecycle cleanup is exact-task-only and retains artifacts whenever ownership is uncertain.

**Tech Stack:** Rust, Clap, Serde/serde_json, sha2/hex, existing `edda_store::{store_root, lock_file, write_atomic}`, `std::process::Command`, Windows `schtasks.exe`.

## Global Constraints

- Follow `docs/superpowers/specs/2026-08-17-scheduler-launch-manifest-design.md` and ratified decision `gh466.scheduler-launch-config`.
- Modify product code only in `crates/edda-cli/src/cmd_reconcile.rs`; no Cargo, crate, trait, daemon, shell, XML-registration, principal, or dependency change.
- `/TR` stays direct native execution and must be at most 261 UTF-16 code units before any filesystem or scheduler mutation.
- Use one exact `Edda-Reconcile-<32 lowercase hex>` task; never enumerate, prefix-match, wildcard-delete, pre-delete, or kill processes.
- Manifest validation fails before ledger access or Codex launch. Unknown or uncertain artifacts are retained.
- No scheduler/PID/drill action occurs during implementation. A fresh immutable-head review must pass before Task #50 resumes.

---

## File map

- Modify and test: `crates/edda-cli/src/cmd_reconcile.rs` — CLI mode, manifest schema/canonical bytes, trust validation, renderer, lifecycle, and focused tests.
- Read only: `crates/edda-store/src/lib.rs` — reuse `store_root`, `lock_file`, and `write_atomic`; do not change its API.
- Read only until the later drill receipt: `docs/plan/task-rail/P2_DRILL_2026-08-16.md`.

### Task 1: Canonical manifest model and trust boundary

**Files:**
- Modify/Test: `crates/edda-cli/src/cmd_reconcile.rs`

**Interfaces:**
- Produces `SchedulerLaunchManifestV1`, `PreparedSchedulerManifest`, `prepare_scheduler_manifest`, and `load_scheduler_manifest` for later tasks.
- Reuses `canonical_main_repo`, `canonical_direct_codex_executable`, `edda_store::project_id_for_root`, `sha2`, and `hex`.

- [ ] **Step 1: Add failing canonicalization and rejection tests**

Add focused tests covering deterministic compact bytes/digest/path, changed config changing digest, unknown/duplicate/noncanonical JSON, oversize input, digest mismatch, wrong project id, invalid repo/Codex, and store-root/reparse escape. Use `tempfile` only from the existing dev-dependency and serialize environment mutation under the existing test lock pattern.

```rust
#[test]
fn scheduler_manifest_is_canonical_content_addressed_and_strict() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let first = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let second = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.path, second.path);
    assert!(first.path.ends_with(format!("{}.json", first.digest)));
    edda_store::write_atomic(&first.path, &first.bytes)?;
    assert_eq!(load_scheduler_manifest(&first.path)?.manifest, first.manifest);
    Ok(())
}
```

- [ ] **Step 2: Run RED**

Run:

```text
cargo test -p edda scheduler_manifest_ --no-fail-fast -- --test-threads=1
```

Expected: compile failure for the missing manifest types/functions.

- [ ] **Step 3: Implement the minimal typed schema and canonical bytes**

Define these private types in `cmd_reconcile.rs`; do not add a generic config module:

```rust
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SchedulerLaunchManifestV1 {
    schema_version: u8,
    project_id: String,
    repo: PathBuf,
    codex_bin: PathBuf,
    max_workers: usize,
    max_attempts: u32,
    lease_ttl_s: u64,
}

struct PreparedSchedulerManifest {
    manifest: SchedulerLaunchManifestV1,
    bytes: Vec<u8>,
    digest: String,
    path: PathBuf,
}

struct LoadedSchedulerManifest {
    manifest: SchedulerLaunchManifestV1,
    repo: PathBuf,
    config: ReconcileConfig,
}
```

Use declaration order plus `serde_json::to_vec` for the exact compact byte form. Hash those bytes with `Sha256`, require a 64-lowercase-hex filename, cap reads at 16 KiB using metadata before `std::fs::read`, reserialize and compare exact bytes, and validate canonical repo/project/Codex through existing helpers. Resolve the prospective store root to an absolute path in memory; after the directory exists, canonicalize it and reject any parent mismatch or reparse escape.

- [ ] **Step 4: Run GREEN and commit**

Run the focused command from Step 2; expected all `scheduler_manifest_` tests PASS. Then:

```text
git add crates/edda-cli/src/cmd_reconcile.rs
git commit -m "feat(task-rail): validate scheduler launch manifests"
```

### Task 2: Compact hidden re-entry and UTF-16 renderer

**Files:**
- Modify/Test: `crates/edda-cli/src/cmd_reconcile.rs`

**Interfaces:**
- Consumes `LoadedSchedulerManifest` from Task 1.
- Changes `windows_scheduler_spec(exe, manifest_path, project_id)` to render the compact direct command.

- [ ] **Step 1: Add failing CLI, renderer, and boundary tests**

Cover hidden `--scheduler-manifest`, conflicts with lifecycle/repo/Codex/run-task/attempt/explicit limits, unrelated-cwd re-entry, the preserved real 356-unit fixture now fitting, exact 261 acceptance, exact 262 rejection, and a surrogate-pair case proving UTF-16 rather than byte/character counting.

```rust
#[test]
fn scheduler_manifest_renderer_enforces_utf16_limit() -> anyhow::Result<()> {
    let accepted = render_scheduler_task_run(&path_with_utf16_len(261))?;
    assert_eq!(accepted.encode_utf16().count(), 261);
    let error = render_scheduler_task_run(&path_with_utf16_len(262))
        .expect_err("262 UTF-16 units must fail")
        .to_string();
    assert!(error.contains("262"));
    assert!(error.contains("261"));
    Ok(())
}
```

- [ ] **Step 2: Run RED**

Run:

```text
cargo test -p edda scheduler_ --no-fail-fast -- --test-threads=1
```

Expected: missing CLI field/renderer and old full-config `/TR` assertions fail.

- [ ] **Step 3: Implement the compact mode without a second reconcile path**

Add only one hidden option:

```rust
#[arg(long, hide = true)]
scheduler_manifest: Option<PathBuf>,
```

Declare Clap conflicts listed in the spec. In `run`, load and validate the manifest first, select its repo and `ReconcileConfig`, then continue through the existing ordinary one-shot persistence/launch path. Do not duplicate the planner or runner code.

Render exactly:

```rust
let task_run = format!(
    "{} reconcile --scheduler-manifest {}",
    quote_windows_argument(exe)?,
    quote_windows_argument(manifest_path)?,
);
let units = task_run.encode_utf16().count();
anyhow::ensure!(
    units <= 261,
    "scheduler task {task_name} /TR is {units} UTF-16 code units; maximum is 261"
);
```

Keep the exact ordered Create/Query/Delete scheduler vectors and `LIMITED` run level unchanged.

- [ ] **Step 4: Run GREEN and commit**

Run the focused scheduler suite; expected all scheduler tests PASS. Then:

```text
git add crates/edda-cli/src/cmd_reconcile.rs
git commit -m "fix(task-rail): shorten scheduler reentry"
```

### Task 3: Atomic install/reinstall and exact verification

**Files:**
- Modify/Test: `crates/edda-cli/src/cmd_reconcile.rs`

**Interfaces:**
- Consumes `PreparedSchedulerManifest` and the compact `WindowsSchedulerSpec`.
- Produces small pure helpers for exact Query XML verification and cleanup decisions; no scheduler trait or injectable production abstraction.

- [ ] **Step 1: Add failing lifecycle/fault tests**

Test identical reinstall reuse, changed config producing a new immutable path, existing-file exact-byte verification, no-replace behavior, write/Create/post-Create-Query failure decisions, exact Query XML matching Command+Arguments, and no mutation marker when validation/preflight fails.

```rust
#[test]
fn scheduler_install_failure_never_overwrites_prior_manifest() -> anyhow::Result<()> {
    let old = prepared_manifest_with_attempts(3)?;
    let new = prepared_manifest_with_attempts(5)?;
    assert_ne!(old.path, new.path);
    assert_eq!(
        manifest_cleanup_decision(&missing_scheduler_output(), &scheduler_exe(), &new.path)?,
        ManifestCleanupDecision::RemoveNewArtifact
    );
    assert_eq!(std::fs::read(&old.path)?, old.bytes);
    Ok(())
}
```

- [ ] **Step 2: Run RED**

Run:

```text
cargo test -p edda scheduler_ --no-fail-fast -- --test-threads=1
```

Expected: missing publish/XML/cleanup helpers.

- [ ] **Step 3: Implement the minimum lifecycle sequencing**

Before mutation, prepare and validate manifest bytes/path/spec and finish UTF-16 preflight. Then acquire `store_root/scheduler-launch/manifest.lock`, create `v1`, and either validate an identical existing artifact or call existing `edda_store::write_atomic` once. Record whether this attempt created the file.

Create the exact task with `/F`, query the exact task with `/XML /HRESULT`, and require bounded XML to contain the exact XML-escaped `<Command>` and `<Arguments>` values. On failure, query only the exact task and remove only a newly created manifest whose non-reference is proved; uncertainty retains it and is included in the error.

Use two private pure helpers rather than an abstraction:

```rust
enum ManifestCleanupDecision { RemoveNewArtifact, Retain }

fn scheduler_query_references_manifest(
    xml: &str,
    executable: &Path,
    manifest: &Path,
) -> anyhow::Result<bool>;

fn manifest_cleanup_decision(
    query: &SchedulerOutput,
    executable: &Path,
    expected_manifest: &Path,
) -> anyhow::Result<ManifestCleanupDecision>;
```

- [ ] **Step 4: Run GREEN and commit**

Run scheduler tests and `cargo test -p edda cmd_reconcile --no-fail-fast -- --test-threads=1`; expected PASS. Then:

```text
git add crates/edda-cli/src/cmd_reconcile.rs
git commit -m "fix(task-rail): persist scheduler launch manifests"
```

### Task 4: Conservative uninstall and full verification

**Files:**
- Modify/Test: `crates/edda-cli/src/cmd_reconcile.rs`

**Interfaces:**
- Consumes strict Query XML recognition from Task 3.
- Leaves Task #50 and `P2_DRILL_2026-08-16.md` untouched until review approval.

- [ ] **Step 1: Add failing uninstall tests**

Cover present exact task with trusted manifest, malformed/untrusted XML retaining artifacts while still removing the exact task, already-missing task doing no sweep, missing Codex not blocking uninstall, delete race using only the accepted missing HRESULT, and post-delete uncertainty retaining artifacts.

```rust
#[test]
fn scheduler_uninstall_never_sweeps_unproven_artifacts() -> anyhow::Result<()> {
    let unrelated = scheduler_manifest_fixture_for_other_project()?;
    let candidate = recover_manifest_candidate("<malformed>", &expected_task())?;
    assert!(candidate.is_none());
    assert!(unrelated.path.exists());
    Ok(())
}
```

- [ ] **Step 2: Run RED, implement, and run GREEN**

Run the focused scheduler suite; expected new tests fail. Implement exact-task Query/Delete/post-Query first. Recover one candidate only from the strict direct-command shape; after absence is proved, delete it under the store lock only after full path/schema/digest/project validation. Never enumerate or sweep the manifest directory. Rerun scheduler and serial `cmd_reconcile`; expected PASS.

- [ ] **Step 3: Run all required gates**

Run separately with a fresh isolated TEMP/target where appropriate:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p edda scheduler_ --no-fail-fast -- --test-threads=1
cargo test -p edda cmd_reconcile --no-fail-fast -- --test-threads=1
cargo test --workspace --no-fail-fast -- --test-threads=1
git diff --check 8d0f71a016d5a0d4122adc59907afa8f6c768388..HEAD
```

Expected: all commands PASS; disclose any first-run flake and require an exact isolated rerun instead of hiding it.

- [ ] **Step 4: Commit, push, and request immutable review**

```text
git add crates/edda-cli/src/cmd_reconcile.rs
git commit -m "fix(task-rail): clean exact scheduler manifests"
git push origin codex/gh-466-scheduler
```

Post `Review Response: Launch Manifest RED` containing the real 356-unit root cause, commit list, RED/GREEN evidence, exact full SHA, and gates. Post `Code Review Handoff: Round 6`; do not self-author a verdict. Freeze the branch.

### Task 5: Independent current-head acceptance and drill restart

**Files:**
- Review only: complete `38c31bad15c4df55cfcc04b0f630bc3dddbb3e58..HEAD` range.
- Later drill-only update: `docs/plan/task-rail/P2_DRILL_2026-08-16.md`.

**Interfaces:**
- Verifier publishes PR-visible `Code Review: Round 6` with P0/P1 counts and exact-head CI.
- Task #50 resumes only after P0=0/P1=0 LGTM.

- [ ] **Step 1: Run immutable independent review**

Verifier checks the full range, ratified decision/spec/plan, amended S2/S4/S5/S14, manifest trust/fault matrix, all local gates, and exact-head GitHub Actions. Any push invalidates the verdict.

- [ ] **Step 2: Restart Task #50 only after LGTM**

Create a new preserved run directory. Build/hash the reviewed binary, capture raw argv/cwd/UTC/exit/stdout/stderr/XML/PID evidence, prove the missing-HRESULT lifecycle and the prior 356-unit fixture from scratch, then run D1 and serial D2-D8. Any product RED stops drills and creates a separately scoped fix/review round. No merge without explicit operator authority.

## Plan self-review

- Spec coverage: schema, digest/path trust, UTF-16 boundary, compact re-entry, install/reinstall/failure/uninstall, amended matrix, gates, immutable review, and drill restart are each mapped above.
- Placeholder scan: every step contains concrete commands, expected outcomes, and named interfaces.
- Type consistency: Task 2 consumes Task 1's loaded manifest; Tasks 3-4 share the exact Query XML and cleanup helpers; Task 5 consumes the frozen implementation SHA only.
