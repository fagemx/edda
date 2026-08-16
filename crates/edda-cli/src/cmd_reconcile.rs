use anyhow::Context;
use clap::Args;
use edda_core::event::{
    new_task_failed_event, new_task_host_session_event, new_task_requeued_event,
    new_task_started_event,
};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
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
    #[arg(long, hide = true)]
    run_task: Option<u64>,
    #[arg(long, hide = true)]
    attempt: Option<u32>,
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

#[derive(Clone)]
struct RunnerPlan {
    task: TaskView,
    attempt: u32,
    worktree: PathBuf,
}

pub fn run(repo_root: &Path, args: ReconcileArgs) -> anyhow::Result<()> {
    let config = ReconcileConfig::from_args(&args);
    if let Some(task_id) = args.run_task {
        let attempt = args
            .attempt
            .context("--run-task requires hidden --attempt")?;
        return run_task(repo_root, task_id, attempt, &config, true);
    }
    if args.attempt.is_some() {
        anyhow::bail!("--attempt is valid only with hidden --run-task");
    }
    let plans = persist_reconciliation(repo_root, &config)?;
    let executable = std::env::current_exe()?;
    let executables = vec![executable; plans.len()];
    let (launched, errors) = launch_plans_with(repo_root, plans, &config, &executables);
    for plan in launched {
        notify_started(repo_root, &plan.task);
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
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .strip_prefix(&left)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn static_prefix(path: &str) -> Option<String> {
    let normalized_path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized_path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let normalized = parts.join("/");
    let end = normalized
        .find(['*', '?', '[', '{'])
        .unwrap_or(normalized.len());
    let prefix = normalized[..end].trim_end_matches('/');
    (!prefix.is_empty()).then_some(prefix.to_string())
}

fn persist_reconciliation(
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<Vec<RunnerPlan>> {
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

    for action in actions {
        match action {
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
                if let Err(error) = append_started(&ledger, task_id, attempt, config.lease_ttl_s) {
                    let _ = ledger.delete_task_lease(task_id, attempt);
                    return Err(error);
                }
                plans.push(RunnerPlan {
                    task,
                    attempt,
                    worktree,
                });
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
                plans.push(RunnerPlan {
                    task,
                    attempt,
                    worktree,
                });
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
                plans.push(RunnerPlan {
                    task,
                    attempt: next_attempt,
                    worktree,
                });
            }
            ReconcileAction::Fail { task_id, reason } => {
                append_failed(&ledger, task_id, &reason)?;
                let attempt = task_view(&views, task_id)?.attempts;
                let _ = ledger.delete_task_lease(task_id, attempt)?;
            }
        }
    }
    if changed {
        let branch = ledger.head_branch()?;
        let _ = edda_derive::rebuild_branch(&ledger, &branch);
    }
    drop(lock);
    Ok(plans)
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
    if FAIL_NEXT_STARTED.with(|flag| flag.replace(false)) {
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
    if FAIL_NEXT_LEASE.with(|flag| flag.replace(false)) {
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
    use edda_ledger::tasks::{TaskStatus, TaskView};
    use edda_ledger::TaskLease;

    fn test_lock(lock: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
        lock.lock().unwrap_or_else(|poison| poison.into_inner())
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

        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
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

            assert!(persist_reconciliation(&repo, &ReconcileConfig::test_defaults()).is_err());
            assert!(ledger.task_lease(1)?.is_none());
            assert!(ledger
                .task_events()?
                .iter()
                .all(|event| event.event_type != "task.started"));
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
                        .map(|plans| plans.len())
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

        DOORBELL_COUNT.store(0, Ordering::SeqCst);
        run_task(&repo, 1, 1, &config, true)?;

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
        assert_ne!(
            ledger.task_lease(1)?.expect("renewed lease").heartbeat_at,
            "2026-08-16T00:00:00Z"
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

        run_task(&repo, 2, 1, &config, false)?;

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
