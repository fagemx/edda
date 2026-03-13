use std::fs;

// Imports from dispatch/mod.rs
use super::{
    apply_context_budget, context_budget, has_active_peers, hook_entrypoint_from_stdin,
    increment_counter, is_same_as_last_inject, mark_nudge_sent, read_counter,
    render_workspace_section, render_write_back_protocol, set_compact_pending,
    take_compact_pending, wrap_context_boundary, write_inject_hash, write_peer_count, HookResult,
    EDDA_BOUNDARY_END, EDDA_BOUNDARY_START,
};
// Imports from sub-modules
use super::events::{
    extract_task_id, is_karvi_project, try_write_commit_event, try_write_merge_event,
};
use super::helpers::{inject_karvi_brief, read_project_state, render_active_plan_from_dir};
use super::session::{
    cleanup_session_state, collect_session_end_warnings, dispatch_session_end,
    dispatch_session_start, dispatch_user_prompt_submit, dispatch_with_workspace_only,
};
use super::tools::{
    check_offlimits, check_pending_requests, dispatch_post_tool_use, dispatch_pre_tool_use,
};
// Imports from crate
use crate::parse::resolve_project_id;
use crate::render;
use crate::signals::{
    save_session_signals, CommitInfo, FileEditCount, SessionSignals, TaskSnapshot,
};

#[test]
fn hook_entrypoint_session_start() {
    crate::with_env_guard(
        &[
            ("EDDA_BRIDGE_AUTO_DIGEST", Some("0")),
            ("EDDA_PLANS_DIR", Some("/nonexistent/plans/dir")),
        ],
        || {
            let stdin = r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":".","transcript_path":"","permission_mode":"default"}"#;
            let result = hook_entrypoint_from_stdin(stdin).unwrap();
            assert!(
                result.stdout.is_some(),
                "write-back protocol should always inject"
            );
            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            let ctx = output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(
                ctx.contains("Write-Back Protocol"),
                "should contain write-back protocol"
            );
            assert!(result.stderr.is_none());
        },
    );
}

#[test]
fn hook_entrypoint_camel_case_input() {
    crate::with_env_guard(&[("EDDA_CLAUDE_AUTO_APPROVE", Some("1"))], || {
        // Claude Code sends camelCase JSON
        let stdin = r#"{"sessionId":"s-camel","hookEventName":"PreToolUse","cwd":".","toolName":"Bash","toolUseId":"tu1"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
    });
}

#[test]
fn hook_entrypoint_pre_tool_use_auto_approve() {
    crate::with_env_guard(&[("EDDA_CLAUDE_AUTO_APPROVE", Some("1"))], || {
        let stdin = r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":".","tool_name":"Bash","tool_use_id":"tu1"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
    });
}

#[test]
fn hook_entrypoint_post_tool_use_no_output() {
    let stdin = r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":".","tool_name":"Bash","tool_use_id":"tu1"}"#;
    let result = hook_entrypoint_from_stdin(stdin).unwrap();
    assert!(result.stdout.is_none());
    assert!(result.stderr.is_none());
}

// transform_context_strips_header_and_cite → moved to render::tests

#[test]
fn render_workspace_section_no_edda_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let result = render_workspace_section(tmp.path().to_str().unwrap(), 2000);
    assert!(result.is_none());
}

#[test]
fn pre_tool_use_with_patterns() {
    // Setup: create temp dir with .edda/patterns/
    let tmp = tempfile::tempdir().unwrap();
    let edda_dir = tmp.path().join(".edda");
    let patterns_dir = edda_dir.join("patterns");
    std::fs::create_dir_all(&patterns_dir).unwrap();

    // Write a test pattern
    let pat = serde_json::json!({
        "id": "test-no-db",
        "trigger": { "file_glob": ["**/*.test.*"], "keywords": [] },
        "rule": "Tests should use API, not direct DB",
        "source": "PR #2587",
        "metadata": { "status": "active", "hit_count": 0 }
    });
    std::fs::write(
        patterns_dir.join("test-no-db.json"),
        serde_json::to_string_pretty(&pat).unwrap(),
    )
    .unwrap();

    // Enable patterns
    crate::with_env_guard(
        &[
            ("EDDA_PATTERNS_ENABLED", Some("1")),
            ("EDDA_CLAUDE_AUTO_APPROVE", Some("1")),
        ],
        || {
            let stdin = serde_json::json!({
                "session_id": "s1",
                "hook_event_name": "PreToolUse",
                "cwd": tmp.path().to_str().unwrap(),
                "tool_name": "Edit",
                "tool_use_id": "tu1",
                "tool_input": {
                    "file_path": "src/foo.test.ts",
                    "old_string": "old",
                    "new_string": "new"
                }
            });

            let result =
                hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
            assert!(result.stdout.is_some());
            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
            let ctx = output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(ctx.contains("test-no-db"));
            assert!(ctx.contains("API"));
        },
    );
}

#[test]
fn compact_pending_flag_lifecycle() {
    // Use a unique fake project id to avoid collisions with real state
    let pid = "test_compact_pending_00";
    let _ = edda_store::ensure_dirs(pid);

    // Initially no flag
    assert!(!take_compact_pending(pid));

    // Set flag
    set_compact_pending(pid);
    let cp_path = edda_store::project_dir(pid)
        .join("state")
        .join("compact_pending");
    assert!(cp_path.exists());

    // Take clears it and returns true once
    assert!(take_compact_pending(pid));
    assert!(!cp_path.exists());

    // Second take returns false
    assert!(!take_compact_pending(pid));

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Karvi Brief Injection Tests ──

#[test]
fn inject_karvi_brief_non_karvi_project() {
    let tmp = tempfile::tempdir().unwrap();
    let result = inject_karvi_brief(tmp.path().to_str().unwrap());
    assert!(result.is_none(), "Should return None for non-karvi project");
}

#[test]
fn inject_karvi_brief_no_task_id_in_branch() {
    let tmp = tempfile::tempdir().unwrap();

    // Create karvi project marker
    let board_path = tmp.path().join("server/board.json");
    fs::create_dir_all(board_path.parent().unwrap()).unwrap();
    fs::write(&board_path, "{}").unwrap();

    // Create git repo with branch without task ID
    let repo_dir = tmp.path();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["checkout", "-b", "feature-branch"])
        .current_dir(repo_dir)
        .output();

    let result = inject_karvi_brief(repo_dir.to_str().unwrap());
    assert!(
        result.is_none(),
        "Should return None when branch has no task ID"
    );
}

#[test]
fn inject_karvi_brief_missing_brief_file() {
    let tmp = tempfile::tempdir().unwrap();

    // Create karvi project marker
    let board_path = tmp.path().join("server/board.json");
    fs::create_dir_all(board_path.parent().unwrap()).unwrap();
    fs::write(&board_path, "{}").unwrap();

    // Create git repo with task ID in branch
    let repo_dir = tmp.path();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["checkout", "-b", "T123-feature"])
        .current_dir(repo_dir)
        .output();

    // Don't create brief file
    let result = inject_karvi_brief(repo_dir.to_str().unwrap());
    assert!(
        result.is_none(),
        "Should return None when brief file is missing"
    );
}

#[test]
fn inject_karvi_brief_success() {
    let tmp = tempfile::tempdir().unwrap();

    // Create karvi project marker
    let board_path = tmp.path().join("server/board.json");
    fs::create_dir_all(board_path.parent().unwrap()).unwrap();
    fs::write(&board_path, "{}").unwrap();

    // Create git repo with task ID in branch
    let repo_dir = tmp.path();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_dir)
        .output();
    // Make initial commit so we can create branches
    fs::write(repo_dir.join("README.md"), "# test").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["checkout", "-b", "T42-add-feature"])
        .current_dir(repo_dir)
        .output();

    // Create brief file
    let briefs_dir = tmp.path().join("server/briefs");
    fs::create_dir_all(&briefs_dir).unwrap();
    let brief_content = "# Task Brief\n\nImplement the feature with these requirements...";
    fs::write(briefs_dir.join("T42.md"), brief_content).unwrap();

    let result = inject_karvi_brief(repo_dir.to_str().unwrap());
    assert!(result.is_some(), "Should return brief content");

    let brief = result.unwrap();
    assert!(
        brief.starts_with("[karvi task brief: T42]\n"),
        "Should start with header"
    );
    assert!(
        brief.contains("# Task Brief"),
        "Should contain brief content"
    );
    assert!(
        brief.contains("Implement the feature"),
        "Should contain requirements"
    );
}

#[test]
fn inject_karvi_brief_truncates_long_content() {
    let tmp = tempfile::tempdir().unwrap();

    // Create karvi project marker
    let board_path = tmp.path().join("server/board.json");
    fs::create_dir_all(board_path.parent().unwrap()).unwrap();
    fs::write(&board_path, "{}").unwrap();

    // Create git repo with task ID in branch
    let repo_dir = tmp.path();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_dir)
        .output();
    // Make initial commit so we can create branches
    fs::write(repo_dir.join("README.md"), "# test").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["checkout", "-b", "T99-long-brief"])
        .current_dir(repo_dir)
        .output();

    // Create long brief file (> 2000 chars)
    let briefs_dir = tmp.path().join("server/briefs");
    fs::create_dir_all(&briefs_dir).unwrap();
    let long_content: String = "X".repeat(3000);
    fs::write(briefs_dir.join("T99.md"), &long_content).unwrap();

    let result = inject_karvi_brief(repo_dir.to_str().unwrap());
    assert!(result.is_some(), "Should return brief content");

    let brief = result.unwrap();
    // Header + content should be truncated
    // Header is "[karvi task brief: T99]\n" = 24 chars, content is 2000 chars
    assert!(
        brief.len() <= 2030,
        "Brief should be truncated to ~2000 chars + header, got {}",
        brief.len()
    );
    assert!(
        brief.starts_with("[karvi task brief: T99]\n"),
        "Should start with header"
    );
}

