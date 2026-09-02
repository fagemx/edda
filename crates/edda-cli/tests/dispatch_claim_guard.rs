//! Cross-process checks for the GH-656 cross-machine claim guard on
//! `edda dispatch`.
//!
//! GitHub is the only shared truth between machines (each has its own
//! `.edda/`), so the guard consults `gh issue view` — here a stub `gh`
//! binary (via `EDDA_GH_BIN`, the same override pattern as
//! `EDDA_CODEX_BIN`) whose fixture JSON represents the issue state. The
//! codex backend bin points at a path that cannot exist, so anything that
//! gets past the guard fails at `verify_available` with a deterministic
//! "codex CLI not found" instead of spawning a real agent: a refusal that
//! exits 2 with a JSON error, and a pass-through that reaches the launcher.

use std::path::{Path, PathBuf};
use std::process::Command;

const UNCLAIMED: &str = r#"{"labels":[],"comments":[]}"#;

/// Another machine claimed the issue: a `lane:4090` label plus a
/// `taking: 4090` comment (the `fleet.cross-machine-claim` convention).
const OTHER_CLAIMED: &str = r#"{"labels":[{"name":"lane:4090"}],"comments":[{"author":{"login":"controller"},"body":"taking: 4090 at 2026-09-02T06:30:00Z","createdAt":"2026-09-02T06:30:00Z"}]}"#;

/// This machine already claimed the issue (idempotent re-dispatch).
const SELF_CLAIMED: &str = r#"{"labels":[{"name":"lane:docs"}],"comments":[{"author":{"login":"controller"},"body":"taking: docs at 2026-09-02T07:06:00Z","createdAt":"2026-09-02T07:06:00Z"}]}"#;

/// Write a stub `gh` into `dir` that marks that it ran (`gh-called.txt`),
/// prints the fixture JSON on stdout, and returns its path for
/// `EDDA_GH_BIN`. Batch file on Windows (spawned through cmd by std),
/// shell script elsewhere.
fn write_stub_gh(dir: &Path, fixture: &str) -> PathBuf {
    std::fs::write(dir.join("issue.json"), fixture).expect("fixture written");
    #[cfg(windows)]
    let stub = {
        let stub = dir.join("gh.bat");
        std::fs::write(
            &stub,
            "@echo off\r\necho called > \"%~dp0gh-called.txt\"\r\ntype \"%~dp0issue.json\"\r\n",
        )
        .expect("stub written");
        stub
    };
    #[cfg(not(windows))]
    let stub = {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("gh");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho called > \"$(dirname \"$0\")/gh-called.txt\"\ncat \"$(dirname \"$0\")/issue.json\"\n",
        )
        .expect("stub written");
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("stub made executable");
        stub
    };
    stub
}

fn stub_called(dir: &Path) -> bool {
    dir.join("gh-called.txt").exists()
}

struct DispatchRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    gh_called: bool,
}

