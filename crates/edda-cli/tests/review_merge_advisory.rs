//! Process-boundary contract test for `edda review merge`'s #1124 split: the
//! fleet-wide drift walk is PRINTED and refuses nothing, while the subject
//! PR's own reading still decides.
//!
//! The in-crate fixtures stop at the `Reads` seam (`merge_inner` + `Fake`),
//! which is where the decision is made but not where the fleet meets it. The
//! contract is a property of the verb the fleet calls, so it is measured the
//! way the fleet runs it: the real binary, a real PATH, and a stub `gh` for
//! the reads. `REVIEW.md`'s evidence probes are help-only by design
//! (`cmd_review/evidence/probes.rs`), so a runtime measurement of this
//! behaviour can only live here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The subject's reviewed head.
const HEAD: &str = "aaaaaaaabbbbbbbbccccccccdddddddd11112222";
/// The head an older verdict was pinned to.
const OTHER: &str = "1111111111111111111111111111111111111111";
/// The neighbour PR's head: nothing on it is reviewed.
const NEIGHBOUR: &str = "2222222222222222222222222222222222222222";
/// The subject PR number.
const SUBJECT: u64 = 42;
/// The unrelated, drifting PR number.
const DRIFTED: u64 = 7;

/// A verdict comment as `gh api .../issues/<n>/comments` returns it: numeric
/// `id` and `updated_at` are the two fields the gate's ordering reads.
fn verdict(id: u64, sha: &str, verdict: &str, updated_at: &str) -> String {
    let body = format!(
        "## Code Review: Round 1 — PR #{SUBJECT} @ {sha}\n\n- escalations: none\n\n### \
         Verdict\n\n{verdict}\n"
    );
    format!(
        r#"{{"id":{id},"body":{body},"author_association":"OWNER","updated_at":"{updated_at}"}}"#,
        body = serde_json::to_string(&body).expect("body encodes")
    )
}

fn shadow_verdict(id: u64, sha: &str, updated_at: &str) -> String {
    let body = format!(
        "## Code Review: Round 2 (SHADOW) — PR #{SUBJECT} @ {sha}\n\n- shadow: true\n\n### \
         Verdict\n\nLGTM (P0=0, P1=0)\n"
    );
    format!(
        r#"{{"id":{id},"body":{body},"author_association":"OWNER","updated_at":"{updated_at}"}}"#,
        body = serde_json::to_string(&body).expect("body encodes")
    )
}

/// The open-PR set the walk enumerates: the subject, and one neighbour that
/// has never been reviewed (the #1124 shape — unrelated drift).
fn open_set() -> String {
    format!(
        r#"[{{"number":{SUBJECT},"headRefOid":"{HEAD}","baseRefName":"main","mergeable":"MERGEABLE"}},
            {{"number":{DRIFTED},"headRefOid":"{NEIGHBOUR}","baseRefName":"main","mergeable":"MERGEABLE"}}]"#
    )
}

