use super::*;

#[test]
fn read_project_state_missing_fields() {
    let _store = crate::isolated_store();
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
    let _store = crate::isolated_store();
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
    let _store = crate::isolated_store();
    // Regression test: when no background threads are spawned (no API key),
    // the channel-based join must complete immediately without hanging.
    let pid = "test_se_bg_join_zero";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let cwd = tempfile::tempdir().unwrap();

    // Disable features that would require external state
    let _cfg = crate::test_config_guard(&[
        ("EDDA_BRIDGE_AUTO_DIGEST", Some("0")),
        ("EDDA_PLANS_DIR", Some("/nonexistent")),
        ("EDDA_POSTMORTEM", Some("0")),
    ]);
    // No EDDA_LLM_API_KEY → all should_run() return false → bg_count=0
    // Use a short join timeout to catch hangs quickly
    let _cfg = crate::test_config_guard(&[("EDDA_BG_JOIN_TIMEOUT_SECS", Some("1"))]);

    let result = dispatch_session_end(pid, "s1", "", cwd.path().to_str().unwrap());
    assert!(
        result.is_ok(),
        "dispatch_session_end should succeed with zero bg threads"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
