//! `edda task` — task rail verbs (TASK_RAIL_V1 §5, P1).
//!
//! Agent verbs: new / start / done / fail. User verbs: list / show.
//! Every state transition is a hash-chained `task.*` event; `done` without
//! a receipt does not exist. Status shown anywhere is derived, never stored.

use clap::Subcommand;
use edda_core::event::{
    new_task_created_event, new_task_done_event, new_task_failed_event, new_task_started_event,
    TaskCreatedParams,
};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::tasks::{self, TaskStatus, TaskView};
use edda_ledger::Ledger;
use std::path::Path;

#[derive(Subcommand)]
pub enum TaskCmd {
    /// Create a task on the rail (agent verb)
    New {
        /// Task title
        title: String,
        /// Agent label this task is assigned to (e.g. "tester")
        #[arg(long)]
        assignee: Option<String>,
        /// Agent transport kind (e.g. "claude-acp", "codex-acp")
        #[arg(long = "agent")]
        agent_kind: Option<String>,
        /// Task id that must be done first (repeatable)
        #[arg(long)]
        after: Vec<u64>,
        /// Plan this task belongs to
        #[arg(long)]
        plan: Option<String>,
        /// wusanto work unit this task delivers (§8 mapping)
        #[arg(long = "work-unit")]
        work_unit: Option<String>,
        /// Brief reference (path or free text) for whoever picks this up
        #[arg(long)]
        brief: Option<String>,
        /// Idempotency key — the same key never creates a twin task
        #[arg(long = "key")]
        idempotency_key: Option<String>,
    },
    /// Take the lease on a task and mark it running (agent verb)
    Start {
        id: u64,
        /// Lease TTL in seconds (recorded now; enforced by the P2 reconciler)
        #[arg(long = "lease-ttl", default_value = "3600")]
        lease_ttl_s: u64,
    },
    /// Complete a task — one action: done + receipt; successors become ready
    Done {
        id: u64,
        /// What was done, verifiable. Required: no receipt, no done.
        #[arg(long)]
        receipt: String,
        /// Evidence path (repeatable)
        #[arg(long = "evidence")]
        evidence_paths: Vec<String>,
    },
    /// Mark a task failed (agent verb)
    Fail {
        id: u64,
        #[arg(long)]
        reason: String,
    },
    /// List tasks with derived status (user verb)
    List {
        /// Filter by assignee label
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by status (blocked|ready|running|done|failed)
        #[arg(long)]
        status: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show one task in full (user verb)
    Show {
        id: u64,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Arguments for creating a task (mirrors `task.created` payload).
pub struct NewTaskArgs<'a> {
    pub title: &'a str,
    pub assignee: Option<&'a str>,
    pub agent_kind: Option<&'a str>,
    pub after: &'a [u64],
    pub plan: Option<&'a str>,
    pub work_unit: Option<&'a str>,
    pub brief: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
}

#[derive(Debug)]
pub struct NewOutcome {
    pub task_id: u64,
    pub status: TaskStatus,
    /// True when an existing task with the same idempotency key was reused.
    pub deduped: bool,
}

#[derive(Debug)]
pub struct StartOutcome {
    pub attempt: u32,
}

#[derive(Debug)]
pub struct DoneOutcome {
    /// Successors unlocked by this completion: (task_id, title, assignee).
    pub unlocked: Vec<(u64, String, Option<String>)>,
}

fn find_view(views: &[TaskView], id: u64) -> anyhow::Result<&TaskView> {
    views
        .iter()
        .find(|v| v.task_id == id)
        .ok_or_else(|| anyhow::anyhow!("task #{id} not found — see `edda task list`"))
}

fn parse_status(s: &str) -> anyhow::Result<TaskStatus> {
    Ok(match s {
        "blocked" => TaskStatus::Blocked,
        "ready" => TaskStatus::Ready,
        "running" => TaskStatus::Running,
        "done" => TaskStatus::Done,
        "failed" => TaskStatus::Failed,
        other => {
            anyhow::bail!("unknown status '{other}' (expected blocked|ready|running|done|failed)")
        }
    })
}

fn do_new(repo_root: &Path, args: &NewTaskArgs<'_>) -> anyhow::Result<NewOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    if let Some(key) = args.idempotency_key {
        if let Some(existing) = tasks::find_by_idempotency_key(&views, key) {
            return Ok(NewOutcome {
                task_id: existing.task_id,
                status: existing.status,
                deduped: true,
            });
        }
    }

    let task_id = tasks::next_task_id(&views);
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_created_event(&TaskCreatedParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        task_id,
        title: args.title,
        assignee: args.assignee,
        agent_kind: args.agent_kind,
        after: args.after,
        plan_id: args.plan,
        work_unit_ref: args.work_unit,
        brief_ref: args.brief,
        idempotency_key: args.idempotency_key,
    })?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    let views = ledger.task_views()?;
    let status = find_view(&views, task_id)?.status;
    Ok(NewOutcome {
        task_id,
        status,
        deduped: false,
    })
}

