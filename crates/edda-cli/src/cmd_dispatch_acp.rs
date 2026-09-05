//! Task-rail wiring for `edda dispatch --agent acp:<target>`.
//!
//! This deliberately enters after the ordinary dispatch command has parsed its
//! common lifecycle controls, but before legacy launchers are constructed.
//! ACP receives its prompt, worktree, permission roots, and resume id only from
//! the task ledger; command-line prompt/session substitutions are refused.

use crate::agent_kind::AgentKind;
use crate::cmd_dispatch::DispatchArgs;
use anyhow::{bail, Context, Result};
use edda_conductor::agent::acp_targets::AcpTarget;
use edda_conductor::runner::acp::{AcpPermissionPolicy, AcpRunner, AcpTaskRequest, LedgerAcpAudit};
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::Ledger;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Validate every ACP-specific local prerequisite before dispatch can write a
/// GitHub claim. This is intentionally callable from the parent command's F1
/// preflight path.
pub(crate) fn preflight(args: &DispatchArgs, cwd: &Path) -> Result<()> {
    validate_args(args)?;
    let task_id = args.task_id.context("ACP dispatch requires --task-id")?;
    let ledger = Ledger::open(cwd).context("opening task ledger for ACP dispatch")?;
    let views = ledger.task_views().context("reading ACP task history")?;
    let task = views
        .iter()
        .find(|view| view.task_id == task_id)
        .context("ACP dispatch task not found")?;
    target_for_task(args.agent, task)?;
    if task.status != TaskStatus::Running {
        bail!("ACP dispatch task #{task_id} must be running before an agent turn");
    }
    scope_roots(cwd, &task.scope_paths)?;
    Ok(())
}

/// Execute an ACP task turn. This honors GH-605's writer claim lifecycle and
/// refuses detached operation until ACP owns the same detached-supervisor
/// protocol; silently skipping either would create a fast path around them.
pub(crate) fn run(args: DispatchArgs) -> Result<i32> {
    validate_args(&args)?;
    let task_id = args.task_id.context("ACP dispatch requires --task-id")?;
    let cwd = match args.cwd.as_deref() {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().context("reading ACP dispatch cwd")?,
    };
    let cwd = cwd
        .canonicalize()
        .context("canonicalizing ACP dispatch cwd")?;
    let ledger = Ledger::open(&cwd).context("opening task ledger for ACP dispatch")?;
    let views = ledger.task_views().context("reading ACP task history")?;
    let task = views
        .iter()
        .find(|view| view.task_id == task_id)
        .context("ACP dispatch task not found")?;
    let target = target_for_task(args.agent, task)?;
    if task.status != TaskStatus::Running {
        bail!("ACP dispatch task #{task_id} must be running before an agent turn");
    }
    let task_roots = scope_roots(&cwd, &task.scope_paths)?;
    let peer_roots = live_peer_roots(&cwd, &views, task_id)?;
    let policy = AcpPermissionPolicy::new(task_roots, peer_roots, is_verifier(task))?;
    let prompt = task_prompt(&cwd, &views, task, &cwd);
    let mcp_program = std::env::current_exe().context("resolving edda MCP executable")?;
    let request = AcpTaskRequest {
        task_id,
        worktree: cwd.clone(),
        prompt,
        endpoint: target.endpoint(is_verifier(task)),
        mcp_server: edda_conductor::runner::acp::AcpEndpoint {
            program: mcp_program,
            args: vec!["mcp".into(), "serve".into()],
        },
        policy,
        prompt_timeout: args.timeout_sec.map(Duration::from_secs),
        // `task.session` is persisted by the runner immediately after new;
        // read its projected history before every later controller turn.
        resume_session_id: task.acp_session_id.clone(),
    };
    // Allocate fallible runtime resources before acquiring a writer claim, so
    // every post-claim path reaches the same release ordering below.
    let runtime = tokio::runtime::Runtime::new().context("starting ACP dispatch runtime")?;
    let claim = crate::dispatch_claim::acquire(&cwd, &format!("acp-task-{task_id}"), &args.owns)?;
    let result = runtime.block_on(
        AcpRunner::new(Arc::new(LedgerAcpAudit::new(&cwd))).run(request, CancellationToken::new()),
    );
    // Like the normal route, release before reporting success. A release
    // failure is a dispatch failure, never an orphaned writer claim.
    if let Some(claim) = claim {
        claim.release()?;
    }
    let result = result?;
    let output = json!({
        "outcome": "done",
        "agent": args.agent.as_str(),
        "task_id": task_id,
        "session_id": result.session_id,
        "stop_reason": format!("{:?}", result.stop_reason),
        "measured": result.usage.is_some(),
        "usage": result.usage,
    });
    if args.json {
        println!("{output}");
    } else {
        println!(
            "Outcome: done\nTask: #{task_id}\nSession: {}\nMeasured: {}",
            output["session_id"], output["measured"]
        );
    }
    Ok(0)
}