#[test]
fn active_plan_selects_latest_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let plans = tmp.path().join("plans");
    fs::create_dir_all(&plans).unwrap();

    let old_plan = plans.join("old-plan.md");
    fs::write(&old_plan, "# Old Plan\nThis is old").unwrap();

    // Small sleep to ensure different mtime
    std::thread::sleep(std::time::Duration::from_millis(50));

    let new_plan = plans.join("new-plan.md");
    fs::write(&new_plan, "# New Plan\nThis is new").unwrap();

    let section = render_active_plan_from_dir(&plans, None).unwrap();
    assert!(section.contains("new-plan.md"), "Should select newest plan");
    assert!(section.contains("# New Plan"));
    assert!(!section.contains("# Old Plan"));
}

#[test]
fn active_plan_truncates_to_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let plans = tmp.path().join("plans");
    fs::create_dir_all(&plans).unwrap();

    let mut content = String::new();
    for i in 0..100 {
        content.push_str(&format!("## Step {i}: do something important\n"));
    }
    fs::write(plans.join("big-plan.md"), &content).unwrap();

    let section = render_active_plan_from_dir(&plans, None).unwrap();
    assert!(section.contains("...(truncated)"));
    assert!(!section.contains("Step 99"));
    // Excerpt should stay under budget (700 chars) + header overhead
    assert!(section.len() < 1000);
}

