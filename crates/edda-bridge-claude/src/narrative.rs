use crate::signals::{
    load_state_vec, render_blocking_section, render_focus_section, CommitInfo, FailedBashCmd,
    FileEditCount, TaskSnapshot,
};

// ── L1 Narrative Composition ──

/// Compose an L1 narrative from all available signals.
///
/// L0 = raw signal lists (WHAT happened)
/// L1 = composed narrative (WHAT + WHY + WHERE STUCK)
///
/// Order: Focus → Blocking → Tasks → Activity Summary
/// L1 sections come first (high-value), raw signals compressed into summary.
pub(crate) fn compose_narrative(project_id: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    // L1: Focus detection (WHERE)
    if let Some(focus) = render_focus_section(project_id) {
        parts.push(focus);
    }

    // L1: Blocking detection (WHERE STUCK)
    if let Some(blocking) = render_blocking_section(project_id) {
        parts.push(blocking);
    }

    // L1: Active tasks (WHAT doing now)
    if let Some(tasks) = render_active_tasks_compact(project_id) {
        parts.push(tasks);
    }

    // L1: Session activity summary (compressed WHAT happened)
    if let Some(activity) = render_activity_summary(project_id) {
        parts.push(activity);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Minimal narrative for conductor mode: activity summary only (1-2 lines).
/// See CONDUCTOR-SPEC.md §10.2.
pub(crate) fn compose_narrative_minimal(project_id: &str) -> Option<String> {
    render_activity_summary(project_id)
}

/// Render active tasks in compact format (in_progress only, no completed noise).
fn render_active_tasks_compact(project_id: &str) -> Option<String> {
    let tasks: Vec<TaskSnapshot> = load_state_vec(project_id, "active_tasks.json", "tasks");
    let active: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|t| t.status == "in_progress")
        .collect();
    let pending: Vec<&TaskSnapshot> = tasks.iter().filter(|t| t.status == "pending").collect();

    if active.is_empty() && pending.is_empty() {
        return None;
    }

    let mut lines = vec!["## Tasks".to_string()];
    for task in active.iter().take(3) {
        lines.push(format!("- \u{1f504} #{} {}", task.id, task.subject));
    }
    for task in pending.iter().take(2) {
        lines.push(format!("- \u{2b1c} #{} {}", task.id, task.subject));
    }
    let remaining = (active.len().saturating_sub(3)) + (pending.len().saturating_sub(2));
    if remaining > 0 {
        lines.push(format!("  ...and {remaining} more"));
    }

    Some(lines.join("\n"))
}

/// Render a compact activity summary (files + commits in one section).
fn render_activity_summary(project_id: &str) -> Option<String> {
    let files: Vec<FileEditCount> = load_state_vec(project_id, "files_modified.json", "files");
    let commits: Vec<CommitInfo> = load_state_vec(project_id, "recent_commits.json", "commits");
    let failed: Vec<FailedBashCmd> =
        load_state_vec(project_id, "failed_commands.json", "failed_commands");

    if files.is_empty() && commits.is_empty() {
        return None;
    }

    let mut lines = vec!["## Session Activity".to_string()];

    // Compact file summary with optional velocity
    if !files.is_empty() {
        let total_edits: usize = files.iter().map(|f| f.count).sum();
        let edit_label = if total_edits == 1 { "edit" } else { "edits" };
        let velocity = read_turn_count(project_id)
            .filter(|&t| t > 0)
            .map(|turns| format!(" over {turns} turns"))
            .unwrap_or_default();
        lines.push(format!(
            "- {} files modified ({} {}{})",
            files.len(),
            total_edits,
            edit_label,
            velocity
        ));
    }

    // Compact commit summary with latest
    if !commits.is_empty() {
        if let Some(latest) = commits.last() {
            lines.push(format!(
                "- {} commits (latest: {} {})",
                commits.len(),
                latest.hash,
                truncate_str(&latest.message, 50)
            ));
        }
    }

    // Failed command count
    if !failed.is_empty() {
        let total_failures: usize = failed.iter().map(|f| f.count).sum();
        lines.push(format!(
            "- {} command failures",
            total_failures
        ));
    }

    Some(lines.join("\n"))
}

/// Read turn count from the hot pack metadata file.
fn read_turn_count(project_id: &str) -> Option<usize> {
    let meta_path = edda_store::project_dir(project_id)
        .join("packs")
        .join("hot.meta.json");
    let content = std::fs::read_to_string(meta_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("turn_count").and_then(|v| v.as_u64()).map(|v| v as usize)
}

/// Truncate a string to max_len chars.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len - 3).collect();
        format!("{truncated}...")
    }
}

// ── Previous Session Digest ──


