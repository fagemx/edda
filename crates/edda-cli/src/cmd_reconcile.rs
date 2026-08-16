use anyhow::Context;
use clap::Args;
use edda_core::event::{
    new_task_failed_event, new_task_host_session_event, new_task_requeued_event,
    new_task_started_event,
};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static DOORBELL_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static DOORBELL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(all(test, windows))]
static FAKE_CODEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
thread_local! {
    static FAIL_NEXT_STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_NEXT_LEASE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_TASK_ID: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[derive(Args)]
pub struct ReconcileArgs {
    #[arg(long, default_value_t = 3)]
    max_workers: usize,
    #[arg(long, default_value_t = 3)]
    max_attempts: u32,
    #[arg(long, default_value_t = 300)]
    lease_ttl_s: u64,
    #[arg(long)]
    codex_bin: Option<PathBuf>,
    #[arg(
        long,
        conflicts_with_all = ["uninstall_scheduler", "run_task", "attempt"]
    )]
    install_scheduler: bool,
    #[arg(
        long,
        conflicts_with_all = ["install_scheduler", "run_task", "attempt"]
    )]
    uninstall_scheduler: bool,
    #[arg(long, hide = true)]
    repo: Option<PathBuf>,
    #[arg(long, hide = true)]
    run_task: Option<u64>,
    #[arg(long, hide = true)]
    attempt: Option<u32>,
    #[arg(
        long,
        hide = true,
        conflicts_with_all = [
            "install_scheduler",
            "uninstall_scheduler",
            "repo",
            "codex_bin",
            "run_task",
            "attempt",
            "max_workers",
            "max_attempts",
            "lease_ttl_s"
        ]
    )]
    scheduler_manifest: Option<PathBuf>,
}

#[derive(Clone)]
struct ReconcileConfig {
    max_workers: usize,
    max_attempts: u32,
    lease_ttl_s: u64,
    codex_bin: PathBuf,
}

impl ReconcileConfig {
    fn defaults() -> Self {
        Self {
            max_workers: 3,
            max_attempts: 3,
            lease_ttl_s: 300,
            codex_bin: PathBuf::from("codex"),
        }
    }

    fn from_args(args: &ReconcileArgs) -> Self {
        let defaults = Self::defaults();
        Self {
            max_workers: args.max_workers,
            max_attempts: args.max_attempts,
            lease_ttl_s: args.lease_ttl_s,
            codex_bin: args
                .codex_bin
                .clone()
                .or_else(|| std::env::var_os("EDDA_CODEX_BIN").map(PathBuf::from))
                .unwrap_or(defaults.codex_bin),
        }
    }
    #[cfg(test)]
    fn test_defaults() -> Self {
        Self::defaults()
    }
}

const SCHEDULER_MANIFEST_MAX_BYTES: u64 = 16 * 1024;

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
    // Task 3 consumes these fields when it publishes immutable manifests.
    #[allow(dead_code)]
    manifest: SchedulerLaunchManifestV1,
    #[allow(dead_code)]
    bytes: Vec<u8>,
    #[allow(dead_code)]
    digest: String,
    path: PathBuf,
}

struct LoadedSchedulerManifest {
    // Task 3 consumes the payload when verifying existing artifacts.
    #[allow(dead_code)]
    manifest: SchedulerLaunchManifestV1,
    repo: PathBuf,
    config: ReconcileConfig,
}

fn scheduler_manifest_directory(store: &Path, must_exist: bool) -> anyhow::Result<PathBuf> {
    let store = std::path::absolute(store)
        .with_context(|| format!("resolve Edda store root {}", store.display()))?;
    let value = store.to_str().context("Edda store root is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "Edda store root contains an unsupported character"
    );
    let store = if store.exists() {
        anyhow::ensure!(store.is_dir(), "Edda store root must be a directory");
        store
            .canonicalize()
            .with_context(|| format!("canonicalize Edda store root {}", store.display()))?
    } else {
        anyhow::ensure!(!must_exist, "Edda store root does not exist");
        store
    };
    let launch = store.join("scheduler-launch");
    if launch.exists() {
        anyhow::ensure!(
            launch.canonicalize()? == launch,
            "scheduler manifest directory escapes the Edda store root"
        );
    }
    let directory = launch.join("v1");
    if directory.exists() {
        anyhow::ensure!(
            directory.canonicalize()? == directory,
            "scheduler manifest directory escapes the Edda store root"
        );
    } else {
        anyhow::ensure!(!must_exist, "scheduler manifest directory does not exist");
    }
    Ok(directory)
}

fn scheduler_manifest_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_scheduler_manifest(
    manifest: SchedulerLaunchManifestV1,
) -> anyhow::Result<LoadedSchedulerManifest> {
    anyhow::ensure!(
        manifest.schema_version == 1,
        "unknown scheduler manifest version"
    );
    let repo = canonical_main_repo(&manifest.repo)?;
    anyhow::ensure!(
        repo == manifest.repo,
        "scheduler manifest repository is not canonical"
    );
    anyhow::ensure!(
        manifest.project_id == edda_store::project_id_for_root(&repo),
        "scheduler manifest project id does not match repository"
    );
    let codex_bin = canonical_direct_codex_executable(&manifest.codex_bin, None)?;
    anyhow::ensure!(
        codex_bin == manifest.codex_bin,
        "scheduler manifest Codex executable is not canonical"
    );
    let config = ReconcileConfig {
        max_workers: manifest.max_workers,
        max_attempts: manifest.max_attempts,
        lease_ttl_s: manifest.lease_ttl_s,
        codex_bin,
    };
    Ok(LoadedSchedulerManifest {
        manifest,
        repo,
        config,
    })
}