#[test]
fn session_start_includes_signals() {
    let pid = "test_session_start_signals";
    let _ = edda_store::ensure_dirs(pid);

    // Save session signals (tasks, files, commits)
    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Fix bug".into(),
            status: "in_progress".into(),
        }],
        files_modified: vec![FileEditCount {
            path: "/repo/crates/foo/src/lib.rs".into(),
            count: 3,
        }],
        commits: vec![CommitInfo {
            hash: "abc1234".into(),
            message: "fix: the bug".into(),
        }],
        failed_commands: vec![],
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    // Write a minimal hot pack so dispatch_session_start has something to read
    let pack_dir = edda_store::project_dir(pid).join("packs");
    let _ = fs::create_dir_all(&pack_dir);
    let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

    crate::with_env_guard(
        &[("EDDA_PLANS_DIR", Some("/nonexistent/plans/dir"))],
        || {
            let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
            assert!(result.stdout.is_some(), "should return output");

            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            let ctx = output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();

            assert!(
                ctx.contains("## Tasks"),
                "should contain Tasks section:\n{ctx}"
            );
            assert!(
                ctx.contains("Fix bug"),
                "should contain task subject:\n{ctx}"
            );
            assert!(
                ctx.contains("Session Activity"),
                "should contain Session Activity section:\n{ctx}"
            );
            assert!(
                ctx.contains("1 files modified"),
                "should contain file count:\n{ctx}"
            );
            assert!(
                ctx.contains("abc1234"),
                "should contain commit hash:\n{ctx}"
            );
        },
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn session_start_no_signals_no_extra_sections() {
    let pid = "test_session_start_no_signals";
    let _ = edda_store::ensure_dirs(pid);

    // Write a minimal hot pack, no signals
    let pack_dir = edda_store::project_dir(pid).join("packs");
    let _ = fs::create_dir_all(&pack_dir);
    let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

    crate::with_env_guard(
        &[("EDDA_PLANS_DIR", Some("/nonexistent/plans/dir"))],
        || {
            let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
            assert!(result.stdout.is_some());

            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            let ctx = output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();

            assert!(
                !ctx.contains("## Tasks"),
                "should not contain Tasks when empty"
            );
            assert!(
                !ctx.contains("Session Activity"),
                "should not contain Session Activity when empty"
            );
            assert!(
                !ctx.contains("Current Focus"),
                "should not contain Focus when empty"
            );
        },
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn active_plan_renders_from_custom_dir() {
    // Test render_active_plan_from_dir directly (avoids env var race in parallel tests)
    let tmp = tempfile::tempdir().unwrap();
    let plans_dir = tmp.path().join("plans");
    fs::create_dir_all(&plans_dir).unwrap();
    fs::write(
        plans_dir.join("test-plan.md"),
        "# My Plan\n\n## Step 1\nDo something\n",
    )
    .unwrap();

    let section = render_active_plan_from_dir(&plans_dir, None).unwrap();
    assert!(section.contains("## Active Plan"));
    assert!(section.contains("test-plan.md"));
    assert!(section.contains("# My Plan"));
    assert!(section.contains("## Step 1"));
}

// ── HookResult tests ──

#[test]
fn hook_result_output_has_stdout_only() {
    let r = HookResult::output("hello".into());
    assert_eq!(r.stdout.as_deref(), Some("hello"));
    assert!(r.stderr.is_none());
}

#[test]
fn hook_result_warning_has_stderr_only() {
    let r = HookResult::warning("oops".into());
    assert!(r.stdout.is_none());
    assert_eq!(r.stderr.as_deref(), Some("oops"));
}

#[test]
fn hook_result_empty_has_nothing() {
    let r = HookResult::empty();
    assert!(r.stdout.is_none());
    assert!(r.stderr.is_none());
}

#[test]
fn hook_result_from_option_some() {
    let r: HookResult = Some("data".to_string()).into();
    assert_eq!(r.stdout.as_deref(), Some("data"));
    assert!(r.stderr.is_none());
}

#[test]
fn hook_result_from_option_none() {
    let r: HookResult = None.into();
    assert!(r.stdout.is_none());
    assert!(r.stderr.is_none());
}

// ── Injection Dedup tests ──

#[test]
fn dedup_skips_identical_context() {
    let pid = "test_dedup_skip";
    let sid = "sess-dedup-1";
    let _ = edda_store::ensure_dirs(pid);

    // First write sets the hash
    write_inject_hash(pid, sid, "hello workspace");
    // Same content → should be detected as identical
    assert!(is_same_as_last_inject(pid, sid, "hello workspace"));

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn dedup_injects_changed_context() {
    let pid = "test_dedup_changed";
    let sid = "sess-dedup-2";
    let _ = edda_store::ensure_dirs(pid);

    write_inject_hash(pid, sid, "version 1");
    // Different content → should NOT be identical
    assert!(!is_same_as_last_inject(pid, sid, "version 2"));

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn dedup_first_call_always_injects() {
    let pid = "test_dedup_first";
    let sid = "sess-dedup-3";
    let _ = edda_store::ensure_dirs(pid);

    // No prior hash → should return false (inject)
    assert!(!is_same_as_last_inject(pid, sid, "anything"));

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── SessionEnd tests ──

#[test]
fn session_end_cleans_state() {
    let pid = "test_session_end_clean";
    let sid = "sess-end-1";
    let _ = edda_store::ensure_dirs(pid);

    // Create state files that should be cleaned
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);
    fs::write(state_dir.join(format!("inject_hash.{sid}")), "abcd").unwrap();
    fs::write(state_dir.join("compact_pending"), "1").unwrap();

    cleanup_session_state(pid, sid, false);

    assert!(
        !state_dir.join(format!("inject_hash.{sid}")).exists(),
        "inject_hash should be cleaned"
    );
    assert!(
        !state_dir.join("compact_pending").exists(),
        "compact_pending should be cleaned"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn session_end_warns_pending_tasks() {
    let pid = "test_session_end_warn";
    let _ = edda_store::ensure_dirs(pid);

    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    // Write active_tasks with some pending
    let tasks = serde_json::json!([
        {"id": "1", "subject": "Fix bug", "status": "in_progress"},
        {"id": "2", "subject": "Add tests", "status": "pending"},
        {"id": "3", "subject": "Done task", "status": "completed"}
    ]);
    fs::write(
        state_dir.join("active_tasks.json"),
        serde_json::to_string(&tasks).unwrap(),
    )
    .unwrap();

    let warning = collect_session_end_warnings(pid);
    assert!(warning.is_some());
    let w = warning.unwrap();
    assert!(w.contains("2 task(s) still pending"));
    assert!(w.contains("Fix bug"));
    assert!(w.contains("Add tests"));
    assert!(!w.contains("Done task"));

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn session_end_no_warning_when_all_completed() {
    let pid = "test_session_end_no_warn";
    let _ = edda_store::ensure_dirs(pid);

    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    let tasks = serde_json::json!([
        {"id": "1", "subject": "Done", "status": "completed"}
    ]);
    fs::write(
        state_dir.join("active_tasks.json"),
        serde_json::to_string(&tasks).unwrap(),
    )
    .unwrap();

    let warning = collect_session_end_warnings(pid);
    assert!(warning.is_none());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Boundary marker tests ──

#[test]
fn wrap_context_boundary_adds_markers() {
    let content = "hello world";
    let wrapped = wrap_context_boundary(content);
    assert!(wrapped.starts_with(EDDA_BOUNDARY_START));
    assert!(wrapped.ends_with(EDDA_BOUNDARY_END));
    assert!(wrapped.contains("hello world"));
}

#[test]
fn session_start_output_has_boundary_markers() {
    let pid = "test_boundary_session_start";
    let _ = edda_store::ensure_dirs(pid);

    let pack_dir = edda_store::project_dir(pid).join("packs");
    let _ = fs::create_dir_all(&pack_dir);
    let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

    crate::with_env_guard(
        &[("EDDA_PLANS_DIR", Some("/nonexistent/plans/dir"))],
        || {
            let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            let ctx = output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();

            assert!(
                ctx.contains(EDDA_BOUNDARY_START),
                "SessionStart should have edda:start marker"
            );
            assert!(
                ctx.contains(EDDA_BOUNDARY_END),
                "SessionStart should have edda:end marker"
            );
        },
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Token budget tests ──

#[test]
fn apply_context_budget_no_truncation() {
    let content = "short content";
    let result = apply_context_budget(content, 8000);
    assert_eq!(result, content);
}

#[test]
fn apply_context_budget_truncates_long_content() {
    let content = "x".repeat(10000);
    let result = apply_context_budget(&content, 500);
    assert!(result.len() <= 550); // budget + truncation notice
    assert!(result.contains("truncated"));
    assert!(result.contains("500 char budget"));
}

#[test]
fn context_budget_uses_env_var() {
    crate::with_env_guard(&[("EDDA_MAX_CONTEXT_CHARS", Some("1234"))], || {
        let budget = context_budget("");
        assert_eq!(budget, 1234);
    });
}

#[test]
fn context_budget_default_without_config() {
    crate::with_env_guard(&[("EDDA_MAX_CONTEXT_CHARS", None)], || {
        let budget = context_budget("/nonexistent/dir");
        assert_eq!(budget, render::DEFAULT_MAX_CONTEXT_CHARS);
    });
}

// ── Body/Tail Budget Split tests ──

#[test]
fn tail_sections_survive_budget_truncation() {
    // Simulate: large body (10K) + tail (write-back + coord), budget = 8000
    let body = "x".repeat(10000);
    let tail_wb = "\n\n## Write-Back Protocol\nRecord decisions with: `edda decide`";
    let tail_coord = "\n\n## Coordination Protocol\nYou are one of 3 agents.";
    let tail = format!("{tail_wb}{tail_coord}");

    let total_budget: usize = 8000;
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = apply_context_budget(&body, body_budget);
    let final_content = format!("{budgeted_body}{tail}");

    assert!(
        final_content.contains("Write-Back Protocol"),
        "write-back protocol must survive: {}",
        &final_content[final_content.len().saturating_sub(200)..]
    );
    assert!(
        final_content.contains("Coordination Protocol"),
        "coordination protocol must survive: {}",
        &final_content[final_content.len().saturating_sub(200)..]
    );
}

#[test]
fn body_truncated_when_tail_present() {
    let body = "y".repeat(10000);
    let tail = "\n\n## Reserved Section\nThis must appear.";
    let total_budget: usize = 8000;
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = apply_context_budget(&body, body_budget);
    let final_content = format!("{budgeted_body}{tail}");

    // Body portion should be truncated
    assert!(
        budgeted_body.contains("truncated"),
        "body should be truncated"
    );
    // Body portion should fit within body_budget (+ truncation notice overhead)
    assert!(
        budgeted_body.len() <= body_budget + 60,
        "body len {} should be near body_budget {}",
        budgeted_body.len(),
        body_budget
    );
    // Tail must be present and complete
    assert!(
        final_content.ends_with("This must appear."),
        "tail must be at the end: {}",
        &final_content[final_content.len().saturating_sub(100)..]
    );
}

#[test]
fn empty_tail_preserves_existing_behavior() {
    let body = "z".repeat(5000);
    let tail = "";
    let total_budget: usize = 8000;
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = apply_context_budget(&body, body_budget);

    // With empty tail, body should not be truncated (5000 < 8000)
    assert!(
        !budgeted_body.contains("truncated"),
        "body should NOT be truncated when under budget"
    );
    assert_eq!(budgeted_body.len(), 5000);
}

// ── Decision Nudge tests ──

#[test]
fn post_tool_use_commit_triggers_nudge() {
    let pid = "test_nudge_commit";
    let sid = "sess-nudge-1";
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "session_id": sid,
        "hook_event_name": "PostToolUse",
        "cwd": ".",
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: switch to postgres\"" }
    });
    let result = dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert!(result.stdout.is_some(), "should produce nudge output");
    let output: serde_json::Value = serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
    let ctx = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        ctx.contains("edda decide"),
        "nudge should mention edda decide"
    );
    assert!(
        ctx.contains("switch to postgres"),
        "nudge should quote commit msg"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_after_decide_cooldown_still_applies() {
    let pid = "test_nudge_suppressed";
    let sid = "sess-nudge-2";
    let _ = edda_store::ensure_dirs(pid);

    // Agent calls edda decide (SelfRecord) — no longer suppresses all future nudges.
    let decide_raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "edda decide \"db=postgres\"" }
    });
    dispatch_post_tool_use(&decide_raw, pid, sid, ".").unwrap();

    // SelfRecord does NOT set cooldown timestamp, so the first real signal fires.
    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: add redis cache\"" }
    });
    let result = dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert!(
        result.stdout.is_some(),
        "should nudge after decide (no global suppression)"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_cooldown_suppresses() {
    let pid = "test_nudge_cooldown";
    let sid = "sess-nudge-3";
    let _ = edda_store::ensure_dirs(pid);

    // First commit → nudge
    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: first commit\"" }
    });
    let result = dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert!(result.stdout.is_some(), "first commit should nudge");

    // Second commit immediately → no nudge (cooldown)
    let raw2 = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: second commit\"" }
    });
    let result2 = dispatch_post_tool_use(&raw2, pid, sid, ".").unwrap();
    assert!(
        result2.stdout.is_none(),
        "second commit should be suppressed by cooldown"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_self_record_increments_decide_count() {
    let pid = "test_nudge_selfrecord";
    let sid = "sess-nudge-4";
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "edda decide \"db=postgres\" --reason \"need JSONB\"" }
    });
    let result = dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert!(
        result.stdout.is_none(),
        "self-record should not produce output"
    );
    assert_eq!(
        read_counter(pid, sid, "decide_count"),
        1,
        "decide_count should be incremented"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn session_end_cleans_nudge_state() {
    let pid = "test_nudge_cleanup";
    let sid = "sess-nudge-5";
    let _ = edda_store::ensure_dirs(pid);

    mark_nudge_sent(pid, sid);

    let state_dir = edda_store::project_dir(pid).join("state");
    assert!(state_dir.join(format!("nudge_ts.{sid}")).exists());

    cleanup_session_state(pid, sid, false);

    assert!(!state_dir.join(format!("nudge_ts.{sid}")).exists());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn write_back_protocol_always_fires() {
    let dir = tempfile::tempdir().unwrap();
    // No .edda/ → still fires (gate removed)
    let result = render_write_back_protocol(dir.path().to_str().unwrap());
    assert!(result.is_some(), "should fire without .edda/");
    let text = result.unwrap();
    assert!(text.contains("Write-Back Protocol"), "header: {text}");
    assert!(text.contains("edda decide"), "decide: {text}");
    assert!(text.contains("edda note"), "note: {text}");
    assert!(text.contains("--tag session"), "tag: {text}");
}

// ── Write-Back Protocol text tests ──

#[test]
fn write_back_protocol_contains_examples() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".edda")).unwrap();
    let text = render_write_back_protocol(dir.path().to_str().unwrap()).unwrap();
    assert!(text.contains("db.engine=postgres"), "example 1: {text}");
    assert!(text.contains("auth.method=JWT"), "example 2: {text}");
    assert!(text.contains("api.style=REST"), "example 3: {text}");
    assert!(text.contains("Do NOT record"), "anti-examples: {text}");
    assert!(text.contains("edda note"), "note command: {text}");
}

// ── Recall Rate Counter tests ──