fn validate_args(args: &DispatchArgs) -> Result<()> {
    if args.detach {
        bail!("ACP dispatch does not yet support --detach; refusing to bypass the detached supervisor lifecycle");
    }
    if args.prompt_file.is_some() || args.session_id.is_some() || args.resume {
        bail!("ACP dispatch derives prompt and session continuity from --task-id; --prompt-file, --session-id, and --resume are not accepted");
    }
    if args.budget_usd.is_some()
        || args.permission_mode.is_some()
        || args.model.is_some()
        || args.thinking.is_some()
        || args.tools.is_some()
        || args.exclude_tools.is_some()
        || args.session_dir.is_some()
        || args.list_models.is_some()
    {
        bail!("ACP dispatch refuses legacy backend options that it cannot enforce");
    }
    Ok(())
}

fn target_for_task(agent: AgentKind, task: &TaskView) -> Result<AcpTarget> {
    let target = AcpTarget::parse(agent.as_str()).context("invalid ACP target")?;
    if task.agent_kind.as_deref().and_then(AcpTarget::parse) != Some(target) {
        bail!("task agent_kind must match selected ACP target");
    }
    Ok(target)
}

fn is_verifier(task: &TaskView) -> bool {
    task.assignee
        .as_deref()
        .is_some_and(|assignee| assignee.contains("verifier"))
}

fn scope_roots(cwd: &Path, scopes: &[String]) -> Result<Vec<PathBuf>> {
    if scopes.is_empty() {
        bail!("ACP task has no owned scope paths; refusing an unbounded permission root");
    }
    scopes
        .iter()
        .map(|scope| {
            if scope.contains(['*', '?', '[', ']']) {
                bail!("ACP task scope {scope:?} is a glob; use a concrete permission root");
            }
            let path = Path::new(scope);
            if path.is_absolute() || scope.split(['/', '\\']).any(|part| part == "..") {
                bail!("ACP task scope must be repository-relative");
            }
            cwd.join(path)
                .canonicalize()
                .with_context(|| format!("ACP task scope does not exist: {scope}"))
        })
        .collect()
}

fn live_peer_roots(cwd: &Path, views: &[TaskView], task_id: u64) -> Result<Vec<PathBuf>> {
    Ok(views
        .iter()
        .filter(|view| view.task_id != task_id && view.status == TaskStatus::Running)
        .flat_map(|view| view.scope_paths.iter())
        .filter(|scope| !scope.contains(['*', '?', '[', ']']))
        .filter_map(|scope| cwd.join(scope).canonicalize().ok())
        .collect())
}

fn task_prompt(repo_root: &Path, views: &[TaskView], task: &TaskView, worktree: &Path) -> String {
    let brief_ref = task.brief_ref.as_deref().unwrap_or("(none)");
    let brief = task
        .brief_ref
        .as_deref()
        .and_then(|reference| read_brief(repo_root, reference).ok())
        .unwrap_or_else(|| "(unavailable)".into());
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
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Task #{id}: {title}\nBrief reference: {brief_ref}\nBrief content (bounded):\n{brief}\nScope: {scope:?}\nDependency receipts:\n{receipts}\nWorktree: {worktree}\n\nPaths outside scope require a durable scope request. Assistant prose is not completion. Complete with:\nedda task done {id} --receipt \"<verifiable result>\" --evidence <path>",
        id = task.task_id,
        title = task.title,
        scope = task.scope_paths,
        worktree = worktree.display(),
    )
}

