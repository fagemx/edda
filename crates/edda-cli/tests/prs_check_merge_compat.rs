//! Process-boundary compatibility contract for GH-1145 d-001.
//!
//! A live `edda prs check-merge <PR>` is only another spelling of the
//! canonical `edda review merge --pr <PR>` product path. Host-agnostic input
//! and direct facts retain their deprecated advisory 0/1 contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PR: u64 = 42;
const HEAD: &str = "aaaaaaaabbbbbbbbccccccccdddddddd11112222";

fn verdict(id: u64, verdict: &str) -> String {
    let body = format!(
        "## Code Review: Round {id} — PR #{PR} @ {HEAD}\n\n- escalations: none\n\n### \
         Verdict\n\n{verdict}\n"
    );
    format!(
        r#"{{"id":{id},"body":{body},"author_association":"OWNER","updated_at":"2026-09-11T10:{id:02}:00Z"}}"#,
        body = serde_json::to_string(&body).expect("body encodes")
    )
}

struct LiveFixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    store: PathBuf,
    gh: PathBuf,
    fixtures: PathBuf,
}

impl LiveFixture {
    fn new(comments: &[String]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let store = dir.path().join("store");
        let bin = dir.path().join("bin");
        let fixtures = dir.path().join("fixtures");
        for path in [&repo, &store, &bin, &fixtures] {
            std::fs::create_dir_all(path).expect("create fixture dir");
        }
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        std::fs::write(
            fixtures.join("open.json"),
            format!(
                r#"[{{"number":{PR},"headRefOid":"{HEAD}","baseRefName":"main","mergeable":"MERGEABLE"}}]"#
            ),
        )
        .expect("open fixture");
        std::fs::write(
            fixtures.join(format!("comments-{PR}.json")),
            format!("[{}]", comments.join(",")),
        )
        .expect("comments fixture");
        std::fs::write(
            fixtures.join(format!("pr-{PR}.json")),
            format!(
                r#"{{"headRefOid":"{HEAD}","state":"OPEN","title":"fix(edda-cli): converge the compatibility command","mergeable":"MERGEABLE"}}"#
            ),
        )
        .expect("PR fixture");
        std::fs::write(
            fixtures.join("check-runs.json"),
            r#"{"check_runs":[{"name":"CI Gate","html_url":"https://example.invalid/runs/1145"}]}"#,
        )
        .expect("check-runs fixture");
        let gh = write_gh_stub(&bin);
        Self {
            _dir: dir,
            repo,
            store,
            gh,
            fixtures,
        }
    }

    fn run_canonical(&self, extra: &[&str]) -> Output {
        let mut args = vec!["review", "merge", "--pr", "42"];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn run_compat(&self, extra: &[&str]) -> Output {
        let mut args = vec!["prs", "check-merge", "42"];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_edda"))
            .args(args)
            .current_dir(&self.repo)
            .env("EDDA_GH_BIN", &self.gh)
            .env("GH_FIXTURE_DIR", &self.fixtures)
            .env("GH_REPO", "fagemx/edda")
            .env("EDDA_REPO", "fagemx/edda")
            .env("EDDA_STORE_ROOT", &self.store)
            .env("EDDA_SESSION_ID", "prs-check-merge-compat")
            .stdin(Stdio::null())
            .output()
            .expect("spawn edda")
    }

    fn clear_calls(&self) {
        let _ = std::fs::remove_file(self.fixtures.join("calls.txt"));
        let _ = std::fs::remove_file(self.fixtures.join("merged.txt"));
        let _ = std::fs::remove_file(self.fixtures.join("merge-body.txt"));
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.fixtures.join("calls.txt")).unwrap_or_default()
    }

    fn set_pr_head(&self, head: &str) {
        std::fs::write(
            self.fixtures.join(format!("pr-{PR}.json")),
            format!(
                r#"{{"headRefOid":"{head}","state":"OPEN","title":"fix(edda-cli): converge the compatibility command","mergeable":"MERGEABLE"}}"#
            ),
        )
        .expect("replace PR fixture");
    }