fn subject_pr() -> String {
    format!(
        r#"{{"headRefOid":"{HEAD}","state":"OPEN","title":"fix(edda-cli): a subject the fleet already reviewed","mergeable":"MERGEABLE"}}"#
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    /// The stub `gh` the verb is pointed at with `EDDA_GH_BIN`.
    gh: PathBuf,
    repo: PathBuf,
    merged_marker: PathBuf,
    calls_marker: PathBuf,
}

impl Fixture {
    /// `open_prs` is the walk's own view; `subject_comments` is what
    /// `gh api .../issues/42/comments` answers.
    fn new(open_prs: &str, subject_comments: &[String]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let bin = dir.path().join("bin");
        let fixtures = dir.path().join("fixtures");
        for path in [&repo, &bin, &fixtures] {
            std::fs::create_dir_all(path).expect("create dir");
        }
        // A real repository: the verb resolves its subject from the checkout.
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "merge@test"][..],
            &["config", "user.name", "merge"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("spawn git");
            assert!(out.status.success(), "git {args:?}");
        }

        let empty = "[]".to_owned();
        for (name, text) in [
            ("open.json", open_prs.to_owned()),
            (
                ".check-runs.json",
                r#"{"check_runs":[{"name":"CI Gate","html_url":"https://example.invalid/run/1"}]}"#
                    .to_owned(),
            ),
            // The stub answers per PR number: the subject's own comments are
            // whatever the test built, and the neighbour stays unreviewed.
            (
                &format!("comments-{SUBJECT}.json"),
                format!("[{}]", subject_comments.join(",")),
            ),
            (format!("comments-{DRIFTED}.json").as_str(), empty),
            (format!("pr-{SUBJECT}.json").as_str(), subject_pr()),
        ] {
            std::fs::write(fixtures.join(name), text).expect("write fixture");
        }

        std::fs::write(fixtures.join("checks.json"), "[]").expect("write checks fixture");
        let gh = stub(&bin);
        let merged_marker = fixtures.join("merged.txt");
        let calls_marker = fixtures.join("calls.txt");
        Self {
            _dir: dir,
            gh,
            repo,
            merged_marker,
            calls_marker,
        }
    }

    fn run(&self, extra: &[&str]) -> (i32, String, String) {
        self.run_with_checks(extra, 0, 0, "[]")
    }

    fn run_with_checks(
        &self,
        extra: &[&str],
        plain_exit: i32,
        diagnostic_exit: i32,
        diagnostic_stdout: &str,
    ) -> (i32, String, String) {
        std::fs::write(
            self._dir.path().join("fixtures/checks.json"),
            diagnostic_stdout,
        )
        .expect("write diagnostic checks fixture");
        let out = Command::new(env!("CARGO_BIN_EXE_edda"))
            .arg("review")
            .arg("merge")
            .arg("--pr")
            .arg(SUBJECT.to_string())
            .args(extra)
            .current_dir(&self.repo)
            .env("EDDA_GH_BIN", &self.gh)
            .env("GH_FIXTURE_DIR", self._dir.path().join("fixtures"))
            .env("GH_CHECKS_EXIT", plain_exit.to_string())
            .env("GH_CHECKS_JSON_EXIT", diagnostic_exit.to_string())
            .env("EDDA_REPO", "fagemx/edda")
            .env("EDDA_STORE_ROOT", self._dir.path().join("store"))
            .env("EDDA_SESSION_ID", "review-merge-advisory-probe")
            .stdin(Stdio::null())
            .output()
            .expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.calls_marker).expect("read gh calls")
    }
}

