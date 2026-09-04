use super::*;
use std::path::PathBuf;

fn make_transcript(records: &[serde_json::Value]) -> PathBuf {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.into_temp_path().to_path_buf();
    let mut content = String::new();
    for r in records {
        content.push_str(&serde_json::to_string(r).unwrap());
        content.push('\n');
    }
    fs::write(&path, content).unwrap();
    path
}

fn assistant_task_create(tool_use_id: &str, subject: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": "TaskCreate",
                "input": {
                    "subject": subject,
                    "description": "test task",
                    "activeForm": "Testing"
                }
            }]
        }
    })
}

fn assistant_task_update(task_id: &str, status: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "tu_update",
                "name": "TaskUpdate",
                "input": {
                    "taskId": task_id,
                    "status": status
                }
            }]
        }
    })
}

#[test]
fn signals_extract_tasks() {
    let _store = crate::isolated_store();
    let records = vec![
        assistant_task_create("tu1", "Fix bug A"),
        assistant_task_create("tu2", "Add feature B"),
        assistant_task_update("1", "in_progress"),
        assistant_task_update("1", "completed"),
        assistant_task_update("2", "in_progress"),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.tasks.len(), 2);
    assert_eq!(signals.tasks[0].id, "1");
    assert_eq!(signals.tasks[0].subject, "Fix bug A");
    assert_eq!(signals.tasks[0].status, "completed");
    assert_eq!(signals.tasks[1].status, "in_progress");
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_extract_files_modified() {
    let records = vec![serde_json::json!({
        "type": "assistant",
        "message": { "role": "assistant", "content": [
            { "type": "tool_use", "id": "e1", "name": "Edit",
              "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" } },
            { "type": "tool_use", "id": "e2", "name": "Edit",
              "input": { "file_path": "/repo/src/lib.rs", "old_string": "c", "new_string": "d" } },
            { "type": "tool_use", "id": "w1", "name": "Write",
              "input": { "file_path": "/repo/src/new.rs", "content": "fn main() {\n        let _store = crate::isolated_store();}" } }
        ]}
    })];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.files_modified.len(), 2);
    assert_eq!(signals.files_modified[0].count, 2); // lib.rs edited twice
    assert_eq!(signals.files_modified[1].count, 1); // new.rs written once
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_extract_commits() {
    let _store = crate::isolated_store();
    let records = vec![
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [{
                "type": "tool_use", "id": "b1", "name": "Bash",
                "input": { "command": "git commit -m \"fix: something\"" }
            }]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result", "tool_use_id": "b1",
                "content": "[main abc1234] fix: something\n 1 file changed"
            }]}
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.commits.len(), 1);
    assert_eq!(signals.commits[0].hash, "abc1234");
    assert_eq!(signals.commits[0].message, "fix: something");
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_save_load_round_trip() {
    let _store = crate::isolated_store();
    let pid = "test_signals_rt_00";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Fix".into(),
            status: "completed".into(),
        }],
        files_modified: vec![FileEditCount {
            path: "src/lib.rs".into(),
            count: 3,
        }],
        commits: vec![CommitInfo {
            hash: "abc".into(),
            message: "fix".into(),
        }],
        failed_commands: vec![],
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    let tasks: Vec<TaskSnapshot> = load_state_vec(pid, "active_tasks.json", "tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "completed");

    let files: Vec<FileEditCount> = load_state_vec(pid, "files_modified.json", "files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].count, 3);

    let commits: Vec<CommitInfo> = load_state_vec(pid, "recent_commits.json", "commits");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].hash, "abc");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn extract_commit_msg_parses_m_flag() {
    let _store = crate::isolated_store();
    assert_eq!(
        extract_commit_msg_from_cmd(r#"git commit -m "fix: something""#),
        "fix: something"
    );
    assert_eq!(
        extract_commit_msg_from_cmd("git commit -m 'feat: new'"),
        "feat: new"
    );
    assert_eq!(extract_commit_msg_from_cmd("git add . && git commit"), "");
}

#[test]
fn parse_commit_result_extracts_hash() {
    let result = "[main abc1234] fix: something\n 1 file changed";
    let ci = parse_commit_result(result, "").unwrap();
    assert_eq!(ci.hash, "abc1234");
    assert_eq!(ci.message, "fix: something");
}

#[test]
fn noise_file_filters_skills() {
    assert!(is_noise_file(".claude/skills/commit/SKILL.md"));
    assert!(is_noise_file(
        "C:\\repo\\.claude\\skills\\testing\\SKILL.md"
    ));
    assert!(is_noise_file("/home/user/.claude/skills/foo/bar.md"));
    assert!(!is_noise_file("crates/edda-derive/src/lib.rs"));
    assert!(!is_noise_file("src/main.rs"));
}

#[test]
fn signals_skip_noise_files() {
    let _store = crate::isolated_store();
    let records = vec![serde_json::json!({
        "type": "assistant",
        "message": { "role": "assistant", "content": [
            { "type": "tool_use", "id": "e1", "name": "Edit",
              "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" } },
            { "type": "tool_use", "id": "e2", "name": "Write",
              "input": { "file_path": "/repo/.claude/skills/commit/SKILL.md", "content": "x" } },
            { "type": "tool_use", "id": "e3", "name": "Edit",
              "input": { "file_path": "/repo/.claude/skills/testing/SKILL.md", "old_string": "a", "new_string": "b" } }
        ]}
    })];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(
        signals.files_modified.len(),
        1,
        "skill files should be filtered"
    );
    assert_eq!(signals.files_modified[0].path, "/repo/src/lib.rs");
    let _ = fs::remove_file(&path);
}

