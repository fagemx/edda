//! Cross-process GH-782 regression tests: real dispatch, isolated gh and agent.
use std::path::{Path, PathBuf};
use std::process::Command;

const UNCLAIMED: &str = r#"{"labels":[],"comments":[]}"#;
const ROUTED: &str = r#"{"labels":[{"name":"lane:feature"},{"name":"lane:4090"}],"comments":[]}"#;
const SELF: &str = r#"{"comments":[{"body":"taking: 4090/worker-1 at t","createdAt":"t"}]}"#;
const OTHER: &str = r#"{"comments":[{"body":"taking: 4090/worker-2 at 2026-09-02T06:30:00Z","createdAt":"2026-09-02T06:30:00Z"}]}"#;

fn stub(dir: &Path, name: &str, windows: &str, unix: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = unix;
        let path = dir.join(format!("{name}.bat"));
        std::fs::write(&path, windows.replace('\n', "\r\n")).unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = windows;
        let path = dir.join(name);
        std::fs::write(&path, unix).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

struct Fixture {
    root: tempfile::TempDir,
    gh: PathBuf,
    agent: PathBuf,
}

impl Fixture {
    fn new(issue: &str, prs: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("issue.json", issue),
            ("prs.json", prs),
            ("self.json", SELF),
            ("prompt.txt", "fixture turn"),
            ("labels.json", "[\"fleet:ready\"]"),
        ] {
            std::fs::write(root.path().join(name), text).unwrap();
        }
        let gh = stub(
            root.path(),
            "gh",
            r#"@echo off
 echo called>>"%GH_TEST_ROOT%\reads.txt"
 (echo %CD%)>>"%GH_TEST_ROOT%\gh-cwd.txt"
 if "%1"=="pr" goto prs
 if "%2"=="view" goto view
 if "%2"=="comment" goto comment
 if "%2"=="edit" goto edit
 exit /b 7
 :prs
 if exist "%GH_TEST_ROOT%\fail-pr" exit /b 1
 type "%GH_TEST_ROOT%\prs.json"
 exit /b 0
 :view
 if exist "%GH_TEST_ROOT%\fail-view" exit /b 1
 if exist "%GH_TEST_ROOT%\comment.txt" (type "%GH_TEST_ROOT%\self.json") else (type "%GH_TEST_ROOT%\issue.json")
 exit /b 0
 :comment
 if exist "%GH_TEST_ROOT%\fail-comment" exit /b 1
 echo %~5>>"%GH_TEST_ROOT%\comment.txt"
 exit /b 0
 :edit
 if exist "%GH_TEST_ROOT%\fail-edit" exit /b 1
 if not "%~4"=="--add-label" exit /b 8
 if not "%~5"=="fleet:claimed" exit /b 8
 if not "%~6"=="--remove-label" exit /b 8
 if not "%~7"=="fleet:ready" exit /b 8
 if not "%~8"=="--add-assignee" exit /b 8
 if not "%~9"=="@me" exit /b 8
 echo ["fleet:claimed"]>"%GH_TEST_ROOT%\labels.json"
 echo @me>"%GH_TEST_ROOT%\assignee.txt"
 exit /b 0
"#,
            r#"#!/bin/sh
set -eu
echo called >> "$GH_TEST_ROOT/reads.txt"
pwd -P >> "$GH_TEST_ROOT/gh-cwd.txt"
case "$1 $2" in
 'pr list') [ ! -f "$GH_TEST_ROOT/fail-pr" ] || exit 1; cat "$GH_TEST_ROOT/prs.json" ;;
 'issue view')
    [ ! -f "$GH_TEST_ROOT/fail-view" ] || exit 1
    if [ -f "$GH_TEST_ROOT/comment.txt" ]; then cat "$GH_TEST_ROOT/self.json"
    else cat "$GH_TEST_ROOT/issue.json"; fi ;;
 'issue comment')
    [ ! -f "$GH_TEST_ROOT/fail-comment" ] || exit 1
    printf '%s\n' "$5" >> "$GH_TEST_ROOT/comment.txt" ;;
 'issue edit')
    [ ! -f "$GH_TEST_ROOT/fail-edit" ] || exit 1
    [ "$4 $5 $6 $7 $8 $9" = '--add-label fleet:claimed --remove-label fleet:ready --add-assignee @me' ] || exit 8
    printf '%s\n' '["fleet:claimed"]' > "$GH_TEST_ROOT/labels.json"
    echo @me > "$GH_TEST_ROOT/assignee.txt" ;;
 *) exit 7 ;;