/// Write the stub `gh` the verb is pointed at with `EDDA_GH_BIN`. Anything
/// other than the reads this verb makes is an unknown call and fails loudly,
/// so a new `gh` invocation cannot slip past the fixture unnoticed.
fn stub(bin: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let bat = r#"@echo off
echo %*>>"%GH_FIXTURE_DIR%\calls.txt"
if not "%GH_REPO%"=="fagemx/edda" ( echo GH_REPO not forwarded 1>&2 & exit /b 9 )
if "%1 %2"=="pr view" ( type "%GH_FIXTURE_DIR%\pr-%3.json" & exit /b 0 )
if "%1 %2"=="pr list" ( type "%GH_FIXTURE_DIR%\open.json" & exit /b 0 )
if "%1 %2"=="pr checks" goto checks
if "%1 %2"=="pr merge" ( echo merged>"%GH_FIXTURE_DIR%\merged.txt" & exit /b 0 )
if "%1 %2"=="api --paginate" goto comments
if "%1"=="api" ( type "%GH_FIXTURE_DIR%\.check-runs.json" & exit /b 0 )
echo unexpected gh call: %* 1>&2
exit /b 9
:checks
if "%5"=="--json" ( type "%GH_FIXTURE_DIR%\checks.json" & exit /b %GH_CHECKS_JSON_EXIT% )
exit /b %GH_CHECKS_EXIT%
:comments
for /f "tokens=5 delims=/" %%A in ("%3") do type "%GH_FIXTURE_DIR%\comments-%%A.json"
exit /b 0
"#;
        let path = bin.join("gh.bat");
        std::fs::write(&path, bat.replace('\n', "\r\n")).expect("write gh stub");
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let sh = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/calls.txt"
[ "$GH_REPO" = 'fagemx/edda' ] || { echo 'GH_REPO not forwarded' >&2; exit 9; }
case "$1 $2" in
  'pr view') cat "$GH_FIXTURE_DIR/pr-$3.json" ;;
  'pr list') cat "$GH_FIXTURE_DIR/open.json" ;;
  'pr checks')
    if [ "$5" = '--json' ]; then
      cat "$GH_FIXTURE_DIR/checks.json"
      exit "${GH_CHECKS_JSON_EXIT:-0}"
    fi
    exit "${GH_CHECKS_EXIT:-0}" ;;
  'pr merge') echo merged > "$GH_FIXTURE_DIR/merged.txt" ;;
  'api --paginate')
    n=$(printf '%s' "$3" | sed -n 's|.*/issues/\([0-9]*\)/comments$|\1|p')
    cat "$GH_FIXTURE_DIR/comments-$n.json" ;;
  'api '*) cat "$GH_FIXTURE_DIR/.check-runs.json" ;;
  *) echo "unexpected gh call: $*" >&2; exit 9 ;;
esac
"#;
        let path = bin.join("gh");
        std::fs::write(&path, sh).expect("write gh stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }
}

fn lgtm_on_head() -> Vec<String> {
    vec![verdict(
        11,
        HEAD,
        "LGTM (P0=0, P1=0)",
        "2026-09-08T11:00:00Z",
    )]
}

// #1124's whole point, at the process boundary: a neighbour nobody reviewed
// does not refuse a subject whose own gate is green. The walk's line is still
// printed — the operator keeps the fleet signal — and the verb accepts.
#[test]
fn a_dirty_neighbour_is_printed_and_does_not_refuse_the_green_subject() {
    let f = Fixture::new(&open_set(), &lgtm_on_head());
    let (code, stdout, stderr) = f.run(&["--check"]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stderr.contains(&format!("#{DRIFTED} {}", &NEIGHBOUR[..12])),
        "the walk's line for the neighbour is not printed: {stderr}"
    );
    assert!(
        stderr.contains("no verdict on head") && stderr.contains("advisory only"),
        "the advisory ruling is missing: {stderr}"
    );
    assert!(
        stdout.contains(&format!("review accepted: PR #{SUBJECT} @ {HEAD}")),
        "{stdout}"
    );
    let calls = f.calls();
    let check_calls: Vec<String> = calls
        .lines()
        .filter(|line| line.starts_with("pr checks"))
        .map(|line| line.replace('"', ""))
        .collect();
    assert_eq!(
        check_calls,
        [format!("pr checks {SUBJECT} --required")],
        "plain success must be the sole Green authority and skip the diagnostic"
    );
}