fn prepare_scheduler_manifest(
    store: &Path,
    repo: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<PreparedSchedulerManifest> {
    let repo = canonical_main_repo(repo)?;
    let codex_bin = canonical_direct_codex_executable(&config.codex_bin, None)?;
    let manifest = SchedulerLaunchManifestV1 {
        schema_version: 1,
        project_id: edda_store::project_id_for_root(&repo),
        repo,
        codex_bin,
        max_workers: config.max_workers,
        max_attempts: config.max_attempts,
        lease_ttl_s: config.lease_ttl_s,
    };
    let bytes = serde_json::to_vec(&manifest)?;
    let digest = scheduler_manifest_digest(&bytes);
    let path = scheduler_manifest_directory(store, false)?.join(format!("{digest}.json"));
    Ok(PreparedSchedulerManifest {
        manifest,
        bytes,
        digest,
        path,
    })
}

fn load_scheduler_manifest(path: &Path) -> anyhow::Result<LoadedSchedulerManifest> {
    anyhow::ensure!(
        path.is_absolute(),
        "scheduler manifest path must be absolute"
    );
    let value = path
        .to_str()
        .context("scheduler manifest path is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "scheduler manifest path contains an unsupported character"
    );
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("scheduler manifest filename is not Unicode")?;
    let expected_digest = filename
        .strip_suffix(".json")
        .context("scheduler manifest filename must end in .json")?;
    anyhow::ensure!(
        expected_digest.len() == 64
            && expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "scheduler manifest filename must contain a 64-character lowercase SHA-256 digest"
    );

    let trusted_directory = scheduler_manifest_directory(&edda_store::store_root(), true)?;
    let source_metadata = path
        .symlink_metadata()
        .with_context(|| format!("inspect scheduler manifest {}", path.display()))?;
    anyhow::ensure!(
        source_metadata.file_type().is_file(),
        "scheduler manifest must be a regular file"
    );
    anyhow::ensure!(
        source_metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize scheduler manifest {}", path.display()))?;
    let parent = path
        .parent()
        .context("scheduler manifest has no parent")?
        .canonicalize()
        .context("canonicalize scheduler manifest parent")?;
    anyhow::ensure!(
        parent == trusted_directory && canonical.parent() == Some(trusted_directory.as_path()),
        "scheduler manifest is outside the trusted Edda store directory"
    );
    let metadata = canonical.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "scheduler manifest must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let bytes = std::fs::read(&canonical)
        .with_context(|| format!("read scheduler manifest {}", canonical.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let manifest: SchedulerLaunchManifestV1 =
        serde_json::from_slice(&bytes).context("parse scheduler launch manifest")?;
    anyhow::ensure!(
        serde_json::to_vec(&manifest)? == bytes,
        "scheduler manifest JSON is not canonical"
    );
    anyhow::ensure!(
        scheduler_manifest_digest(&bytes) == expected_digest,
        "scheduler manifest digest does not match filename"
    );
    validate_scheduler_manifest(manifest)
}

#[derive(Clone)]
struct RunnerPlan {
    task: TaskView,
    attempt: u32,
    worktree: PathBuf,
}

struct PersistOutcome {
    plans: Vec<RunnerPlan>,
    errors: Vec<String>,
}

pub fn run(repo_root: &Path, args: ReconcileArgs) -> anyhow::Result<()> {
    let scheduler_manifest = args
        .scheduler_manifest
        .as_deref()
        .map(load_scheduler_manifest)
        .transpose()?;
    let repo_root = match scheduler_manifest.as_ref() {
        Some(loaded) => loaded.repo.clone(),
        None => match args.repo.as_deref() {
            Some(repo) => canonical_main_repo(repo)?,
            None => repo_root.to_path_buf(),
        },
    };
    if args.install_scheduler {
        let config = ReconcileConfig::from_args(&args);
        return scheduler_lifecycle(&repo_root, Some(&config));
    }
    if args.uninstall_scheduler {
        return scheduler_lifecycle(&repo_root, None);
    }
    let config = scheduler_manifest
        .map(|loaded| loaded.config)
        .unwrap_or_else(|| ReconcileConfig::from_args(&args));
    if let Some(task_id) = args.run_task {
        let attempt = args
            .attempt
            .context("--run-task requires hidden --attempt")?;
        return run_task(&repo_root, task_id, attempt, &config, true);
    }
    if args.attempt.is_some() {
        anyhow::bail!("--attempt is valid only with hidden --run-task");
    }
    let persisted = persist_reconciliation(&repo_root, &config)?;
    let plans = persisted.plans;
    let executable = std::env::current_exe()?;
    let executables = vec![executable; plans.len()];
    let (launched, mut errors) = launch_plans_with(&repo_root, plans, &config, &executables);
    errors.extend(persisted.errors);
    for plan in launched {
        notify_started(&repo_root, &plan.task);
        println!(
            "dispatched task #{} attempt {} in {}",
            plan.task.task_id,
            plan.attempt,
            plan.worktree.display()
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(errors.join("\n"));
    }
}

#[cfg(any(windows, test))]
const MISSING_TASK_HRESULT: u32 = 0x8007_0002;
#[cfg(windows)]
const SCHEDULER_OUTPUT_LIMIT: usize = 4096;

#[cfg(any(windows, test))]
#[derive(Debug, Eq, PartialEq)]
enum SchedulerTaskState {
    Present,
    Missing,
}

#[cfg(any(windows, test))]
struct SchedulerOutput {
    code: u32,
    stdout: String,
    stderr: String,
}

#[cfg(any(windows, test))]
impl SchedulerOutput {
    #[cfg(test)]
    fn for_test(code: u32, stdout: &str, stderr: &str) -> Self {
        Self {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn description(&self) -> String {
        format!(
            "code=0x{:08x} ({}) stdout={:?} stderr={:?}",
            self.code, self.code as i32, self.stdout, self.stderr
        )
    }
}

#[cfg(any(windows, test))]
struct WindowsSchedulerSpec {
    task_name: String,
    create_args: Vec<String>,
    query_args: Vec<String>,
}

fn canonical_main_repo(repo: &Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(repo.is_absolute(), "--repo must be absolute");
    let repo = repo
        .canonicalize()
        .with_context(|| format!("canonicalize repository {}", repo.display()))?;
    anyhow::ensure!(repo.is_dir(), "--repo must name a directory");
    edda_ledger::EddaPaths::find_root(&repo)
        .context("--repo must name an initialized Edda workspace")?
        .canonicalize()
        .context("canonicalize Edda workspace root")
}

#[cfg(any(windows, test))]
fn quote_windows_argument(path: &Path) -> anyhow::Result<String> {
    let value = path.to_str().context("scheduler path is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "scheduler path contains an unsupported character"
    );
    let trailing_backslashes = value
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    Ok(format!("\"{value}{}\"", "\\".repeat(trailing_backslashes)))
}

#[cfg(any(windows, test))]
fn windows_path_is_absolute(path: &Path) -> anyhow::Result<bool> {
    let value = path.to_str().context("scheduler path is not Unicode")?;
    let bytes = value.as_bytes();
    let separator = |byte| matches!(byte, b'\\' | b'/');
    let drive_rooted = |candidate: &[u8]| {
        candidate.len() >= 3
            && candidate[0].is_ascii_alphabetic()
            && candidate[1] == b':'
            && separator(candidate[2])
    };
    let unc_rooted = |candidate: &str| {
        let mut parts = candidate.split(['\\', '/']);
        parts.next().is_some_and(|part| !part.is_empty())
            && parts.next().is_some_and(|part| !part.is_empty())
    };

    if drive_rooted(bytes) {
        return Ok(true);
    }
    if bytes.len() < 2 || !separator(bytes[0]) || !separator(bytes[1]) {
        return Ok(false);
    }
    if bytes.len() >= 4 && bytes[2] == b'?' && separator(bytes[3]) {
        let rest = &value[4..];
        if drive_rooted(rest.as_bytes()) {
            return Ok(true);
        }
        let rest_bytes = rest.as_bytes();
        return Ok(rest_bytes.len() >= 4
            && rest_bytes[..3].eq_ignore_ascii_case(b"UNC")
            && separator(rest_bytes[3])
            && unc_rooted(&rest[4..]));
    }
    Ok(unc_rooted(&value[2..]))
}

fn validate_canonical_direct_codex_target(canonical: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        canonical.is_absolute()
            && canonical.is_file()
            && canonical
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")),
        "canonical Codex executable {} must be an absolute native .exe file",
        canonical.display()
    );
    Ok(())
}

fn canonical_direct_codex_executable(
    command: &Path,
    search_path: Option<&std::ffi::OsStr>,
) -> anyhow::Result<PathBuf> {
    let has_parent = command
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    let path = search_path
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let candidates = if command.is_absolute() || has_parent {
        vec![command.to_path_buf()]
    } else {
        path.as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .map(|directory| directory.join(command))
            .collect()
    };

    for candidate in candidates {
        let candidate = match candidate
            .extension()
            .and_then(|extension| extension.to_str())
        {
            None => candidate.with_extension("exe"),
            Some(extension) if extension.eq_ignore_ascii_case("exe") => candidate,
            Some(_) => continue,
        };
        if !candidate.is_file() {
            continue;
        }
        let canonical = candidate
            .canonicalize()
            .with_context(|| format!("canonicalize Codex executable {}", candidate.display()))?;
        validate_canonical_direct_codex_target(&canonical)?;
        return Ok(canonical);
    }
    anyhow::bail!(
        "Codex executable {} must resolve to an absolute native .exe file",
        command.display()
    )
}

#[cfg(any(windows, test))]
fn windows_scheduler_task_name(project_id: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        project_id.len() == 32
            && project_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "project id must be 32 lowercase hexadecimal characters"
    );
    Ok(format!("Edda-Reconcile-{project_id}"))
}

#[cfg(any(windows, test))]
fn windows_scheduler_management_args(
    project_id: &str,
) -> anyhow::Result<(String, Vec<String>, Vec<String>)> {
    let task_name = windows_scheduler_task_name(project_id)?;
    let strings = |items: &[&str]| items.iter().map(|item| (*item).into()).collect();
    let query_args = strings(&["/Query", "/TN", &task_name, "/XML", "/HRESULT"]);
    let delete_args = strings(&["/Delete", "/TN", &task_name, "/F", "/HRESULT"]);
    Ok((task_name, query_args, delete_args))
}

#[cfg(any(windows, test))]
fn render_scheduler_task_run(
    exe: &Path,
    manifest_path: &Path,
    task_name: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        windows_path_is_absolute(exe)?,
        "scheduler executable must be absolute"
    );
    anyhow::ensure!(
        windows_path_is_absolute(manifest_path)?,
        "scheduler manifest path must be absolute"
    );
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
    Ok(task_run)
}

#[cfg(any(windows, test))]
fn windows_scheduler_spec(
    exe: &Path,
    manifest_path: &Path,
    project_id: &str,
) -> anyhow::Result<WindowsSchedulerSpec> {
    let (task_name, query_args, _) = windows_scheduler_management_args(project_id)?;
    let task_run = render_scheduler_task_run(exe, manifest_path, &task_name)?;
    let strings = |items: &[&str]| items.iter().map(|item| (*item).into()).collect();
    Ok(WindowsSchedulerSpec {
        create_args: strings(&[
            "/Create", "/SC", "MINUTE", "/MO", "1", "/TN", &task_name, "/TR", &task_run, "/RL",
            "LIMITED", "/F", "/HRESULT",
        ]),
        query_args,
        task_name,
    })
}

#[cfg(any(windows, test))]
fn classify_scheduler_query(output: &SchedulerOutput) -> anyhow::Result<SchedulerTaskState> {
    match output.code {
        0 => Ok(SchedulerTaskState::Present),
        MISSING_TASK_HRESULT => Ok(SchedulerTaskState::Missing),
        _ => anyhow::bail!("scheduler query failed: {}", output.description()),
    }
}

#[cfg(any(windows, test))]
fn require_scheduler_state(
    output: &SchedulerOutput,
    expected: SchedulerTaskState,
    operation: &str,
    task_name: &str,
) -> anyhow::Result<()> {
    let actual = classify_scheduler_query(output)
        .with_context(|| format!("scheduler {operation} failed for task {task_name}"))?;
    anyhow::ensure!(
        actual == expected,
        "scheduler {operation} expected {expected:?} for task {task_name}, got {actual:?}: {}",
        output.description()
    );
    Ok(())
}

#[cfg(windows)]
fn run_schtasks(args: &[String]) -> anyhow::Result<SchedulerOutput> {
    let output = Command::new("schtasks.exe")
        .args(args)
        .output()
        .context("launch schtasks.exe")?;
    let signed_code = output
        .status
        .code()
        .context("schtasks.exe terminated by signal")?;
    let bounded = |bytes: &[u8]| {
        String::from_utf8_lossy(&bytes[..bytes.len().min(SCHEDULER_OUTPUT_LIMIT)]).into_owned()
    };
    Ok(SchedulerOutput {
        code: signed_code as u32,
        stdout: bounded(&output.stdout),
        stderr: bounded(&output.stderr),
    })
}

fn scheduler_lifecycle(
    repo: &Path,
    install_config: Option<&ReconcileConfig>,
) -> anyhow::Result<()> {
    #[cfg(not(windows))]
    {
        let _ = (repo, install_config);
        anyhow::bail!("Windows Task Scheduler is supported only on Windows")
    }
    #[cfg(windows)]
    {
        let repo = canonical_main_repo(repo)?;
        let project_id = edda_store::project_id_for_root(&repo);
        let (task_name, query_args, delete_args) = windows_scheduler_management_args(&project_id)?;
        if let Some(config) = install_config {
            let executable = std::env::current_exe()?.canonicalize()?;
            let mut config = config.clone();
            config.codex_bin = canonical_direct_codex_executable(&config.codex_bin, None)?;
            let manifest = prepare_scheduler_manifest(&edda_store::store_root(), &repo, &config)?;
            let spec = windows_scheduler_spec(&executable, &manifest.path, &project_id)?;
            let created = run_schtasks(&spec.create_args)
                .with_context(|| format!("scheduler Create failed for task {}", spec.task_name))?;
            anyhow::ensure!(
                created.code == 0,
                "scheduler Create failed for {}: {}",
                spec.task_name,
                created.description()
            );
            let queried = run_schtasks(&spec.query_args)
                .with_context(|| format!("scheduler Query failed for task {}", spec.task_name))?;
            require_scheduler_state(
                &queried,
                SchedulerTaskState::Present,
                "post-Create Query",
                &spec.task_name,
            )?;
            println!(
                "installed scheduler task {} for {}",
                spec.task_name,
                repo.display()
            );
            return Ok(());
        }

        let before = run_schtasks(&query_args)
            .with_context(|| format!("scheduler Query failed for task {task_name}"))?;
        if classify_scheduler_query(&before)
            .with_context(|| format!("scheduler Query failed for task {task_name}"))?
            == SchedulerTaskState::Missing
        {
            println!(
                "scheduler task {} already absent for {}",
                task_name,
                repo.display()
            );
            return Ok(());
        }
        let deleted = run_schtasks(&delete_args)
            .with_context(|| format!("scheduler Delete failed for task {task_name}"))?;
        anyhow::ensure!(
            deleted.code == 0 || deleted.code == MISSING_TASK_HRESULT,
            "scheduler Delete failed for {}: {}",
            task_name,
            deleted.description()
        );
        let after = run_schtasks(&query_args)
            .with_context(|| format!("scheduler Query failed for task {task_name}"))?;
        require_scheduler_state(
            &after,
            SchedulerTaskState::Missing,
            "post-Delete Query",
            &task_name,
        )?;
        println!(
            "uninstalled scheduler task {} for {}",
            task_name,
            repo.display()
        );
        Ok(())
    }
}

fn launch_plans_with(
    repo_root: &Path,
    plans: Vec<RunnerPlan>,
    config: &ReconcileConfig,
    executables: &[PathBuf],
) -> (Vec<RunnerPlan>, Vec<String>) {
    let mut launched = Vec::new();
    let mut errors = Vec::new();
    for (plan, executable) in plans.into_iter().zip(executables) {
        if let Err(error) = launch_runner_with(
            executable,
            repo_root,
            plan.task.task_id,
            plan.attempt,
            config,
        ) {
            let reason = format!("runner-spawn-failed: {error:#}");
            if let Err(cleanup) = finish_runner(
                repo_root,
                plan.task.task_id,
                plan.attempt,
                Some(&reason),
                true,
                config,
            ) {
                errors.push(format!(
                    "task #{} spawn: {error:#}; cleanup: {cleanup:#}",
                    plan.task.task_id
                ));
            } else {
                errors.push(format!("task #{} spawn: {error:#}", plan.task.task_id));
            }
            continue;
        }
        launched.push(plan);
    }
    (launched, errors)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReconcileAction {
    Start {
        task_id: u64,
        attempt: u32,
    },
    Resume {
        task_id: u64,
        attempt: u32,
        session_id: String,
    },
    Requeue {
        task_id: u64,
        next_attempt: u32,
        reason: String,
    },
    Fail {
        task_id: u64,
        reason: String,
    },
}

fn plan_actions(
    views: &[TaskView],
    leases: &[TaskLease],
    live_claims: &[Vec<String>],
    now: &str,
    max_workers: usize,
    max_attempts: u32,
) -> Vec<ReconcileAction> {
    let mut ordered: Vec<&TaskView> = views.iter().collect();
    ordered.sort_by_key(|view| view.task_id);
    let lease_for = |task_id| leases.iter().find(|lease| lease.task_id == task_id);
    let is_live = |view: &TaskView| {
        lease_for(view.task_id)
            .is_some_and(|lease| lease.attempt == view.attempts && lease.expires_at.as_str() > now)
    };
    let mut occupied: Vec<Vec<String>> = live_claims
        .iter()
        .filter(|paths| !paths.is_empty())
        .cloned()
        .collect();
    occupied.extend(
        ordered
            .iter()
            .filter(|view| view.status == TaskStatus::Running && is_live(view))
            .map(|view| occupied_scope(&view.scope_paths)),
    );
    let mut slots = max_workers.saturating_sub(
        ordered
            .iter()
            .filter(|view| view.status == TaskStatus::Running && is_live(view))
            .count(),
    );
    let mut actions = Vec::new();

    for view in ordered {
        match view.status {
            TaskStatus::Running if is_live(view) => continue,
            TaskStatus::Running => {
                if view.attempts >= max_attempts {
                    actions.push(ReconcileAction::Fail {
                        task_id: view.task_id,
                        reason: "retry-cap-exhausted".into(),
                    });
                } else if slots > 0 && !conflicts(&view.scope_paths, &occupied) {
                    slots -= 1;
                    occupied.push(occupied_scope(&view.scope_paths));
                    let resume = (view.session_agent_kind.as_deref() == Some("codex")
                        && view.session_attempt == Some(view.attempts))
                    .then(|| view.session_id.clone())
                    .flatten();
                    if let Some(session_id) = resume {
                        actions.push(ReconcileAction::Resume {
                            task_id: view.task_id,
                            attempt: view.attempts,
                            session_id,
                        });
                    } else {
                        actions.push(ReconcileAction::Requeue {
                            task_id: view.task_id,
                            next_attempt: view.attempts + 1,
                            reason: "expired-without-session".into(),
                        });
                    }
                }
            }
            TaskStatus::Ready if slots > 0 && !conflicts(&view.scope_paths, &occupied) => {
                slots -= 1;
                occupied.push(occupied_scope(&view.scope_paths));
                actions.push(ReconcileAction::Start {
                    task_id: view.task_id,
                    attempt: view.attempts + 1,
                });
            }
            TaskStatus::Failed
                if view.attempts < max_attempts
                    && slots > 0
                    && !conflicts(&view.scope_paths, &occupied) =>
            {
                slots -= 1;
                occupied.push(occupied_scope(&view.scope_paths));
                actions.push(ReconcileAction::Start {
                    task_id: view.task_id,
                    attempt: view.attempts + 1,
                });
            }
            _ => {}
        }
    }
    actions
}

fn conflicts(scope: &[String], occupied: &[Vec<String>]) -> bool {
    (scope.is_empty() && !occupied.is_empty())
        || occupied.iter().any(|other| {
            other.is_empty()
                || scope
                    .iter()
                    .any(|path| other.iter().any(|other| paths_overlap(path, other)))
        })
}

fn occupied_scope(scope: &[String]) -> Vec<String> {
    if scope.is_empty() {
        vec![String::new()]
    } else {
        scope.to_vec()
    }
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let Some(left) = static_prefix(left) else {
        return true;
    };
    let Some(right) = static_prefix(right) else {
        return true;
    };
    left.value == right.value
        || left
            .value
            .strip_prefix(&right.value)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .value
            .strip_prefix(&left.value)
            .is_some_and(|rest| rest.starts_with('/'))
        || (left.glob && right.value.starts_with(&left.value))
        || (right.glob && left.value.starts_with(&right.value))
}

struct StaticPrefix {
    value: String,
    glob: bool,
}

fn static_prefix(path: &str) -> Option<StaticPrefix> {
    let normalized_path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized_path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => parts.push(part),
        }
    }
    let normalized = parts.join("/");
    let end = normalized
        .find(['*', '?', '[', '{'])
        .unwrap_or(normalized.len());
    let prefix = normalized[..end].trim_end_matches('/');
    (!prefix.is_empty()).then_some(StaticPrefix {
        value: prefix.to_string(),
        glob: end < normalized.len(),
    })
}

fn persist_reconciliation(
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<PersistOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let lock = acquire_workspace_lock(&ledger.paths)?;
    let views = ledger.task_views()?;
    let leases: Vec<TaskLease> = views
        .iter()
        .filter_map(|view| ledger.task_lease(view.task_id).transpose())
        .collect::<anyhow::Result<_>>()?;
    let project_id = edda_store::project_id(repo_root);
    let claims = edda_bridge_claude::peers::discover_active_peers(&project_id, "")
        .into_iter()
        .map(|peer| peer.claimed_paths)
        .collect::<Vec<_>>();
    let now = clock_now();
    let actions = plan_actions(
        &views,
        &leases,
        &claims,
        &now,
        config.max_workers,
        config.max_attempts,
    );
    // Git preparation happens for the complete batch before the first immutable
    // dispatch event. A later refusal must not strand an earlier RUNNING task.
    let prepared = actions
        .iter()
        .filter_map(|action| match action {
            ReconcileAction::Start { task_id, attempt } => Some((*task_id, *attempt, false)),
            ReconcileAction::Resume {
                task_id, attempt, ..
            } => Some((*task_id, *attempt, true)),
            ReconcileAction::Requeue {
                task_id,
                next_attempt,
                ..
            } => Some((*task_id, *next_attempt, false)),
            ReconcileAction::Fail { .. } => None,
        })
        .map(|(task_id, attempt, resume)| {
            let task = task_view(&views, task_id)?.clone();
            let worktree = ensure_attempt_worktree(repo_root, &task, attempt, resume)?;
            Ok((task_id, task, attempt, worktree))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let changed = !actions.is_empty();
    let mut plans = Vec::new();
    let mut errors = Vec::new();

    for action in actions {
        let result = (|| -> anyhow::Result<Option<RunnerPlan>> {
            Ok(match action {
                ReconcileAction::Start { task_id, attempt } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    if task.status == TaskStatus::Failed {
                        append_requeued(&ledger, task_id, attempt)?;
                    }
                    replace_lease(&ledger, task_id, attempt, config.lease_ttl_s)?;
                    if let Err(error) =
                        append_started(&ledger, task_id, attempt, config.lease_ttl_s)
                    {
                        let _ = ledger.delete_task_lease(task_id, attempt);
                        return Err(error);
                    }
                    Some(RunnerPlan {
                        task,
                        attempt,
                        worktree,
                    })
                }
                ReconcileAction::Resume {
                    task_id,
                    attempt,
                    session_id,
                } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    replace_lease(&ledger, task_id, attempt, config.lease_ttl_s)?;
                    let _ = session_id;
                    Some(RunnerPlan {
                        task,
                        attempt,
                        worktree,
                    })
                }
                ReconcileAction::Requeue {
                    task_id,
                    next_attempt,
                    ..
                } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == next_attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    append_requeued(&ledger, task_id, next_attempt)?;
                    replace_lease(&ledger, task_id, next_attempt, config.lease_ttl_s)?;
                    if let Err(error) =
                        append_started(&ledger, task_id, next_attempt, config.lease_ttl_s)
                    {
                        let _ = ledger.delete_task_lease(task_id, next_attempt);
                        return Err(error);
                    }
                    Some(RunnerPlan {
                        task,
                        attempt: next_attempt,
                        worktree,
                    })
                }
                ReconcileAction::Fail { task_id, reason } => {
                    append_failed(&ledger, task_id, &reason)?;
                    let attempt = task_view(&views, task_id)?.attempts;
                    let _ = ledger.delete_task_lease(task_id, attempt)?;
                    None
                }
            })
        })();
        match result {
            Ok(Some(plan)) => plans.push(plan),
            Ok(None) => {}
            Err(error) => errors.push(format!("task action persistence failed: {error:#}")),
        }
    }
    if changed {
        let branch = ledger.head_branch()?;
        let _ = edda_derive::rebuild_branch(&ledger, &branch);
    }
    drop(lock);
    Ok(PersistOutcome { plans, errors })
}

fn acquire_workspace_lock(paths: &edda_ledger::EddaPaths) -> anyhow::Result<WorkspaceLock> {
    let mut last = None;
    for _ in 0..120 {
        match WorkspaceLock::acquire(paths) {
            Ok(lock) => return Ok(lock),
            Err(error) => last = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(last.expect("lock retry records an error"))
}

fn task_view(views: &[TaskView], task_id: u64) -> anyhow::Result<&TaskView> {
    views
        .iter()
        .find(|view| view.task_id == task_id)
        .ok_or_else(|| anyhow::anyhow!("task #{task_id} disappeared during reconciliation"))
}

fn append_started(ledger: &Ledger, task_id: u64, attempt: u32, ttl_s: u64) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_TASK_ID.with(|target| target.get().is_none_or(|target| target == task_id))
        && FAIL_NEXT_STARTED.with(|flag| flag.replace(false))
    {
        anyhow::bail!("injected task.started append failure");
    }
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    ledger.append_event(&new_task_started_event(
        &branch,
        parent_hash.as_deref(),
        task_id,
        ttl_s,
        attempt,
    )?)
}

fn append_requeued(ledger: &Ledger, task_id: u64, attempt: u32) -> anyhow::Result<()> {
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    ledger.append_event(&new_task_requeued_event(
        &branch,
        parent_hash.as_deref(),
        task_id,
        attempt,
    )?)
}

fn append_failed(ledger: &Ledger, task_id: u64, reason: &str) -> anyhow::Result<()> {
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    ledger.append_event(&new_task_failed_event(
        &branch,
        parent_hash.as_deref(),
        task_id,
        reason,
    )?)
}

fn replace_lease(ledger: &Ledger, task_id: u64, attempt: u32, ttl_s: u64) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_TASK_ID.with(|target| target.get().is_none_or(|target| target == task_id))
        && FAIL_NEXT_LEASE.with(|flag| flag.replace(false))
    {
        anyhow::bail!("injected lease replacement failure");
    }
    let heartbeat_at = clock_now();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(ttl_s as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    ledger.upsert_task_lease(&TaskLease {
        task_id,
        attempt,
        owner: format!("reconcile-{}-{task_id}-{attempt}", std::process::id()),
        expires_at,
        heartbeat_at,
    })
}

fn clock_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn attempt_branch(task_id: u64, attempt: u32) -> String {
    format!("codex/task-{task_id}-attempt-{attempt}")
}

fn attempt_worktree_path(repo_root: &Path, task_id: u64, attempt: u32) -> anyhow::Result<PathBuf> {
    let canonical = repo_root.canonicalize()?;
    #[cfg(windows)]
    let canonical = {
        let display = canonical.to_string_lossy();
        PathBuf::from(display.strip_prefix(r"\\?\").unwrap_or(&display))
    };
    let parent = canonical
        .parent()
        .context("repository root has no parent")?
        .to_path_buf();
    let project = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .context("repository root has no usable project name")?;
    Ok(parent
        .join(".edda-worktrees")
        .join(format!("{project}-task-{task_id}-attempt-{attempt}")))
}

fn ensure_attempt_worktree(
    repo_root: &Path,
    task: &TaskView,
    attempt: u32,
    allow_existing_resume_state: bool,
) -> anyhow::Result<PathBuf> {
    let branch = attempt_branch(task.task_id, attempt);
    let worktree = attempt_worktree_path(repo_root, task.task_id, attempt)?;
    git(repo_root, ["rev-parse", "--is-inside-work-tree"])
        .context("reconciliation requires a Git worktree")?;
    if worktree.exists() {
        let listed = git(repo_root, ["worktree", "list", "--porcelain"])?;
        if !worktree_registered_for_branch(&listed, &worktree, &branch) {
            anyhow::bail!(
                "refusing existing attempt worktree {}: it is unseen or does not match {branch}",
                worktree.display()
            );
        }
        // A same-attempt Codex resume owns this exact registered branch. Its
        // local edits and commits are recovery state, so inspect but never reset
        // or clean it. New and replacement attempts remain conservative below.
        if allow_existing_resume_state {
            return Ok(worktree);
        }
        if !git(&worktree, ["status", "--porcelain"])?.trim().is_empty() {
            anyhow::bail!("refusing dirty attempt worktree {}", worktree.display());
        }
        if !git_success(repo_root, ["merge-base", "--is-ancestor", &branch, "HEAD"])? {
            anyhow::bail!("refusing attempt branch {branch}: it contains an unseen commit");
        }
        return Ok(worktree);
    }
    if git_success(
        repo_root,
        [
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )? {
        anyhow::bail!("refusing existing unseen attempt branch {branch}");
    }
    let parent = worktree
        .parent()
        .context("attempt worktree has no parent")?;
    std::fs::create_dir_all(parent)?;
    git(
        repo_root,
        [
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree.to_string_lossy(),
            "HEAD",
        ],
    )?;
    Ok(worktree)
}

fn worktree_registered_for_branch(listing: &str, worktree: &Path, branch: &str) -> bool {
    let expected = worktree.to_string_lossy().replace('\\', "/");
    listing.split("\n\n").any(|entry| {
        let mut path_matches = false;
        let mut branch_matches = false;
        for line in entry.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                path_matches = path.replace('\\', "/").eq_ignore_ascii_case(&expected);
            }
            if line == format!("branch refs/heads/{branch}") {
                branch_matches = true;
            }
        }
        path_matches && branch_matches
    })
}

fn git<I, S>(cwd: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_success<I, S>(cwd: &Path, args: I) -> anyhow::Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Ok(Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()?
        .success())
}

fn launch_runner_with(
    exe: &Path,
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    config: &ReconcileConfig,
) -> anyhow::Result<()> {
    let mut command = Command::new(exe);
    command
        .arg("reconcile")
        .arg("--max-workers")
        .arg(config.max_workers.to_string())
        .arg("--max-attempts")
        .arg(config.max_attempts.to_string())
        .arg("--lease-ttl-s")
        .arg(config.lease_ttl_s.to_string())
        .arg("--codex-bin")
        .arg(&config.codex_bin)
        .arg("--run-task")
        .arg(task_id.to_string())
        .arg("--attempt")
        .arg(attempt.to_string())
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn()?;
    Ok(())
}

fn notify_started(repo_root: &Path, task: &TaskView) {
    let Some(assignee) = &task.assignee else {
        return;
    };
    let config = edda_notify::NotifyConfig::load(&edda_ledger::EddaPaths::discover(repo_root));
    edda_notify::dispatch(
        &config,
        &edda_notify::NotifyEvent::TaskAssigned {
            task_id: task.task_id,
            title: task.title.clone(),
            assignee: assignee.clone(),
        },
    );
}

fn run_task(
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    config: &ReconcileConfig,
    ring_doorbell: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    if !renew_lease(&ledger, task_id, attempt, config.lease_ttl_s)? {
        if ring_doorbell {
            launch_runner_doorbell(repo_root, config)?;
        }
        return Ok(());
    }
    let result = (|| -> anyhow::Result<()> {
        let task = task_view(&ledger.task_views()?, task_id)?.clone();
        let is_same_attempt_resume = task.session_agent_kind.as_deref() == Some("codex")
            && task.session_attempt == Some(attempt)
            && task.session_id.is_some();
        let worktree = ensure_attempt_worktree(repo_root, &task, attempt, is_same_attempt_resume)
            .context("runner-setup-failed: attempt worktree")?;
        let prompt = runner_prompt(repo_root, &ledger.task_views()?, &task, attempt, &worktree);
        tokio::runtime::Runtime::new()?.block_on(async {
            let mut server =
                edda_conductor::agent::codex_app_server::CodexAppServer::spawn(&config.codex_bin)
                    .await
                    .context("runner-failed: Codex App Server spawn")?;
            let thread_id = server
                .open_thread(
                    &worktree,
                    task.session_id
                        .as_deref()
                        .filter(|_| task.session_attempt == Some(attempt)),
                )
                .await
                .context("runner-failed: Codex thread start/resume")?;
            if !record_session_if_current(
                repo_root,
                task_id,
                attempt,
                &thread_id,
                config.lease_ttl_s,
            )? {
                return Ok(());
            }
            run_turn_with_renewals(
                &mut server,
                repo_root,
                task_id,
                attempt,
                config.lease_ttl_s,
                &thread_id,
                &prompt,
            )
            .await
            .context("runner-failed: Codex turn")
        })
    })();
    let reason = result
        .as_ref()
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_else(|| "ended-without-receipt".into());
    let cleanup = finish_runner(
        repo_root,
        task_id,
        attempt,
        Some(&reason),
        ring_doorbell,
        config,
    );
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("runner finalization failed: {cleanup:#}")))
        }
    }
}