esac
"#,
        );
        let agent = stub(
            root.path(),
            "codex",
            r#"@echo off
 if "%1"=="--version" (echo codex-cli 0.1.0& exit /b 0)
 echo started>"%GH_TEST_ROOT%\agent-started.txt"
 if exist "%GH_TEST_ROOT%\comment.txt" copy /y "%GH_TEST_ROOT%\comment.txt" "%GH_TEST_ROOT%\spawn-comment.txt" >nul
 copy /y "%GH_TEST_ROOT%\labels.json" "%GH_TEST_ROOT%\spawn-labels.json" >nul
 if exist "%GH_TEST_ROOT%\assignee.txt" copy /y "%GH_TEST_ROOT%\assignee.txt" "%GH_TEST_ROOT%\spawn-assignee.txt" >nul
 exit /b 1
"#,
            r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'codex-cli 0.1.0'; exit 0; fi
echo started > "$GH_TEST_ROOT/agent-started.txt"
[ ! -f "$GH_TEST_ROOT/comment.txt" ] || cp "$GH_TEST_ROOT/comment.txt" "$GH_TEST_ROOT/spawn-comment.txt"
cp "$GH_TEST_ROOT/labels.json" "$GH_TEST_ROOT/spawn-labels.json"
[ ! -f "$GH_TEST_ROOT/assignee.txt" ] || cp "$GH_TEST_ROOT/assignee.txt" "$GH_TEST_ROOT/spawn-assignee.txt"
exit 1
"#,
        );
        Self { root, gh, agent }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.run_with_prompt(args, &self.root.path().join("prompt.txt"))
    }

    /// Same as [`Self::run`], but with an explicit prompt file so tests can
    /// point at one that does not exist (round 2, F1).
    fn run_with_prompt(&self, args: &[&str], prompt: &Path) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_edda"))
            .args([
                "dispatch",
                "--agent",
                "codex",
                "--json",
                "--timeout-sec",
                "5",
            ])
            .args(args)
            .arg("--prompt-file")
            .arg(prompt)
            .env("EDDA_GH_BIN", &self.gh)
            .env("EDDA_CODEX_BIN", &self.agent)
            .env("GH_TEST_ROOT", self.root.path())
            .env("EDDA_STORE_ROOT", self.root.path().join("store"))
            .env_remove("EDDA_MACHINE")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("dispatch runs")
    }

    fn dispatch(&self) -> std::process::Output {
        self.run(&["--issue", "656", "--machine", "4090/worker-1"])
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.path().join(name)).unwrap_or_default()
    }

    fn refuse(&self, output: std::process::Output) -> String {
        assert_eq!(
            output.status.code(),
            Some(2),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["outcome"], "claim_refused");
        assert_eq!(self.read("agent-started.txt"), "");
        value["error"].as_str().unwrap().into()
    }
}

#[test]
fn same_machine_other_role_refuses_with_identity_and_timestamp() {
    let f = Fixture::new(OTHER, "[]");
    let error = f.refuse(f.dispatch());
    assert!(error.contains("4090/worker-2"));
    assert!(error.contains("2026-09-02T06:30:00Z"));
    assert_eq!(f.read("comment.txt"), "");
}

#[test]
fn routing_labels_allow_claim_and_comment_and_queue_exist_before_agent_spawn() {
    let f = Fixture::new(ROUTED, "[]");
    let output = f.dispatch();
    assert_ne!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(f
        .read("spawn-comment.txt")
        .starts_with("taking: 4090/worker-1 at "));
    assert!(f.read("spawn-labels.json").contains("fleet:claimed"));
    assert!(!f.read("spawn-labels.json").contains("fleet:ready"));
    assert_eq!(f.read("spawn-assignee.txt").trim(), "@me");
    let first = f.read("comment.txt");
    f.dispatch();
    assert_eq!(
        f.read("comment.txt"),
        first,
        "second dispatch must not duplicate claim"
    );
}

#[test]
fn self_claim_proceeds_and_repairs_queue_without_duplicate_comment() {
    let f = Fixture::new(SELF, "[]");
    f.dispatch();
    assert_eq!(f.read("comment.txt"), "");
    assert!(f.read("agent-started.txt").contains("started"));
    assert!(f.read("spawn-labels.json").contains("fleet:claimed"));
}

#[test]
fn merged_title_and_open_branch_prs_refuse_even_without_comments() {
    for (prs, expected) in [
        (
            r#"[{"number":716,"state":"MERGED","title":"fix (GH-656)","headRefName":"x"}]"#,
            "#716 (merged)",
        ),
        (
            r#"[{"number":900,"state":"OPEN","title":"x","headRefName":"codex/GH656-fix"}]"#,
            "open PR #900",
        ),
    ] {
        let f = Fixture::new(UNCLAIMED, prs);
        let error = f.refuse(f.dispatch());
        assert!(error.contains(expected), "{error}");
        assert_eq!(f.read("comment.txt"), "");
    }
}

