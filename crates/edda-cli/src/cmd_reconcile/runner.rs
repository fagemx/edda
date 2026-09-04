use anyhow::Context;
use edda_core::event::{
    new_task_failed_event, new_task_host_session_event, new_task_requeued_event,
    new_task_started_event,
};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::WORKSPACE_LOCK_WAIT_BUDGET;
use super::ReconcileConfig;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
pub(super) static DOORBELL_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
thread_local! {
    pub(super) static FAIL_NEXT_STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static FAIL_NEXT_LEASE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static FAIL_TASK_ID: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(super) fn acquire_workspace_lock(
    paths: &edda_ledger::EddaPaths,
) -> anyhow::Result<WorkspaceLock> {
    let deadline = std::time::Instant::now() + WORKSPACE_LOCK_WAIT_BUDGET;
    loop {
        match WorkspaceLock::acquire(paths) {
            Ok(lock) => return Ok(lock),
            Err(error) => {
                if std::time::Instant::now() >= deadline {
                    return Err(error);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

pub(super) fn task_view(views: &[TaskView], task_id: u64) -> anyhow::Result<&TaskView> {
    views
        .iter()
        .find(|view| view.task_id == task_id)
        .ok_or_else(|| anyhow::anyhow!("task #{task_id} disappeared during reconciliation"))
}

pub(super) fn append_started(
    ledger: &Ledger,
    task_id: u64,
    attempt: u32,
    ttl_s: u64,
) -> anyhow::Result<()> {
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

pub(super) fn append_requeued(ledger: &Ledger, task_id: u64, attempt: u32) -> anyhow::Result<()> {
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    ledger.append_event(&new_task_requeued_event(
        &branch,
        parent_hash.as_deref(),
        task_id,
        attempt,
    )?)
}

pub(super) fn append_failed(ledger: &Ledger, task_id: u64, reason: &str) -> anyhow::Result<()> {
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    ledger.append_event(&new_task_failed_event(
        &branch,
        parent_hash.as_deref(),
        task_id,
        reason,
    )?)
}

pub(super) fn replace_lease(
    ledger: &Ledger,
    task_id: u64,
    attempt: u32,
    ttl_s: u64,
) -> anyhow::Result<()> {
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

pub(super) fn clock_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(super) fn attempt_branch(task_id: u64, attempt: u32) -> String {
    format!("codex/task-{task_id}-attempt-{attempt}")
}

pub(super) fn attempt_worktree_path(
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
) -> anyhow::Result<PathBuf> {
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

pub(super) fn ensure_attempt_worktree(
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

pub(super) fn worktree_registered_for_branch(listing: &str, worktree: &Path, branch: &str) -> bool {
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

pub(super) fn git<I, S>(cwd: &Path, args: I) -> anyhow::Result<String>
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

pub(super) fn git_success<I, S>(cwd: &Path, args: I) -> anyhow::Result<bool>
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

pub(super) fn launch_runner_with(
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

pub(super) fn notify_started(repo_root: &Path, task: &TaskView) {
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

pub(super) fn run_task(
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

pub(super) fn renew_lease(
    ledger: &Ledger,
    task_id: u64,
    attempt: u32,
    ttl_s: u64,
) -> anyhow::Result<bool> {
    let heartbeat_at = clock_now();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(ttl_s as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    ledger.renew_task_lease(task_id, attempt, &expires_at, &heartbeat_at)
}

pub(super) fn record_session_if_current(
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

pub(super) fn finish_runner(
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

pub(super) fn launch_runner_doorbell(
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<()> {
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

pub(super) fn runner_prompt(
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

pub(super) fn read_brief(repo_root: &Path, reference: &str) -> anyhow::Result<String> {
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