/// Run the real `edda dispatch` binary with the stub `gh` and a codex
/// backend that cannot exist, so the outcome distinguishes: guard refusal
/// (exit 2 + JSON), guard pass-through reaching the launcher ("codex CLI
/// not found"), or a pre-guard CLI error.
fn run_dispatch(root: &Path, fixture: &str, extra_args: &[&str]) -> DispatchRun {
    let stub = write_stub_gh(root, fixture);
    let prompt = root.join("prompt.txt");
    std::fs::write(&prompt, "do the thing").expect("prompt written");

    let edda_bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
    let output = Command::new(&edda_bin)
        .args(["dispatch", "--agent", "codex", "--json"])
        .args(extra_args)
        .arg("--prompt-file")
        .arg(&prompt)
        .env("EDDA_GH_BIN", &stub)
        .env("EDDA_CODEX_BIN", root.join("no-such-codex"))
        .env("EDDA_STORE_ROOT", root.join("store"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("edda binary runs");
    DispatchRun {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        gh_called: stub_called(root),
    }
}

/// GH-656 doneWhen: `edda dispatch --issue N --machine docs` against an
/// issue claimed by another machine must exit 2, start no agent, and put
/// the reason (the other machine's marker) in `--json`'s `error`.
#[test]
fn dispatch_to_an_issue_claimed_by_another_machine_refuses_with_exit_2() {
    let root = tempfile::tempdir().expect("test root");
    let run = run_dispatch(
        root.path(),
        OTHER_CLAIMED,
        &["--issue", "656", "--machine", "docs"],
    );

    assert_eq!(
        run.code,
        Some(2),
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    let value: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be exactly one JSON object ({e}): {}",
            run.stdout
        )
    });
    assert_eq!(value["outcome"].as_str(), Some("claim_refused"));
    let error = value["error"].as_str().expect("error must explain why");
    assert!(
        error.contains("4090"),
        "error must name the claiming machine, got: {error}"
    );
    assert!(
        error.contains("2026-09-02T06:30:00Z"),
        "error must render the claim timestamp, got: {error}"
    );
    assert!(
        !run.stderr.contains("codex CLI not found"),
        "the agent must not be spawned after a claim refusal, stderr: {}",
        run.stderr
    );
}

/// An unclaimed issue is dispatched normally: the guard consulted `gh`
/// (the stub ran) and let the turn through to the backend.
#[test]
fn dispatch_to_an_unclaimed_issue_proceeds_to_the_backend() {
    let root = tempfile::tempdir().expect("test root");
    let run = run_dispatch(
        root.path(),
        UNCLAIMED,
        &["--issue", "656", "--machine", "docs"],
    );

    assert!(
        run.gh_called,
        "the guard must consult gh before dispatching"
    );
    assert_ne!(run.code, Some(2), "unclaimed must not be refused");
    assert!(
        run.stderr.contains("codex CLI not found"),
        "pass-through must reach backend verification, stderr: {}",
        run.stderr
    );
}

/// Our own earlier claim is idempotent: dispatch proceeds, same as
/// unclaimed.
#[test]
fn dispatch_to_an_issue_claimed_by_this_machine_proceeds() {
    let root = tempfile::tempdir().expect("test root");
    let run = run_dispatch(
        root.path(),
        SELF_CLAIMED,
        &["--issue", "656", "--machine", "docs"],
    );

    assert_ne!(run.code, Some(2), "self-claimed must not be refused");
    assert!(
        run.stderr.contains("codex CLI not found"),
        "pass-through must reach backend verification, stderr: {}",
        run.stderr
    );
}

/// Without `--issue` the guard must not run at all: dispatch behavior is
/// byte-for-byte unchanged, and `gh` is never consulted.
#[test]
fn dispatch_without_issue_flag_skips_the_guard_entirely() {
    let root = tempfile::tempdir().expect("test root");
    let run = run_dispatch(root.path(), OTHER_CLAIMED, &[]);

    assert!(
        !run.gh_called,
        "without --issue no claim check may run (gh was invoked)"
    );
    assert_ne!(run.code, Some(2));
    assert!(
        run.stderr.contains("codex CLI not found"),
        "dispatch must reach the backend exactly as before, stderr: {}",
        run.stderr
    );
}

/// Honesty rule (GH-574 shape): an explicit `--machine` with no `--issue`
/// can never fire the guard, so it is refused instead of silently dropped.
#[test]
fn machine_without_issue_is_refused_not_silently_dropped() {
    let root = tempfile::tempdir().expect("test root");
    let run = run_dispatch(root.path(), UNCLAIMED, &["--machine", "docs"]);

    assert_ne!(run.code, Some(0), "the combination must not dispatch");
    assert!(
        run.stderr.contains("--issue"),
        "the refusal must point at the missing --issue, stderr: {}",
        run.stderr
    );
    assert!(!run.gh_called);
}
