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

#[cfg(any(windows, test))]
use std::sync::atomic::AtomicU64;

#[cfg(any(windows, test))]
use std::borrow::Cow;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static DOORBELL_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(any(windows, test))]
static SCHEDULER_MANIFEST_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static DOORBELL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(all(test, windows))]
static FAKE_CODEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(all(test, windows))]
const FAKE_CODEX_STARTUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
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

#[cfg(any(windows, test))]
struct PreparedSchedulerManifest {
    manifest: SchedulerLaunchManifestV1,
    bytes: Vec<u8>,
    digest: String,
    path: PathBuf,
}

struct LoadedSchedulerManifest {
    #[cfg(any(windows, test))]
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
    let store = prospective_canonical_store_root(&store, must_exist)?;
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

fn prospective_canonical_store_root(store: &Path, must_exist: bool) -> anyhow::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut existing = store;
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("Edda store root has no existing ancestor")?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .context("Edda store root has no existing ancestor")?;
    }
    anyhow::ensure!(existing.is_dir(), "Edda store root must be a directory");
    anyhow::ensure!(
        !must_exist || missing.is_empty(),
        "Edda store root does not exist"
    );
    let mut canonical = existing
        .canonicalize()
        .with_context(|| format!("canonicalize Edda store root {}", existing.display()))?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
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
        #[cfg(any(windows, test))]
        manifest,
        repo,
        config,
    })
}

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
fn publish_scheduler_manifest(prepared: &PreparedSchedulerManifest) -> anyhow::Result<bool> {
    let directory = prepared
        .path
        .parent()
        .context("scheduler manifest path has no version directory")?;
    let launch_directory = directory
        .parent()
        .context("scheduler manifest path has no launch directory")?;
    let store = edda_store::store_root();
    anyhow::ensure!(
        scheduler_manifest_directory(&store, false)? == directory,
        "scheduler manifest path is outside the trusted Edda store directory"
    );
    let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
    anyhow::ensure!(
        scheduler_manifest_directory(&store, false)? == directory,
        "scheduler manifest directory changed during lock acquisition"
    );
    std::fs::create_dir_all(directory).with_context(|| {
        format!(
            "create scheduler manifest directory {}",
            directory.display()
        )
    })?;
    anyhow::ensure!(
        scheduler_manifest_directory(&store, true)? == directory,
        "scheduler manifest directory changed during publication"
    );

    if prepared.path.exists() {
        validate_existing_scheduler_manifest(prepared)?;
        return Ok(false);
    }

    let sequence =
        SCHEDULER_MANIFEST_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = directory.join(format!(
        ".{}.{}.{}.tmp",
        prepared.digest,
        std::process::id(),
        sequence
    ));
    edda_store::write_atomic(&temp, &prepared.bytes)
        .with_context(|| format!("write scheduler manifest temporary file {}", temp.display()))?;
    link_scheduler_manifest_noclobber(&temp, prepared)
}

#[cfg(any(windows, test))]
fn validate_existing_scheduler_manifest(
    prepared: &PreparedSchedulerManifest,
) -> anyhow::Result<()> {
    let loaded = load_scheduler_manifest(&prepared.path)?;
    anyhow::ensure!(
        loaded.manifest == prepared.manifest && std::fs::read(&prepared.path)? == prepared.bytes,
        "existing scheduler manifest does not contain the expected bytes"
    );
    Ok(())
}