fn renew_lease(ledger: &Ledger, task_id: u64, attempt: u32, ttl_s: u64) -> anyhow::Result<bool> {
    let heartbeat_at = clock_now();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(ttl_s as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    ledger.renew_task_lease(task_id, attempt, &expires_at, &heartbeat_at)
}

fn record_session_if_current(
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    thread_id: &str,
    ttl_s: u64,
) -> anyhow::Result<bool> {
    let ledger = Ledger::open(repo_root)?;
    let lock = acquire_workspace_lock(&ledger.paths)?;
    let current = renew_lease(&ledger, task_id, attempt, ttl_s)?;
    if current {
        let branch = ledger.head_branch()?;
        let parent_hash = ledger.last_event_hash()?;
        ledger.append_event(&new_task_host_session_event(
            &branch,
            parent_hash.as_deref(),
            task_id,
            "codex",
            thread_id,
            attempt,
        )?)?;
        let _ = edda_derive::rebuild_branch(&ledger, &branch);
    }
    drop(lock);
    Ok(current)
}

async fn run_turn_with_renewals(
    server: &mut edda_conductor::agent::codex_app_server::CodexAppServer,
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    ttl_s: u64,
    thread_id: &str,
    prompt: &str,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs((ttl_s / 2).max(1)));
    interval.tick().await;
    let turn = server.run_turn(thread_id, prompt);
    tokio::pin!(turn);
    loop {
        tokio::select! {
            result = &mut turn => return result.map(|_| ()),
            _ = interval.tick() => {
                let ledger = Ledger::open(repo_root)?;
                if !renew_lease(&ledger, task_id, attempt, ttl_s)? {
                    return Ok(());
                }
            }
        }
    }
}