#[test]
fn required_check_diagnostics_refuse_every_observed_shape_without_squashing() {
    let blocked = r#"[{"name":"build","state":"FAILURE","bucket":"fail"},
        {"name":"test","state":"PENDING","bucket":"pending"}]"#;
    let raced = r#"[{"name":"build","state":"SUCCESS","bucket":"pass"},
        {"name":"docs","state":"SKIPPED","bucket":"skipping"}]"#;
    for (label, diagnostic_exit, diagnostic_stdout, expected_exit, messages) in [
        (
            "exit 1 empty stdout",
            1,
            "",
            2,
            &[
                "no structured report",
                "retry",
                "waiting for CI",
                "GitHub connectivity",
                "repository access",
            ][..],
        ),
        (
            "exit 8 empty stdout",
            8,
            "",
            2,
            &[
                "no structured report",
                "retry",
                "waiting for CI",
                "GitHub connectivity",
                "repository access",
            ][..],
        ),
        (
            "empty JSON array",
            0,
            "[]",
            1,
            &["no required-check rows"][..],
        ),
        (
            "failed and pending rows despite JSON exit 0",
            0,
            blocked,
            1,
            &["build (bucket=fail", "test (bucket=pending"][..],
        ),
        (
            "all-pass race",
            0,
            raced,
            1,
            &[
                "state changed during the read",
                "run `edda review merge` again",
            ][..],
        ),
        ("malformed JSON", 0, "{", 2, &["JSON is malformed"][..]),
        (
            "API error shape",
            1,
            r#"{"message":"rate limit"}"#,
            2,
            &["unknown row shape"][..],
        ),
        ("unexpected exit", 9, "[]", 2, &["exited unexpectedly"][..]),
    ] {
        let f = Fixture::new(&open_set(), &lgtm_on_head());
        let (code, stdout, stderr) =
            f.run_with_checks(&["--merge"], 1, diagnostic_exit, diagnostic_stdout);
        assert_eq!(
            code, expected_exit,
            "{label}: stdout: {stdout}\nstderr: {stderr}"
        );
        for message in messages {
            assert!(
                stderr.contains(message),
                "{label}: missing {message:?}: {stderr}"
            );
        }
        if diagnostic_stdout.is_empty() {
            assert!(
                !stderr.contains("no required-check rows are currently reported"),
                "{label}: empty stdout falsely claimed absence: {stderr}"
            );
        }
        assert!(
            !f.merged_marker.exists(),
            "{label}: a refusal issued gh pr merge"
        );
        let calls = f.calls();
        let check_calls: Vec<String> = calls
            .lines()
            .filter(|line| line.starts_with("pr checks"))
            .map(|line| line.replace('"', ""))
            .collect();
        assert_eq!(
            check_calls,
            [
                format!("pr checks {SUBJECT} --required"),
                format!("pr checks {SUBJECT} --required --json name,state,bucket"),
            ],
            "{label}: required-check argv did not preserve the exact PR and field order"
        );
    }
}

// Round 4's P0, measured the same way: the walk's own (creation) order reads a
// later verdict as newest and stale, while the gate's (edit) order reads the
// older, head-pinned LGTM as newest and green. The two disagreeing is a
// refusal — attributed to the subject, not to the fleet.
#[test]
fn the_subjects_own_stale_reading_refuses() {
    let comments = vec![
        // Created first, edited last: newest by `updated_at`.
        verdict(11, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T12:00:00Z"),
        // Created last, never edited: newest by comment order, and stale.
        verdict(
            12,
            OTHER,
            "Changes Requested, P0=0, P1=1",
            "2026-09-08T11:00:00Z",
        ),
    ];
    let f = Fixture::new(&open_set(), &comments);
    let (code, stdout, stderr) = f.run(&["--check"]);
    assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stderr.contains("newest authoritative §7 round in comment order reads stale from"),
        "the refusal is not attributed to the subject's own reading: {stderr}"
    );
    assert!(
        !stderr.contains("PR #7"),
        "the neighbour was named by a refusal of the subject: {stderr}"
    );
}

// Round 4's second P0 and Round 8's P1, at the same boundary: the split really
// merges, and a SHADOW round — newest by edit order — decides nothing.
#[test]
fn the_green_subject_merges_and_a_shadow_round_decides_nothing() {
    let mut comments = lgtm_on_head();
    comments.push(shadow_verdict(12, OTHER, "2026-09-08T12:00:00Z"));
    let f = Fixture::new(&open_set(), &comments);
    let (code, stdout, stderr) = f.run(&["--merge"]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        f.merged_marker.is_file(),
        "the squash was never issued: {stderr}"
    );
    assert!(
        stderr.contains("advisory only"),
        "the walk's ruling is missing from a merge: {stderr}"
    );
}