fn do_start(repo_root: &Path, id: u64, lease_ttl_s: u64) -> anyhow::Result<StartOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    match v.status {
        TaskStatus::Done => anyhow::bail!("task #{id} is already done"),
        TaskStatus::Running => {
            anyhow::bail!("task #{id} is already running (attempt {})", v.attempts)
        }
        TaskStatus::Blocked => {
            let unmet: Vec<String> = v
                .after
                .iter()
                .filter(|d| {
                    views
                        .iter()
                        .find(|x| x.task_id == **d)
                        .is_none_or(|x| x.status != TaskStatus::Done)
                })
                .map(|d| format!("#{d}"))
                .collect();
            anyhow::bail!("task #{id} is blocked — unmet deps: {}", unmet.join(", "));
        }
        // Ready = normal start; Failed = retry.
        TaskStatus::Ready | TaskStatus::Failed => {}
    }
    let attempt = v.attempts + 1;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_started_event(&branch, parent_hash.as_deref(), id, lease_ttl_s, attempt)?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(StartOutcome { attempt })
}

fn do_done(
    repo_root: &Path,
    id: u64,
    receipt: &str,
    evidence_paths: &[String],
) -> anyhow::Result<DoneOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    let correction = v.status == TaskStatus::Done;
    match v.status {
        TaskStatus::Running | TaskStatus::Done => {}
        TaskStatus::Ready if v.attempts > 0 => {}
        TaskStatus::Ready | TaskStatus::Blocked => anyhow::bail!(
            "task #{id} has not been started — run `edda task start {id}` first \
             (start/done pairs are what make the ledger replayable)"
        ),
        TaskStatus::Failed => {
            anyhow::bail!("task #{id} is failed — run `edda task start {id}` to retry, then done")
        }
    }
    if receipt.trim().is_empty() {
        anyhow::bail!(
            "a completion without a receipt does not exist — pass --receipt with real content"
        );
    }

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_done_event(&branch, parent_hash.as_deref(), id, receipt, evidence_paths)?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    let unlocked = if correction {
        Vec::new()
    } else {
        let after_views = ledger.task_views()?;
        tasks::ready_successors_of(&after_views, id)
            .into_iter()
            .map(|s| (s.task_id, s.title.clone(), s.assignee.clone()))
            .collect()
    };
    Ok(DoneOutcome { unlocked })
}

fn do_fail(repo_root: &Path, id: u64, reason: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    if v.status != TaskStatus::Running {
        anyhow::bail!(
            "task #{id} is not running ({}) — only a running task can fail",
            v.status
        );
    }

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_failed_event(&branch, parent_hash.as_deref(), id, reason)?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(())
}