fn finish_runner(
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    failure_reason: Option<&str>,
    ring_doorbell: bool,
    config: &ReconcileConfig,
) -> anyhow::Result<()> {
    let cleanup = (|| -> anyhow::Result<()> {
        let ledger = Ledger::open(repo_root)?;
        let lock = acquire_workspace_lock(&ledger.paths)?;
        let mut result = Ok(());
        let owned = ledger
            .task_lease(task_id)?
            .is_some_and(|lease| lease.attempt == attempt);
        if owned {
            let view = task_view(&ledger.task_views()?, task_id)?.clone();
            if view.status != TaskStatus::Done {
                if let Some(reason) = failure_reason {
                    if let Err(error) = append_failed(&ledger, task_id, reason) {
                        result = Err(error);
                    } else {
                        let branch = ledger.head_branch()?;
                        if let Err(error) = edda_derive::rebuild_branch(&ledger, &branch) {
                            result = Err(error);
                        }
                    }
                }
            }
            if let Err(error) = ledger.delete_task_lease(task_id, attempt) {
                result = Err(error);
            }
        }
        drop(lock);
        result
    })();
    if ring_doorbell {
        if let Err(error) = launch_runner_doorbell(repo_root, config) {
            if cleanup.is_ok() {
                return Err(error);
            }
        }
    }
    cleanup
}

