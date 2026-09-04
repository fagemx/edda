use anyhow::Context;
use clap::Args;
use edda_ledger::tasks::TaskView;
use std::path::{Path, PathBuf};

mod codex_target;
mod manifest;
mod plan;
mod runner;

#[cfg(any(windows, test))]
mod scheduler_windows;
#[cfg(any(windows, test))]
mod scheduler_xml;

#[cfg(test)]
mod tests;

use codex_target::*;
use manifest::*;
use plan::*;
use runner::*;
#[cfg(any(windows, test))]
use scheduler_windows::*;

pub(super) const SCHEDULER_MANIFEST_MAX_BYTES: u64 = 16 * 1024;

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
pub(super) struct ReconcileConfig {
    pub(super) max_workers: usize,
    pub(super) max_attempts: u32,
    pub(super) lease_ttl_s: u64,
    pub(super) codex_bin: PathBuf,
}

impl ReconcileConfig {
    pub(super) fn defaults() -> Self {
        Self {
            max_workers: 3,
            max_attempts: 3,
            lease_ttl_s: 300,
            codex_bin: PathBuf::from("codex"),
        }
    }

    pub(super) fn from_args(args: &ReconcileArgs) -> Self {
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
    pub(super) fn test_defaults() -> Self {
        Self::defaults()
    }
}

#[derive(Clone)]
pub(super) struct RunnerPlan {
    pub(super) task: TaskView,
    pub(super) attempt: u32,
    pub(super) worktree: PathBuf,
}

pub(super) struct PersistOutcome {
    pub(super) plans: Vec<RunnerPlan>,
    pub(super) errors: Vec<String>,
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

pub(super) fn scheduler_lifecycle(
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

pub(super) fn launch_plans_with(
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