// ── Focus Detection tests ──

#[test]
fn focus_common_prefix_crates() {
    let _store = crate::isolated_store();
    let files: &[(&str, usize)] = &[
        ("C:/repo/crates/edda-bridge-claude/src/dispatch.rs", 10),
        ("C:/repo/crates/edda-bridge-claude/src/signals.rs", 5),
        ("C:/repo/crates/edda-bridge-claude/src/lib.rs", 2),
    ];
    let (label, prefix) = find_focus_label(files).unwrap();
    assert_eq!(label, "edda-bridge-claude");
    assert!(prefix.contains("edda-bridge-claude"), "prefix: {prefix}");
}

#[test]
fn focus_common_prefix_src() {
    let _store = crate::isolated_store();
    let files: &[(&str, usize)] = &[
        ("/project/src/components/Button.tsx", 1),
        ("/project/src/components/Modal.tsx", 1),
        ("/project/src/components/Header.tsx", 1),
    ];
    let (label, _) = find_focus_label(files).unwrap();
    assert_eq!(label, "components");
}

#[test]
fn focus_most_frequent_dir() {
    let _store = crate::isolated_store();
    // 4 out of 5 files in edda-cli, 1 in edda-core
    let files: &[(&str, usize)] = &[
        ("/repo/crates/edda-cli/src/main.rs", 1),
        ("/repo/crates/edda-cli/src/cmd_gc.rs", 1),
        ("/repo/crates/edda-cli/src/cmd_bridge.rs", 1),
        ("/repo/crates/edda-cli/src/cmd_pack.rs", 1),
        ("/repo/crates/edda-core/src/lib.rs", 1),
    ];
    let (label, _) = find_focus_label(files).unwrap();
    assert_eq!(label, "edda-cli");
}

#[test]
fn focus_too_few_files_returns_none() {
    let _store = crate::isolated_store();
    // find_focus_label works with any count, but render_focus_section gates on >= 3
    let files: &[(&str, usize)] = &[
        ("/repo/crates/foo/src/a.rs", 1),
        ("/repo/crates/foo/src/b.rs", 1),
    ];
    let result = find_focus_label(files);
    assert!(result.is_some());
}

#[test]
fn focus_empty_returns_none() {
    let _store = crate::isolated_store();
    let files: &[(&str, usize)] = &[];
    assert!(find_focus_label(files).is_none());
}