#[cfg(any(windows, test))]
fn link_scheduler_manifest_noclobber(
    temp: &Path,
    prepared: &PreparedSchedulerManifest,
) -> anyhow::Result<bool> {
    match std::fs::hard_link(temp, &prepared.path) {
        Ok(()) => {
            std::fs::remove_file(temp).with_context(|| {
                format!(
                    "remove scheduler manifest temporary file {}",
                    temp.display()
                )
            })?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(temp).with_context(|| {
                format!(
                    "remove scheduler manifest temporary file {}",
                    temp.display()
                )
            })?;
            validate_existing_scheduler_manifest(prepared)?;
            Ok(false)
        }
        Err(error) => {
            let cleanup = std::fs::remove_file(temp);
            match cleanup {
                Ok(()) => Err(error).with_context(|| {
                    format!(
                        "atomically publish scheduler manifest {}",
                        prepared.path.display()
                    )
                }),
                Err(cleanup_error) => anyhow::bail!(
                    "atomically publish scheduler manifest {}: {error}; retain temporary file {} because cleanup failed: {cleanup_error}",
                    prepared.path.display(),
                    temp.display()
                ),
            }
        }
    }
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
#[cfg(any(windows, test))]
const SCHEDULER_OUTPUT_LIMIT: usize = 4096;

#[cfg(any(windows, test))]
#[derive(Debug, Eq, PartialEq)]
enum SchedulerTaskState {
    Present,
    Missing,
}

#[cfg(any(windows, test))]
#[derive(Debug, Eq, PartialEq)]
enum ManifestCleanupDecision {
    RemoveNewArtifact,
    Retain,
}

#[cfg(any(windows, test))]
struct SchedulerOutput {
    code: u32,
    stdout_raw: Vec<u8>,
    stdout: String,
    stderr: String,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

#[cfg(any(windows, test))]
impl SchedulerOutput {
    #[cfg(test)]
    fn for_test(code: u32, stdout: &str, stderr: &str) -> Self {
        Self::for_test_with_lengths(code, stdout, stderr, stdout.len(), stderr.len())
    }

    #[cfg(test)]
    fn for_test_with_lengths(
        code: u32,
        stdout: &str,
        stderr: &str,
        stdout_bytes: usize,
        stderr_bytes: usize,
    ) -> Self {
        Self::for_test_bytes_with_lengths(
            code,
            stdout.as_bytes(),
            stderr.as_bytes(),
            stdout_bytes,
            stderr_bytes,
        )
    }

    #[cfg(test)]
    fn for_test_bytes(code: u32, stdout: &[u8], stderr: &[u8]) -> Self {
        Self::for_test_bytes_with_lengths(code, stdout, stderr, stdout.len(), stderr.len())
    }

    #[cfg(test)]
    fn for_test_bytes_with_stdout_len(
        code: u32,
        stdout: &[u8],
        stderr: &[u8],
        stdout_bytes: usize,
    ) -> Self {
        Self::for_test_bytes_with_lengths(code, stdout, stderr, stdout_bytes, stderr.len())
    }

    #[cfg(test)]
    fn for_test_bytes_with_lengths(
        code: u32,
        stdout: &[u8],
        stderr: &[u8],
        stdout_bytes: usize,
        stderr_bytes: usize,
    ) -> Self {
        let stdout_raw = stdout[..stdout.len().min(SCHEDULER_OUTPUT_LIMIT)].to_vec();
        Self {
            code,
            stdout: String::from_utf8_lossy(&stdout_raw).into_owned(),
            stdout_raw,
            stderr: String::from_utf8_lossy(&stderr[..stderr.len().min(SCHEDULER_OUTPUT_LIMIT)])
                .into_owned(),
            stdout_bytes,
            stderr_bytes,
        }
    }

    fn xml(&self) -> anyhow::Result<Cow<'_, str>> {
        anyhow::ensure!(
            self.stdout_bytes <= SCHEDULER_OUTPUT_LIMIT,
            "scheduler Query XML is {} bytes; maximum bounded output is {}",
            self.stdout_bytes,
            SCHEDULER_OUTPUT_LIMIT
        );
        let bytes = self.stdout_raw.as_slice();
        anyhow::ensure!(
            !bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff])
                && !bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]),
            "scheduler Query XML uses an unsupported UTF-32 encoding"
        );
        let decode_utf16 = |encoded: &[u8], little_endian: bool| -> anyhow::Result<String> {
            anyhow::ensure!(
                encoded.len().is_multiple_of(2),
                "scheduler Query XML contains odd-length UTF-16"
            );
            let units = encoded
                .chunks_exact(2)
                .map(|bytes| {
                    if little_endian {
                        u16::from_le_bytes([bytes[0], bytes[1]])
                    } else {
                        u16::from_be_bytes([bytes[0], bytes[1]])
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&units).context("scheduler Query XML contains malformed UTF-16")
        };
        if let Some(encoded) = bytes.strip_prefix(&[0xff, 0xfe]) {
            return Ok(Cow::Owned(decode_utf16(encoded, true)?));
        }
        if let Some(encoded) = bytes.strip_prefix(&[0xfe, 0xff]) {
            return Ok(Cow::Owned(decode_utf16(encoded, false)?));
        }
        if bytes.starts_with(&[0x3c, 0x00, 0x3f, 0x00]) {
            return Ok(Cow::Owned(decode_utf16(bytes, true)?));
        }
        if bytes.starts_with(&[0x00, 0x3c, 0x00, 0x3f]) {
            return Ok(Cow::Owned(decode_utf16(bytes, false)?));
        }
        let utf8 = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        Ok(Cow::Borrowed(
            std::str::from_utf8(utf8).context("scheduler Query XML is not valid UTF-8")?,
        ))
    }

    fn description(&self) -> String {
        format!(
            "code=0x{:08x} ({}) stdout_bytes={} stderr_bytes={} stdout={:?} stderr={:?}",
            self.code,
            self.code as i32,
            self.stdout_bytes,
            self.stderr_bytes,
            self.stdout,
            self.stderr
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

#[cfg(any(windows, test))]
fn windows_manifest_path_components(path: &Path) -> anyhow::Result<(&str, &str)> {
    let value = path
        .to_str()
        .context("scheduler manifest path is not Unicode")?;
    let (parent, filename) = value
        .rsplit_once(['\\', '/'])
        .context("scheduler manifest path has no Windows parent")?;
    anyhow::ensure!(
        !parent.is_empty() && !filename.is_empty(),
        "scheduler manifest path has no Windows filename"
    );
    Ok((parent, filename))
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
fn decode_scheduler_xml_value(value: &str) -> anyhow::Result<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(ampersand) = remaining.find('&') {
        let literal = &remaining[..ampersand];
        anyhow::ensure!(
            !literal.contains('<'),
            "scheduler Query XML contains nested markup"
        );
        decoded.push_str(literal);
        let entity = &remaining[ampersand..];
        let semicolon = entity
            .find(';')
            .context("scheduler Query XML contains an unterminated entity")?;
        decoded.push(match &entity[..=semicolon] {
            "&amp;" => '&',
            "&lt;" => '<',
            "&gt;" => '>',
            "&quot;" => '"',
            "&apos;" => '\'',
            unknown => anyhow::bail!("scheduler Query XML contains unknown entity {unknown}"),
        });
        remaining = &entity[semicolon + 1..];
    }
    anyhow::ensure!(
        !remaining.contains('<'),
        "scheduler Query XML contains nested markup"
    );
    decoded.push_str(remaining);
    Ok(decoded)
}

#[cfg(any(windows, test))]
fn scheduler_command_matches_executable(command: &str, executable: &Path) -> anyhow::Result<bool> {
    let expected = executable
        .to_str()
        .context("scheduler executable is not Unicode")?;
    Ok(command == expected || command == quote_windows_argument(executable)?)
}

#[cfg(any(windows, test))]
fn scheduler_query_references_manifest(
    xml: &str,
    executable: &Path,
    manifest: &Path,
) -> anyhow::Result<bool> {
    let arguments = format!(
        "reconcile --scheduler-manifest {}",
        quote_windows_argument(manifest)?
    );
    let actions = scheduler_direct_exec_values(xml)?;
    let [(command, actual_arguments)] = actions.as_slice() else {
        return Ok(false);
    };
    Ok(
        scheduler_command_matches_executable(command, executable)?
            && actual_arguments == &arguments,
    )
}

#[cfg(any(windows, test))]
fn manifest_cleanup_decision(
    query: &SchedulerOutput,
    executable: &Path,
    expected_manifest: &Path,
) -> anyhow::Result<ManifestCleanupDecision> {
    match classify_scheduler_query(query)? {
        SchedulerTaskState::Missing => return Ok(ManifestCleanupDecision::RemoveNewArtifact),
        SchedulerTaskState::Present => {}
    }
    let xml = query.xml()?;
    let actions = scheduler_direct_exec_values(xml.as_ref())?;
    let expected_arguments = format!(
        "reconcile --scheduler-manifest {}",
        quote_windows_argument(expected_manifest)?
    );
    if let [(command, arguments)] = actions.as_slice() {
        if scheduler_command_matches_executable(command, executable)?
            && arguments == &expected_arguments
        {
            return Ok(ManifestCleanupDecision::Retain);
        }
    }
    anyhow::ensure!(
        !actions.is_empty(),
        "scheduler Query did not contain an Exec action: {}",
        query.description()
    );
    let (expected_parent, expected_filename) = windows_manifest_path_components(expected_manifest)?;

    for (command, arguments) in actions {
        anyhow::ensure!(
            scheduler_command_matches_executable(&command, executable)?,
            "scheduler Query command did not match the direct Edda executable: {}",
            query.description()
        );
        let path = arguments
            .strip_prefix("reconcile --scheduler-manifest \"")
            .and_then(|value| value.strip_suffix('"'))
            .context("scheduler Query Arguments were not a strict manifest command")?;
        anyhow::ensure!(
            !path.contains('"'),
            "scheduler Query manifest path contains a quote"
        );
        let candidate = Path::new(path);
        let (candidate_parent, candidate_filename) = windows_manifest_path_components(candidate)?;
        let trusted_filename = candidate_filename
            .strip_suffix(".json")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        anyhow::ensure!(
            windows_path_is_absolute(candidate)?
                && candidate_parent == expected_parent
                && candidate_filename != expected_filename
                && trusted_filename,
            "scheduler Query did not prove a different trusted manifest path: {}",
            query.description()
        );
    }
    Ok(ManifestCleanupDecision::RemoveNewArtifact)
}

#[cfg(any(windows, test))]
fn remove_unreferenced_scheduler_manifest(path: &Path) -> String {
    let removal = (|| -> anyhow::Result<()> {
        let launch_directory = path
            .parent()
            .and_then(Path::parent)
            .context("scheduler manifest path has no launch directory")?;
        let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
        std::fs::remove_file(path)
            .with_context(|| format!("remove unreferenced scheduler manifest {}", path.display()))
    })();
    match removal {
        Ok(()) => format!(
            "new scheduler manifest removed after exact-task Query proved it unreferenced: {}",
            path.display()
        ),
        Err(error) => format!(
            "new scheduler manifest retained because exact-file cleanup failed for {}: {error:#}",
            path.display()
        ),
    }
}

#[cfg(any(windows, test))]
fn scheduler_direct_exec_values(xml: &str) -> anyhow::Result<Vec<(String, String)>> {
    anyhow::ensure!(
        xml.len() <= SCHEDULER_OUTPUT_LIMIT,
        "scheduler Query XML exceeds the bounded output limit"
    );
    let valid_name = |name: &str| {
        let mut bytes = name.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':'))
            && bytes.all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'-')
            })
    };
    let mut cursor = 0;
    let mut stack: Vec<&str> = Vec::new();
    let mut seen_root = false;
    let mut seen_declaration = false;
    let mut actions_count = 0;
    let mut execs = Vec::new();
    let mut command = None;
    let mut arguments = None;
    let mut capture: Option<(&str, String)> = None;

    while cursor < xml.len() {
        let start = xml[cursor..]
            .find('<')
            .map(|offset| cursor + offset)
            .unwrap_or(xml.len());
        let text = &xml[cursor..start];
        if let Some((_, value)) = capture.as_mut() {
            value.push_str(text);
        } else if stack.is_empty() {
            anyhow::ensure!(
                text.trim().is_empty(),
                "scheduler Query XML has text outside the Task root"
            );
        }
        if start == xml.len() {
            cursor = start;
            break;
        }

        if xml[start..].starts_with("<!--") {
            let comment = &xml[start + "<!--".len()..];
            let end = comment
                .find("-->")
                .context("scheduler Query XML has an unterminated comment")?;
            anyhow::ensure!(
                !comment[..end].contains("--"),
                "scheduler Query XML has a malformed comment"
            );
            cursor = start + "<!--".len() + end + "-->".len();
            continue;
        }
        if xml[start..].starts_with("<?") {
            anyhow::ensure!(
                stack.is_empty()
                    && !seen_root
                    && !seen_declaration
                    && xml[start..].starts_with("<?xml"),
                "scheduler Query XML has an unsupported processing instruction"
            );
            let end = xml[start + 2..]
                .find("?>")
                .context("scheduler Query XML has an unterminated declaration")?;
            seen_declaration = true;
            cursor = start + 2 + end + 2;
            continue;
        }
        anyhow::ensure!(
            !xml[start..].starts_with("<!"),
            "scheduler Query XML has unsupported markup"
        );

        let mut quote = None;
        let mut end = None;
        for (offset, character) in xml[start + 1..].char_indices() {
            if let Some(expected) = quote {
                anyhow::ensure!(
                    character != '<',
                    "scheduler Query XML has malformed tag attributes"
                );
                if character == expected {
                    quote = None;
                }
            } else {
                match character {
                    '\'' | '"' => quote = Some(character),
                    '>' => {
                        end = Some(start + 1 + offset);
                        break;
                    }
                    '<' => anyhow::bail!("scheduler Query XML has a malformed tag"),
                    _ => {}
                }
            }
        }
        let end = end.context("scheduler Query XML has an unterminated tag")?;
        let raw = xml[start + 1..end].trim();
        cursor = end + 1;

        if let Some(closing) = raw.strip_prefix('/') {
            let name = closing.trim();
            anyhow::ensure!(
                valid_name(name) && name.len() == closing.len(),
                "scheduler Query XML has a malformed closing tag"
            );
            anyhow::ensure!(
                stack.last() == Some(&name),
                "scheduler Query XML has mismatched element nesting"
            );
            if matches!(name, "Command" | "Arguments") {
                let (kind, value) = capture
                    .take()
                    .context("scheduler Exec value did not close directly")?;
                anyhow::ensure!(kind == name, "scheduler Exec values closed out of order");
                let decoded = decode_scheduler_xml_value(&value)?;
                if name == "Command" {
                    command = Some(decoded);
                } else {
                    arguments = Some(decoded);
                }
            } else if name == "Exec" && stack.as_slice() == ["Task", "Actions", "Exec"] {
                execs.push((
                    command
                        .take()
                        .context("scheduler Exec action has no Command")?,
                    arguments
                        .take()
                        .context("scheduler Exec action has no Arguments")?,
                ));
            }
            stack.pop();
            continue;
        }

        let (open, self_closing) = raw
            .strip_suffix('/')
            .map_or((raw, false), |open| (open.trim_end(), true));
        let name_end = open.find(char::is_whitespace).unwrap_or(open.len());
        let name = &open[..name_end];
        anyhow::ensure!(
            valid_name(name),
            "scheduler Query XML has a malformed element name"
        );
        let mut attributes = &open[name_end..];
        let mut attribute_names = Vec::new();
        loop {
            attributes = attributes.trim_start();
            if attributes.is_empty() {
                break;
            }
            let name_end = attributes
                .find(|character: char| character.is_whitespace() || character == '=')
                .unwrap_or(attributes.len());
            let attribute_name = &attributes[..name_end];
            anyhow::ensure!(
                valid_name(attribute_name) && !attribute_names.contains(&attribute_name),
                "scheduler Query XML has a malformed or duplicate attribute"
            );
            attribute_names.push(attribute_name);
            attributes = attributes[name_end..].trim_start();
            attributes = attributes
                .strip_prefix('=')
                .context("scheduler Query XML attribute has no equals sign")?
                .trim_start();
            let delimiter = attributes
                .chars()
                .next()
                .filter(|character| matches!(character, '\'' | '"'))
                .context("scheduler Query XML attribute value is not quoted")?;
            attributes = &attributes[delimiter.len_utf8()..];
            let value_end = attributes
                .find(delimiter)
                .context("scheduler Query XML attribute value is unterminated")?;
            anyhow::ensure!(
                !attributes[..value_end].contains('<'),
                "scheduler Query XML attribute value contains markup"
            );
            attributes = &attributes[value_end + delimiter.len_utf8()..];
            anyhow::ensure!(
                attributes.is_empty() || attributes.chars().next().is_some_and(char::is_whitespace),
                "scheduler Query XML attributes are not separated"
            );
        }

        anyhow::ensure!(
            capture.is_none(),
            "scheduler Exec value contains nested markup"
        );
        if stack.is_empty() {
            anyhow::ensure!(
                !seen_root && name == "Task" && !self_closing,
                "scheduler Query XML does not have one complete Task root"
            );
            seen_root = true;
        }
        if name == "Actions" {
            anyhow::ensure!(
                stack.as_slice() == ["Task"] && !self_closing,
                "scheduler Actions container is not a direct Task child"
            );
            actions_count += 1;
        } else if name == "Exec" {
            anyhow::ensure!(
                stack.as_slice() == ["Task", "Actions"] && !self_closing,
                "scheduler Exec action is not a direct Actions child"
            );
            command = None;
            arguments = None;
        } else if matches!(name, "Command" | "Arguments") {
            anyhow::ensure!(
                stack.as_slice() == ["Task", "Actions", "Exec"],
                "scheduler Exec value is not a direct Exec child"
            );
            let value = if name == "Command" {
                &mut command
            } else {
                &mut arguments
            };
            anyhow::ensure!(
                value.is_none(),
                "scheduler Exec action has a duplicate {name}"
            );
            if self_closing {
                *value = Some(String::new());
            } else {
                capture = Some((name, String::new()));
            }
        }
        if !self_closing {
            stack.push(name);
        }
    }

    anyhow::ensure!(
        cursor == xml.len(),
        "scheduler Query XML was not fully scanned"
    );
    anyhow::ensure!(
        seen_root && stack.is_empty() && capture.is_none(),
        "scheduler Query XML has incomplete element nesting"
    );
    anyhow::ensure!(
        actions_count == 1,
        "scheduler Query XML must have exactly one direct Actions container"
    );
    Ok(execs)
}