#[test]
fn gh_read_or_write_failure_refuses_without_starting_agent() {
    for mode in ["pr", "view", "comment", "edit"] {
        let f = Fixture::new(UNCLAIMED, "[]");
        std::fs::write(f.root.path().join(format!("fail-{mode}")), "").unwrap();
        let error = f.refuse(f.dispatch());
        assert!(error.contains("failed"), "{error}");
        if mode != "edit" {
            assert_eq!(f.read("comment.txt"), "");
        }
    }
}

#[test]
fn malformed_pr_json_fails_closed() {
    let f = Fixture::new(UNCLAIMED, "not json");
    assert!(f.refuse(f.dispatch()).contains("parse gh PR history"));
}

#[test]
fn no_issue_skips_guard_and_explicit_machine_without_issue_is_rejected() {
    let f = Fixture::new(OTHER, "[]");
    f.run(&[]);
    assert_eq!(f.read("reads.txt"), "");
    assert!(f.read("agent-started.txt").contains("started"));
    let output = f.run(&["--machine", "4090/worker-1"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--issue"));
    assert_eq!(f.read("reads.txt"), "");
}

// ── Round 2 (review850 F1/F2/F4) ──

/// F1: an unreadable prompt is a local prerequisite failure — it must fail
/// before the durable claim writes, with no gh call and no agent started.
#[test]
fn missing_prompt_file_writes_no_claim_and_starts_no_agent() {
    let f = Fixture::new(ROUTED, "[]");
    let missing = f.root.path().join("no-such-prompt.txt");
    let output = f.run_with_prompt(&["--issue", "656", "--machine", "4090/worker-1"], &missing);
    assert_ne!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(f.read("reads.txt"), "", "gh must not be called");
    assert_eq!(f.read("comment.txt"), "", "no claim comment may be written");
    assert_eq!(f.read("agent-started.txt"), "");
}

/// F1: a nonexistent --cwd fails the same local-prerequisite gate.
#[test]
fn invalid_cwd_writes_no_claim_and_starts_no_agent() {
    let f = Fixture::new(ROUTED, "[]");
    let missing_dir = f.root.path().join("no-such-dir");
    let cwd_arg = missing_dir.to_string_lossy().into_owned();
    let output = f.run_with_prompt(
        &[
            "--cwd",
            cwd_arg.as_str(),
            "--issue",
            "656",
            "--machine",
            "4090/worker-1",
        ],
        &f.root.path().join("prompt.txt"),
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(f.read("reads.txt"), "", "gh must not be called");
    assert_eq!(f.read("comment.txt"), "");
    assert_eq!(f.read("agent-started.txt"), "");
}

/// F2: every gh read and write must run in the selected dispatch
/// repository (--cwd), not in whatever directory edda was started from.
/// The stub gh records its working directory on every call.
#[test]
fn claim_guard_gh_calls_bind_to_the_selected_dispatch_cwd() {
    let f = Fixture::new(ROUTED, "[]");
    let dir_a = f.root.path().join("repo-a");
    let dir_b = f.root.path().join("repo-b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    for dir in [&dir_a, &dir_b] {
        let _ = std::fs::remove_file(f.root.path().join("gh-cwd.txt"));
        let output = f.run_with_prompt(
            &[
                "--cwd",
                dir.to_string_lossy().as_ref(),
                "--issue",
                "656",
                "--machine",
                "4090/worker-1",
            ],
            &f.root.path().join("prompt.txt"),
        );
        assert_ne!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let recorded = f.read("gh-cwd.txt");
        assert!(!recorded.is_empty(), "the guard must call gh");
        let expected = std::fs::canonicalize(dir).unwrap();
        for line in recorded.lines() {
            assert_eq!(
                std::fs::canonicalize(line).unwrap(),
                expected,
                "every gh call must run in the dispatch --cwd"
            );
        }
    }
}

/// F4: a bare machine token (no role) is the #782 usage error — exit 2
/// through the claim_refused JSON path, before any gh call or agent.
#[test]
fn bare_machine_identity_exits_2_as_usage_before_any_gh_call_or_agent() {
    let f = Fixture::new(ROUTED, "[]");
    let output = f.run(&["--issue", "656", "--machine", "4090"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "claim_refused");
    assert_eq!(value["machine"], "4090");
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("<machine>/<role>"));
    assert_eq!(f.read("reads.txt"), "", "gh must not be called");
    assert_eq!(f.read("agent-started.txt"), "");
}