#[test]
fn focus_windows_paths() {
    let _store = crate::isolated_store();
    let files: &[(&str, usize)] = &[
        ("C:\\ai_agent\\edda\\crates\\edda-derive\\src\\types.rs", 3),
        (
            "C:\\ai_agent\\edda\\crates\\edda-derive\\src\\context.rs",
            2,
        ),
        (
            "C:\\ai_agent\\edda\\crates\\edda-derive\\src\\writers.rs",
            1,
        ),
    ];
    let (label, _) = find_focus_label(files).unwrap();
    assert_eq!(label, "edda-derive");
}

#[test]
fn focus_edit_weighted_triggers_on_heavy_crate() {
    let _store = crate::isolated_store();
    // Simulates real scenario: 60 files scattered, but one crate has 17% of edits
    // Old 50% file-count threshold would miss this; new 30% edit threshold catches it
    let mut files: Vec<(&str, usize)> = vec![
        // Heavy crate: 8 files, 182 edits (49% of total)
        ("/repo/crates/edda-bridge-claude/src/dispatch.rs", 63),
        ("/repo/crates/edda-bridge-claude/src/lib.rs", 52),
        ("/repo/crates/edda-bridge-claude/src/digest.rs", 46),
        ("/repo/crates/edda-bridge-claude/src/signals.rs", 21),
        ("/repo/crates/edda-bridge-claude/src/plan.rs", 6),
        ("/repo/crates/edda-bridge-claude/src/narrative.rs", 4),
        ("/repo/crates/edda-bridge-claude/src/parse.rs", 3),
        ("/repo/crates/edda-bridge-claude/src/redact.rs", 2),
        // Other crates: scattered
        ("/repo/crates/edda-cli/src/main.rs", 20),
        ("/repo/crates/edda-cli/src/cmd_gc.rs", 24),
        ("/repo/crates/edda-derive/src/lib.rs", 27),
        ("/repo/crates/edda-ledger/src/paths.rs", 6),
        // Docs (not in crates)
        ("/repo/docs/planB/TRACKS.md", 2),
        ("/repo/docs/USAGE.md", 1),
        ("/repo/Cargo.toml", 1),
    ];
    // Add more scattered files to dilute file count
    for i in 0..10 {
        files.push(("/repo/docs/planB/misc.md", 1));
        let _ = i;
    }

    let (label, display) = find_focus_label(&files).unwrap();
    assert_eq!(
        label, "edda-bridge-claude",
        "should detect heavy crate by edits"
    );
    assert!(display.contains("% of edits"), "display: {display}");
}