fn do_list(
    repo_root: &Path,
    assignee: Option<&str>,
    status: Option<&str>,
) -> anyhow::Result<Vec<TaskView>> {
    let ledger = Ledger::open(repo_root)?;
    let mut views = ledger.task_views()?;
    if let Some(a) = assignee {
        views.retain(|v| v.assignee.as_deref() == Some(a));
    }
    if let Some(s) = status {
        let want = parse_status(s)?;
        views.retain(|v| v.status == want);
    }
    Ok(views)
}

fn do_show(repo_root: &Path, id: u64) -> anyhow::Result<TaskView> {
    let ledger = Ledger::open(repo_root)?;
    let views = ledger.task_views()?;
    Ok(find_view(&views, id)?.clone())
}

pub fn execute(cmd: TaskCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        TaskCmd::New {
            title,
            assignee,
            agent_kind,
            after,
            plan,
            work_unit,
            brief,
            idempotency_key,
        } => {
            let outcome = do_new(
                repo_root,
                &NewTaskArgs {
                    title: &title,
                    assignee: assignee.as_deref(),
                    agent_kind: agent_kind.as_deref(),
                    after: &after,
                    plan: plan.as_deref(),
                    work_unit: work_unit.as_deref(),
                    brief: brief.as_deref(),
                    idempotency_key: idempotency_key.as_deref(),
                },
            )?;
            if outcome.deduped {
                println!(
                    "Task #{} already exists for this key — reusing it (no twin created).",
                    outcome.task_id
                );
            } else {
                println!(
                    "Created task #{} '{}' [{}]",
                    outcome.task_id, title, outcome.status
                );
                if !after.is_empty() {
                    let deps: Vec<String> = after.iter().map(|d| format!("#{d}")).collect();
                    println!("  after: {}", deps.join(", "));
                }
            }
            Ok(())
        }
        TaskCmd::Start { id, lease_ttl_s } => {
            let outcome = do_start(repo_root, id, lease_ttl_s)?;
            println!(
                "Started task #{id} (attempt {}, lease {lease_ttl_s}s)",
                outcome.attempt
            );
            Ok(())
        }
        TaskCmd::Done {
            id,
            receipt,
            evidence_paths,
        } => {
            let outcome = do_done(repo_root, id, &receipt, &evidence_paths)?;
            println!("Task #{id} done — receipt recorded.");
            for (sid, title, assignee) in &outcome.unlocked {
                match assignee {
                    Some(a) => println!("  → task #{sid} '{title}' now ready (assignee: {a})"),
                    None => println!("  → task #{sid} '{title}' now ready"),
                }
            }
            Ok(())
        }
        TaskCmd::Fail { id, reason } => {
            do_fail(repo_root, id, &reason)?;
            println!("Task #{id} marked failed: {reason}");
            Ok(())
        }
        TaskCmd::List {
            assignee,
            status,
            json,
        } => {
            let views = do_list(repo_root, assignee.as_deref(), status.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
                return Ok(());
            }
            if views.is_empty() {
                println!("No tasks on the rail.");
                println!("Create one: edda task new \"title\" --assignee <label>");
                return Ok(());
            }
            for v in &views {
                let mut extras: Vec<String> = Vec::new();
                if let Some(a) = &v.assignee {
                    extras.push(format!("assignee: {a}"));
                }
                if !v.after.is_empty() {
                    let deps: Vec<String> = v.after.iter().map(|d| format!("#{d}")).collect();
                    extras.push(format!("after: {}", deps.join(",")));
                }
                if v.attempts > 0 {
                    extras.push(format!("attempts: {}", v.attempts));
                }
                let extra = if extras.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", extras.join(", "))
                };
                println!("#{} [{}] {}{}", v.task_id, v.status, v.title, extra);
            }
            Ok(())
        }
        TaskCmd::Show { id, json } => {
            let v = do_show(repo_root, id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            println!("Task #{}: {}", v.task_id, v.title);
            println!("  status:   {}", v.status);
            if let Some(a) = &v.assignee {
                println!("  assignee: {a}");
            }
            if let Some(k) = &v.agent_kind {
                println!("  agent:    {k}");
            }
            if !v.after.is_empty() {
                let deps: Vec<String> = v.after.iter().map(|d| format!("#{d}")).collect();
                println!("  after:    {}", deps.join(", "));
            }
            if let Some(w) = &v.work_unit_ref {
                println!("  work-unit: {w}");
            }
            if let Some(b) = &v.brief_ref {
                println!("  brief:    {b}");
            }
            if v.attempts > 0 {
                println!("  attempts: {}", v.attempts);
            }
            if let Some(r) = &v.receipt {
                println!("  receipt:  {r}");
            }
            if !v.evidence_paths.is_empty() {
                println!("  evidence: {}", v.evidence_paths.join(", "));
            }
            if let Some(f) = &v.failure_reason {
                println!("  last-failure: {f}");
            }
            if let Some(s) = &v.acp_session_id {
                println!("  acp-session: {s}");
            }
            println!("  created:  {}", v.created_ts);
            println!("  updated:  {}", v.updated_ts);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("edda_cmdtask_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Ledger::ensure_initialized(&dir).unwrap();
        dir
    }

    fn args<'a>(title: &'a str, after: &'a [u64]) -> NewTaskArgs<'a> {
        NewTaskArgs {
            title,
            assignee: Some("tester"),
            agent_kind: None,
            after,
            plan: None,
            work_unit: None,
            brief: None,
            idempotency_key: None,
        }
    }

    #[test]
    fn new_assigns_sequential_ids_and_derives_status() {
        let ws = temp_ws("seq");
        let a = do_new(&ws, &args("build", &[])).unwrap();
        let b = do_new(&ws, &args("test", &[1])).unwrap();
        assert_eq!(a.task_id, 1);
        assert_eq!(b.task_id, 2);
        assert_eq!(a.status, TaskStatus::Ready);
        assert_eq!(b.status, TaskStatus::Blocked);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn new_with_idempotency_key_dedupes() {
        let ws = temp_ws("dedupe");
        let mut first = args("build", &[]);
        first.idempotency_key = Some("wu1-build");
        let a = do_new(&ws, &first).unwrap();
        assert!(!a.deduped);

        let mut twin = args("build again", &[]);
        twin.idempotency_key = Some("wu1-build");
        let b = do_new(&ws, &twin).unwrap();
        assert!(b.deduped);
        assert_eq!(a.task_id, b.task_id);

        let views = do_list(&ws, None, None).unwrap();
        assert_eq!(views.len(), 1);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn start_ready_task_records_attempt() {
        let ws = temp_ws("start");
        do_new(&ws, &args("build", &[])).unwrap();
        let s = do_start(&ws, 1, 3600).unwrap();
        assert_eq!(s.attempt, 1);
        assert_eq!(do_show(&ws, 1).unwrap().status, TaskStatus::Running);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn start_blocked_task_errors() {
        let ws = temp_ws("startblocked");
        do_new(&ws, &args("build", &[])).unwrap();
        do_new(&ws, &args("test", &[1])).unwrap();
        let err = do_start(&ws, 2, 3600).unwrap_err().to_string();
        assert!(err.contains("blocked"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn start_running_task_errors() {
        let ws = temp_ws("startrun");
        do_new(&ws, &args("build", &[])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        assert!(do_start(&ws, 1, 3600).is_err());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn start_done_task_errors() {
        let ws = temp_ws("startdone");
        do_new(&ws, &args("build", &[])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        do_done(&ws, 1, "built", &[]).unwrap();
        assert!(do_start(&ws, 1, 3600).is_err());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn done_running_task_unlocks_successor() {
        let ws = temp_ws("unlock");
        do_new(&ws, &args("build", &[])).unwrap();
        do_new(&ws, &args("test", &[1])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        let outcome = do_done(&ws, 1, "built ok", &[]).unwrap();
        assert_eq!(outcome.unlocked.len(), 1);
        assert_eq!(outcome.unlocked[0].0, 2);
        assert_eq!(outcome.unlocked[0].1, "test");
        assert_eq!(outcome.unlocked[0].2.as_deref(), Some("tester"));
        assert_eq!(do_show(&ws, 2).unwrap().status, TaskStatus::Ready);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn done_without_start_errors() {
        let ws = temp_ws("doneskip");
        do_new(&ws, &args("build", &[])).unwrap();
        let err = do_done(&ws, 1, "receipt", &[]).unwrap_err().to_string();
        assert!(err.contains("start"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn done_twice_corrects_receipt_without_reunlocking_successor() {
        let ws = temp_ws("donetwice");
        do_new(&ws, &args("build", &[])).unwrap();
        do_new(&ws, &args("test", &[1])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        assert_eq!(do_done(&ws, 1, "built", &[]).unwrap().unlocked.len(), 1);

        let correction = do_done(&ws, 1, "corrected receipt", &[]).unwrap();
        assert!(correction.unlocked.is_empty());
        assert_eq!(
            do_show(&ws, 1).unwrap().receipt.as_deref(),
            Some("corrected receipt")
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn late_done_after_requeue_is_accepted() {
        let ws = temp_ws("latedone");
        do_new(&ws, &args("build", &[])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        do_fail(&ws, 1, "lease expired").unwrap();
        {
            let ledger = Ledger::open(&ws).unwrap();
            let _lock = WorkspaceLock::acquire(&ledger.paths).unwrap();
            let branch = ledger.head_branch().unwrap();
            let parent_hash = ledger.last_event_hash().unwrap();
            let event =
                edda_core::event::new_task_requeued_event(&branch, parent_hash.as_deref(), 1, 2)
                    .unwrap();
            ledger.append_event(&event).unwrap();
        }

        do_done(&ws, 1, "late but real", &[]).unwrap();
        assert_eq!(do_show(&ws, 1).unwrap().status, TaskStatus::Done);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn done_with_blank_receipt_errors() {
        let ws = temp_ws("blankreceipt");
        do_new(&ws, &args("build", &[])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        let err = do_done(&ws, 1, "   ", &[]).unwrap_err().to_string();
        assert!(err.contains("receipt"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn fail_running_task_records_reason_and_start_retries() {
        let ws = temp_ws("failretry");
        do_new(&ws, &args("build", &[])).unwrap();
        do_start(&ws, 1, 3600).unwrap();
        do_fail(&ws, 1, "crash").unwrap();
        let v = do_show(&ws, 1).unwrap();
        assert_eq!(v.status, TaskStatus::Failed);
        assert_eq!(v.failure_reason.as_deref(), Some("crash"));

        let s = do_start(&ws, 1, 3600).unwrap();
        assert_eq!(s.attempt, 2);
        assert_eq!(do_show(&ws, 1).unwrap().status, TaskStatus::Running);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn fail_non_running_task_errors() {
        let ws = temp_ws("failready");
        do_new(&ws, &args("build", &[])).unwrap();
        assert!(do_fail(&ws, 1, "nope").is_err());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn list_filters_by_assignee_and_status() {
        let ws = temp_ws("filters");
        do_new(&ws, &args("build", &[])).unwrap();
        let mut other = args("docs", &[]);
        other.assignee = Some("writer");
        do_new(&ws, &other).unwrap();

        let tester_only = do_list(&ws, Some("tester"), None).unwrap();
        assert_eq!(tester_only.len(), 1);
        assert_eq!(tester_only[0].title, "build");

        do_start(&ws, 1, 3600).unwrap();
        let running = do_list(&ws, None, Some("running")).unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].task_id, 1);

        assert!(do_list(&ws, None, Some("bogus")).is_err());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn verbs_on_missing_task_error() {
        let ws = temp_ws("missing");
        assert!(do_start(&ws, 99, 3600).is_err());
        assert!(do_done(&ws, 99, "r", &[]).is_err());
        assert!(do_fail(&ws, 99, "r").is_err());
        assert!(do_show(&ws, 99).is_err());
        let _ = std::fs::remove_dir_all(&ws);
    }
}