    fn write_live_claim(&self) {
        let project_id = edda_store::project_id(&self.repo);
        let state = self.store.join("projects").join(project_id).join("state");
        std::fs::create_dir_all(&state).expect("claim state dir");
        let now = chrono::Utc::now().to_rfc3339();
        let heartbeat = serde_json::json!({
            "session_id": "other-live-session",
            "started_at": now,
            "last_heartbeat": now,
            "label": "other",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        std::fs::write(
            state.join("session.other-live-session.json"),
            serde_json::to_vec(&heartbeat).unwrap(),
        )
        .expect("heartbeat");
        let claim = Command::new(env!("CARGO_BIN_EXE_edda"))
            .args([
                "claim",
                "other-review",
                "--subject",
                "pr:42",
                "--session",
                "other-live-session",
            ])
            .current_dir(&self.repo)
            .env("EDDA_STORE_ROOT", &self.store)
            .output()
            .expect("write claim");
        assert!(claim.status.success(), "stderr={:?}", claim.stderr);
    }
}

fn write_gh_stub(bin: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let script = r#"@echo off
echo %*>>"%GH_FIXTURE_DIR%\calls.txt"
if not "%GH_REPO%"=="fagemx/edda" ( echo GH_REPO not forwarded 1>&2 & exit /b 9 )
if "%1 %2"=="pr view" ( type "%GH_FIXTURE_DIR%\pr-%3.json" & exit /b 0 )
if "%1 %2"=="pr list" ( type "%GH_FIXTURE_DIR%\open.json" & exit /b 0 )
if "%1 %2"=="pr checks" ( exit /b 0 )
if "%1 %2"=="pr merge" ( more >"%GH_FIXTURE_DIR%\merge-body.txt" & echo merged>"%GH_FIXTURE_DIR%\merged.txt" & exit /b 0 )
if "%1 %2"=="api --paginate" goto comments
if "%1"=="api" ( type "%GH_FIXTURE_DIR%\check-runs.json" & exit /b 0 )
echo unexpected gh call: %* 1>&2
exit /b 9
:comments
for /f "tokens=5 delims=/" %%A in ("%3") do type "%GH_FIXTURE_DIR%\comments-%%A.json"
exit /b 0
"#;
        let path = bin.join("gh.bat");
        std::fs::write(&path, script.replace('\n', "\r\n")).expect("write gh stub");
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/calls.txt"
[ "$GH_REPO" = 'fagemx/edda' ] || { echo 'GH_REPO not forwarded' >&2; exit 9; }
case "$1 $2" in
  'pr view') cat "$GH_FIXTURE_DIR/pr-$3.json" ;;
  'pr list') cat "$GH_FIXTURE_DIR/open.json" ;;
  'pr checks') exit 0 ;;
  'pr merge') cat > "$GH_FIXTURE_DIR/merge-body.txt"; echo merged > "$GH_FIXTURE_DIR/merged.txt" ;;
  'api --paginate')
    n=$(printf '%s' "$3" | sed -n 's|.*/issues/\([0-9]*\)/comments$|\1|p')
    cat "$GH_FIXTURE_DIR/comments-$n.json" ;;
  'api '*) cat "$GH_FIXTURE_DIR/check-runs.json" ;;
  *) echo "unexpected gh call: $*" >&2; exit 9 ;;
esac
"#;
        let path = bin.join("gh");
        std::fs::write(&path, script).expect("write gh stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }
}

fn triple(output: &Output) -> (i32, String, String) {
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn help_marks_the_two_modes_and_the_advisory_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_edda"))
        .args(["prs", "check-merge", "--help"])
        .output()
        .expect("spawn help");
    let (code, stdout, stderr) = triple(&output);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("Deprecated compatibility"));
    assert!(stdout.contains("forward to `edda review merge`"));
    assert!(stdout.contains("not R6 evidence"));
    assert!(stdout.contains("--allowed-reviewers"));
    assert!(stdout.contains("Unsupported for live PR forwarding"));
}