#[cfg(any(windows, test))]
fn recover_scheduler_manifest_candidate(
    xml: &str,
    executable: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let actions = scheduler_direct_exec_values(xml)?;
    let [(command, arguments)] = actions.as_slice() else {
        return Ok(None);
    };
    if !scheduler_command_matches_executable(command, executable)? {
        return Ok(None);
    }
    let Some(value) = arguments
        .strip_prefix("reconcile --scheduler-manifest \"")
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Ok(None);
    };
    let candidate = PathBuf::from(value);
    if !(candidate.is_absolute() || windows_path_is_absolute(&candidate)?)
        || format!(
            "reconcile --scheduler-manifest {}",
            quote_windows_argument(&candidate)?
        ) != *arguments
    {
        return Ok(None);
    }
    Ok(Some(candidate))
}

#[cfg(any(windows, test))]
fn claim_and_remove_scheduler_manifest_under_lock(
    path: &Path,
    quarantine: &Path,
    expected_bytes: &[u8],
    repo: &Path,
    project_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.parent() == quarantine.parent(),
        "scheduler manifest quarantine must be in the same directory"
    );
    anyhow::ensure!(
        !quarantine.exists(),
        "scheduler manifest quarantine already exists"
    );
    std::fs::rename(path, quarantine).with_context(|| {
        format!(
            "atomically claim scheduler manifest {} as {}",
            path.display(),
            quarantine.display()
        )
    })?;

    let revalidation = (|| -> anyhow::Result<()> {
        let trusted_directory = scheduler_manifest_directory(&edda_store::store_root(), true)?;
        let source_metadata = quarantine.symlink_metadata().with_context(|| {
            format!(
                "inspect scheduler manifest quarantine {}",
                quarantine.display()
            )
        })?;
        anyhow::ensure!(
            source_metadata.file_type().is_file(),
            "scheduler manifest quarantine must be a regular file"
        );
        anyhow::ensure!(
            source_metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
            "scheduler manifest quarantine exceeds 16 KiB"
        );
        let canonical = quarantine.canonicalize().with_context(|| {
            format!(
                "canonicalize scheduler manifest quarantine {}",
                quarantine.display()
            )
        })?;
        let parent = quarantine
            .parent()
            .context("scheduler manifest quarantine has no parent")?
            .canonicalize()
            .context("canonicalize scheduler manifest quarantine parent")?;
        anyhow::ensure!(
            parent == trusted_directory && canonical.parent() == Some(trusted_directory.as_path()),
            "scheduler manifest quarantine is outside the trusted Edda store directory"
        );
        let bytes = std::fs::read(&canonical).with_context(|| {
            format!("read scheduler manifest quarantine {}", canonical.display())
        })?;
        anyhow::ensure!(
            bytes.len() as u64 <= SCHEDULER_MANIFEST_MAX_BYTES,
            "scheduler manifest quarantine exceeds 16 KiB"
        );
        anyhow::ensure!(
            bytes == expected_bytes,
            "scheduler manifest entry changed before the atomic quarantine claim"
        );
        let manifest: SchedulerLaunchManifestV1 = serde_json::from_slice(&bytes)
            .context("parse quarantined scheduler launch manifest")?;
        anyhow::ensure!(
            serde_json::to_vec(&manifest)? == bytes,
            "scheduler manifest quarantine JSON is not canonical"
        );
        let loaded = validate_scheduler_manifest(manifest)?;
        anyhow::ensure!(
            loaded.repo == repo && loaded.manifest.project_id == project_id,
            "scheduler manifest quarantine does not belong to the exact task project"
        );
        Ok(())
    })();
    if let Err(error) = revalidation {
        anyhow::bail!(
            "retain quarantine {} because the claimed scheduler manifest failed revalidation: {error:#}",
            quarantine.display()
        );
    }
    std::fs::remove_file(quarantine).with_context(|| {
        format!(
            "retain quarantine {} because exact-file removal failed",
            quarantine.display()
        )
    })
}