fn read_brief(repo_root: &Path, reference: &str) -> Result<String> {
    let path = Path::new(reference);
    if path.is_absolute() || reference.split(['/', '\\']).any(|part| part == "..") {
        bail!("brief reference must be a repository-relative path");
    }
    let bytes = std::fs::read(repo_root.join(path))?;
    let mut content = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned();
    if bytes.len() > 4096 {
        content.push_str("\n[brief truncated]");
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_requires_task_agent_kind_match() {
        let mut task = test_task();
        task.agent_kind = Some("acp:grok".into());
        assert_eq!(
            target_for_task(AgentKind::AcpGrok, &task).unwrap(),
            AcpTarget::Grok
        );
        assert!(target_for_task(AgentKind::AcpPi, &task).is_err());
    }

    #[test]
    fn prompt_contains_task_facts_and_receipts() {
        let root = tempfile::tempdir().unwrap();
        let task = test_task();
        let prior = TaskView {
            task_id: 8,
            receipt: Some("prior receipt".into()),
            evidence_paths: vec!["proof.txt".into()],
            ..test_task()
        };
        let prompt = task_prompt(root.path(), &[prior, task.clone()], &task, root.path());
        assert!(prompt.contains("Task #9: ACP task"));
        assert!(prompt.contains("#8/prior receipt"));
        assert!(prompt.contains("Scope: [\"work\"]"));
    }

    /// A minimal valid ACP dispatch: agent and task id, nothing else.
    fn acp_args() -> DispatchArgs {
        DispatchArgs {
            owns: vec![],
            detach: false,
            build_lane: None,
            detach_log_dir: None,
            agent: AgentKind::AcpGrok,
            task_id: Some(9),
            prompt_file: None,
            session_id: None,
            resume: false,
            cwd: None,
            budget_usd: None,
            timeout_sec: None,
            permission_mode: None,
            model: None,
            thinking: None,
            tools: None,
            exclude_tools: None,
            session_dir: None,
            list_models: None,
            issue: None,
            machine: None,
            json: false,
        }
    }

    #[test]
    fn acp_dispatch_refuses_substitutes_and_unenforceable_options() {
        assert!(validate_args(&acp_args()).is_ok());

        let mut detach = acp_args();
        detach.detach = true;
        let error = validate_args(&detach).unwrap_err();
        assert!(error.to_string().contains("--detach"), "{error}");

        let mut prompt = acp_args();
        prompt.prompt_file = Some("prompt.txt".into());
        let error = validate_args(&prompt).unwrap_err();
        assert!(error.to_string().contains("--prompt-file"), "{error}");

        let mut session = acp_args();
        session.session_id = Some("s".into());
        let error = validate_args(&session).unwrap_err();
        assert!(error.to_string().contains("--session-id"), "{error}");

        let mut resume = acp_args();
        resume.resume = true;
        let error = validate_args(&resume).unwrap_err();
        assert!(error.to_string().contains("--resume"), "{error}");

        let mut legacy = acp_args();
        legacy.model = Some("m".into());
        legacy.thinking = Some("high".into());
        let error = validate_args(&legacy).unwrap_err();
        assert!(
            error.to_string().contains("legacy backend options"),
            "{error}"
        );
    }

    #[test]
    fn scope_roots_reject_globs_absolute_traversal_and_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("work")).unwrap();

        assert!(scope_roots(root.path(), &[]).is_err());
        assert!(scope_roots(root.path(), &["*.rs".into()]).is_err());
        assert!(scope_roots(root.path(), &["../outside".into()]).is_err());
        assert!(scope_roots(root.path(), &["nope".into()]).is_err());
        let absolute = root.path().join("work");
        assert!(scope_roots(root.path(), &[absolute.display().to_string()]).is_err());

        let roots = scope_roots(root.path(), &["work".into()]).unwrap();
        assert_eq!(
            roots,
            vec![root.path().join("work").canonicalize().unwrap()]
        );
    }

    #[test]
    fn live_peer_roots_ignore_globs_and_missing_paths_but_include_running_peers() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("peer")).unwrap();

        let mut peer = test_task();
        peer.task_id = 8;
        peer.scope_paths = vec!["peer".into(), "glob-*.rs".into(), "missing-dir".into()];
        let mine = test_task();

        let peers = live_peer_roots(root.path(), &[peer, mine], 9).unwrap();
        assert_eq!(
            peers,
            vec![root.path().join("peer").canonicalize().unwrap()]
        );
    }

    fn test_task() -> TaskView {
        TaskView {
            task_id: 9,
            title: "ACP task".into(),
            assignee: Some("worker".into()),
            agent_kind: None,
            after: vec![8],
            scope_paths: vec!["work".into()],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            status: TaskStatus::Running,
            attempts: 1,
            receipt: None,
            evidence_paths: vec![],
            acp_session_id: None,
            session_id: None,
            session_agent_kind: None,
            session_attempt: None,
            failure_reason: None,
            created_ts: String::new(),
            updated_ts: String::new(),
            created_event_id: String::new(),
        }
    }
}