#[test]
fn live_check_and_json_are_byte_identical_to_canonical_output_and_exit() {
    let fixture = LiveFixture::new(&[verdict(1, "LGTM (P0=0, P1=0)")]);
    for extra in [&[][..], &["--json"][..]] {
        let canonical = triple(&fixture.run_canonical(extra));
        fixture.clear_calls();
        let compat = triple(&fixture.run_compat(extra));
        assert_eq!(compat, canonical, "forwarded output or exit drifted");
        assert_eq!(compat.0, 0);
    }
}

#[test]
fn live_refusal_and_cannot_judge_exits_match_canonical() {
    let blocked = LiveFixture::new(&[
        verdict(1, "Changes Requested, P0=0, P1=1"),
        verdict(2, "LGTM (P0=0, P1=0)"),
    ]);
    let canonical = triple(&blocked.run_canonical(&[]));
    blocked.clear_calls();
    let compat = triple(&blocked.run_compat(&[]));
    assert_eq!(compat, canonical);
    assert_eq!(compat.0, 1, "standing blocker did not refuse");

    let unreadable = LiveFixture::new(&[verdict(1, "LGTM (P0=0, P1=0)")]);
    unreadable.set_pr_head("not-a-head");
    let canonical = triple(&unreadable.run_canonical(&[]));
    unreadable.clear_calls();
    let compat = triple(&unreadable.run_compat(&[]));
    assert_eq!(compat, canonical);
    assert_eq!(compat.0, 2, "unreadable head was not cannot-judge");
}

#[test]
fn legacy_scalar_flags_cannot_bypass_the_canonical_union() {
    let fixture = LiveFixture::new(&[
        verdict(1, "Changes Requested, P0=0, P1=1"),
        verdict(2, "LGTM (P0=0, P1=0)"),
    ]);
    let output = fixture.run_compat(&[
        "--head-sha",
        HEAD,
        "--verdict",
        "LGTM",
        "--verdict-sha",
        HEAD,
        "--ci-green",
        "--merge",
    ]);
    let (code, stdout, stderr) = triple(&output);
    assert_eq!(code, 1, "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.contains("union over every §7 verdict comment"));
    assert!(!fixture.fixtures.join("merged.txt").exists());
    let calls = fixture.calls().replace('"', "");
    assert!(
        calls.contains(&format!(
            "api --paginate repos/{{owner}}/{{repo}}/issues/{PR}/comments"
        )),
        "the canonical trusted-comment reader was bypassed: {calls}"
    );
}

#[test]
fn live_merge_uses_the_canonical_pinned_squash_argv_and_receipt() {
    let fixture = LiveFixture::new(&[verdict(1, "LGTM (P0=0, P1=0)")]);
    let (code, stdout, stderr) = triple(&fixture.run_compat(&["--merge"]));
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let calls = fixture.calls().replace('"', "");
    let expected = format!(
        "pr merge {PR} --squash --match-head-commit {HEAD} --subject fix(edda-cli): converge the compatibility command (#{PR}) --body-file -"
    );
    assert!(calls.lines().any(|line| line == expected), "calls={calls}");
    let body = std::fs::read_to_string(fixture.fixtures.join("merge-body.txt"))
        .expect("canonical merge receipt body");
    assert!(body.contains(HEAD));
    assert!(body.contains("Round 1"));
    assert!(body.contains("https://example.invalid/runs/1145"));
}

#[test]
fn live_allowed_reviewers_and_force_refuse_instead_of_being_ignored() {
    let fixture = LiveFixture::new(&[verdict(1, "LGTM (P0=0, P1=0)")]);
    for (extra, expected) in [
        (
            &["--allowed-reviewers", "alice,bob"][..],
            "--allowed-reviewers is unsupported in live-forward mode",
        ),
        (
            &["--force"][..],
            "--force is unsupported in live-forward mode",
        ),
    ] {
        fixture.clear_calls();
        let (code, stdout, stderr) = triple(&fixture.run_compat(extra));
        assert_eq!(code, 2, "stdout={stdout}\nstderr={stderr}");
        assert!(stderr.contains(expected));
        if extra[0] == "--allowed-reviewers" {
            assert!(stderr.contains("author_association"));
        }
        assert!(fixture.calls().is_empty(), "refusal still read GitHub");
    }
}

#[test]
fn a_live_process_claim_does_not_wrap_the_canonical_path() {
    let fixture = LiveFixture::new(&[verdict(1, "LGTM (P0=0, P1=0)")]);
    fixture.write_live_claim();
    fixture.clear_calls();
    let (code, stdout, stderr) = triple(&fixture.run_compat(&[]));
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    assert!(!stdout.contains("Claim Notice"));
    assert!(!stderr.contains("claimed by active session"));
}

struct OfflineFixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    store: PathBuf,
}