#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{save_session_signals, SessionSignals};
    use std::fs;

    #[test]
    fn narrative_empty_returns_none() {
        let pid = "test_narrative_empty";
        let _ = edda_store::ensure_dirs(pid);
        let result = compose_narrative(pid);
        assert!(result.is_none());
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn compose_narrative_minimal_returns_activity_only() {
        let pid = "test_narrative_minimal";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Fix bug".into(),
                status: "in_progress".into(),
            }],
            files_modified: vec![FileEditCount {
                path: "src/lib.rs".into(),
                count: 3,
            }],
            commits: vec![CommitInfo {
                hash: "abc1234".into(),
                message: "fix: bug".into(),
            }],
            failed_commands: vec![],
        };
        save_session_signals(pid, "test-session", &signals);

        // compose_narrative_minimal should return only activity summary, not tasks/focus
        let result = compose_narrative_minimal(pid);
        assert!(result.is_some(), "should return activity summary");
        let text = result.unwrap();
        assert!(text.contains("Session Activity"), "should have activity: {text}");
        assert!(text.contains("1 files modified"), "should have files: {text}");
        assert!(!text.contains("## Tasks"), "should NOT have tasks section: {text}");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn compose_narrative_minimal_none_when_empty() {
        let pid = "test_narrative_minimal_empty";
        let _ = edda_store::ensure_dirs(pid);
        let result = compose_narrative_minimal(pid);
        assert!(result.is_none(), "empty signals should return None");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn narrative_with_signals_has_structure() {
        let pid = "test_narrative_struct";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![
                TaskSnapshot {
                    id: "1".into(),
                    subject: "Fix the auth bug".into(),
                    status: "in_progress".into(),
                },
                TaskSnapshot {
                    id: "2".into(),
                    subject: "Add tests".into(),
                    status: "pending".into(),
                },
            ],
            files_modified: vec![
                FileEditCount { path: "/repo/crates/edda-bridge-claude/src/dispatch.rs".into(), count: 10 },
                FileEditCount { path: "/repo/crates/edda-bridge-claude/src/signals.rs".into(), count: 5 },
                FileEditCount { path: "/repo/crates/edda-bridge-claude/src/lib.rs".into(), count: 2 },
            ],
            commits: vec![
                CommitInfo { hash: "abc1234".into(), message: "fix: auth flow".into() },
            ],
            failed_commands: vec![
                FailedBashCmd {
                    command_base: "cargo test -p edda-bridge-claude".into(),
                    stderr_snippet: "thread 'test' panicked".into(),
                    count: 2,
                },
            ],
        };
        save_session_signals(pid, "test-session", &signals);

        let narrative = compose_narrative(pid).unwrap();

        // L1 sections should come first
        let focus_pos = narrative.find("Current Focus");
        let blocking_pos = narrative.find("Blocking");
        let tasks_pos = narrative.find("## Tasks");
        let activity_pos = narrative.find("Session Activity");

        // Focus before blocking before tasks before activity
        if let (Some(f), Some(b)) = (focus_pos, blocking_pos) {
            assert!(f < b, "Focus should come before Blocking");
        }
        if let (Some(b), Some(t)) = (blocking_pos, tasks_pos) {
            assert!(b < t, "Blocking should come before Tasks");
        }
        if let (Some(t), Some(a)) = (tasks_pos, activity_pos) {
            assert!(t < a, "Tasks should come before Activity");
        }

        // Content checks
        assert!(narrative.contains("Fix the auth bug"), "should include task subject");
        assert!(narrative.contains("3 files modified"), "should include file count");
        assert!(narrative.contains("1 commits"), "should include commit count");
        assert!(narrative.contains("cargo test"), "should include failed cmd");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn activity_summary_compact() {
        let pid = "test_activity_compact";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![],
            files_modified: vec![
                FileEditCount { path: "a.rs".into(), count: 5 },
                FileEditCount { path: "b.rs".into(), count: 3 },
            ],
            commits: vec![
                CommitInfo { hash: "aaa".into(), message: "first".into() },
                CommitInfo { hash: "bbb".into(), message: "second commit".into() },
            ],
            failed_commands: vec![],
        };
        save_session_signals(pid, "test-session", &signals);

        let summary = render_activity_summary(pid).unwrap();
        assert!(summary.contains("2 files modified (8 edits)"));
        assert!(summary.contains("2 commits (latest: bbb second commit)"));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn activity_summary_with_velocity() {
        let pid = "test_activity_velocity";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![],
            files_modified: vec![
                FileEditCount { path: "a.rs".into(), count: 50 },
                FileEditCount { path: "b.rs".into(), count: 30 },
            ],
            commits: vec![],
            failed_commands: vec![],
        };
        save_session_signals(pid, "test-session", &signals);

        // Write a hot.meta.json with turn_count
        let packs_dir = edda_store::project_dir(pid).join("packs");
        let _ = fs::create_dir_all(&packs_dir);
        let meta = serde_json::json!({
            "project_id": pid,
            "session_id": "test-session",
            "git_branch": "main",
            "turn_count": 10,
            "budget_chars": 8000
        });
        fs::write(packs_dir.join("hot.meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();

        let summary = render_activity_summary(pid).unwrap();
        assert!(summary.contains("over 10 turns"), "should show velocity: {summary}");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn truncate_str_short_unchanged() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long_truncated() {
        let result = truncate_str("this is a long message", 15);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 15);
    }

}
