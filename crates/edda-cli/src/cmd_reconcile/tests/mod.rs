use super::*;
use clap::Parser;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
use std::path::{Path, PathBuf};
use std::process::Command;

mod manifest;
mod plan;
mod runner;
mod scheduler;

#[cfg(windows)]
pub(super) static FAKE_CODEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(windows)]
pub(super) const FAKE_CODEX_STARTUP_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(30);

pub(super) static DOORBELL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Parser)]
pub(super) struct SchedulerCli {
    #[command(flatten)]
    args: ReconcileArgs,
}

pub(super) fn test_lock(lock: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(super) static CODEX_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) struct CodexBinEnvGuard {
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

pub(super) fn codex_bin_env_guard(value: &str) -> CodexBinEnvGuard {
    let lock = test_lock(&CODEX_BIN_ENV_LOCK);
    let previous = std::env::var_os("EDDA_CODEX_BIN");
    std::env::set_var("EDDA_CODEX_BIN", value);
    CodexBinEnvGuard {
        previous,
        _lock: lock,
    }
}

pub(super) fn codex_bin_env() -> Option<std::ffi::OsString> {
    let _lock = test_lock(&CODEX_BIN_ENV_LOCK);
    std::env::var_os("EDDA_CODEX_BIN")
}

#[cfg(not(windows))]
pub(super) fn scheduler_config(codex_bin: &str) -> ReconcileConfig {
    ReconcileConfig {
        max_workers: 3,
        max_attempts: 3,
        lease_ttl_s: 300,
        codex_bin: PathBuf::from(codex_bin),
    }
}

pub(super) fn manifest_path_for_task_run_utf16_len(target: usize) -> PathBuf {
    let fixed = r#""C:\e.exe" reconcile --scheduler-manifest "C:\.json""#
        .encode_utf16()
        .count();
    PathBuf::from(format!(r"C:\{}.json", "x".repeat(target - fixed)))
}

pub(super) fn scheduler_xml_utf16_bytes(xml: &str, little_endian: bool, bom: bool) -> Vec<u8> {
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

pub(super) struct SchedulerManifestFixture {
    _store_guard: edda_store::test_support::IsolatedStoreRoot,
    _root: tempfile::TempDir,
    store: PathBuf,
    repo: PathBuf,
    codex: PathBuf,
    config: ReconcileConfig,
}

pub(super) fn scheduler_manifest_fixture() -> anyhow::Result<SchedulerManifestFixture> {
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

pub(super) fn write_scheduler_manifest_candidate(
    store: &Path,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    use sha2::Digest;

    let digest = hex::encode(sha2::Sha256::digest(bytes));
    let path = store
        .join("scheduler-launch")
        .join("v1")
        .join(format!("{digest}.json"));
    edda_store::write_atomic(&path, bytes)?;
    Ok(path)
}

#[cfg(windows)]
pub(super) fn allow_fake_turn_after_durable_session(
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

pub(super) fn task(id: u64, status: TaskStatus, scope: &[&str]) -> TaskView {
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

pub(super) fn lease(task_id: u64, attempt: u32, expires_at: &str) -> TaskLease {
    TaskLease {
        task_id,
        attempt,
        owner: format!("runner-{task_id}-{attempt}"),
        expires_at: expires_at.into(),
        heartbeat_at: "2026-08-16T00:00:00Z".into(),
    }
}

pub(super) fn create_task(
    ledger: &Ledger,
    task_id: u64,
    scope_paths: &[String],
) -> anyhow::Result<()> {
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

pub(super) fn init_git(repo: &Path) -> anyhow::Result<()> {
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