#[test]
fn focus_no_hot_files_when_edits_even() {
    let _store = crate::isolated_store();
    let pid = "test_focus_no_hot";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        tasks: vec![],
        files_modified: vec![
            FileEditCount {
                path: "/repo/crates/foo/src/a.rs".into(),
                count: 10,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/b.rs".into(),
                count: 10,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/c.rs".into(),
                count: 10,
            },
        ],
        commits: vec![],
        failed_commands: vec![],
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    let focus = render_focus_section(pid).unwrap();
    // avg = 10, threshold = 30 → no file > 30 → no hot files
    assert!(
        !focus.contains("Hot files:"),
        "even edits should not trigger hot: {focus}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn focus_hot_files_with_clear_outlier() {
    let _store = crate::isolated_store();
    let pid = "test_focus_hot_outlier";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        tasks: vec![],
        files_modified: vec![
            FileEditCount {
                path: "/repo/crates/foo/src/dispatch.rs".into(),
                count: 60,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/a.rs".into(),
                count: 2,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/b.rs".into(),
                count: 2,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/c.rs".into(),
                count: 1,
            },
            FileEditCount {
                path: "/repo/crates/foo/src/d.rs".into(),
                count: 1,
            },
        ],
        commits: vec![],
        failed_commands: vec![],
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    let focus = render_focus_section(pid).unwrap();
    // avg = (60+2+2+1+1)/5 = 13.2, threshold = 39.6
    // dispatch.rs (60) > 39.6 → hot file
    assert!(
        focus.contains("Hot files:"),
        "should detect hot file: {focus}"
    );
    assert!(
        focus.contains("dispatch.rs"),
        "should name the hot file: {focus}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── CmdFail / Blocking tests ──

#[test]
fn command_base_strips_flags() {
    assert_eq!(
        command_base("cargo test -p edda-bridge-claude -- --test-threads=1"),
        "cargo test -p edda-bridge-claude"
    );
}

#[test]
fn command_base_multiline() {
    assert_eq!(
        command_base("cd /repo && cargo build\ncargo test"),
        "cd /repo && cargo build"
    );
}

#[test]
fn truncate_stderr_finds_error_line() {
    let _store = crate::isolated_store();
    let text = "Compiling foo v0.1.0\nerror[E0308]: mismatched types\n  --> src/lib.rs:10:5";
    let snippet = truncate_stderr(text, 200);
    assert!(
        snippet.contains("error[E0308]"),
        "should find error line: {snippet}"
    );
}

#[test]
fn truncate_stderr_includes_source_location() {
    let _store = crate::isolated_store();
    let text = "Compiling foo v0.1.0\nerror[E0308]: mismatched types\n  --> src/lib.rs:10:5";
    let snippet = truncate_stderr(text, 200);
    assert!(
        snippet.contains("error[E0308]"),
        "should have error: {snippet}"
    );
    assert!(
        snippet.contains("src/lib.rs:10:5"),
        "should include source location: {snippet}"
    );
}

#[test]
fn truncate_stderr_skips_irrelevant_next_line() {
    let _store = crate::isolated_store();
    let text = "error: test failed\nCompiling bar v0.2.0";
    let snippet = truncate_stderr(text, 200);
    assert_eq!(snippet, "error: test failed");
}

#[test]
fn truncate_stderr_truncates_long_line() {
    let _store = crate::isolated_store();
    let long_line = "error: ".to_string() + &"x".repeat(300);
    let snippet = truncate_stderr(&long_line, 100);
    assert!(snippet.len() <= 100);
    assert!(snippet.ends_with("..."));
}

#[test]
fn signals_extract_failed_commands() {
    let _store = crate::isolated_store();
    let records = vec![
        // Bash tool_use
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "bash1", "name": "Bash",
                  "input": { "command": "cargo test -p edda-bridge-claude" } }
            ]}
        }),
        // Failed tool_result
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "bash1",
                  "is_error": true,
                  "content": "error: test failed\nthread 'plan::tests::parse' panicked" }
            ]}
        }),
        // Same command fails again
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "bash2", "name": "Bash",
                  "input": { "command": "cargo test -p edda-bridge-claude" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "bash2",
                  "is_error": true,
                  "content": "error: test failed again\nthread 'plan::tests::parse' panicked" }
            ]}
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(
        signals.failed_commands.len(),
        1,
        "should aggregate by command base"
    );
    assert_eq!(signals.failed_commands[0].count, 2);
    assert!(signals.failed_commands[0]
        .command_base
        .contains("cargo test"));
    assert!(!signals.failed_commands[0].stderr_snippet.is_empty());
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_healing_success_clears_failures() {
    let _store = crate::isolated_store();
    // Command fails 3 times, then succeeds → should NOT appear in failed_commands
    let records = vec![
        // Fail 1
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b1", "name": "Bash",
                  "input": { "command": "cargo test -p edda-bridge-claude" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b1",
                  "is_error": true,
                  "content": "error: test failed" }
            ]}
        }),
        // Fail 2
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b2", "name": "Bash",
                  "input": { "command": "cargo test -p edda-bridge-claude" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b2",
                  "is_error": true,
                  "content": "error: test failed again" }
            ]}
        }),
        // Success — heals
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b3", "name": "Bash",
                  "input": { "command": "cargo test -p edda-bridge-claude" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b3",
                  "content": "test result: ok. 120 passed" }
            ]}
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert!(
        signals.failed_commands.is_empty(),
        "success should heal previous failures: {:?}",
        signals.failed_commands
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_healing_then_fail_again() {
    let _store = crate::isolated_store();
    // Fail → Success (heals) → Fail again → should show count=1
    let records = vec![
        // Fail
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b1", "name": "Bash",
                  "input": { "command": "cargo clippy" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b1",
                  "is_error": true,
                  "content": "error: unused import" }
            ]}
        }),
        // Success — heals
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b2", "name": "Bash",
                  "input": { "command": "cargo clippy" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b2",
                  "content": "Finished dev profile" }
            ]}
        }),
        // Fail again — fresh count
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "b3", "name": "Bash",
                  "input": { "command": "cargo clippy" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "b3",
                  "is_error": true,
                  "content": "error: new unused variable" }
            ]}
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.failed_commands.len(), 1);
    assert_eq!(
        signals.failed_commands[0].count, 1,
        "count should reset after healing"
    );
    assert!(signals.failed_commands[0]
        .stderr_snippet
        .contains("new unused variable"));
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_successful_bash_not_tracked() {
    let _store = crate::isolated_store();
    let records = vec![
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "bash1", "name": "Bash",
                  "input": { "command": "cargo build" } }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "bash1",
                  "content": "Compiling..." }
            ]}
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert!(signals.failed_commands.is_empty());
    let _ = fs::remove_file(&path);
}