#[test]
fn counter_increment_and_read() {
    let pid = "test_counter_ops";
    let sid = "sess-counter-1";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Read non-existent counter returns 0
    assert_eq!(read_counter(pid, sid, "nudge_count"), 0);

    // Increment 3 times, read, assert 3
    increment_counter(pid, sid, "nudge_count");
    increment_counter(pid, sid, "nudge_count");
    increment_counter(pid, sid, "nudge_count");
    assert_eq!(read_counter(pid, sid, "nudge_count"), 3);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_increments_nudge_counter() {
    let pid = "test_nudge_counter";
    let sid = "sess-nudge-cnt-1";
    let _ = edda_store::ensure_dirs(pid);

    // First commit → nudge emitted → counter = 1
    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: first\"" }
    });
    let result = dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert!(result.stdout.is_some(), "should produce nudge");
    assert_eq!(read_counter(pid, sid, "nudge_count"), 1);

    // Reset cooldown by removing nudge_ts
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::remove_file(state_dir.join(format!("nudge_ts.{sid}")));

    // Second commit → nudge emitted → counter = 2
    let raw2 = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: second\"" }
    });
    let result2 = dispatch_post_tool_use(&raw2, pid, sid, ".").unwrap();
    assert!(
        result2.stdout.is_some(),
        "should produce nudge after cooldown reset"
    );
    assert_eq!(read_counter(pid, sid, "nudge_count"), 2);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_increments_decide_counter() {
    let pid = "test_decide_counter";
    let sid = "sess-decide-cnt-1";
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "edda decide \"db=postgres\" --reason \"need JSONB\"" }
    });
    dispatch_post_tool_use(&raw, pid, sid, ".").unwrap();
    assert_eq!(read_counter(pid, sid, "decide_count"), 1);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn session_end_cleans_recall_counters() {
    let pid = "test_counter_cleanup";
    let sid = "sess-counter-clean";
    let _ = edda_store::ensure_dirs(pid);

    // Create counter files
    increment_counter(pid, sid, "nudge_count");
    increment_counter(pid, sid, "decide_count");

    let state_dir = edda_store::project_dir(pid).join("state");
    assert!(state_dir.join(format!("nudge_count.{sid}")).exists());
    assert!(state_dir.join(format!("decide_count.{sid}")).exists());

    cleanup_session_state(pid, sid, false);

    assert!(!state_dir.join(format!("nudge_count.{sid}")).exists());
    assert!(!state_dir.join(format!("decide_count.{sid}")).exists());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn signal_count_incremented_for_all_signals() {
    let pid = "test_signal_count_all";
    let sid = "sess-sig-cnt";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Commit signal → signal_count +1
    let raw1 = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: add auth\"" }
    });
    dispatch_post_tool_use(&raw1, pid, sid, ".").unwrap();
    assert_eq!(read_counter(pid, sid, "signal_count"), 1);

    // SelfRecord signal → signal_count +1 (even though no nudge sent)
    let raw2 = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "edda decide \"db=postgres\"" }
    });
    dispatch_post_tool_use(&raw2, pid, sid, ".").unwrap();
    assert_eq!(read_counter(pid, sid, "signal_count"), 2);

    // DependencyAdd signal → signal_count +1 (suppressed by cooldown)
    let raw3 = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "cargo add serde" }
    });
    dispatch_post_tool_use(&raw3, pid, sid, ".").unwrap();
    assert_eq!(read_counter(pid, sid, "signal_count"), 3);

    // signal_count >= nudge_count always
    assert!(read_counter(pid, sid, "signal_count") >= read_counter(pid, sid, "nudge_count"));

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// nudge_cooldown_env_var_override → moved to state::tests