#[cfg(any(windows, test))]
fn remove_trusted_scheduler_manifest(path: &Path, repo: &Path, project_id: &str) -> String {
    let removal = (|| -> anyhow::Result<()> {
        let store = edda_store::store_root();
        let directory = scheduler_manifest_directory(&store, true)?;
        let launch_directory = directory
            .parent()
            .context("scheduler manifest path has no launch directory")?;
        let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
        anyhow::ensure!(
            scheduler_manifest_directory(&store, true)? == directory,
            "scheduler manifest directory changed during uninstall"
        );
        let loaded = load_scheduler_manifest(path)?;
        anyhow::ensure!(
            loaded.repo == repo && loaded.manifest.project_id == project_id,
            "scheduler manifest does not belong to the exact task project"
        );
        let expected_bytes = serde_json::to_vec(&loaded.manifest)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("scheduler manifest filename is not Unicode")?;
        let sequence =
            SCHEDULER_MANIFEST_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let quarantine = directory.join(format!(
            ".{filename}.{}.{}.uninstall-quarantine",
            std::process::id(),
            sequence
        ));
        claim_and_remove_scheduler_manifest_under_lock(
            path,
            &quarantine,
            &expected_bytes,
            repo,
            project_id,
        )
    })();
    match removal {
        Ok(()) => format!(
            "removed trusted scheduler manifest after exact-task absence proof: {}",
            path.display()
        ),
        Err(error) => format!(
            "scheduler manifest retained because exact-file validation or removal failed for {}: {error:#}",
            path.display()
        ),
    }
}