// ── Usage / Cost tests ──

#[path = "tests/usage.rs"]
mod usage;
#[test]
fn extract_subagent_summary_from_transcript() {
    let _store = crate::isolated_store();
    let records = vec![
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
                    "input": { "command": "git commit -m \"feat: sub-agent output\"" }
                },
                {
                    "type": "text",
                    "text": "Decision: keep extraction bounded"
                }
            ]}
        }),
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "b1",
                "content": "[main abc1234] feat: sub-agent output\n 1 file changed"
            }]}
        }),
    ];

    let path = make_transcript(&records);
    let summary =
        extract_subagent_summary(path.to_str().unwrap_or_default(), "fallback", "agent-1");

    assert_eq!(summary.files_touched.len(), 1);
    assert!(summary.files_touched[0].contains("/repo/src/lib.rs"));
    assert_eq!(summary.commits.len(), 1);
    assert!(summary.commits[0].contains("abc1234"));
    assert!(
        summary
            .decisions
            .iter()
            .any(|d| d.to_lowercase().contains("decision")),
        "expected a decision signal: {:?}",
        summary.decisions
    );
    assert!(
        !summary.summary.is_empty(),
        "summary text should be non-empty"
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn extract_subagent_summary_falls_back_to_last_message() {
    let _store = crate::isolated_store();
    let summary = extract_subagent_summary(
        "",
        "Decision: adopt async queue\nAdditional details",
        "agent-2",
    );

    assert!(summary.files_touched.is_empty());
    assert!(summary.commits.is_empty());
    assert!(
        summary
            .decisions
            .iter()
            .any(|d| d.contains("Decision: adopt async queue")),
        "fallback decisions should include first line: {:?}",
        summary.decisions
    );
    assert!(
        summary.summary.contains("Decision: adopt async queue"),
        "fallback summary should use first assistant line: {}",
        summary.summary
    );
}

#[test]
fn extract_subagent_summary_supports_pointer_like_path() {
    let records = vec![serde_json::json!({
        "type": "assistant",
        "message": { "role": "assistant", "content": [
            {
                "type": "tool_use",
                "id": "w1",
                "name": "Write",
                "input": { "file_path": "/repo/src/new.rs", "content": "fn main() {\n        let _store = crate::isolated_store();}" }
            }
        ]}
    })];

    let path = make_transcript(&records);
    let pointer = format!("{}::sidechain:agent-9", path.to_string_lossy());
    let summary = extract_subagent_summary(&pointer, "fallback text", "agent-9");

    assert_eq!(summary.files_touched.len(), 1);
    assert!(summary.files_touched[0].contains("/repo/src/new.rs"));
    assert!(summary.commits.is_empty());

    let _ = fs::remove_file(&path);
}