impl OfflineFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let store = dir.path().join("store");
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        Self {
            _dir: dir,
            repo,
            store,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_edda"))
            .args(args)
            .current_dir(&self.repo)
            .env("EDDA_STORE_ROOT", &self.store)
            .env("EDDA_SESSION_ID", "offline-compat")
            .stdin(Stdio::null())
            .output()
            .expect("spawn edda")
    }
}

#[test]
fn offline_direct_json_keeps_schema_and_zero_one_contract() {
    let fixture = OfflineFixture::new();
    let valid = fixture.run(&[
        "prs",
        "check-merge",
        "--head-sha",
        HEAD,
        "--verdict",
        "approved",
        "--verdict-sha",
        HEAD,
        "--ci-green",
        "--json",
    ]);
    let (code, stdout, stderr) = triple(&valid);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.contains("Deprecated advisory scalar diagnostics only"));
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("compat JSON");
    assert_eq!(value["can_merge"], true);
    assert!(value.get("advisory").is_none());
    assert!(value.get("deprecated").is_none());

    let not_lgtm = fixture.run(&[
        "prs",
        "check-merge",
        "--head-sha",
        HEAD,
        "--verdict",
        "not lgtm",
        "--verdict-sha",
        HEAD,
        "--ci-green",
        "--json",
    ]);
    let (code, stdout, stderr) = triple(&not_lgtm);
    assert_eq!(code, 1, "stdout={stdout}\nstderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("refusal JSON");
    assert_eq!(value["can_merge"], false);
    assert!(value["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str().unwrap().contains("not LGTM")));
}

#[test]
fn offline_input_refuses_contradictory_ci_and_cannot_merge() {
    let fixture = OfflineFixture::new();
    let input = fixture.repo.join("input.json");
    std::fs::write(
        &input,
        serde_json::json!({
            "head_sha": HEAD,
            "pr": 42,
            "verdict": "LGTM",
            "verdict_sha": HEAD,
            "p0_count": 0,
            "p1_count": 0,
            "required_ci_green": true,
            "failed_checks": ["CI Gate (failure)"]
        })
        .to_string(),
    )
    .expect("input fixture");
    let path = input.to_str().unwrap();
    let (code, stdout, stderr) =
        triple(&fixture.run(&["prs", "check-merge", "--input", path, "--json"]));
    assert_eq!(code, 1, "stdout={stdout}\nstderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("refusal JSON");
    assert!(value["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason
            .as_str()
            .unwrap()
            .contains("contradicts nonempty failed_checks")));

    let (code, stdout, stderr) =
        triple(&fixture.run(&["prs", "check-merge", "--input", path, "--merge"]));
    assert_eq!(code, 1, "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.contains("--merge is forbidden"));
    assert!(stderr.contains("edda review merge --pr <PR> --merge"));
}
