use std::path::Path;

use crate::parse::*;
use crate::signals::SubagentSummary;

/// Best-effort write of a `commit` event to the workspace ledger.
/// Uses try-lock: silently skips if workspace is locked by another process.
pub(super) fn try_write_commit_event(raw: &serde_json::Value, msg: &str) {
    let cwd = get_str(raw, "cwd");
    if cwd.is_empty() {
        return;
    }
    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(&cwd)) else {
        return;
    };
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return;
    };
    let Ok(_lock) = edda_ledger::WorkspaceLock::acquire(&ledger.paths) else {
        return; // locked by another process — skip
    };
    let Ok(branch) = ledger.head_branch() else {
        return;
    };
    let Ok(parent_hash) = ledger.last_event_hash() else {
        return;
    };
    let mut params = edda_core::event::CommitEventParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        title: msg,
        purpose: None,
        prev_summary: "",
        contribution: msg,
        evidence: vec![],
        labels: vec!["auto_detect".to_string()],
    };
    if let Ok(event) = edda_core::event::new_commit_event(&mut params) {
        let _ = ledger.append_event(&event);
    }
}

/// Best-effort write of a `merge` event to the workspace ledger.
/// Uses try-lock: silently skips if workspace is locked by another process.
pub(super) fn try_write_merge_event(raw: &serde_json::Value, src: &str, strategy: &str) {
    let cwd = get_str(raw, "cwd");
    if cwd.is_empty() {
        return;
    }
    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(&cwd)) else {
        return;
    };
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return;
    };
    let Ok(_lock) = edda_ledger::WorkspaceLock::acquire(&ledger.paths) else {
        return;
    };
    let Ok(branch) = ledger.head_branch() else {
        return;
    };
    let Ok(parent_hash) = ledger.last_event_hash() else {
        return;
    };
    if let Ok(event) = edda_core::event::new_merge_event(
        &branch,
        parent_hash.as_deref(),
        src,
        &branch,
        strategy,
        &[],
    ) {
        let _ = ledger.append_event(&event);
    }
}

/// Check if current directory is a karvi project (has server/board.json).
pub(super) fn is_karvi_project(cwd: &str) -> bool {
    Path::new(cwd).join("server/board.json").exists()
}

/// Extract karvi task ID from branch name (e.g., "feat/task-T2-auth" -> "T2").
pub(super) fn extract_task_id(branch: &str) -> Option<String> {
    let re = regex::Regex::new(r"T\d+").ok()?;
    re.find(branch).map(|m| m.as_str().to_string())
}

/// Post milestone signal to karvi API (fire-and-forget, best-effort).
pub(super) fn try_post_karvi_signal(
    cwd: &str,
    signal: &crate::nudge::NudgeSignal,
    session_id: &str,
    project_id: &str,
) {
    let branch = crate::peers::detect_git_branch_in(cwd);
    let branch_str = branch.as_deref().unwrap_or("");

    let task_ids: Vec<String> = extract_task_id(branch_str)
        .map(|id| vec![id])
        .unwrap_or_default();

    let (signal_type, content) = match signal {
        crate::nudge::NudgeSignal::Commit(msg) => ("COMMIT", format!("Committed: {}", msg)),
        crate::nudge::NudgeSignal::Merge(src, strategy) => {
            ("MERGE", format!("Merged {} ({})", src, strategy))
        }
        crate::nudge::NudgeSignal::DependencyAdd(pkg) => {
            ("DEPENDENCY", format!("Added dependency: {}", pkg))
        }
        crate::nudge::NudgeSignal::ConfigChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            ("CONFIG", format!("Modified config: {}", name))
        }
        crate::nudge::NudgeSignal::SchemaChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            ("SCHEMA", format!("Modified schema: {}", name))
        }
        crate::nudge::NudgeSignal::NewModule(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            ("MODULE", format!("Created module: {}", name))
        }
        crate::nudge::NudgeSignal::SelfRecord => return, // Don't post self-record
    };

    let karvi_url =
        std::env::var("KARVI_URL").unwrap_or_else(|_| "http://localhost:3461".to_string());
    let url = format!("{}/api/signals", karvi_url);

    let payload = serde_json::json!({
        "type": "agent_milestone",
        "by": format!("edda:{}", session_id),
        "content": content,
        "refs": task_ids,
        "data": {
            "signal_type": signal_type,
            "branch": branch_str,
            "session_id": session_id,
            "project_id": project_id,
        }
    });

    // Fire-and-forget HTTP POST with timeout
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .new_agent();

    if let Err(e) = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .send(payload.to_string())
    {
        tracing::warn!(error = %e, "karvi write-back failed");
    }
}

/// Best-effort write of a task-completed `note` event to workspace ledger.
/// Uses try-lock: silently skips if workspace is locked by another process.
pub(super) fn try_write_task_completed_note_event(cwd: &str, task_id: &str, task_subject: &str) {
    if cwd.is_empty() || task_id.is_empty() {
        return;
    }

    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(cwd)) else {
        return;
    };
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return;
    };
    let Ok(_lock) = edda_ledger::WorkspaceLock::acquire(&ledger.paths) else {
        return;
    };
    let Ok(branch) = ledger.head_branch() else {
        return;
    };
    let Ok(parent_hash) = ledger.last_event_hash() else {
        return;
    };

    let text = if task_subject.is_empty() {
        format!("Task completed: {task_id}")
    } else {
        format!("Task completed: {task_subject} ({task_id})")
    };

    let tags = vec!["session".to_string(), "task".to_string()];
    if let Ok(event) =
        edda_core::event::new_note_event(&branch, parent_hash.as_deref(), "agent", &text, &tags)
    {
        let _ = ledger.append_event(&event);
    }
}

/// Best-effort write of a sub-agent completion `note` event to workspace ledger.
/// Uses try-lock: silently skips if workspace is locked by another process.
pub(super) fn try_write_subagent_completed_note_event(
    cwd: &str,
    agent_id: &str,
    agent_type: &str,
    summary: &SubagentSummary,
) {
    if cwd.is_empty() || agent_id.is_empty() {
        return;
    }

    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(cwd)) else {
        return;
    };
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return;
    };
    let Ok(_lock) = edda_ledger::WorkspaceLock::acquire(&ledger.paths) else {
        return;
    };
    let Ok(branch) = ledger.head_branch() else {
        return;
    };
    let Ok(parent_hash) = ledger.last_event_hash() else {
        return;
    };

    let mut details = Vec::new();
    if !summary.summary.is_empty() {
        details.push(summary.summary.clone());
    }
    if !summary.files_touched.is_empty() {
        details.push(format!("{} files", summary.files_touched.len()));
    }
    if !summary.commits.is_empty() {
        details.push(format!("{} commits", summary.commits.len()));
    }
    if !summary.decisions.is_empty() {
        details.push(format!("{} decisions", summary.decisions.len()));
    }

    let agent_label = if agent_type.is_empty() {
        agent_id.to_string()
    } else {
        format!("{agent_type}:{agent_id}")
    };
    let text = if details.is_empty() {
        format!("Sub-agent completed: {agent_label}")
    } else {
        format!(
            "Sub-agent completed: {agent_label} — {}",
            details.join(", ")
        )
    };

    let tags = vec!["session".to_string(), "subagent".to_string()];
    if let Ok(event) =
        edda_core::event::new_note_event(&branch, parent_hash.as_deref(), "agent", &text, &tags)
    {
        let _ = ledger.append_event(&event);
    }
}