fn launch_runner_doorbell(repo_root: &Path, config: &ReconcileConfig) -> anyhow::Result<()> {
    #[cfg(test)]
    {
        let _ = (repo_root, config);
        DOORBELL_COUNT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    #[cfg(not(test))]
    {
        let exe = std::env::current_exe()?;
        let mut command = Command::new(exe);
        command
            .arg("reconcile")
            .arg("--max-workers")
            .arg(config.max_workers.to_string())
            .arg("--max-attempts")
            .arg(config.max_attempts.to_string())
            .arg("--lease-ttl-s")
            .arg(config.lease_ttl_s.to_string())
            .arg("--codex-bin")
            .arg(&config.codex_bin)
            .current_dir(repo_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.spawn()?;
        Ok(())
    }
}

fn runner_prompt(
    repo_root: &Path,
    views: &[TaskView],
    task: &TaskView,
    attempt: u32,
    worktree: &Path,
) -> String {
    let receipts = task
        .after
        .iter()
        .filter_map(|id| views.iter().find(|view| view.task_id == *id))
        .filter_map(|view| {
            view.receipt.as_ref().map(|receipt| {
                format!(
                    "#{}/{} evidence={:?}",
                    view.task_id, receipt, view.evidence_paths
                )
            })
        })
        .collect::<Vec<_>>();
    let brief_ref = task.brief_ref.as_deref().unwrap_or("(none)");
    let brief = task
        .brief_ref
        .as_deref()
        .and_then(|reference| read_brief(repo_root, reference).ok())
        .unwrap_or_else(|| "(unavailable)".into());
    format!(
        "Task #{id}: {title}\nBrief reference: {brief_ref}\nBrief content (bounded):\n{brief}\nScope: {scope:?}\nDependency receipts:\n{receipts}\nBranch: {branch}\nWorktree: {worktree}\nAttempt: {attempt}\n\nPaths outside scope require a durable scope request. Assistant prose is not completion. Complete with:\nedda task done {id} --receipt \"<verifiable result>\" --evidence <path>",
        id = task.task_id,
        title = task.title,
        brief_ref = brief_ref,
        brief = brief,
        scope = task.scope_paths,
        receipts = receipts.join("\n"),
        branch = attempt_branch(task.task_id, attempt),
        worktree = worktree.display(),
    )
}

fn read_brief(repo_root: &Path, reference: &str) -> anyhow::Result<String> {
    let path = Path::new(reference);
    if path.is_absolute() || reference.split(['/', '\\']).any(|part| part == "..") {
        anyhow::bail!("brief reference must be a repository-relative path");
    }
    let bytes = std::fs::read(repo_root.join(path))?;
    let truncated = bytes.len() > 4096;
    let mut content = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned();
    if truncated {
        content.push_str("\n[brief truncated]");
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use edda_ledger::tasks::{TaskStatus, TaskView};
    use edda_ledger::TaskLease;

    #[derive(Parser)]
    struct SchedulerCli {
        #[command(flatten)]
        args: ReconcileArgs,
    }

    fn test_lock(lock: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
        lock.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[cfg(not(windows))]
    fn scheduler_config(codex_bin: &str) -> ReconcileConfig {
        ReconcileConfig {
            max_workers: 3,
            max_attempts: 3,
            lease_ttl_s: 300,
            codex_bin: PathBuf::from(codex_bin),
        }
    }

    fn manifest_path_for_task_run_utf16_len(target: usize) -> PathBuf {
        let fixed = r#""C:\e.exe" reconcile --scheduler-manifest "C:\.json""#
            .encode_utf16()
            .count();
        PathBuf::from(format!(r"C:\{}.json", "x".repeat(target - fixed)))
    }

    struct SchedulerManifestFixture {
        _store_guard: crate::test_support::IsolatedStore,
        _root: tempfile::TempDir,
        store: PathBuf,
        repo: PathBuf,
        codex: PathBuf,
        config: ReconcileConfig,
    }

    fn scheduler_manifest_fixture() -> anyhow::Result<SchedulerManifestFixture> {
        let store_guard = crate::test_support::isolated_store();
        let store = edda_store::store_root();
        let root = tempfile::tempdir()?;
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo)?;
        Ledger::ensure_initialized(&repo)?;
        let repo = repo.canonicalize()?;
        let codex = root.path().join("codex.exe");
        std::fs::write(&codex, b"MZ")?;
        let codex = codex.canonicalize()?;
        let config = ReconcileConfig {
            max_workers: 3,
            max_attempts: 4,
            lease_ttl_s: 300,
            codex_bin: codex.clone(),
        };
        Ok(SchedulerManifestFixture {
            _store_guard: store_guard,
            _root: root,
            store,
            repo,
            codex,
            config,
        })
    }

    fn write_scheduler_manifest_candidate(store: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        use sha2::Digest;

        let digest = hex::encode(sha2::Sha256::digest(bytes));
        let path = store
            .join("scheduler-launch")
            .join("v1")
            .join(format!("{digest}.json"));
        edda_store::write_atomic(&path, bytes)?;
        Ok(path)
    }

    #[test]
    fn scheduler_manifest_is_canonical_content_addressed_and_strict() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let first = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let second = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.path, second.path);
        assert!(first.path.ends_with(format!("{}.json", first.digest)));
        assert!(!first.bytes.ends_with(b"\n"));
        assert_eq!(first.manifest.schema_version, 1);
        assert_eq!(
            first.manifest.project_id,
            edda_store::project_id_for_root(&fixture.repo)
        );
        edda_store::write_atomic(&first.path, &first.bytes)?;
        let loaded = load_scheduler_manifest(&first.path)?;
        assert_eq!(loaded.manifest, first.manifest);
        assert_eq!(loaded.repo, fixture.repo);
        assert_eq!(loaded.config.codex_bin, fixture.codex);
        assert_eq!(loaded.config.max_workers, fixture.config.max_workers);
        assert_eq!(loaded.config.max_attempts, fixture.config.max_attempts);
        assert_eq!(loaded.config.lease_ttl_s, fixture.config.lease_ttl_s);
        Ok(())
    }

    #[test]
    fn scheduler_manifest_changed_config_changes_digest() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let first = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let mut changed = fixture.config.clone();
        changed.max_workers += 1;
        let second = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &changed)?;

        assert_ne!(first.bytes, second.bytes);
        assert_ne!(first.digest, second.digest);
        assert_ne!(first.path, second.path);
        Ok(())
    }

    #[test]
    fn scheduler_manifest_rejects_unknown_duplicate_and_noncanonical_json() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

        let mut unknown_field: serde_json::Value = serde_json::from_slice(&prepared.bytes)?;
        unknown_field
            .as_object_mut()
            .expect("manifest object")
            .insert("extra".into(), true.into());
        let path = write_scheduler_manifest_candidate(
            &fixture.store,
            &serde_json::to_vec(&unknown_field)?,
        )?;
        assert!(load_scheduler_manifest(&path).is_err());

        let mut unknown_version = prepared.manifest.clone();
        unknown_version.schema_version = 2;
        let path = write_scheduler_manifest_candidate(
            &fixture.store,
            &serde_json::to_vec(&unknown_version)?,
        )?;
        assert!(load_scheduler_manifest(&path).is_err());

        let canonical = String::from_utf8(prepared.bytes.clone())?;
        let duplicate = canonical.replacen('{', r#"{"schema_version":1,"#, 1);
        let path = write_scheduler_manifest_candidate(&fixture.store, duplicate.as_bytes())?;
        assert!(load_scheduler_manifest(&path).is_err());

        let mut noncanonical = prepared.bytes;
        noncanonical.push(b'\n');
        let path = write_scheduler_manifest_candidate(&fixture.store, &noncanonical)?;
        assert!(load_scheduler_manifest(&path).is_err());
        Ok(())
    }

    #[test]
    fn scheduler_manifest_rejects_oversize_and_digest_mismatch() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

        let oversized = vec![b' '; 16 * 1024 + 1];
        let path = write_scheduler_manifest_candidate(&fixture.store, &oversized)?;
        assert!(load_scheduler_manifest(&path).is_err());

        let mismatch = prepared
            .path
            .with_file_name(format!("{}.json", "0".repeat(64)));
        edda_store::write_atomic(&mismatch, &prepared.bytes)?;
        assert!(load_scheduler_manifest(&mismatch).is_err());
        Ok(())
    }

    #[test]
    fn scheduler_manifest_revalidates_project_repo_and_codex() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        assert!(prepare_scheduler_manifest(
            &fixture.store,
            &fixture.repo.join("missing"),
            &fixture.config
        )
        .is_err());
        let mut invalid_codex = fixture.config.clone();
        invalid_codex.codex_bin = fixture.repo.join("codex.cmd");
        assert!(prepare_scheduler_manifest(&fixture.store, &fixture.repo, &invalid_codex).is_err());

        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let mut wrong_project = prepared.manifest.clone();
        wrong_project.project_id = "0".repeat(32);
        let path = write_scheduler_manifest_candidate(
            &fixture.store,
            &serde_json::to_vec(&wrong_project)?,
        )?;
        assert!(load_scheduler_manifest(&path).is_err());

        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        std::fs::remove_file(&fixture.codex)?;
        assert!(load_scheduler_manifest(&prepared.path).is_err());
        Ok(())
    }

    #[test]
    fn scheduler_manifest_rejects_store_root_and_reparse_escape() -> anyhow::Result<()> {
        {
            let fixture = scheduler_manifest_fixture()?;
            let prepared =
                prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
            std::fs::create_dir_all(fixture.store.join("scheduler-launch").join("v1"))?;
            let outside = fixture._root.path().join("outside");
            let escaped = outside.join(format!("{}.json", prepared.digest));
            edda_store::write_atomic(&escaped, &prepared.bytes)?;
            assert!(load_scheduler_manifest(&escaped).is_err());
        }

        let fixture = scheduler_manifest_fixture()?;
        let launch = fixture.store.join("scheduler-launch");
        let reparse_target = fixture._root.path().join("reparse-target");
        std::fs::create_dir(&reparse_target)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&reparse_target, &launch)?;
        #[cfg(windows)]
        if let Err(error) = std::os::windows::fs::symlink_dir(&reparse_target, &launch) {
            anyhow::ensure!(
                error.raw_os_error() == Some(1314),
                "create scheduler manifest directory symlink: {error}"
            );
            let output = Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(&launch)
                .arg(&reparse_target)
                .output()?;
            anyhow::ensure!(
                output.status.success(),
                "create scheduler manifest directory junction: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config).is_err()
        );
        Ok(())
    }

    #[test]
    fn scheduler_cli_parses_manifest_reentry_and_rejects_conflicting_modes() {
        use clap::CommandFactory;

        let parsed = SchedulerCli::try_parse_from([
            "test",
            "--repo",
            r"C:\ai projects\sample",
            "--install-scheduler",
        ])
        .expect("scheduler arguments");
        assert_eq!(
            parsed.args.repo.as_deref(),
            Some(Path::new(r"C:\ai projects\sample"))
        );
        assert!(parsed.args.install_scheduler);
        assert!(SchedulerCli::try_parse_from([
            "test",
            "--install-scheduler",
            "--uninstall-scheduler"
        ])
        .is_err());
        assert!(SchedulerCli::try_parse_from([
            "test",
            "--install-scheduler",
            "--run-task",
            "7",
            "--attempt",
            "1"
        ])
        .is_err());

        let manifest = r"C:\store\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json";
        let reentry = SchedulerCli::try_parse_from(["test", "--scheduler-manifest", manifest])
            .expect("manifest scheduler re-entry arguments");
        assert_eq!(
            reentry.args.scheduler_manifest.as_deref(),
            Some(Path::new(manifest))
        );
        assert!(!SchedulerCli::command()
            .render_long_help()
            .to_string()
            .contains("--scheduler-manifest"));

        for conflict in [
            &["--install-scheduler"][..],
            &["--uninstall-scheduler"][..],
            &["--repo", r"C:\repo"][..],
            &["--codex-bin", r"C:\codex.exe"][..],
            &["--run-task", "7"][..],
            &["--attempt", "1"][..],
            &["--max-workers", "2"][..],
            &["--max-attempts", "5"][..],
            &["--lease-ttl-s", "17"][..],
        ] {
            let mut args = vec!["test", "--scheduler-manifest", manifest];
            args.extend_from_slice(conflict);
            assert!(SchedulerCli::try_parse_from(args).is_err(), "{conflict:?}");
        }
    }

    #[test]
    fn scheduler_codex_config_prefers_cli_then_environment() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = test_lock(&ENV_LOCK);
        let previous = std::env::var_os("EDDA_CODEX_BIN");
        std::env::set_var("EDDA_CODEX_BIN", r"C:\environment\codex.exe");

        let explicit = SchedulerCli::try_parse_from([
            "test",
            "--install-scheduler",
            "--codex-bin",
            r"C:\explicit\codex.exe",
        ])
        .expect("explicit Codex path");
        assert_eq!(
            ReconcileConfig::from_args(&explicit.args).codex_bin,
            PathBuf::from(r"C:\explicit\codex.exe")
        );

        let inherited = SchedulerCli::try_parse_from(["test", "--install-scheduler"])
            .expect("environment Codex path");
        assert_eq!(
            ReconcileConfig::from_args(&inherited.args).codex_bin,
            PathBuf::from(r"C:\environment\codex.exe")
        );

        match previous {
            Some(value) => std::env::set_var("EDDA_CODEX_BIN", value),
            None => std::env::remove_var("EDDA_CODEX_BIN"),
        }
    }

    #[test]
    fn scheduler_renderer_emits_exact_project_scoped_argv() -> anyhow::Result<()> {
        let manifest = Path::new(
            r"C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json",
        );
        let spec = windows_scheduler_spec(
            Path::new(r"C:\Program Files\edda\edda.exe"),
            manifest,
            "0123456789abcdef0123456789abcdef",
        )?;

        assert_eq!(
            spec.task_name,
            "Edda-Reconcile-0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            spec.create_args[8],
            r#""C:\Program Files\edda\edda.exe" reconcile --scheduler-manifest "C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json""#
        );
        assert_eq!(
            spec.create_args,
            [
                "/Create",
                "/SC",
                "MINUTE",
                "/MO",
                "1",
                "/TN",
                "Edda-Reconcile-0123456789abcdef0123456789abcdef",
                "/TR",
                r#""C:\Program Files\edda\edda.exe" reconcile --scheduler-manifest "C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json""#,
                "/RL",
                "LIMITED",
                "/F",
                "/HRESULT",
            ]
        );
        assert_eq!(
            spec.query_args,
            [
                "/Query",
                "/TN",
                "Edda-Reconcile-0123456789abcdef0123456789abcdef",
                "/XML",
                "/HRESULT",
            ]
        );
        Ok(())
    }

    #[test]
    fn scheduler_renderer_is_stable_and_quotes_terminal_backslashes() -> anyhow::Result<()> {
        let id = "0123456789abcdef0123456789abcdef";
        let first = windows_scheduler_spec(
            Path::new(r"C:\edda\edda.exe"),
            Path::new(r"C:\manifest\"),
            id,
        )?;
        let second = windows_scheduler_spec(
            Path::new(r"C:\edda\edda.exe"),
            Path::new(r"C:\manifest\"),
            id,
        )?;

        assert_eq!(first.create_args, second.create_args);
        assert_eq!(
            first.create_args[8],
            r#""C:\edda\edda.exe" reconcile --scheduler-manifest "C:\manifest\\""#
        );
        Ok(())
    }

    #[test]
    fn scheduler_manifest_renderer_fits_the_preserved_356_unit_fixture() -> anyhow::Result<()> {
        let executable =
            Path::new(r"\\?\C:\ai_agent\edda-target-gh466-drill-20260816T163456Z\debug\edda.exe");
        let repository = r"\\?\C:\ai_agent\edda-drills\20260816T163456Z\repo";
        let codex = r"\\?\C:\Users\synvoke\AppData\Roaming\npm\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe";
        let old = format!(
            "{} reconcile --repo \"{repository}\" --max-workers 1 --max-attempts 3 --lease-ttl-s 120 --codex-bin \"{codex}\"",
            quote_windows_argument(executable)?,
        );
        assert_eq!(old.encode_utf16().count(), 356);

        let manifest = Path::new(
            r"\\?\C:\Users\synvoke\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json",
        );
        let rendered = render_scheduler_task_run(
            executable,
            manifest,
            "Edda-Reconcile-75ab49a9590f5e1105b928c63a3c0be5",
        )?;
        assert_eq!(rendered.encode_utf16().count(), 238);
        Ok(())
    }

    #[test]
    fn scheduler_manifest_renderer_enforces_utf16_limit() -> anyhow::Result<()> {
        let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
        let accepted_path = manifest_path_for_task_run_utf16_len(261);
        let accepted =
            render_scheduler_task_run(Path::new(r"C:\e.exe"), &accepted_path, task_name)?;
        assert_eq!(accepted.encode_utf16().count(), 261);

        let rejected_path = manifest_path_for_task_run_utf16_len(262);
        let error = render_scheduler_task_run(Path::new(r"C:\e.exe"), &rejected_path, task_name)
            .expect_err("262 UTF-16 units must fail")
            .to_string();
        assert!(error.contains("262"));
        assert!(error.contains("261"));
        Ok(())
    }

    #[test]
    fn scheduler_manifest_renderer_counts_surrogate_pairs_as_two_utf16_units() {
        let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
        let ascii = manifest_path_for_task_run_utf16_len(261);
        let with_pair = PathBuf::from(ascii.to_string_lossy().replacen('x', "😀", 1));
        let unbounded = format!(
            r#""C:\e.exe" reconcile --scheduler-manifest "{}""#,
            with_pair.display()
        );
        assert_eq!(unbounded.chars().count(), 261);
        assert_eq!(unbounded.encode_utf16().count(), 262);
        assert!(render_scheduler_task_run(Path::new(r"C:\e.exe"), &with_pair, task_name).is_err());
    }

    #[test]
    fn scheduler_codex_resolver_requires_a_canonical_direct_exe() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let native = dir.path().join("codex.exe");
        std::fs::write(&native, b"MZ")?;
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, b"@echo off")?;
        let search_path = std::env::join_paths([dir.path()])?;

        assert_eq!(
            canonical_direct_codex_executable(Path::new("codex"), Some(&search_path))?,
            native.canonicalize()?
        );
        assert_eq!(
            canonical_direct_codex_executable(&native, None)?,
            native.canonicalize()?
        );
        assert!(canonical_direct_codex_executable(&shim, None).is_err());
        assert!(
            canonical_direct_codex_executable(Path::new("missing-codex"), Some(&search_path))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn scheduler_codex_resolver_revalidates_the_canonical_target_extension() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let native = dir.path().join("codex.exe");
        std::fs::write(&native, b"MZ")?;
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, b"@echo off")?;

        validate_canonical_direct_codex_target(&native.canonicalize()?)?;
        let error = validate_canonical_direct_codex_target(&shim.canonicalize()?)
            .expect_err("a canonical .cmd target must not be schedulable");
        assert!(error
            .to_string()
            .contains("must be an absolute native .exe file"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scheduler_codex_resolver_rejects_exe_alias_to_cmd_target() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, b"#!/bin/sh")?;
        let alias = dir.path().join("codex.exe");
        std::os::unix::fs::symlink(&shim, &alias)?;

        let error = canonical_direct_codex_executable(&alias, None)
            .expect_err("an .exe alias to a .cmd target must not be schedulable");
        assert!(error.to_string().contains("absolute native .exe file"));
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_target_does_not_require_a_codex_executable() -> anyhow::Result<()> {
        let (task_name, query_args, delete_args) =
            windows_scheduler_management_args("0123456789abcdef0123456789abcdef")?;
        assert_eq!(task_name, "Edda-Reconcile-0123456789abcdef0123456789abcdef");
        assert_eq!(
            query_args,
            [
                "/Query",
                "/TN",
                "Edda-Reconcile-0123456789abcdef0123456789abcdef",
                "/XML",
                "/HRESULT",
            ]
        );
        assert_eq!(
            delete_args,
            [
                "/Delete",
                "/TN",
                "Edda-Reconcile-0123456789abcdef0123456789abcdef",
                "/F",
                "/HRESULT",
            ]
        );
        Ok(())
    }

    #[test]
    fn scheduler_windows_absolute_paths_are_host_neutral() -> anyhow::Result<()> {
        for path in [
            r"C:\edda\edda.exe",
            "C:/edda/edda.exe",
            r"\\server\share\edda.exe",
            r"\\?\C:\edda\edda.exe",
            r"\\?\UNC\server\share\edda.exe",
        ] {
            assert!(windows_path_is_absolute(Path::new(path))?, "{path}");
        }
        for path in [
            "edda.exe",
            r"C:edda.exe",
            r"\edda.exe",
            r"\\server",
            r"\\?\C:edda.exe",
            r"\\?\UNC\server",
        ] {
            assert!(!windows_path_is_absolute(Path::new(path))?, "{path}");
        }
        Ok(())
    }

    #[test]
    fn scheduler_renderer_rejects_ambiguous_inputs() {
        let executable = Path::new(r"C:\edda\edda.exe");
        let manifest = Path::new(r"C:\manifest.json");
        for id in [
            "0123456789abcdef0123456789abcde",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde*",
        ] {
            assert!(windows_scheduler_spec(executable, manifest, id).is_err());
        }
        assert!(windows_scheduler_spec(
            executable,
            Path::new("C:\\manifest\"quoted.json"),
            "0123456789abcdef0123456789abcdef",
        )
        .is_err());
        assert!(windows_scheduler_spec(
            Path::new("edda.exe"),
            manifest,
            "0123456789abcdef0123456789abcdef",
        )
        .is_err());
        assert!(windows_scheduler_spec(
            executable,
            Path::new("manifest.json"),
            "0123456789abcdef0123456789abcdef",
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn scheduler_renderer_rejects_non_unicode_paths() {
        use std::os::windows::ffi::OsStringExt;

        let invalid = std::ffi::OsString::from_wide(&[b'C' as u16, b':' as u16, 0xd800]);
        assert!(windows_scheduler_spec(
            Path::new(r"C:\edda\edda.exe"),
            Path::new(&invalid),
            "0123456789abcdef0123456789abcdef",
        )
        .is_err());
    }

    #[test]
    fn scheduler_query_classifier_accepts_only_success_and_verified_missing_hresult() {
        let present = SchedulerOutput::for_test(0, "xml", "");
        assert_eq!(
            classify_scheduler_query(&present).expect("present"),
            SchedulerTaskState::Present
        );
        let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing");
        assert_eq!(
            classify_scheduler_query(&missing).expect("missing"),
            SchedulerTaskState::Missing
        );
        for code in [5, 0x8007_0005, 0xdead_beef] {
            let error = classify_scheduler_query(&SchedulerOutput::for_test(code, "", "failure"))
                .expect_err("non-missing failures remain errors")
                .to_string();
            assert!(error.contains(&format!("0x{code:08x}")));
            assert!(error.contains(&(code as i32).to_string()));
        }
    }

    #[test]
    fn scheduler_expected_state_mismatch_preserves_bounded_process_output() {
        let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
        let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing detail");
        let install_error = require_scheduler_state(
            &missing,
            SchedulerTaskState::Present,
            "post-Create Query",
            task_name,
        )
        .expect_err("Create verification must reject missing")
        .to_string();
        assert!(install_error.contains("post-Create Query"));
        assert!(install_error.contains(task_name));
        assert!(install_error.contains("0x80070002"));
        assert!(install_error.contains("missing detail"));

        let present = SchedulerOutput::for_test(0, "present xml", "");
        let uninstall_error = require_scheduler_state(
            &present,
            SchedulerTaskState::Missing,
            "post-Delete Query",
            task_name,
        )
        .expect_err("Delete verification must reject present")
        .to_string();
        assert!(uninstall_error.contains("post-Delete Query"));
        assert!(uninstall_error.contains(task_name));
        assert!(uninstall_error.contains("0x00000000"));
        assert!(uninstall_error.contains("present xml"));
    }

    #[cfg(not(windows))]
    #[test]
    fn scheduler_lifecycle_is_explicitly_unsupported_off_windows() {
        let config = scheduler_config("/tmp/codex.exe");
        let error = scheduler_lifecycle(Path::new("/tmp/repo"), Some(&config))
            .expect_err("non-Windows scheduler must fail")
            .to_string();
        assert!(error.contains("supported only on Windows"));
    }

    #[test]
    fn scheduler_repo_reentry_requires_absolute_existing_path_and_resolves_main_worktree(
    ) -> anyhow::Result<()> {
        assert!(canonical_main_repo(Path::new("relative/repo")).is_err());
        let dir = tempfile::tempdir()?;
        assert!(canonical_main_repo(&dir.path().join("missing")).is_err());

        let parent = dir.path().join("parent git");
        std::fs::create_dir(&parent)?;
        init_git(&parent)?;
        assert!(canonical_main_repo(&parent).is_err());
        let nested = parent.join("nested edda");
        std::fs::create_dir(&nested)?;
        Ledger::ensure_initialized(&nested)?;
        assert_eq!(canonical_main_repo(&nested)?, nested.canonicalize()?);

        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("scheduler"))?;
        Ledger::ensure_initialized(&repo)?;
        let worktree = dir.path().join("linked worktree");
        std::fs::create_dir_all(&worktree)?;
        let gitdir = repo.join(".git").join("worktrees").join("scheduler");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.canonicalize()?.display()),
        )?;

        assert_eq!(canonical_main_repo(&worktree)?, repo.canonicalize()?);
        assert_eq!(
            edda_store::project_id(&worktree),
            edda_store::project_id(&repo)
        );
        Ok(())
    }

    #[test]
    fn scheduler_manifest_reentry_runs_against_its_repo_from_an_unrelated_root(
    ) -> anyhow::Result<()> {
        let mut fixture = scheduler_manifest_fixture()?;
        fixture.config.max_workers = 0;
        fixture.config.max_attempts = 1;
        let unrelated = fixture._root.path().join("unrelated cwd");
        std::fs::create_dir(&unrelated)?;
        let ledger = Ledger::open(&fixture.repo)?;
        create_task(&ledger, 91, &["src/scheduled.rs".into()])?;
        append_started(&ledger, 91, 1, 1)?;
        ledger.upsert_task_lease(&lease(91, 1, "2026-08-16T00:00:00Z"))?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;

        run(
            &unrelated,
            ReconcileArgs {
                max_workers: 3,
                max_attempts: 3,
                lease_ttl_s: 300,
                codex_bin: None,
                install_scheduler: false,
                uninstall_scheduler: false,
                repo: None,
                run_task: None,
                attempt: None,
                scheduler_manifest: Some(prepared.path),
            },
        )?;

        let view = ledger.task_views()?.remove(0);
        assert_eq!(view.status, TaskStatus::Failed);
        assert_eq!(view.failure_reason.as_deref(), Some("retry-cap-exhausted"));
        assert!(!unrelated.join(".edda").exists());
        Ok(())
    }

    #[cfg(windows)]
    fn allow_fake_turn_after_durable_session(
        repo: std::path::PathBuf,
        task_id: u64,
        attempt: u32,
        challenge: std::path::PathBuf,
        allow: std::path::PathBuf,
        deny: std::path::PathBuf,
    ) -> std::thread::JoinHandle<anyhow::Result<()>> {
        std::thread::spawn(move || {
            for _ in 0..200 {
                if challenge.exists() {
                    let view = Ledger::open(&repo)?
                        .task_views()?
                        .into_iter()
                        .find(|view| view.task_id == task_id);
                    let valid = view.is_some_and(|view| {
                        view.session_agent_kind.as_deref() == Some("codex")
                            && view.session_attempt == Some(attempt)
                            && view.session_id.as_deref() == Some("fake-thread")
                    });
                    std::fs::write(if valid { allow } else { deny }, "gate")?;
                    anyhow::ensure!(valid, "fake observed turn before durable current session");
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            anyhow::bail!("fake never challenged turn gate")
        })
    }

    fn task(id: u64, status: TaskStatus, scope: &[&str]) -> TaskView {
        TaskView {
            task_id: id,
            title: format!("task {id}"),
            assignee: None,
            agent_kind: None,
            after: Vec::new(),
            scope_paths: scope.iter().map(|path| (*path).to_string()).collect(),
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            status,
            attempts: 0,
            receipt: None,
            evidence_paths: Vec::new(),
            acp_session_id: None,
            session_id: None,
            session_agent_kind: None,
            session_attempt: None,
            failure_reason: None,
            created_ts: "2026-08-16T00:00:00Z".into(),
            updated_ts: "2026-08-16T00:00:00Z".into(),
            created_event_id: format!("evt-{id}"),
        }
    }

    fn lease(task_id: u64, attempt: u32, expires_at: &str) -> TaskLease {
        TaskLease {
            task_id,
            attempt,
            owner: format!("runner-{task_id}-{attempt}"),
            expires_at: expires_at.into(),
            heartbeat_at: "2026-08-16T00:00:00Z".into(),
        }
    }

    fn create_task(ledger: &Ledger, task_id: u64, scope_paths: &[String]) -> anyhow::Result<()> {
        let parent_hash = ledger.last_event_hash()?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: parent_hash.as_deref(),
                task_id,
                title: &format!("task {task_id}"),
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths,
            },
        )?)?;
        Ok(())
    }

    #[test]
    fn planner_starts_ready_tasks_by_id_subject_to_wip() {
        let actions = plan_actions(
            &[
                task(9, TaskStatus::Ready, &["src/nine.rs"]),
                task(2, TaskStatus::Ready, &["src/two.rs"]),
            ],
            &[],
            &[],
            "2026-08-16T01:00:00Z",
            1,
            3,
        );

        assert_eq!(
            actions,
            vec![ReconcileAction::Start {
                task_id: 2,
                attempt: 1
            }]
        );
    }

    #[test]
    fn planner_leaves_live_running_and_resumes_expired_bound_session() {
        let mut live = task(1, TaskStatus::Running, &["src/live.rs"]);
        live.attempts = 1;
        let mut expired = task(2, TaskStatus::Running, &["src/expired.rs"]);
        expired.attempts = 2;
        expired.session_id = Some("thread-2".into());
        expired.session_agent_kind = Some("codex".into());
        expired.session_attempt = Some(2);
        let actions = plan_actions(
            &[live, expired],
            &[
                lease(1, 1, "2026-08-16T02:00:00Z"),
                lease(2, 2, "2026-08-16T00:00:00Z"),
            ],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );

        assert_eq!(
            actions,
            vec![ReconcileAction::Resume {
                task_id: 2,
                attempt: 2,
                session_id: "thread-2".into(),
            }]
        );
    }

    #[test]
    fn planner_requeues_expired_unresumable_work_and_stops_at_retry_cap() {
        let mut retry = task(1, TaskStatus::Running, &["src/retry.rs"]);
        retry.attempts = 1;
        let mut capped = task(2, TaskStatus::Running, &["src/capped.rs"]);
        capped.attempts = 3;
        let actions = plan_actions(
            &[retry, capped],
            &[
                lease(1, 1, "2026-08-16T00:00:00Z"),
                lease(2, 3, "2026-08-16T00:00:00Z"),
            ],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );

        assert_eq!(
            actions,
            vec![
                ReconcileAction::Requeue {
                    task_id: 1,
                    next_attempt: 2,
                    reason: "expired-without-session".into(),
                },
                ReconcileAction::Fail {
                    task_id: 2,
                    reason: "retry-cap-exhausted".into(),
                },
            ]
        );
    }

    #[test]
    fn planner_resumes_only_a_codex_session_bound_to_the_current_attempt() {
        let mut task = task(1, TaskStatus::Running, &["src/retry.rs"]);
        task.attempts = 2;
        task.session_id = Some("old-thread".into());
        task.session_agent_kind = Some("codex".into());
        task.session_attempt = Some(1);
        let actions = plan_actions(
            &[task],
            &[lease(1, 2, "2026-08-16T00:00:00Z")],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );

        assert_eq!(
            actions,
            vec![ReconcileAction::Requeue {
                task_id: 1,
                next_attempt: 3,
                reason: "expired-without-session".into(),
            }]
        );
    }

    #[test]
    fn planner_treats_exact_expiry_as_expired_and_blocks_missing_dependencies() {
        let mut live = task(1, TaskStatus::Running, &["src/live.rs"]);
        live.attempts = 1;
        let blocked = task(2, TaskStatus::Blocked, &["src/blocked.rs"]);
        assert!(plan_actions(
            &[live.clone(), blocked],
            &[lease(1, 1, "2026-08-16T01:00:00Z")],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        )
        .iter()
        .any(|action| matches!(action, ReconcileAction::Requeue { task_id: 1, .. })));
        assert!(plan_actions(
            &[live],
            &[lease(1, 1, "2026-08-16T01:00:01Z")],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        )
        .is_empty());
    }

    #[test]
    fn planner_prevents_selected_scopes_from_overlapping_each_other() {
        let actions = plan_actions(
            &[
                task(1, TaskStatus::Ready, &["src/auth"]),
                task(2, TaskStatus::Ready, &["src/auth/login.rs"]),
                task(3, TaskStatus::Ready, &["src/billing.rs"]),
            ],
            &[],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );
        assert_eq!(
            actions,
            vec![
                ReconcileAction::Start {
                    task_id: 1,
                    attempt: 1
                },
                ReconcileAction::Start {
                    task_id: 3,
                    attempt: 1
                },
            ]
        );
    }

    #[test]
    fn planner_treats_an_empty_selected_scope_as_repo_wide() {
        let actions = plan_actions(
            &[
                task(1, TaskStatus::Ready, &[]),
                task(2, TaskStatus::Ready, &["src/other.rs"]),
            ],
            &[],
            &[],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );
        assert_eq!(
            actions,
            vec![ReconcileAction::Start {
                task_id: 1,
                attempt: 1
            }]
        );
    }

    #[test]
    fn planner_ignores_live_peers_without_claimed_paths() {
        let actions = plan_actions(
            &[task(1, TaskStatus::Ready, &[])],
            &[],
            &[Vec::new()],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );
        assert_eq!(
            actions,
            vec![ReconcileAction::Start {
                task_id: 1,
                attempt: 1
            }]
        );
    }

    #[test]
    fn planner_blocks_declared_claimed_and_empty_scopes_conservatively() {
        let actions = plan_actions(
            &[
                task(1, TaskStatus::Ready, &["src/auth/*.rs"]),
                task(2, TaskStatus::Ready, &["src/auth/login.rs"]),
                task(3, TaskStatus::Ready, &[]),
                task(4, TaskStatus::Ready, &["src/other.rs"]),
            ],
            &[],
            &[vec!["src/auth".into()]],
            "2026-08-16T01:00:00Z",
            3,
            3,
        );

        assert_eq!(
            actions,
            vec![ReconcileAction::Start {
                task_id: 4,
                attempt: 1
            }]
        );
    }

    #[test]
    fn static_prefixes_normalize_separators_and_glob_suffixes() {
        assert!(paths_overlap("src\\auth\\*.rs", "src/auth/login.rs"));
        assert!(paths_overlap("src/auth?", "src/auth/login.rs"));
        assert!(paths_overlap("src/auth[ab]", "src/auth/login.rs"));
        assert!(paths_overlap("src/auth{a,b}", "src/auth/login.rs"));
        assert!(paths_overlap(
            "src/./auth/../auth/*.rs",
            "src/auth/login.rs"
        ));
        assert!(paths_overlap("src/foo*", "src/foobar.rs"));
        assert!(paths_overlap("src/auth?", "src/authX"));
        assert!(paths_overlap("../outside", "src/billing.rs"));
        assert!(!paths_overlap("src/auth.rs", "src/billing.rs"));
        assert!(paths_overlap("*", "src/billing.rs"));
    }

    #[test]
    fn runner_prompt_embeds_bounded_brief_and_dependency_evidence() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("brief.md"),
            format!("brief-{}", "x".repeat(5000)),
        )?;
        let mut dependency = task(1, TaskStatus::Done, &[]);
        dependency.receipt = Some("dependency complete".into());
        dependency.evidence_paths = vec!["proof/a.txt".into(), "proof/b.txt".into()];
        let mut current = task(2, TaskStatus::Ready, &["src/reconcile.rs"]);
        current.after = vec![1];
        current.brief_ref = Some("brief.md".into());
        let worktree = dir.path().join("worktree");

        let prompt = runner_prompt(
            dir.path(),
            &[dependency, current.clone()],
            &current,
            3,
            &worktree,
        );

        assert!(prompt.contains("brief-"));
        assert!(prompt.contains("[brief truncated]"));
        assert!(
            prompt.contains("#1/dependency complete evidence=[\"proof/a.txt\", \"proof/b.txt\"]")
        );
        assert!(prompt.contains("codex/task-2-attempt-3"));
        assert!(prompt.contains(&worktree.display().to_string()));
        assert!(prompt.contains("edda task done 2"));
        Ok(())
    }

    #[test]
    fn reconciliation_persists_one_attempt_before_any_runner_launch() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        let event =
            edda_core::event::new_task_created_event(&edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "reconcile me",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/reconcile.rs".into()],
            })?;
        ledger.append_event(&event)?;

        let first = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;
        let second = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;

        assert_eq!(first.plans.len(), 1);
        assert!(second.plans.is_empty());
        let events = ledger.task_events()?;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "task.started")
                .count(),
            1
        );
        assert_eq!(ledger.task_lease(1)?.expect("current lease").attempt, 1);
        Ok(())
    }

    #[test]
    fn event_and_lease_boundary_failures_never_leave_ownerless_started_truth() -> anyhow::Result<()>
    {
        for fail_started in [false, true] {
            let dir = tempfile::tempdir()?;
            let repo = dir.path().join("repo");
            std::fs::create_dir(&repo)?;
            init_git(&repo)?;
            edda_ledger::Ledger::ensure_initialized(&repo)?;
            let ledger = edda_ledger::Ledger::open(&repo)?;
            create_task(&ledger, 1, &["src/boundary.rs".into()])?;
            if fail_started {
                FAIL_NEXT_STARTED.with(|flag| flag.set(true));
            } else {
                FAIL_NEXT_LEASE.with(|flag| flag.set(true));
            }

            let outcome = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;
            assert!(outcome.plans.is_empty());
            assert_eq!(outcome.errors.len(), 1);
            assert!(ledger.task_lease(1)?.is_none());
            assert!(ledger
                .task_events()?
                .iter()
                .all(|event| event.event_type != "task.started"));
        }
        Ok(())
    }

    #[test]
    fn middle_persistence_fault_returns_first_and_later_launchable_plans() -> anyhow::Result<()> {
        for fail_started in [false, true] {
            let dir = tempfile::tempdir()?;
            let repo = dir.path().join("repo");
            std::fs::create_dir(&repo)?;
            init_git(&repo)?;
            edda_ledger::Ledger::ensure_initialized(&repo)?;
            let ledger = edda_ledger::Ledger::open(&repo)?;
            for id in 1..=3 {
                create_task(&ledger, id, &[format!("src/{id}.rs")])?;
            }
            FAIL_TASK_ID.with(|target| target.set(Some(2)));
            if fail_started {
                FAIL_NEXT_STARTED.with(|flag| flag.set(true));
            } else {
                FAIL_NEXT_LEASE.with(|flag| flag.set(true));
            }
            let outcome = persist_reconciliation(
                &repo,
                &ReconcileConfig {
                    max_workers: 3,
                    ..ReconcileConfig::test_defaults()
                },
            )?;
            FAIL_TASK_ID.with(|target| target.set(None));

            assert_eq!(outcome.errors.len(), 1);
            assert_eq!(
                outcome
                    .plans
                    .iter()
                    .map(|plan| plan.task.task_id)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            assert!(ledger.task_lease(2)?.is_none());
            assert!(ledger.task_events()?.iter().all(|event| !(event.event_type
                == "task.started"
                && event.payload["task_id"] == 2)));
        }
        Ok(())
    }

    fn init_git(repo: &Path) -> anyhow::Result<()> {
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()?;
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Edda Test",
                "-c",
                "user.email=edda@example.test",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ])
            .current_dir(repo)
            .status()?;
        assert!(status.success());
        Ok(())
    }

    #[test]
    fn git_preparation_failure_leaves_no_phantom_dispatch() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("not-a-git-repo");
        std::fs::create_dir(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "must not dispatch",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/nope.rs".into()],
            },
        )?)?;

        assert!(persist_reconciliation(&repo, &ReconcileConfig::test_defaults()).is_err());
        assert!(ledger.task_lease(1)?.is_none());
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.started"));
        Ok(())
    }

    #[test]
    fn batch_preflights_every_worktree_before_the_first_started_event() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        create_task(&ledger, 1, &["src/one.rs".into()])?;
        create_task(&ledger, 2, &["src/two.rs".into()])?;
        let blocked = attempt_worktree_path(&repo, 2, 1)?;
        std::fs::create_dir_all(&blocked)?;
        std::fs::write(blocked.join("unseen.txt"), "preserve")?;

        let result = persist_reconciliation(
            &repo,
            &ReconcileConfig {
                max_workers: 2,
                ..ReconcileConfig::test_defaults()
            },
        );

        assert!(result.is_err());
        assert!(ledger.task_lease(1)?.is_none());
        assert!(ledger.task_lease(2)?.is_none());
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.started"));
        assert_eq!(
            std::fs::read_to_string(blocked.join("unseen.txt"))?,
            "preserve"
        );
        Ok(())
    }

    #[test]
    fn same_attempt_resume_preserves_dirty_and_ahead_worktree_state() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        let worktree = ensure_attempt_worktree(
            &repo,
            &task(3, TaskStatus::Running, &["src/resume.rs"]),
            1,
            false,
        )?;
        std::fs::write(worktree.join("dirty.txt"), "keep")?;
        Command::new("git")
            .args(["add", "dirty.txt"])
            .current_dir(&worktree)
            .status()?;
        Command::new("git")
            .args([
                "-c",
                "user.name=Edda Test",
                "-c",
                "user.email=edda@example.test",
                "commit",
                "-qm",
                "recovery state",
            ])
            .current_dir(&worktree)
            .status()?;
        std::fs::write(worktree.join("untracked.txt"), "also keep")?;

        assert_eq!(
            ensure_attempt_worktree(
                &repo,
                &task(3, TaskStatus::Running, &["src/resume.rs"]),
                1,
                true,
            )?,
            worktree
        );
        assert_eq!(std::fs::read_to_string(worktree.join("dirty.txt"))?, "keep");
        assert_eq!(
            std::fs::read_to_string(worktree.join("untracked.txt"))?,
            "also keep"
        );
        assert!(ensure_attempt_worktree(
            &repo,
            &task(3, TaskStatus::Running, &["src/resume.rs"]),
            1,
            false,
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn completion_from_linked_worktree_uses_the_original_ledger() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        create_task(&ledger, 4, &["src/done.rs".into()])?;
        let view = ledger.task_views()?.remove(0);
        let worktree = ensure_attempt_worktree(&repo, &view, 1, false)?;
        append_started(&ledger, 4, 1, 300)?;

        let resolved = edda_ledger::EddaPaths::find_root(&worktree).expect("original ledger root");
        crate::cmd_task::execute(
            crate::cmd_task::TaskCmd::Done {
                id: 4,
                receipt: "completed from attempt worktree".into(),
                evidence_paths: vec!["evidence.txt".into()],
            },
            &resolved,
        )?;

        assert_eq!(resolved.canonicalize()?, repo.canonicalize()?);
        assert!(!worktree.join(".edda").exists());
        assert_eq!(ledger.task_views()?[0].status, TaskStatus::Done);
        Ok(())
    }

    #[test]
    fn simultaneous_reconciles_create_one_attempt_and_release_the_lock() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "one attempt",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/one.rs".into()],
            },
        )?)?;
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let repo = repo.clone();
                let gate = gate.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    persist_reconciliation(&repo, &ReconcileConfig::test_defaults())
                        .map(|outcome| outcome.plans.len())
                })
            })
            .collect();
        let dispatched: usize = handles
            .into_iter()
            .map(|handle| handle.join().expect("reconcile thread"))
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .sum();

        assert_eq!(dispatched, 1);
        assert_eq!(
            ledger
                .task_events()?
                .iter()
                .filter(|event| event.event_type == "task.started")
                .count(),
            1
        );
        assert_eq!(ledger.task_lease(1)?.expect("lease").attempt, 1);
        let lock = WorkspaceLock::acquire(&ledger.paths)?;
        drop(lock);
        Ok(())
    }

    #[test]
    fn attempt_worktree_reuses_matching_state_and_refuses_dirty_state() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        let worktree =
            ensure_attempt_worktree(&repo, &task(9, TaskStatus::Ready, &["src/x.rs"]), 2, false)?;
        assert_eq!(
            worktree,
            ensure_attempt_worktree(&repo, &task(9, TaskStatus::Ready, &["src/x.rs"]), 2, false)?
        );
        assert_eq!(attempt_branch(9, 2), "codex/task-9-attempt-2");
        std::fs::write(worktree.join("untracked.txt"), "keep")?;
        assert!(ensure_attempt_worktree(
            &repo,
            &task(9, TaskStatus::Ready, &["src/x.rs"]),
            2,
            false
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(worktree.join("untracked.txt"))?,
            "keep"
        );
        Ok(())
    }

    #[test]
    fn failed_retry_records_requeue_before_the_replacement_start() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "retry",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/retry.rs".into()],
            },
        )?)?;
        append_started(&ledger, 1, 1, 300)?;
        append_failed(&ledger, 1, "crash")?;

        persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;

        let kinds: Vec<_> = ledger
            .task_events()?
            .into_iter()
            .map(|event| event.event_type)
            .collect();
        assert_eq!(kinds[kinds.len() - 2..], ["task.requeued", "task.started"]);
        assert_eq!(ledger.task_lease(1)?.expect("replacement").attempt, 2);
        Ok(())
    }

    #[test]
    fn stale_runner_cannot_append_session_or_failure_for_replacement() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path();
        edda_ledger::Ledger::ensure_initialized(repo)?;
        let ledger = edda_ledger::Ledger::open(repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 7,
                title: "replacement safety",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/safety.rs".into()],
            },
        )?)?;
        ledger.upsert_task_lease(&TaskLease {
            task_id: 7,
            attempt: 2,
            owner: "new-runner".into(),
            expires_at: "2026-08-16T02:00:00Z".into(),
            heartbeat_at: "2026-08-16T01:00:00Z".into(),
        })?;

        assert!(!record_session_if_current(repo, 7, 1, "old-thread", 300)?);
        finish_runner(
            repo,
            7,
            1,
            Some("old runner"),
            false,
            &ReconcileConfig::test_defaults(),
        )?;

        assert_eq!(ledger.task_lease(7)?.expect("replacement lease").attempt, 2);
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.session"));
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.failed"));
        Ok(())
    }

    #[test]
    fn initially_stale_runner_rings_doorbell_without_mutating_replacement() -> anyhow::Result<()> {
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path();
        edda_ledger::Ledger::ensure_initialized(repo)?;
        let ledger = edda_ledger::Ledger::open(repo)?;
        create_task(&ledger, 12, &["src/stale.rs".into()])?;
        ledger.upsert_task_lease(&lease(12, 2, "2026-08-16T02:00:00Z"))?;
        DOORBELL_COUNT.store(0, Ordering::SeqCst);

        run_task(repo, 12, 1, &ReconcileConfig::test_defaults(), true)?;

        assert_eq!(ledger.task_lease(12)?.expect("replacement").attempt, 2);
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        assert!(ledger.task_events()?.iter().all(|event| {
            event.event_type != "task.session" && event.event_type != "task.failed"
        }));
        Ok(())
    }

    #[test]
    fn owned_finalization_records_reason_deletes_only_its_lease_and_rings_once(
    ) -> anyhow::Result<()> {
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path();
        edda_ledger::Ledger::ensure_initialized(repo)?;
        let ledger = edda_ledger::Ledger::open(repo)?;
        create_task(&ledger, 8, &["src/finalize.rs".into()])?;
        ledger.upsert_task_lease(&lease(8, 1, "2026-08-16T02:00:00Z"))?;
        DOORBELL_COUNT.store(0, Ordering::SeqCst);

        finish_runner(
            repo,
            8,
            1,
            Some("runner-failed: test setup"),
            true,
            &ReconcileConfig::test_defaults(),
        )?;

        assert!(ledger.task_lease(8)?.is_none());
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(
            ledger.task_events()?.last().expect("failure event").payload["reason"],
            "runner-failed: test setup"
        );
        Ok(())
    }

    #[test]
    fn runner_spawn_failure_is_compensated_without_a_live_lease() -> anyhow::Result<()> {
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path();
        edda_ledger::Ledger::ensure_initialized(repo)?;
        let ledger = edda_ledger::Ledger::open(repo)?;
        create_task(&ledger, 9, &["src/spawn.rs".into()])?;
        ledger.upsert_task_lease(&lease(9, 1, "2026-08-16T02:00:00Z"))?;
        let config = ReconcileConfig::test_defaults();
        let missing = repo.join("missing-runner.exe");

        let error = launch_runner_with(&missing, repo, 9, 1, &config).unwrap_err();
        let reason = format!("runner-spawn-failed: {error:#}");
        DOORBELL_COUNT.store(0, Ordering::SeqCst);
        finish_runner(repo, 9, 1, Some(&reason), true, &config)?;

        assert!(ledger.task_lease(9)?.is_none());
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        assert!(ledger
            .task_events()?
            .iter()
            .any(|event| event.payload["reason"]
                .as_str()
                .unwrap_or_default()
                .starts_with("runner-spawn-failed:")));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn first_spawn_failure_does_not_prevent_later_plan_launch() -> anyhow::Result<()> {
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path();
        edda_ledger::Ledger::ensure_initialized(repo)?;
        let ledger = edda_ledger::Ledger::open(repo)?;
        create_task(&ledger, 10, &["src/one.rs".into()])?;
        create_task(&ledger, 11, &["src/two.rs".into()])?;
        ledger.upsert_task_lease(&lease(10, 1, "2026-08-16T02:00:00Z"))?;
        ledger.upsert_task_lease(&lease(11, 1, "2026-08-16T02:00:00Z"))?;
        let launched_file = repo.join("later-launch.txt");
        let launcher = repo.join("later-launch.cmd");
        std::fs::write(
            &launcher,
            "@echo off\r\necho launched > \"%~dp0later-launch.txt\"\r\n",
        )?;
        let views = ledger.task_views()?;
        let plans = vec![
            RunnerPlan {
                task: task_view(&views, 10)?.clone(),
                attempt: 1,
                worktree: repo.join("attempt-10"),
            },
            RunnerPlan {
                task: task_view(&views, 11)?.clone(),
                attempt: 1,
                worktree: repo.join("attempt-11"),
            },
        ];
        DOORBELL_COUNT.store(0, Ordering::SeqCst);
        let (launched, errors) = launch_plans_with(
            repo,
            plans,
            &ReconcileConfig::test_defaults(),
            &[repo.join("missing.exe"), launcher],
        );

        assert_eq!(launched.len(), 1);
        assert_eq!(errors.len(), 1);
        for _ in 0..40 {
            if launched_file.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(launched_file.exists());
        assert!(ledger.task_lease(10)?.is_none());
        assert_eq!(ledger.task_lease(11)?.expect("later lease").attempt, 1);
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn fake_runner_records_session_in_main_ledger_before_turn_and_fails_without_receipt(
    ) -> anyhow::Result<()> {
        let _fake = test_lock(&FAKE_CODEX_LOCK);
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "fake runner",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: Some("brief.md"),
                idempotency_key: None,
                scope_paths: &["src/runner.rs".into()],
            },
        )?)?;
        append_started(&ledger, 1, 1, 300)?;
        ledger.upsert_task_lease(&lease(1, 1, "2026-08-16T02:00:00Z"))?;
        let worktree = ensure_attempt_worktree(&repo, &ledger.task_views()?.remove(0), 1, false)?;
        let fake = fake_codex(dir.path(), 0, false)?;
        let mut config = ReconcileConfig::test_defaults();
        config.codex_bin = fake;

        let challenge = dir.path().join("turn.challenge");
        let allow = dir.path().join("turn.allow");
        let deny = dir.path().join("turn.deny");
        std::env::set_var("EDDA_FAKE_CHALLENGE", &challenge);
        std::env::set_var("EDDA_FAKE_ALLOW", &allow);
        std::env::set_var("EDDA_FAKE_DENY", &deny);
        let observer =
            allow_fake_turn_after_durable_session(repo.clone(), 1, 1, challenge, allow, deny);

        DOORBELL_COUNT.store(0, Ordering::SeqCst);
        let run_result = run_task(&repo, 1, 1, &config, true);
        let observer_result = observer.join();
        std::env::remove_var("EDDA_FAKE_CHALLENGE");
        std::env::remove_var("EDDA_FAKE_ALLOW");
        std::env::remove_var("EDDA_FAKE_DENY");
        run_result?;
        observer_result.expect("observer thread")?;

        assert_eq!(
            edda_ledger::EddaPaths::find_root(&worktree)
                .expect("original ledger root")
                .canonicalize()?,
            repo.canonicalize()?
        );
        assert!(!worktree.join(".edda").exists());
        let events = ledger.task_events()?;
        let session = events
            .iter()
            .position(|event| event.event_type == "task.session")
            .unwrap();
        let failed = events
            .iter()
            .position(|event| event.event_type == "task.failed")
            .unwrap();
        assert!(session < failed);
        assert_eq!(events[session].payload["agent_kind"], "codex");
        assert_eq!(events[session].payload["attempt"], 1);
        assert_eq!(events[failed].payload["reason"], "ended-without-receipt");
        assert!(ledger.task_lease(1)?.is_none());
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        let requests = std::fs::read_to_string(dir.path().join("fake-codex.log"))?;
        assert!(requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"method\":\"turn/start\""));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn periodic_renewal_stops_old_runner_before_failure_after_lease_replacement(
    ) -> anyhow::Result<()> {
        let _fake = test_lock(&FAKE_CODEX_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        ledger.append_event(&edda_core::event::new_task_created_event(
            &edda_core::event::TaskCreatedParams {
                branch: "main",
                parent_hash: None,
                task_id: 1,
                title: "long fake runner",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan_id: None,
                work_unit_ref: None,
                brief_ref: None,
                idempotency_key: None,
                scope_paths: &["src/runner.rs".into()],
            },
        )?)?;
        append_started(&ledger, 1, 1, 1)?;
        ledger.upsert_task_lease(&lease(1, 1, "2026-08-16T02:00:00Z"))?;
        let fake = fake_codex(dir.path(), 2, false)?;
        let mut config = ReconcileConfig::test_defaults();
        config.lease_ttl_s = 1;
        config.codex_bin = fake;
        let runner_repo = repo.clone();
        let runner_config = config.clone();
        let runner =
            std::thread::spawn(move || run_task(&runner_repo, 1, 1, &runner_config, false));

        for _ in 0..100 {
            if ledger
                .task_events()?
                .iter()
                .any(|event| event.event_type == "task.session")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(ledger
            .task_events()?
            .iter()
            .any(|event| event.event_type == "task.session"));
        let after_session = ledger.task_lease(1)?.expect("session lease");
        let mut saw_periodic_renewal = false;
        for _ in 0..100 {
            let current = ledger.task_lease(1)?.expect("current lease");
            if current.heartbeat_at != after_session.heartbeat_at
                || current.expires_at != after_session.expires_at
            {
                saw_periodic_renewal = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            saw_periodic_renewal,
            "runner crossed a periodic renewal interval"
        );
        ledger.upsert_task_lease(&TaskLease {
            task_id: 1,
            attempt: 2,
            owner: "replacement".into(),
            expires_at: "2026-08-16T03:00:00Z".into(),
            heartbeat_at: "2026-08-16T01:00:00Z".into(),
        })?;
        runner.join().expect("runner thread")?;

        assert_eq!(ledger.task_lease(1)?.expect("replacement").attempt, 2);
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.failed"));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn fake_runner_resumes_the_current_attempt_session_before_turn() -> anyhow::Result<()> {
        let _fake = test_lock(&FAKE_CODEX_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        create_task(&ledger, 2, &["src/resume.rs".into()])?;
        append_started(&ledger, 2, 1, 300)?;
        ledger.upsert_task_lease(&lease(2, 1, "2026-08-16T02:00:00Z"))?;
        assert!(record_session_if_current(&repo, 2, 1, "saved-thread", 300)?);
        let fake = fake_codex(dir.path(), 0, false)?;
        let mut config = ReconcileConfig::test_defaults();
        config.codex_bin = fake;

        let challenge = dir.path().join("resume.challenge");
        let allow = dir.path().join("resume.allow");
        let deny = dir.path().join("resume.deny");
        std::env::set_var("EDDA_FAKE_CHALLENGE", &challenge);
        std::env::set_var("EDDA_FAKE_ALLOW", &allow);
        std::env::set_var("EDDA_FAKE_DENY", &deny);
        let observer =
            allow_fake_turn_after_durable_session(repo.clone(), 2, 1, challenge, allow, deny);

        let run_result = run_task(&repo, 2, 1, &config, false);
        let observer_result = observer.join();
        std::env::remove_var("EDDA_FAKE_CHALLENGE");
        std::env::remove_var("EDDA_FAKE_ALLOW");
        std::env::remove_var("EDDA_FAKE_DENY");
        run_result?;
        observer_result.expect("observer thread")?;

        let requests = std::fs::read_to_string(dir.path().join("fake-codex.log"))?;
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(requests.contains("\"method\":\"turn/start\""));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn fake_permission_request_is_rejected_then_finalized_once() -> anyhow::Result<()> {
        let _fake = test_lock(&FAKE_CODEX_LOCK);
        let _doorbell = test_lock(&DOORBELL_LOCK);
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        create_task(&ledger, 5, &["src/permission.rs".into()])?;
        append_started(&ledger, 5, 1, 300)?;
        ledger.upsert_task_lease(&lease(5, 1, "2026-08-16T02:00:00Z"))?;
        let mut config = ReconcileConfig::test_defaults();
        config.codex_bin = fake_codex(dir.path(), 0, true)?;
        DOORBELL_COUNT.store(0, Ordering::SeqCst);

        let error = run_task(&repo, 5, 1, &config, true).expect_err("permission must fail");

        assert!(error.to_string().contains("runner-failed"));
        assert!(ledger.task_lease(5)?.is_none());
        assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
        let failed = ledger
            .task_events()?
            .into_iter()
            .find(|event| event.event_type == "task.failed")
            .expect("permission failure event");
        assert!(failed.payload["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("requestApproval"));
        Ok(())
    }

    #[cfg(windows)]
    fn fake_codex(dir: &Path, delay_s: u64, permission: bool) -> anyhow::Result<PathBuf> {
        let script = dir.join("fake-codex.ps1");
        let log = dir.join("fake-codex.log");
        std::fs::write(
            &script,
            r#"$ErrorActionPreference = 'Stop'
function Read-Line { $line = [Console]::In.ReadLine(); Add-Content -LiteralPath 'LOGFILE' -Value $line }
function Write-Line([string]$line) { [Console]::Out.WriteLine($line); [Console]::Out.Flush() }
Read-Line
Write-Line '{"id":1,"result":{}}'
Read-Line
Read-Line
Write-Line '{"id":2,"result":{"thread":{"id":"fake-thread"}}}'
Read-Line
if ($env:EDDA_FAKE_CHALLENGE) {
  New-Item -ItemType File -Force -Path $env:EDDA_FAKE_CHALLENGE | Out-Null
  for ($i = 0; $i -lt 200; $i++) {
    if (Test-Path $env:EDDA_FAKE_ALLOW) { break }
    if (Test-Path $env:EDDA_FAKE_DENY) { exit 7 }
    Start-Sleep -Milliseconds 10
  }
  if (-not (Test-Path $env:EDDA_FAKE_ALLOW)) { exit 7 }
}
Start-Sleep -Seconds DELAY
EVENTS
Write-Line '{"id":3,"result":{"turn":{"id":"fake-turn"}}}'
"#
            .replace("DELAY", &delay_s.to_string())
            .replace("LOGFILE", &log.to_string_lossy().replace("'", "''"))
            .replace(
                "EVENTS",
                if permission {
                    "Write-Line '{\"id\":\"approval-1\",\"method\":\"item/commandExecution/requestApproval\",\"params\":{}}'"
                } else {
                    "Write-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"fake-thread\",\"turnId\":\"fake-turn\",\"item\":{\"type\":\"agentMessage\",\"text\":\"prose only\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"fake-thread\",\"turn\":{\"id\":\"fake-turn\",\"status\":\"completed\"}}}'"
                },
            ),
        )?;
        let launcher = dir.join("fake-codex.cmd");
        std::fs::write(
            &launcher,
            "@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0fake-codex.ps1\"\r\n",
        )?;
        Ok(launcher)
    }
}