#[cfg(any(windows, test))]
fn uninstall_scheduler_task_with(
    repo: &Path,
    executable: &Path,
    project_id: &str,
    mut run: impl FnMut(&[String]) -> anyhow::Result<SchedulerOutput>,
) -> anyhow::Result<String> {
    let (task_name, query_args, delete_args) = windows_scheduler_management_args(project_id)?;
    let before =
        run(&query_args).with_context(|| format!("scheduler Query failed for task {task_name}"))?;
    if classify_scheduler_query(&before)
        .with_context(|| format!("scheduler Query failed for task {task_name}"))?
        == SchedulerTaskState::Missing
    {
        return Ok(format!(
            "scheduler task {} already absent for {}",
            task_name,
            repo.display()
        ));
    }
    let candidate = before
        .xml()
        .and_then(|xml| recover_scheduler_manifest_candidate(xml.as_ref(), executable));
    let deleted = run(&delete_args)
        .with_context(|| format!("scheduler Delete failed for task {task_name}"))?;
    anyhow::ensure!(
        deleted.code == 0 || deleted.code == MISSING_TASK_HRESULT,
        "scheduler Delete failed for {}: {}",
        task_name,
        deleted.description()
    );
    let after =
        run(&query_args).with_context(|| format!("scheduler Query failed for task {task_name}"))?;
    require_scheduler_state(
        &after,
        SchedulerTaskState::Missing,
        "post-Delete Query",
        &task_name,
    )?;
    let cleanup = match candidate {
        Ok(Some(path)) => remove_trusted_scheduler_manifest(&path, repo, project_id),
        Ok(None) => "scheduler manifest retained because the exact task did not prove one strict direct manifest command".into(),
        Err(error) => format!(
            "scheduler manifest retained because the pre-Delete Query was not trustworthy: {error:#}"
        ),
    };
    Ok(format!(
        "uninstalled scheduler task {} for {}; {}",
        task_name,
        repo.display(),
        cleanup
    ))
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
    let bounded = |bytes: &[u8]| bytes[..bytes.len().min(SCHEDULER_OUTPUT_LIMIT)].to_vec();
    let stdout_raw = bounded(&output.stdout);
    let stderr_raw = bounded(&output.stderr);
    Ok(SchedulerOutput {
        code: signed_code as u32,
        stdout: String::from_utf8_lossy(&stdout_raw).into_owned(),
        stdout_raw,
        stderr: String::from_utf8_lossy(&stderr_raw).into_owned(),
        stdout_bytes: output.stdout.len(),
        stderr_bytes: output.stderr.len(),
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
        if let Some(config) = install_config {
            let executable = std::env::current_exe()?.canonicalize()?;
            let mut config = config.clone();
            config.codex_bin = canonical_direct_codex_executable(&config.codex_bin, None)?;
            let manifest = prepare_scheduler_manifest(&edda_store::store_root(), &repo, &config)?;
            let spec = windows_scheduler_spec(&executable, &manifest.path, &project_id)?;
            let artifact_created = publish_scheduler_manifest(&manifest)?;
            let install = (|| -> anyhow::Result<()> {
                let created = run_schtasks(&spec.create_args).with_context(|| {
                    format!("scheduler Create failed for task {}", spec.task_name)
                })?;
                anyhow::ensure!(
                    created.code == 0,
                    "scheduler Create failed for {}: {}",
                    spec.task_name,
                    created.description()
                );
                let queried = run_schtasks(&spec.query_args).with_context(|| {
                    format!(
                        "scheduler post-Create Query failed for task {}",
                        spec.task_name
                    )
                })?;
                require_scheduler_state(
                    &queried,
                    SchedulerTaskState::Present,
                    "post-Create Query",
                    &spec.task_name,
                )?;
                anyhow::ensure!(
                    scheduler_query_references_manifest(
                        queried.xml()?.as_ref(),
                        &executable,
                        &manifest.path,
                    )?,
                    "scheduler post-Create Query returned a different command for task {}: {}",
                    spec.task_name,
                    queried.description()
                );
                Ok(())
            })();
            if let Err(install_error) = install {
                let cleanup = match run_schtasks(&spec.query_args) {
                    Ok(query) if !artifact_created => format!(
                        "existing scheduler manifest retained after exact-task cleanup Query: {}",
                        query.description()
                    ),
                    Ok(query) => {
                        match manifest_cleanup_decision(&query, &executable, &manifest.path) {
                            Ok(ManifestCleanupDecision::RemoveNewArtifact) => {
                                remove_unreferenced_scheduler_manifest(&manifest.path)
                            }
                            Ok(ManifestCleanupDecision::Retain) => format!(
                                "new scheduler manifest retained because the exact task references it: {}",
                                query.description()
                            ),
                            Err(error) => format!(
                                "new scheduler manifest retained because cleanup was uncertain: {error:#}"
                            ),
                        }
                    }
                    Err(error) => format!(
                        "{} scheduler manifest retained because cleanup Query failed for task {}: {error:#}",
                        if artifact_created { "new" } else { "existing" }, spec.task_name
                    ),
                };
                anyhow::bail!("{install_error:#}; {cleanup}");
            }
            println!(
                "installed scheduler task {} for {}",
                spec.task_name,
                repo.display()
            );
            return Ok(());
        }

        let executable = std::env::current_exe()?.canonicalize()?;
        println!(
            "{}",
            uninstall_scheduler_task_with(&repo, &executable, &project_id, run_schtasks)?
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

    static CODEX_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct CodexBinEnvGuard {
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for CodexBinEnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("EDDA_CODEX_BIN", value),
                None => std::env::remove_var("EDDA_CODEX_BIN"),
            }
        }
    }

    fn codex_bin_env_guard(value: &str) -> CodexBinEnvGuard {
        let lock = test_lock(&CODEX_BIN_ENV_LOCK);
        let previous = std::env::var_os("EDDA_CODEX_BIN");
        std::env::set_var("EDDA_CODEX_BIN", value);
        CodexBinEnvGuard {
            previous,
            _lock: lock,
        }
    }

    fn codex_bin_env() -> Option<std::ffi::OsString> {
        let _lock = test_lock(&CODEX_BIN_ENV_LOCK);
        std::env::var_os("EDDA_CODEX_BIN")
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

    fn scheduler_xml_utf16_bytes(xml: &str, little_endian: bool, bom: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(if little_endian {
                &[0xff, 0xfe]
            } else {
                &[0xfe, 0xff]
            });
        }
        for unit in xml.encode_utf16() {
            let encoded = if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            };
            bytes.extend_from_slice(&encoded);
        }
        bytes
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
    fn scheduler_manifest_publish_reuses_identical_bytes_without_replacing() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

        assert!(publish_scheduler_manifest(&prepared)?);
        assert!(!publish_scheduler_manifest(&prepared)?);
        assert_eq!(std::fs::read(&prepared.path)?, prepared.bytes);

        std::fs::write(&prepared.path, b"different")?;
        assert!(publish_scheduler_manifest(&prepared).is_err());
        assert_eq!(std::fs::read(&prepared.path)?, b"different");
        Ok(())
    }

    #[test]
    fn scheduler_manifest_first_publish_from_absent_store_root_is_loadable() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        std::fs::remove_dir_all(&fixture.store)?;
        assert!(!fixture.store.exists());

        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        assert!(publish_scheduler_manifest(&prepared)?);
        assert_eq!(
            prepared.path.parent(),
            Some(scheduler_manifest_directory(&fixture.store, true)?.as_path())
        );
        assert_eq!(
            load_scheduler_manifest(&prepared.path)?.manifest,
            prepared.manifest
        );
        assert!(!publish_scheduler_manifest(&prepared)?);
        Ok(())
    }

    #[test]
    fn scheduler_manifest_atomic_link_never_replaces_a_racer() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let directory = prepared.path.parent().context("manifest directory")?;
        std::fs::create_dir_all(directory)?;
        let temp = directory.join("race.tmp");
        edda_store::write_atomic(&temp, &prepared.bytes)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;

        assert!(!link_scheduler_manifest_noclobber(&temp, &prepared)?);
        assert!(!temp.exists());
        assert_eq!(std::fs::read(&prepared.path)?, prepared.bytes);

        std::fs::write(&prepared.path, b"racer bytes")?;
        edda_store::write_atomic(&temp, &prepared.bytes)?;
        assert!(link_scheduler_manifest_noclobber(&temp, &prepared).is_err());
        assert!(!temp.exists());
        assert_eq!(std::fs::read(&prepared.path)?, b"racer bytes");
        Ok(())
    }

    #[test]
    fn scheduler_manifest_changed_install_retains_prior_artifact() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let old = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        assert!(publish_scheduler_manifest(&old)?);

        let mut changed = fixture.config.clone();
        changed.max_attempts += 1;
        let new = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &changed)?;
        assert_ne!(old.path, new.path);
        assert!(publish_scheduler_manifest(&new)?);
        assert_eq!(std::fs::read(&old.path)?, old.bytes);
        assert_eq!(std::fs::read(&new.path)?, new.bytes);
        Ok(())
    }

    #[test]
    fn scheduler_manifest_write_failure_precedes_scheduler_cleanup() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        std::fs::create_dir_all(fixture.store.join("scheduler-launch"))?;
        std::fs::write(
            fixture.store.join("scheduler-launch").join("v1"),
            b"blocked",
        )?;

        assert!(publish_scheduler_manifest(&prepared).is_err());
        assert!(!prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_manifest_publish_validates_containment_before_mutation() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let mut prepared =
            prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let outside = fixture._root.path().join("outside-store");
        prepared.path = outside
            .join("scheduler-launch")
            .join("v1")
            .join(format!("{}.json", prepared.digest));

        assert!(publish_scheduler_manifest(&prepared).is_err());
        assert!(!outside.exists());
        Ok(())
    }

    #[test]
    fn scheduler_query_xml_requires_exact_escaped_command_and_arguments() -> anyhow::Result<()> {
        let executable = Path::new(r"C:\Program Files\Edda & Co\edda.exe");
        let manifest = Path::new(r"C:\Store & State\scheduler-launch\v1\expected.json");
        let xml = r#"<Task><Actions><Exec><Command>C:\Program Files\Edda &amp; Co\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\Store &amp; State\scheduler-launch\v1\expected.json&quot;</Arguments></Exec></Actions></Task>"#;
        assert!(scheduler_query_references_manifest(
            xml, executable, manifest
        )?);

        let wrong_arguments = xml.replace("expected.json&quot;", "expected.json&quot; --extra");
        assert!(!scheduler_query_references_manifest(
            &wrong_arguments,
            executable,
            manifest
        )?);
        let wrong_command = xml.replace("edda.exe</Command>", "other.exe</Command>");
        assert!(!scheduler_query_references_manifest(
            &wrong_command,
            executable,
            manifest
        )?);
        Ok(())
    }

    #[test]
    fn scheduler_query_accepts_exact_windows_quoted_executable_only() -> anyhow::Result<()> {
        let executable = Path::new(r"\\?\C:\Program Files\Edda\edda.exe");
        let manifest = Path::new(r"C:\Store\scheduler-launch\v1\expected.json");
        let arguments = r#"reconcile --scheduler-manifest &quot;C:\Store\scheduler-launch\v1\expected.json&quot;"#;
        let xml = |command: &str| {
            format!(
                "<Task><Actions><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments></Exec></Actions></Task>"
            )
        };

        let quoted = xml(r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#);
        assert!(scheduler_query_references_manifest(
            &quoted, executable, manifest
        )?);
        assert_eq!(
            recover_scheduler_manifest_candidate(&quoted, executable)?,
            Some(manifest.to_path_buf())
        );
        assert_eq!(
            manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, &quoted, ""),
                executable,
                manifest,
            )?,
            ManifestCleanupDecision::Retain
        );

        let literal_quoted = xml(r#""\\?\C:\Program Files\Edda\edda.exe""#);
        assert!(scheduler_query_references_manifest(
            &literal_quoted,
            executable,
            manifest,
        )?);

        for rejected in [
            r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; --extra"#,
            r#"&quot;\\?\C:\Program Files\Edda\edda.exe"#,
            r#"&quot;&quot;\\?\C:\Program Files\Edda\edda.exe&quot;&quot;"#,
            r#" &quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#,
            r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; "#,
            r#"&quot;C:\other\edda.exe&quot;"#,
            r#"cmd.exe /c &quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#,
            r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; &quot;C:\other.exe&quot;"#,
        ] {
            let rejected = xml(rejected);
            assert!(!scheduler_query_references_manifest(
                &rejected, executable, manifest
            )?);
            assert_eq!(
                recover_scheduler_manifest_candidate(&rejected, executable)?,
                None
            );
            assert!(manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, &rejected, ""),
                executable,
                manifest,
            )
            .is_err());
        }

        let entity_trick = xml(r#"&#34;\\?\C:\Program Files\Edda\edda.exe&#34;"#);
        assert!(scheduler_query_references_manifest(&entity_trick, executable, manifest).is_err());
        assert!(recover_scheduler_manifest_candidate(&entity_trick, executable).is_err());
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &entity_trick, ""),
            executable,
            manifest,
        )
        .is_err());

        let extra_exec = quoted.replace(
            "</Actions>",
            "<Exec><Command>cmd.exe</Command><Arguments>/c exit</Arguments></Exec></Actions>",
        );
        assert!(!scheduler_query_references_manifest(
            &extra_exec,
            executable,
            manifest,
        )?);
        assert_eq!(
            recover_scheduler_manifest_candidate(&extra_exec, executable)?,
            None
        );
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &extra_exec, ""),
            executable,
            manifest,
        )
        .is_err());

        let commented_match = quoted.replace("<Exec>", "<!-- <Exec>").replace(
            "</Exec>",
            "</Exec> --><Exec><Command>cmd.exe</Command><Arguments>/c exit</Arguments></Exec>",
        );
        assert!(
            scheduler_query_references_manifest(&commented_match, executable, manifest).is_err()
        );
        assert!(recover_scheduler_manifest_candidate(&commented_match, executable).is_err());
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &commented_match, ""),
            executable,
            manifest,
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn windows_manifest_path_components_are_host_neutral() -> anyhow::Result<()> {
        assert_eq!(
            windows_manifest_path_components(Path::new(
                r"C:\store\scheduler-launch\v1\manifest.json"
            ))?,
            (r"C:\store\scheduler-launch\v1", "manifest.json")
        );
        assert_eq!(
            windows_manifest_path_components(Path::new(
                r"\\?\C:\store\scheduler-launch\v1\manifest.json"
            ))?,
            (r"\\?\C:\store\scheduler-launch\v1", "manifest.json")
        );
        assert_eq!(
            windows_manifest_path_components(Path::new(r"\\?\UNC\server\share\v1\manifest.json"))?,
            (r"\\?\UNC\server\share\v1", "manifest.json")
        );
        assert_eq!(
            windows_manifest_path_components(Path::new(
                "C:/store/scheduler-launch/v1/manifest.json"
            ))?,
            ("C:/store/scheduler-launch/v1", "manifest.json")
        );
        assert!(windows_manifest_path_components(Path::new(r"C:\")).is_err());
        Ok(())
    }

    #[test]
    fn scheduler_query_ignores_execution_time_limit_setting() -> anyhow::Result<()> {
        let executable = Path::new(r"C:\Program Files\Edda\edda.exe");
        let manifest = Path::new(
            r"C:\Store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Settings>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>C:\Program Files\Edda\edda.exe</Command>
      <Arguments>reconcile --scheduler-manifest &quot;C:\Store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments>
    </Exec>
  </Actions>
</Task>"#;

        assert!(scheduler_query_references_manifest(
            xml, executable, manifest
        )?);
        assert_eq!(
            recover_scheduler_manifest_candidate(xml, executable)?,
            Some(manifest.to_path_buf())
        );
        assert_eq!(
            manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, xml, ""),
                executable,
                manifest,
            )?,
            ManifestCleanupDecision::Retain
        );
        Ok(())
    }

    #[test]
    fn scheduler_query_scans_every_exec_before_deciding() {
        let executable = Path::new(r"C:\edda\edda.exe");
        let expected = Path::new(
            r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let expected_xml = r#"<Task><Actions><Exec><Command>C:\edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec><Exec><Command>truncated"#;
        assert!(scheduler_query_references_manifest(expected_xml, executable, expected).is_err());
        let cut_open = format!(
            "{}<Exec",
            expected_xml.trim_end_matches("<Exec><Command>truncated")
        );
        assert!(scheduler_query_references_manifest(&cut_open, executable, expected).is_err());
        let self_closing = format!(
            "{}<Exec/>",
            expected_xml.trim_end_matches("<Exec><Command>truncated")
        );
        assert!(scheduler_query_references_manifest(&self_closing, executable, expected).is_err());

        let different_xml = expected_xml.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
        );
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &different_xml, ""),
            executable,
            expected,
        )
        .is_err());
        let different_cut_open = cut_open.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
        );
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &different_cut_open, ""),
            executable,
            expected,
        )
        .is_err());
    }

    #[test]
    fn scheduler_query_compares_decoded_xml_element_values() -> anyhow::Result<()> {
        let executable = Path::new(r"C:\O'Brien & Sons\edda.exe");
        let expected = Path::new(
            r"C:\Store & State\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let literal_xml = r#"<Task><Actions><Exec><Command>C:\O'Brien &amp; Sons\edda.exe</Command><Arguments>reconcile --scheduler-manifest "C:\Store &amp; State\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json"</Arguments></Exec></Actions></Task>"#;
        assert!(scheduler_query_references_manifest(
            literal_xml,
            executable,
            expected,
        )?);

        let named_xml = literal_xml
            .replace("O'Brien", "O&apos;Brien")
            .replace('"', "&quot;");
        assert!(scheduler_query_references_manifest(
            &named_xml, executable, expected,
        )?);

        let unknown_entity = literal_xml.replace("&amp;", "&unknown;");
        assert!(
            scheduler_query_references_manifest(&unknown_entity, executable, expected).is_err()
        );

        let different = literal_xml.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
        );
        assert_eq!(
            manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, &different, ""),
                executable,
                expected,
            )?,
            ManifestCleanupDecision::RemoveNewArtifact
        );
        Ok(())
    }

    #[test]
    fn scheduler_manifest_cleanup_requires_proved_non_reference() -> anyhow::Result<()> {
        let executable = Path::new(r"C:\edda\edda.exe");
        let expected = Path::new(
            r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing");
        assert_eq!(
            manifest_cleanup_decision(&missing, executable, expected)?,
            ManifestCleanupDecision::RemoveNewArtifact
        );

        let expected_xml = r#"<Task><Actions><Exec><Command>C:\edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec></Actions></Task>"#;
        assert_eq!(
            manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, expected_xml, ""),
                executable,
                expected,
            )?,
            ManifestCleanupDecision::Retain
        );

        let previous_xml = expected_xml.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
        );
        assert_eq!(
            manifest_cleanup_decision(
                &SchedulerOutput::for_test(0, &previous_xml, ""),
                executable,
                expected,
            )?,
            ManifestCleanupDecision::RemoveNewArtifact
        );

        let aliased_expected_xml = expected_xml.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "&#97;aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let non_content_addressed_xml = expected_xml.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
            "previous.json",
        );
        let different_directory_xml = previous_xml.replace(
            r"C:\store\scheduler-launch\v1",
            r"C:\other\scheduler-launch\v1",
        );
        for uncertain in [
            SchedulerOutput::for_test(5, "", "access denied"),
            SchedulerOutput::for_test(0, "<Task />", ""),
            SchedulerOutput::for_test(0, &aliased_expected_xml, ""),
            SchedulerOutput::for_test(0, &non_content_addressed_xml, ""),
            SchedulerOutput::for_test(0, &different_directory_xml, ""),
            SchedulerOutput::for_test(
                0,
                r#"<Task><Actions><Exec><Command>C:\other.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json&quot;</Arguments></Exec></Actions></Task>"#,
                "",
            ),
        ] {
            assert!(manifest_cleanup_decision(&uncertain, executable, expected).is_err());
        }
        Ok(())
    }

    #[test]
    fn scheduler_manifest_cleanup_failure_retains_original_error_first() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = root
            .path()
            .join("scheduler-launch")
            .join("v1")
            .join(format!("{}.json", "a".repeat(64)));
        std::fs::create_dir_all(&manifest)?;

        let cleanup = remove_unreferenced_scheduler_manifest(&manifest);
        let combined = format!("scheduler Create failed; {cleanup}");
        assert!(manifest.is_dir());
        assert!(cleanup.contains("retained"));
        assert!(combined.starts_with("scheduler Create failed;"));
        Ok(())
    }

    #[test]
    fn scheduler_preflight_failure_creates_no_manifest_directory() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let _prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        let rejected_path = manifest_path_for_task_run_utf16_len(262);

        assert!(render_scheduler_task_run(
            Path::new(r"C:\e.exe"),
            &rejected_path,
            "Edda-Reconcile-0123456789abcdef0123456789abcdef",
        )
        .is_err());
        assert!(!fixture.store.join("scheduler-launch").exists());
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
        let _environment = codex_bin_env_guard(r"C:\environment\codex.exe");

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
    }

    #[test]
    fn scheduler_codex_environment_guard_restores_after_unwind() {
        let previous = codex_bin_env();
        let result = std::panic::catch_unwind(|| {
            let _environment = codex_bin_env_guard(r"C:\environment\codex.exe");
            panic!("test unwind");
        });

        assert!(result.is_err());
        assert_eq!(codex_bin_env(), previous);
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

    fn scheduler_manifest_xml(executable: &Path, manifest: &Path) -> anyhow::Result<String> {
        let escape = |value: &str| {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        };
        Ok(format!(
            "<Task><Actions><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions></Task>",
            escape(executable.to_str().context("test executable path")?),
            escape(&format!(
                "reconcile --scheduler-manifest {}",
                quote_windows_argument(manifest)?
            )),
        ))
    }

    #[test]
    fn scheduler_uninstall_removes_only_a_trusted_exact_manifest_after_absence(
    ) -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let (_, query_args, delete_args) = windows_scheduler_management_args(&project_id)?;
        let xml = format!(
            "<?xml version=\"1.0\"?>{}",
            scheduler_manifest_xml(&fixture.codex, &prepared.path)?
                .replace("<Actions>", "<!-- harmless scheduler comment --><Actions>")
        );
        let outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ];
        let mut calls = Vec::new();
        let mut outputs = outputs.into_iter();

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |args| {
            assert!(
                prepared.path.exists(),
                "artifact removed before absence proof"
            );
            calls.push(args.to_vec());
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, [query_args.clone(), delete_args, query_args]);
        assert!(!prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_structural_xml_ignores_commented_matching_action() -> anyhow::Result<()>
    {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let xml = format!(
            "<?xml version=\"1.0\"?>{}",
            scheduler_manifest_xml(&fixture.codex, &prepared.path)?
                .replace("<Actions>", "<!-- <Actions>")
                .replace("</Actions>", "</Actions> -->")
        );
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let mut calls = 0;

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            calls += 1;
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, 3);
        assert!(prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_structural_xml_rejects_malformed_unrelated_nesting() -> anyhow::Result<()>
    {
        for wrap in [
            "<Settings><Unclosed></Settings>{actions}",
            "<Unclosed>{actions}",
        ] {
            let fixture = scheduler_manifest_fixture()?;
            let prepared =
                prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
            edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
            let complete = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
            let actions = complete
                .strip_prefix("<Task>")
                .and_then(|xml| xml.strip_suffix("</Task>"))
                .context("test scheduler XML Task wrapper")?;
            let xml = format!("<Task>{}</Task>", wrap.replace("{actions}", actions));
            let project_id = edda_store::project_id_for_root(&fixture.repo);
            let mut outputs = [
                SchedulerOutput::for_test(0, &xml, ""),
                SchedulerOutput::for_test(0, "", ""),
                SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
            ]
            .into_iter();
            let mut calls = 0;

            uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
                calls += 1;
                outputs.next().context("unexpected scheduler call")
            })?;

            assert_eq!(calls, 3);
            assert!(
                prepared.path.exists(),
                "wrapper {wrap:?} authorized cleanup"
            );
        }
        Ok(())
    }

    #[test]
    fn scheduler_manifest_quarantine_revalidates_the_claimed_entry_before_removal(
    ) -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let loaded = load_scheduler_manifest(&prepared.path)?;
        let expected_bytes = serde_json::to_vec(&loaded.manifest)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let quarantine = prepared.path.with_file_name("swap-test.quarantine");
        std::fs::write(&prepared.path, b"replacement")?;

        let error = claim_and_remove_scheduler_manifest_under_lock(
            &prepared.path,
            &quarantine,
            &expected_bytes,
            &fixture.repo,
            &project_id,
        )
        .expect_err("a replacement must be retained after the atomic claim")
        .to_string();

        assert!(!prepared.path.exists());
        assert!(quarantine.exists());
        assert_eq!(std::fs::read(&quarantine)?, b"replacement");
        assert!(error.contains("retain quarantine"));
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_malformed_query_retains_artifacts_but_removes_task() -> anyhow::Result<()>
    {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, "<Task><Exec", ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let mut calls = 0;

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            calls += 1;
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, 3);
        assert!(prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_truncated_outer_xml_retains_manifest_but_removes_task(
    ) -> anyhow::Result<()> {
        for ending in ["", "</Actions>", "</Task>"] {
            let fixture = scheduler_manifest_fixture()?;
            let prepared =
                prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
            edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
            let complete = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
            let body = complete
                .strip_suffix("</Actions></Task>")
                .context("test scheduler XML suffix")?;
            let xml = format!("{body}{ending}");
            let project_id = edda_store::project_id_for_root(&fixture.repo);
            let mut outputs = [
                SchedulerOutput::for_test(0, &xml, ""),
                SchedulerOutput::for_test(0, "", ""),
                SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
            ]
            .into_iter();
            let mut calls = 0;

            uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
                calls += 1;
                outputs.next().context("unexpected scheduler call")
            })?;

            assert_eq!(calls, 3);
            assert!(
                prepared.path.exists(),
                "ending {ending:?} authorized cleanup"
            );
        }
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_untrusted_manifest_does_not_block_task_removal() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let mut wrong_project = prepared.manifest.clone();
        wrong_project.project_id = "0".repeat(32);
        let untrusted = write_scheduler_manifest_candidate(
            &fixture.store,
            &serde_json::to_vec(&wrong_project)?,
        )?;
        let xml = scheduler_manifest_xml(&fixture.codex, &untrusted)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let mut calls = 0;

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            calls += 1;
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, 3);
        assert!(untrusted.exists());
        assert!(prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_never_sweeps_unproven_artifacts() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let (_, query_args, _) = windows_scheduler_management_args(&project_id)?;
        let mut calls = Vec::new();

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |args| {
            calls.push(args.to_vec());
            Ok(SchedulerOutput::for_test(
                MISSING_TASK_HRESULT,
                "",
                "missing",
            ))
        })?;

        assert_eq!(calls, [query_args]);
        assert!(prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_missing_codex_retains_manifest_without_blocking_task_removal(
    ) -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let xml = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
        std::fs::remove_file(&fixture.codex)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            outputs.next().context("unexpected scheduler call")
        })?;

        assert!(prepared.path.exists());
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_delete_race_accepts_only_missing_hresult() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let xml = "<Task><Actions /></Task>";
        for (delete_code, succeeds) in [(MISSING_TASK_HRESULT, true), (0x8007_0005, false)] {
            let mut outputs = [
                SchedulerOutput::for_test(0, xml, ""),
                SchedulerOutput::for_test(delete_code, "", "delete detail"),
                SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
            ]
            .into_iter();
            let result =
                uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
                    outputs.next().context("unexpected scheduler call")
                });
            assert_eq!(result.is_ok(), succeeds, "delete code 0x{delete_code:08x}");
        }
        Ok(())
    }

    #[test]
    fn scheduler_uninstall_post_delete_uncertainty_retains_manifest() -> anyhow::Result<()> {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let xml = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(0, &xml, "still present"),
        ]
        .into_iter();

        assert!(
            uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| outputs
                .next()
                .context("unexpected scheduler call"),)
            .is_err()
        );
        assert!(prepared.path.exists());
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
    fn scheduler_query_rejects_truncated_xml_output() {
        let output = SchedulerOutput::for_test_with_lengths(
            0,
            "<Task />",
            "",
            SCHEDULER_OUTPUT_LIMIT + 1,
            0,
        );
        let error = output
            .xml()
            .expect_err("truncated Query XML must be rejected")
            .to_string();
        assert!(error.contains(&(SCHEDULER_OUTPUT_LIMIT + 1).to_string()));
        assert!(error.contains(&SCHEDULER_OUTPUT_LIMIT.to_string()));
        assert!(manifest_cleanup_decision(
            &output,
            Path::new(r"C:\edda\edda.exe"),
            Path::new(
                r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json"
            ),
        )
        .is_err());
    }

    #[test]
    fn scheduler_query_decodes_raw_utf16_xml_with_non_ascii_paths() -> anyhow::Result<()> {
        let executable = Path::new(r"C:\工具\Edda\edda.exe");
        let manifest = Path::new(
            r"C:\儲存\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        );
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?><Task><Actions><Exec><Command>C:\工具\Edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\儲存\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec></Actions></Task>"#;

        let utf8_xml = xml.replace("UTF-16", "UTF-8");
        for bytes in [
            utf8_xml.as_bytes().to_vec(),
            [b"\xef\xbb\xbf", utf8_xml.as_bytes()].concat(),
        ] {
            let output = SchedulerOutput::for_test_bytes(0, &bytes, b"");
            let decoded = output.xml()?;
            assert!(scheduler_query_references_manifest(
                decoded.as_ref(),
                executable,
                manifest,
            )?);
        }
        for (little_endian, bom) in [(true, true), (false, true), (true, false), (false, false)] {
            let bytes = scheduler_xml_utf16_bytes(xml, little_endian, bom);
            let output = SchedulerOutput::for_test_bytes(0, &bytes, b"");
            let decoded = output.xml()?;
            assert!(scheduler_query_references_manifest(
                decoded.as_ref(),
                executable,
                manifest,
            )?);
            assert_eq!(
                manifest_cleanup_decision(&output, executable, manifest)?,
                ManifestCleanupDecision::Retain
            );
        }
        Ok(())
    }

    #[test]
    fn scheduler_query_rejects_malformed_raw_xml_but_keeps_diagnostics_lossy() {
        for bytes in [
            vec![0xff, 0xfe, b'<'],
            vec![0xff, 0xfe, 0x00, 0xd8],
            vec![0x00, 0x00, 0xfe, 0xff],
            vec![0x80, 0x81],
        ] {
            assert!(SchedulerOutput::for_test_bytes(0, &bytes, b"")
                .xml()
                .is_err());
        }

        let overflow = SchedulerOutput::for_test_bytes_with_stdout_len(
            0,
            b"<Task />",
            b"",
            SCHEDULER_OUTPUT_LIMIT + 1,
        );
        assert!(overflow.xml().is_err());

        let localized = SchedulerOutput::for_test_bytes(MISSING_TASK_HRESULT, b"", &[0xff]);
        assert_eq!(
            classify_scheduler_query(&localized).expect("non-XML diagnostics stay lossy"),
            SchedulerTaskState::Missing
        );
        assert!(localized.description().contains('\u{fffd}'));
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
            let deadline = std::time::Instant::now() + FAKE_CODEX_STARTUP_BUDGET;
            while std::time::Instant::now() < deadline {
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

        let session_deadline = std::time::Instant::now() + FAKE_CODEX_STARTUP_BUDGET;
        while std::time::Instant::now() < session_deadline {
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
    fn fake_runner_resumes_current_attempt_after_slow_startup_before_turn() -> anyhow::Result<()> {
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

        std::thread::sleep(std::time::Duration::from_millis(2_100));

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
  $startupDeadline = [System.DateTime]::UtcNow.AddMilliseconds(STARTUP_BUDGET_MS)
  while ([System.DateTime]::UtcNow -lt $startupDeadline) {
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
            .replace(
                "STARTUP_BUDGET_MS",
                &FAKE_CODEX_STARTUP_BUDGET.as_millis().to_string(),
            )
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