#[test]
fn session_end_cleans_signal_count() {
    let pid = "test_signal_count_cleanup";
    let sid = "sess-sig-clean";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    increment_counter(pid, sid, "signal_count");

    let state_dir = edda_store::project_dir(pid).join("state");
    assert!(state_dir.join(format!("signal_count.{sid}")).exists());

    cleanup_session_state(pid, sid, false);

    assert!(!state_dir.join(format!("signal_count.{sid}")).exists());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn has_active_peers_false_when_solo() {
    let pid = "test_dispatch_solo_gate";
    let _ = edda_store::ensure_dirs(pid);
    // No heartbeat files → no peers
    assert!(!has_active_peers(pid, "my-session"));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn has_active_peers_true_when_peer_exists() {
    let pid = "test_dispatch_peer_gate";
    let _ = edda_store::ensure_dirs(pid);
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    // Create a fresh peer heartbeat file
    let peer_path = state_dir.join("session.peer-session.json");
    fs::write(&peer_path, r#"{"session_id":"peer-session"}"#).unwrap();

    assert!(has_active_peers(pid, "my-session"));
    // Own session should be excluded
    assert!(!has_active_peers(pid, "peer-session"));

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Issue #148 Gap 3: Cross-session binding visibility via dispatch ──

#[test]
fn cross_session_binding_visible_via_user_prompt_submit() {
    let pid = "test_xsess_bind_vis";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    // Create temp cwd (no .edda/ — workspace section will be None)
    let cwd = std::env::temp_dir().join("edda_xsess_vis_cwd");
    let _ = fs::create_dir_all(&cwd);

    // Multi-session: write heartbeats for s1 and s2
    let signals = crate::signals::SessionSignals::default();
    crate::peers::write_heartbeat(pid, "s1", &signals, Some("auth"));
    crate::peers::write_heartbeat(pid, "s2", &signals, Some("billing"));

    // Session A (s1) writes a binding
    crate::peers::write_binding(pid, "s1", "auth", "db.engine", "postgres");

    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");
    // Session B (s2) dispatches UserPromptSubmit — should see the binding
    let result = dispatch_user_prompt_submit(pid, "s2", "", cwd.to_str().unwrap()).unwrap();
    assert!(
        result.stdout.is_some(),
        "should return output (not dedup-skipped)"
    );

    let output: serde_json::Value = serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
    let ctx = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("db.engine"),
        "should contain binding key, got:\n{ctx}"
    );
    assert!(
        ctx.contains("postgres"),
        "should contain binding value, got:\n{ctx}"
    );

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    crate::peers::remove_heartbeat(pid, "s1");
    crate::peers::remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn user_prompt_submit_dedup_skips_identical_state() {
    let pid = "test_ups_dedup";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let cwd = std::env::temp_dir().join("edda_dedup_cwd");
    let _ = fs::create_dir_all(&cwd);

    // Write a binding so there's something to inject
    crate::peers::write_binding(pid, "s1", "auth", "cache.backend", "redis");

    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");
    // First call — should produce output
    let r1 = dispatch_user_prompt_submit(pid, "dedup-sess", "", cwd.to_str().unwrap()).unwrap();
    assert!(r1.stdout.is_some(), "first call should return output");

    // Second call with identical state — should be dedup-skipped
    let r2 = dispatch_user_prompt_submit(pid, "dedup-sess", "", cwd.to_str().unwrap()).unwrap();
    assert!(
        r2.stdout.is_none(),
        "second call should be dedup-skipped (empty)"
    );

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

// ── Issue #148 Gap 5: Solo session binding visibility ──

#[test]
fn solo_session_still_sees_bindings_via_prompt_submit() {
    let pid = "test_solo_bind_vis";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let cwd = std::env::temp_dir().join("edda_solo_vis_cwd");
    let _ = fs::create_dir_all(&cwd);

    // Write binding — no heartbeats (solo mode)
    crate::peers::write_binding(pid, "solo-s", "solo", "api.style", "GraphQL");

    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");
    let result = dispatch_user_prompt_submit(pid, "solo-s", "", cwd.to_str().unwrap()).unwrap();
    assert!(result.stdout.is_some(), "solo session should see bindings");
    let output: serde_json::Value = serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
    let ctx = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("GraphQL"),
        "solo session should see binding value, got:\n{ctx}"
    );

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

// ── Issue #148 Gap 5: Solo → multi-session transition ──

#[test]
fn solo_to_multi_session_transition() {
    let pid = "test_solo_multi_trans";
    // Clean slate to avoid interference from other tests
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    // Phase 1: Only own heartbeat → solo (no active peers)
    let own_hb = state_dir.join("session.s1.json");
    fs::write(&own_hb, r#"{"session_id":"s1"}"#).unwrap();
    assert!(
        !has_active_peers(pid, "s1"),
        "should be solo with only own heartbeat"
    );

    // Phase 2: Peer appears → multi-session
    let peer_hb = state_dir.join("session.s2.json");
    fs::write(&peer_hb, r#"{"session_id":"s2"}"#).unwrap();
    assert!(
        has_active_peers(pid, "s1"),
        "should detect peer after heartbeat written"
    );

    // Phase 3: Peer goes stale → back to solo
    // Sleep to ensure file mtime is in the past, then set threshold to 0
    std::thread::sleep(std::time::Duration::from_millis(1100));
    crate::with_env_guard(&[("EDDA_PEER_STALE_SECS", Some("0"))], || {
        assert!(
            !has_active_peers(pid, "s1"),
            "peer should be stale after threshold=0 with old mtime"
        );
    });

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Issue #148 Gap 7: SessionEnd unclaim gating ──

#[test]
fn session_end_unclaim_only_with_active_peers() {
    let pid = "test_se_unclaim_gate";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

    let cwd = std::env::temp_dir().join("edda_se_unclaim_cwd");
    let _ = fs::create_dir_all(&cwd);

    // Write claims for two sessions
    crate::peers::write_claim(pid, "s1", "auth", &["src/auth.rs".into()]);
    crate::peers::write_claim(pid, "s2", "billing", &["src/bill.rs".into()]);

    // SessionEnd with peers_active=false — should NOT write unclaim
    let _ = dispatch_session_end(pid, "s1", "", cwd.to_str().unwrap(), false);

    // Read coordination.jsonl and check no unclaim for s1
    let coord_path = edda_store::project_dir(pid)
        .join("state")
        .join("coordination.jsonl");
    let content = fs::read_to_string(&coord_path).unwrap_or_default();
    let unclaim_count = content
        .lines()
        .filter(|l| l.contains("\"unclaim\"") && l.contains("s1"))
        .count();
    assert_eq!(unclaim_count, 0, "no unclaim when peers_active=false");

    // Write fresh claim for s3 and end with peers_active=true — SHOULD write unclaim
    crate::peers::write_claim(pid, "s3", "infra", &["infra/main.tf".into()]);
    let _ = dispatch_session_end(pid, "s3", "", cwd.to_str().unwrap(), true);

    let content2 = fs::read_to_string(&coord_path).unwrap_or_default();
    let unclaim_s3 = content2
        .lines()
        .filter(|l| l.contains("\"unclaim\"") && l.contains("s3"))
        .count();
    assert!(unclaim_s3 > 0, "should have unclaim when peers_active=true");

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn session_end_reads_counters_before_cleanup() {
    let pid = "test_se_counters";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let sid = "counter-sess";
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

    let cwd = std::env::temp_dir().join("edda_se_counter_cwd");
    let _ = fs::create_dir_all(&cwd);

    // Set up counters
    increment_counter(pid, sid, "decide_count");
    increment_counter(pid, sid, "decide_count");
    increment_counter(pid, sid, "decide_count");
    increment_counter(pid, sid, "nudge_count");
    increment_counter(pid, sid, "nudge_count");
    increment_counter(pid, sid, "signal_count");

    // Verify counters exist before SessionEnd
    let state_dir = edda_store::project_dir(pid).join("state");
    assert!(state_dir.join(format!("decide_count.{sid}")).exists());
    assert!(state_dir.join(format!("nudge_count.{sid}")).exists());
    assert!(state_dir.join(format!("signal_count.{sid}")).exists());

    // SessionEnd should read counters then clean them up
    let result = dispatch_session_end(pid, sid, "", cwd.to_str().unwrap(), false);
    assert!(result.is_ok(), "session_end should not error");

    // Counter files should be cleaned up
    assert!(
        !state_dir.join(format!("decide_count.{sid}")).exists(),
        "decide_count should be cleaned"
    );
    assert!(
        !state_dir.join(format!("nudge_count.{sid}")).exists(),
        "nudge_count should be cleaned"
    );
    assert!(
        !state_dir.join(format!("signal_count.{sid}")).exists(),
        "signal_count should be cleaned"
    );

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

// ── Issue #11: Late Peer Detection ──

#[test]
fn late_peer_detection_injects_full_protocol() {
    let pid = "test_late_peer_full";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

    let cwd = std::env::temp_dir().join("edda_late_peer_full_cwd");
    let _ = fs::create_dir_all(&cwd);

    let sid = "solo-sess";
    // No peer_count file yet (virgin session) → prev_count = 0
    // Create a peer heartbeat to simulate a second agent joining
    let signals = crate::signals::SessionSignals::default();
    crate::peers::write_heartbeat(pid, "peer-a", &signals, Some("billing"));

    let result =
        dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit").unwrap();
    assert!(result.stdout.is_some(), "should return output");

    let output: serde_json::Value = serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
    let ctx = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("Coordination Protocol") || ctx.contains("edda claim"),
        "should contain full coordination protocol on first peer detection, got:\n{ctx}"
    );

    // Verify peer_count state file was written
    let state_dir = edda_store::project_dir(pid).join("state");
    let count_file = state_dir.join(format!("peer_count.{sid}"));
    assert!(count_file.exists(), "peer_count file should be created");
    let count: usize = fs::read_to_string(&count_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(count, 1, "peer_count should be 1");

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    crate::peers::remove_heartbeat(pid, "peer-a");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn subsequent_prompts_use_lightweight_updates() {
    let pid = "test_late_peer_subsequent";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

    let cwd = std::env::temp_dir().join("edda_late_peer_subseq_cwd");
    let _ = fs::create_dir_all(&cwd);

    let sid = "known-peers-sess";
    // Pre-set peer_count to 1 (peer already known from previous prompt)
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);
    fs::write(state_dir.join(format!("peer_count.{sid}")), "1").unwrap();

    // Peer heartbeat still active
    let signals = crate::signals::SessionSignals::default();
    crate::peers::write_heartbeat(pid, "peer-b", &signals, Some("auth"));

    let result =
        dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit").unwrap();
    assert!(result.stdout.is_some(), "should return output");

    let output: serde_json::Value = serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
    let ctx = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    // Should have lightweight peer updates, not full protocol
    assert!(
        ctx.contains("## Peers"),
        "should contain lightweight peer header, got:\n{ctx}"
    );

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    crate::peers::remove_heartbeat(pid, "peer-b");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn solo_session_writes_zero_peer_count() {
    let pid = "test_late_peer_solo";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

    let cwd = std::env::temp_dir().join("edda_late_peer_solo_cwd");
    let _ = fs::create_dir_all(&cwd);

    let sid = "solo-only";
    // No peers — dispatch should still work, writing peer_count = 0
    let _ = dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit");

    let state_dir = edda_store::project_dir(pid).join("state");
    let count_file = state_dir.join(format!("peer_count.{sid}"));
    assert!(
        count_file.exists(),
        "peer_count file should be created even for solo"
    );
    let count: usize = fs::read_to_string(&count_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(count, 0, "peer_count should be 0 for solo session");

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn peer_count_cleaned_on_session_end() {
    let pid = "test_peer_count_clean";
    let _ = edda_store::ensure_dirs(pid);
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    let sid = "clean-sess";
    fs::write(state_dir.join(format!("peer_count.{sid}")), "2").unwrap();
    assert!(state_dir.join(format!("peer_count.{sid}")).exists());

    cleanup_session_state(pid, sid, false);

    assert!(
        !state_dir.join(format!("peer_count.{sid}")).exists(),
        "peer_count should be cleaned on session end"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Auto-write ledger event tests ──

#[test]
fn try_write_commit_event_creates_event() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"feat: add auth\"" },
        "cwd": workspace.to_str().unwrap()
    });

    try_write_commit_event(&raw, "feat: add auth");

    // Verify event was written
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let commit_events: Vec<_> = events.iter().filter(|e| e.event_type == "commit").collect();
    assert_eq!(commit_events.len(), 1);
    assert_eq!(
        commit_events[0].payload["title"].as_str().unwrap(),
        "feat: add auth"
    );
    assert!(commit_events[0].payload["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str() == Some("auto_detect")));
}

#[test]
fn try_write_merge_event_creates_event() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "gh pr merge 42 --squash" },
        "cwd": workspace.to_str().unwrap()
    });

    try_write_merge_event(&raw, "PR#42", "squash");

    // Verify event was written
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let merge_events: Vec<_> = events.iter().filter(|e| e.event_type == "merge").collect();
    assert_eq!(merge_events.len(), 1);
    assert_eq!(merge_events[0].payload["src"].as_str().unwrap(), "PR#42");
    assert_eq!(
        merge_events[0].payload["reason"].as_str().unwrap(),
        "squash"
    );
}

#[test]
fn try_write_commit_event_skips_when_no_workspace() {
    // cwd with no .edda workspace — should silently skip
    let tmp = tempfile::tempdir().unwrap();
    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" },
        "cwd": tmp.path().to_str().unwrap()
    });
    // Should not panic or error
    try_write_commit_event(&raw, "test");
}

#[test]
fn try_write_commit_event_skips_when_empty_cwd() {
    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" }
    });
    // No cwd field — should silently skip
    try_write_commit_event(&raw, "test");
}

// ── Auto-claim in PostToolUse (#56) ──

#[test]
fn post_tool_use_edit_triggers_auto_claim() {
    let pid = "test_post_edit_autoclaim";
    let sid = "sess-autoclaim-1";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "tool_name": "Edit",
        "input": { "file_path": "crates/edda-store/src/lib.rs" }
    });
    let _ = dispatch_post_tool_use(&raw, pid, sid, ".");

    // Should have auto-claimed edda-store
    let board = crate::peers::compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == sid);
    assert!(claim.is_some(), "Edit should trigger auto-claim");
    assert_eq!(claim.unwrap().label, "edda-store");

    crate::peers::remove_autoclaim_state(pid, sid);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_write_triggers_auto_claim() {
    let pid = "test_post_write_autoclaim";
    let sid = "sess-autoclaim-2";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "tool_name": "Write",
        "input": { "file_path": "crates/edda-bridge-claude/src/dispatch.rs" }
    });
    let _ = dispatch_post_tool_use(&raw, pid, sid, ".");

    let board = crate::peers::compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == sid);
    assert!(claim.is_some(), "Write should trigger auto-claim");
    assert_eq!(claim.unwrap().label, "edda-bridge-claude");

    crate::peers::remove_autoclaim_state(pid, sid);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn post_tool_use_bash_does_not_auto_claim() {
    let pid = "test_post_bash_no_autoclaim";
    let sid = "sess-autoclaim-3";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "ls" }
    });
    let _ = dispatch_post_tool_use(&raw, pid, sid, ".");

    let board = crate::peers::compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == sid);
    assert!(claim.is_none(), "Bash should NOT trigger auto-claim");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Request nudge in PreToolUse (#56) ──

#[test]
fn pre_tool_use_request_nudge_cooldown() {
    let pid = "test_pre_req_nudge_cd";
    let sid = "sess-req-nudge-1";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Setup: pending request for this session
    crate::peers::write_claim(pid, sid, "auth", &["src/auth/*".into()]);
    crate::peers::write_request(pid, "s2", "billing", "auth", "Need AuthToken export");
    // Must have peer_count > 0 for solo gate to pass
    write_peer_count(pid, sid, 1);

    // Counter starts at 0 → 0 % 3 == 0 → should nudge
    let nudge0 = check_pending_requests(pid, sid);
    assert!(nudge0.is_some(), "counter=0: should nudge");
    assert!(nudge0.unwrap().contains("Need AuthToken export"));

    // Counter is now 1 → 1 % 3 != 0 → no nudge
    let nudge1 = check_pending_requests(pid, sid);
    assert!(nudge1.is_none(), "counter=1: cooldown, no nudge");

    // Counter is now 2 → 2 % 3 != 0 → no nudge
    let nudge2 = check_pending_requests(pid, sid);
    assert!(nudge2.is_none(), "counter=2: cooldown, no nudge");

    // Counter is now 3 → 3 % 3 == 0 → should nudge again
    let nudge3 = check_pending_requests(pid, sid);
    assert!(nudge3.is_some(), "counter=3: should nudge again");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn pre_tool_use_no_pending_no_nudge() {
    let pid = "test_pre_no_pending";
    let sid = "sess-req-nudge-2";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // peer_count > 0 but no requests → no nudge
    write_peer_count(pid, sid, 1);
    let nudge = check_pending_requests(pid, sid);
    assert!(nudge.is_none(), "no pending requests → no nudge");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn pre_tool_use_solo_skips_counter_io() {
    let pid = "test_pre_solo_skip";
    let sid = "sess-req-nudge-3";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    // Solo session (peer_count=0) with pending request → still no nudge
    crate::peers::write_claim(pid, sid, "auth", &["src/auth/*".into()]);
    crate::peers::write_request(pid, "s2", "billing", "auth", "Need AuthToken");
    // peer_count defaults to 0 (solo)

    let nudge = check_pending_requests(pid, sid);
    assert!(nudge.is_none(), "solo session should skip nudge entirely");

    // Counter should NOT have been incremented
    assert_eq!(
        read_counter(pid, sid, "request_nudge_count"),
        0,
        "solo gate should skip counter I/O"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Branch Guard Tests ──

/// Helper: write a heartbeat with a specific branch for testing.
fn write_test_heartbeat(pid: &str, sid: &str, branch: Option<&str>) {
    let _ = edda_store::ensure_dirs(pid);
    let hb = crate::peers::SessionHeartbeat {
        session_id: sid.to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        last_heartbeat: "2026-01-01T00:00:00Z".to_string(),
        label: "test".to_string(),
        focus_files: vec![],
        active_tasks: vec![],
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: vec![],
        branch: branch.map(|s| s.to_string()),
        current_phase: None,
        parent_session_id: None,
    };
    let path = edda_store::project_dir(pid)
        .join("state")
        .join(format!("session.{sid}.json"));
    let _ = fs::create_dir_all(path.parent().unwrap());
    fs::write(&path, serde_json::to_string_pretty(&hb).unwrap()).unwrap();
}

#[test]
fn branch_guard_match_allows() {
    let pid = "test_branch_guard_match";
    let sid = "s1";
    // Write heartbeat with current branch so it matches
    let current = crate::peers::detect_git_branch_in(".").unwrap_or("main".into());
    write_test_heartbeat(pid, sid, Some(&current));

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    // Should not block (either allow or empty)
    if let Some(out) = &result.stdout {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let decision = v
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(|d| d.as_str())
            .unwrap_or("allow");
        assert_ne!(decision, "block", "matching branch should not block");
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn branch_guard_mismatch_blocks() {
    let pid = "test_branch_guard_mismatch";
    let sid = "s1";
    // Write heartbeat with a branch that won't match
    write_test_heartbeat(pid, sid, Some("nonexistent-branch-xyz"));

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    let out = result.stdout.expect("should produce output on block");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let decision = v
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(|d| d.as_str())
        .unwrap();
    assert_eq!(decision, "block", "mismatched branch should block");
    let reason = v
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(|d| d.as_str())
        .unwrap();
    assert!(
        reason.contains("nonexistent-branch-xyz"),
        "reason should mention claimed branch"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn branch_guard_no_heartbeat_allows() {
    let pid = "test_branch_guard_no_hb";
    let sid = "s_no_hb";
    let _ = edda_store::ensure_dirs(pid);
    // No heartbeat written — guard should allow

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    if let Some(out) = &result.stdout {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let decision = v
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(|d| d.as_str())
            .unwrap_or("allow");
        assert_ne!(decision, "block", "no heartbeat should not block");
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn branch_guard_amend_allows() {
    let pid = "test_branch_guard_amend";
    let sid = "s1";
    write_test_heartbeat(pid, sid, Some("nonexistent-branch-xyz"));

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit --amend -m \"fix\"" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    if let Some(out) = &result.stdout {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let decision = v
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(|d| d.as_str())
            .unwrap_or("allow");
        assert_ne!(
            decision, "block",
            "git commit --amend should not be guarded"
        );
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn branch_guard_non_commit_allows() {
    let pid = "test_branch_guard_non_commit";
    let sid = "s1";
    write_test_heartbeat(pid, sid, Some("nonexistent-branch-xyz"));

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "cargo build" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    if let Some(out) = &result.stdout {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let decision = v
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(|d| d.as_str())
            .unwrap_or("allow");
        assert_ne!(decision, "block", "non-commit commands should not block");
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn branch_guard_no_claimed_branch_allows() {
    // Heartbeat exists but branch is None — should allow (no claim to enforce)
    let pid = "test_branch_guard_no_claim";
    let sid = "s1";
    write_test_heartbeat(pid, sid, None);

    let raw = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git commit -m \"test\"" }
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    if let Some(out) = &result.stdout {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let decision = v
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(|d| d.as_str())
            .unwrap_or("allow");
        assert_ne!(decision, "block", "no claimed branch should not block");
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn subagent_start_creates_heartbeat() {
    let pid = "test_subagent_start";
    let _ = edda_store::ensure_dirs(pid);

    // Directly call write_subagent_heartbeat (what SubagentStart dispatch does)
    crate::peers::write_subagent_heartbeat(pid, "agent-xyz", "parent-sess", "sub:Explore", ".");

    let hb =
        crate::peers::read_heartbeat(pid, "agent-xyz").expect("sub-agent heartbeat should exist");
    assert_eq!(hb.label, "sub:Explore");
    assert_eq!(hb.parent_session_id.as_deref(), Some("parent-sess"));

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn subagent_stop_removes_heartbeat() {
    let pid = "test_subagent_stop";
    let _ = edda_store::ensure_dirs(pid);

    // Create heartbeat
    crate::peers::write_subagent_heartbeat(pid, "agent-abc", "parent-sess", "sub:Plan", ".");
    assert!(crate::peers::read_heartbeat(pid, "agent-abc").is_some());

    // Remove it (what SubagentStop dispatch does)
    crate::peers::remove_heartbeat(pid, "agent-abc");

    assert!(
        crate::peers::read_heartbeat(pid, "agent-abc").is_none(),
        "heartbeat should be removed after SubagentStop"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn subagent_stop_writes_summary_and_removes_heartbeat() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    let project_id = resolve_project_id(workspace.to_str().unwrap());
    let _ = edda_store::ensure_dirs(&project_id);

    let transcript = workspace.join("subagent.jsonl");
    let transcript_content = [
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                {
                    "type": "tool_use",
                    "id": "e1",
                    "name": "Edit",
                    "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" }
                },
                {
                    "type": "tool_use",
                    "id": "b1",
                    "name": "Bash",
                    "input": { "command": "git commit -m \"feat: sub-agent\"" }
                }
            ]}
        })
        .to_string(),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "b1",
                "content": "[main abc1234] feat: sub-agent\n 1 file changed"
            }]}
        })
        .to_string(),
    ]
    .join("\n");
    fs::write(&transcript, format!("{transcript_content}\n")).unwrap();

    crate::peers::write_subagent_heartbeat(
        &project_id,
        "agent-abc",
        "parent-sess",
        "sub:Plan",
        workspace.to_str().unwrap(),
    );
    assert!(crate::peers::read_heartbeat(&project_id, "agent-abc").is_some());

    let stdin = serde_json::json!({
        "session_id": "parent-sess",
        "hook_event_name": "SubagentStop",
        "cwd": workspace.to_str().unwrap(),
        "agent_id": "agent-abc",
        "agent_type": "Plan",
        "agent_transcript_path": transcript.to_string_lossy(),
        "last_assistant_message": "fallback summary"
    })
    .to_string();

    let result = hook_entrypoint_from_stdin(&stdin).unwrap();
    assert!(result.stdout.is_none());

    // Heartbeat removed as final cleanup
    assert!(
        crate::peers::read_heartbeat(&project_id, "agent-abc").is_none(),
        "heartbeat should be removed after SubagentStop"
    );

    // Coordination summary event written
    let board = crate::peers::compute_board_state(&project_id);
    assert_eq!(board.subagent_completions.len(), 1);
    let sub = &board.subagent_completions[0];
    assert_eq!(sub.parent_session_id, "parent-sess");
    assert_eq!(sub.agent_id, "agent-abc");
    assert_eq!(sub.agent_type, "Plan");
    assert_eq!(sub.files_touched.len(), 1);
    assert_eq!(sub.commits.len(), 1);

    // Workspace note written for watch rendering
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let note_events: Vec<_> = events
        .iter()
        .filter(|e| {
            e.event_type == "note"
                && e.payload["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Sub-agent completed")
        })
        .collect();
    assert_eq!(note_events.len(), 1, "should write sub-agent note event");

    let _ = fs::remove_dir_all(edda_store::project_dir(&project_id));
}

#[test]
fn subagent_stop_fallback_summary_when_transcript_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    let project_id = resolve_project_id(workspace.to_str().unwrap());
    let _ = edda_store::ensure_dirs(&project_id);

    crate::peers::write_subagent_heartbeat(
        &project_id,
        "agent-fallback",
        "parent-sess",
        "sub:Explore",
        workspace.to_str().unwrap(),
    );

    let stdin = serde_json::json!({
        "session_id": "parent-sess",
        "hook_event_name": "SubagentStop",
        "cwd": workspace.to_str().unwrap(),
        "agent_id": "agent-fallback",
        "agent_type": "Explore",
        "agent_transcript_path": workspace.join("missing.jsonl").to_string_lossy(),
        "last_assistant_message": "Decision: use fallback parser"
    })
    .to_string();

    let _ = hook_entrypoint_from_stdin(&stdin).unwrap();

    let board = crate::peers::compute_board_state(&project_id);
    assert_eq!(board.subagent_completions.len(), 1);
    let sub = &board.subagent_completions[0];
    assert!(
        sub.summary.contains("Decision: use fallback parser"),
        "fallback summary should be sourced from last assistant message"
    );

    assert!(
        crate::peers::read_heartbeat(&project_id, "agent-fallback").is_none(),
        "heartbeat should be removed even on fallback path"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(&project_id));
}

#[test]
fn subagent_orphan_cleanup_on_session_end() {
    let pid = "test_subagent_orphan";
    let _ = edda_store::ensure_dirs(pid);

    // Create parent heartbeat
    write_test_heartbeat(pid, "parent-sess", Some("main"));

    // Create sub-agent heartbeats
    crate::peers::write_subagent_heartbeat(pid, "sub-1", "parent-sess", "sub:Explore", ".");
    crate::peers::write_subagent_heartbeat(pid, "sub-2", "parent-sess", "sub:Plan", ".");

    // Run cleanup (what SessionEnd does)
    cleanup_session_state(pid, "parent-sess", false);

    // All sub-agent heartbeats should be cleaned up
    assert!(
        crate::peers::read_heartbeat(pid, "sub-1").is_none(),
        "sub-1 should be cleaned up on parent SessionEnd"
    );
    assert!(
        crate::peers::read_heartbeat(pid, "sub-2").is_none(),
        "sub-2 should be cleaned up on parent SessionEnd"
    );
    // Parent heartbeat also removed (standard behavior)
    assert!(crate::peers::read_heartbeat(pid, "parent-sess").is_none());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn subagent_start_no_agent_id_is_noop() {
    let pid = "test_subagent_no_id";
    let _ = edda_store::ensure_dirs(pid);

    // Empty agent_id should not create any heartbeat
    crate::peers::write_subagent_heartbeat(pid, "", "parent-sess", "sub:Explore", ".");

    // Heartbeat file for empty id would be "session..json" — should not exist
    // or at least not be discoverable as a valid peer
    let state_dir = edda_store::project_dir(pid).join("state");
    let heartbeat_files: Vec<_> = fs::read_dir(&state_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("session.") && n.ends_with(".json") && n != "session..json"
        })
        .collect();
    assert!(
        heartbeat_files.is_empty(),
        "no valid heartbeat should be created for empty agent_id"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Karvi integration tests ──

#[test]
fn extract_task_id_from_branch() {
    assert_eq!(extract_task_id("feat/task-T2-auth"), Some("T2".to_string()));
    assert_eq!(extract_task_id("fix/T5-login-bug"), Some("T5".to_string()));
    assert_eq!(
        extract_task_id("feature/T123-complex-feature"),
        Some("T123".to_string())
    );
    assert_eq!(extract_task_id("main"), None);
    assert_eq!(extract_task_id("feature/no-task-id"), None);
}

#[test]
fn is_karvi_project_detection() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path();

    // Not a karvi project initially
    assert!(!is_karvi_project(path.to_str().unwrap()));

    // Create server/board.json
    fs::create_dir_all(path.join("server")).unwrap();
    fs::write(path.join("server/board.json"), "{}").unwrap();

    // Now it's a karvi project
    assert!(is_karvi_project(path.to_str().unwrap()));
}

#[test]
fn task_completed_writes_coordination_event() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    let project_id = resolve_project_id(workspace.to_str().unwrap());
    let _ = edda_store::ensure_dirs(&project_id);

    let stdin = serde_json::json!({
        "session_id": "sess-tc1",
        "hook_event_name": "TaskCompleted",
        "cwd": workspace.to_str().unwrap(),
        "task_id": "task-abc",
        "task_subject": "Implement auth module",
        "task_description": "Add JWT-based authentication"
    })
    .to_string();

    let result = hook_entrypoint_from_stdin(&stdin).unwrap();
    // TaskCompleted does not support hookSpecificOutput
    assert!(result.stdout.is_none());
    assert!(result.stderr.is_none());

    // Verify coordination.jsonl contains the task_completed event
    let coord_path = edda_store::project_dir(&project_id)
        .join("state")
        .join("coordination.jsonl");
    let content = fs::read_to_string(&coord_path).unwrap();
    assert!(
        content.contains("task_completed"),
        "coordination.jsonl should contain task_completed event type"
    );
    assert!(
        content.contains("task-abc"),
        "coordination.jsonl should contain task_id"
    );
    assert!(
        content.contains("Implement auth module"),
        "coordination.jsonl should contain task_subject"
    );
}

#[test]
fn task_completed_skips_when_task_id_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let project_id = resolve_project_id(workspace.to_str().unwrap());
    let _ = edda_store::ensure_dirs(&project_id);

    let stdin = serde_json::json!({
        "session_id": "sess-tc2",
        "hook_event_name": "TaskCompleted",
        "cwd": workspace.to_str().unwrap(),
        "task_id": "",
        "task_subject": "Should be skipped"
    })
    .to_string();

    let result = hook_entrypoint_from_stdin(&stdin).unwrap();
    assert!(result.stdout.is_none());
    assert!(result.stderr.is_none());

    // No coordination.jsonl should be created (no events written)
    let coord_path = edda_store::project_dir(&project_id)
        .join("state")
        .join("coordination.jsonl");
    assert!(
        !coord_path.exists() || fs::read_to_string(&coord_path).unwrap().is_empty(),
        "no coordination event should be written when task_id is empty"
    );
}

// ── Off-limits enforcement tests ──

#[test]
fn offlimits_disabled_by_default() {
    let pid = "test-offlimits-disabled";
    let sid = "s-self";
    let _ = edda_store::ensure_dirs(pid);

    // Even with a peer claim, check_offlimits returns None when peer_count == 0
    crate::peers::write_claim(pid, "s-peer", "peer-agent", &["src/auth/*".into()]);
    let result = check_offlimits(pid, sid, "src/auth/login.rs");
    assert!(result.is_none(), "should not block when peer_count == 0");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_blocks_peer_claimed_file() {
    let pid = "test-offlimits-blocks";
    let sid = "s-self-blocks";
    let peer_sid = "s-peer-blocks";
    let _ = edda_store::ensure_dirs(pid);

    // Create peer heartbeat so it's discovered as active
    crate::peers::write_heartbeat_minimal(pid, peer_sid, "store-refactor");
    // Create peer claim
    crate::peers::write_claim(
        pid,
        peer_sid,
        "store-refactor",
        &["crates/edda-store/*".into()],
    );
    // Set peer count > 0
    write_peer_count(pid, sid, 1);

    let result = check_offlimits(pid, sid, "crates/edda-store/src/lib.rs");
    assert!(result.is_some(), "should block file claimed by active peer");
    let (label, glob) = result.unwrap();
    assert_eq!(label, "store-refactor");
    assert_eq!(glob, "crates/edda-store/*");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_allows_own_claimed_file() {
    let pid = "test-offlimits-self";
    let sid = "s-self-own";
    let _ = edda_store::ensure_dirs(pid);

    // Create own heartbeat and claim
    crate::peers::write_heartbeat_minimal(pid, sid, "my-agent");
    crate::peers::write_claim(pid, sid, "my-agent", &["src/auth/*".into()]);
    write_peer_count(pid, sid, 1);

    let result = check_offlimits(pid, sid, "src/auth/login.rs");
    assert!(result.is_none(), "should not block own claimed files");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_allows_unclaimed_file() {
    let pid = "test-offlimits-unclaimed";
    let sid = "s-self-unclaimed";
    let peer_sid = "s-peer-unclaimed";
    let _ = edda_store::ensure_dirs(pid);

    // Create peer with claims on different paths
    crate::peers::write_heartbeat_minimal(pid, peer_sid, "db-agent");
    crate::peers::write_claim(pid, peer_sid, "db-agent", &["crates/edda-store/*".into()]);
    write_peer_count(pid, sid, 1);

    let result = check_offlimits(pid, sid, "crates/edda-bridge-claude/src/lib.rs");
    assert!(
        result.is_none(),
        "should allow files not claimed by any peer"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_skips_stale_claims() {
    let pid = "test-offlimits-stale";
    let sid = "s-self-stale";
    let stale_sid = "s-stale-peer";
    let _ = edda_store::ensure_dirs(pid);

    // Write a claim but NO heartbeat for the peer -> peer won't be discovered as active
    crate::peers::write_claim(
        pid,
        stale_sid,
        "stale-agent",
        &["crates/edda-store/*".into()],
    );
    write_peer_count(pid, sid, 1);

    let result = check_offlimits(pid, sid, "crates/edda-store/src/lib.rs");
    assert!(
        result.is_none(),
        "should not block on claims from stale/missing peers"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_env_var_enables_enforcement() {
    let pid = "test-offlimits-env";
    let sid = "s-self-env";
    let peer_sid = "s-peer-env";
    let _ = edda_store::ensure_dirs(pid);

    // Set up peer
    crate::peers::write_heartbeat_minimal(pid, peer_sid, "env-agent");
    crate::peers::write_claim(pid, peer_sid, "env-agent", &["src/core/*".into()]);
    write_peer_count(pid, sid, 1);

    // Enable enforcement via env var
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_ENFORCE_OFFLIMITS", "1");

    let raw = serde_json::json!({
        "session_id": sid,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/core/engine.rs",
            "old_string": "fn old()",
            "new_string": "fn new()"
        },
        "cwd": "."
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    let output_str = result.stdout.expect("should produce output");
    let output: serde_json::Value = serde_json::from_str(&output_str).unwrap();
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecision"], "block",
        "should block Edit on peer-claimed file"
    );
    let reason = output["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(
        reason.contains("Off-limits"),
        "reason should mention Off-limits"
    );
    assert!(
        reason.contains("env-agent"),
        "reason should mention peer label"
    );

    std::env::remove_var("EDDA_ENFORCE_OFFLIMITS");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn offlimits_skips_non_edit_tools() {
    let pid = "test-offlimits-bash";
    let sid = "s-self-bash";
    let peer_sid = "s-peer-bash";
    let _ = edda_store::ensure_dirs(pid);

    // Set up peer with claim
    crate::peers::write_heartbeat_minimal(pid, peer_sid, "bash-agent");
    crate::peers::write_claim(pid, peer_sid, "bash-agent", &["src/*".into()]);
    write_peer_count(pid, sid, 1);

    // Enable enforcement
    let _eg = crate::ENV_LOCK.lock().unwrap();
    std::env::set_var("EDDA_ENFORCE_OFFLIMITS", "1");
    std::env::set_var("EDDA_CLAUDE_AUTO_APPROVE", "1");

    let raw = serde_json::json!({
        "session_id": sid,
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "cat src/main.rs"
        },
        "cwd": "."
    });
    let result = dispatch_pre_tool_use(&raw, ".", pid, sid).unwrap();
    let output_str = result.stdout.expect("should have output");
    let output: serde_json::Value = serde_json::from_str(&output_str).unwrap();
    // Should be "allow" (auto-approve), not "block"
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecision"], "allow",
        "Bash tool should not be blocked by off-limits"
    );

    std::env::remove_var("EDDA_ENFORCE_OFFLIMITS");
    std::env::remove_var("EDDA_CLAUDE_AUTO_APPROVE");
    drop(_eg);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Karvi Board State (read_project_state) Tests ──

#[test]
fn read_project_state_non_karvi() {
    let tmp = tempfile::tempdir().unwrap();
    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(result.is_none(), "Should return None for non-karvi project");
}

#[test]
fn read_project_state_malformed_json() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();
    fs::write(tmp.path().join("server/board.json"), "not valid json {{{").unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(
        result.is_none(),
        "Should return None for malformed board.json"
    );
}

#[test]
fn read_project_state_minimal_board() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();
    let board = serde_json::json!({
        "taskPlan": {
            "goal": "implement auth",
            "phase": "execution"
        }
    });
    fs::write(
        tmp.path().join("server/board.json"),
        serde_json::to_string(&board).unwrap(),
    )
    .unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(result.is_some(), "Should return summary for minimal board");
    let summary = result.unwrap();
    assert!(summary.contains("[karvi board]"));
    assert!(summary.contains("Goal: implement auth"));
    assert!(summary.contains("Phase: execution"));
}

#[test]
fn read_project_state_full_board() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();
    let board = serde_json::json!({
        "taskPlan": {
            "goal": "build the app",
            "phase": "testing",
            "tasks": [
                {
                    "id": "T1",
                    "subject": "setup project",
                    "status": "completed",
                    "review": { "state": "approved" }
                },
                {
                    "id": "T2",
                    "subject": "implement auth",
                    "status": "in_progress",
                    "assigned": "engineer_pro"
                },
                {
                    "id": "T3",
                    "subject": "deploy",
                    "status": "blocked",
                    "depends": ["T2"]
                }
            ]
        },
        "lessons": [
            { "rule": "always run tests" },
            { "rule": "check types" }
        ],
        "signals": [
            { "content": "test coverage at 85%" },
            { "content": "auth module ready" },
            { "content": "schema migrated" }
        ]
    });
    fs::write(
        tmp.path().join("server/board.json"),
        serde_json::to_string(&board).unwrap(),
    )
    .unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(result.is_some());
    let summary = result.unwrap();

    assert!(summary.contains("[karvi board]"));
    assert!(summary.contains("Goal: build the app"));
    assert!(summary.contains("Phase: testing"));
    assert!(summary.contains("T1 \"setup project\" (completed, approved)"));
    assert!(summary.contains("T2 \"implement auth\" (in_progress, assigned: engineer_pro)"));
    assert!(summary.contains("T3 \"deploy\" (blocked, blocked by T2)"));
    assert!(summary.contains("Lessons: 2 active"));
    assert!(summary.contains("Signals:"));
    assert!(summary.contains("\"test coverage at 85%\""));
    assert!(summary.contains("\"auth module ready\""));
    assert!(summary.contains("\"schema migrated\""));
}

#[test]
fn read_project_state_respects_500_char_budget() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();

    // Create a board with many tasks to exceed 500 chars
    let mut tasks = Vec::new();
    for i in 0..20 {
        tasks.push(serde_json::json!({
            "id": format!("T{i}"),
            "subject": format!("a very long task subject number {i} that takes space"),
            "status": "in_progress",
            "assigned": "engineer_pro",
            "depends": ["T0", "T1"]
        }));
    }
    let board = serde_json::json!({
        "taskPlan": {
            "goal": "a goal that is quite long to help push the char count",
            "phase": "execution",
            "tasks": tasks
        },
        "lessons": [
            { "rule": "r1" }, { "rule": "r2" }, { "rule": "r3" },
            { "rule": "r4" }, { "rule": "r5" }
        ],
        "signals": [
            { "content": "signal one" },
            { "content": "signal two" },
            { "content": "signal three" }
        ]
    });
    fs::write(
        tmp.path().join("server/board.json"),
        serde_json::to_string(&board).unwrap(),
    )
    .unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(result.is_some());
    let summary = result.unwrap();
    // 500 chars + "...(truncated)" suffix
    assert!(
        summary.len() <= 500 + 15,
        "Summary should be at most 515 chars, got {}",
        summary.len()
    );
    assert!(summary.contains("...(truncated)"));
}

#[test]
fn read_project_state_missing_fields() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();

    // Empty JSON object — no taskPlan at all
    fs::write(tmp.path().join("server/board.json"), "{}").unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(
        result.is_some(),
        "Should still return header for valid JSON"
    );
    let summary = result.unwrap();
    assert!(summary.contains("[karvi board]"));
    // Should NOT contain Goal/Phase/Tasks since taskPlan is missing
    assert!(!summary.contains("Goal:"));
    assert!(!summary.contains("Phase:"));
}

#[test]
fn read_project_state_empty_tasks() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("server")).unwrap();
    let board = serde_json::json!({
        "taskPlan": {
            "goal": "test",
            "phase": "idle",
            "tasks": []
        }
    });
    fs::write(
        tmp.path().join("server/board.json"),
        serde_json::to_string(&board).unwrap(),
    )
    .unwrap();

    let result = read_project_state(tmp.path().to_str().unwrap());
    assert!(result.is_some());
    let summary = result.unwrap();
    assert!(summary.contains("Tasks: (none)"));
}

// ── Issue #287: SessionEnd background thread join ──

#[test]
fn session_end_bg_threads_joined_zero_threads() {
    // Regression test: when no background threads are spawned (no API key),
    // the channel-based join must complete immediately without hanging.
    let pid = "test_se_bg_join_zero";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let cwd = tempfile::tempdir().unwrap();

    // Disable features that would require external state
    std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
    std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");
    std::env::set_var("EDDA_POSTMORTEM", "0");
    // No EDDA_LLM_API_KEY → all should_run() return false → bg_count=0
    std::env::remove_var("EDDA_LLM_API_KEY");
    // Use a short join timeout to catch hangs quickly
    std::env::set_var("EDDA_BG_JOIN_TIMEOUT_SECS", "1");

    let result = dispatch_session_end(pid, "s1", "", cwd.path().to_str().unwrap(), false);
    assert!(result.is_ok(), "dispatch_session_end should succeed with zero bg threads");

    std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    std::env::remove_var("EDDA_PLANS_DIR");
    std::env::remove_var("EDDA_POSTMORTEM");
    std::env::remove_var("EDDA_BG_JOIN_TIMEOUT_SECS");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
