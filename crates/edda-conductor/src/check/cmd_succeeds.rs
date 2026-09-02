#[cfg(test)]
use crate::check::engine::CheckEngine;
use crate::check::{mask_secrets, output_tail, CheckOutput};
#[cfg(test)]
use crate::plan::schema::CheckSpec;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;

/// Per-stream capture cap (GH-540): keep the TAIL, where fatal diagnostics
/// live (LNK1104's payload, `error: test failed`).
const OUTPUT_TAIL_CHARS: usize = 2000;

/// Output substrings that identify a machine-layer build failure (GH-540).
/// The full linker-fatal signature — not the bare code — is required (review
/// round 1): a bare `LNK1104` token also shows up in agent output that merely
/// DISCUSSES the error (e.g. a cargo test panicking on an assertion about the
/// LNK1104 message), and classifying that environmental would hand a product
/// failure free retries. link.exe's fatal line is
/// `LINK : fatal error LNK1104: cannot open file '...'`, so the
/// `fatal error LNK1104` pair is the signature. The capture must still reach
/// the tail of the stream that names it. (Content classification, unlike the
/// GH-529 timeout: a linker fault has no constructor-level signal, the output
/// text is the only evidence.)
const ENVIRONMENTAL_PATTERNS: &[&str] = &["fatal error LNK1104"];

fn is_environmental(stderr: &str, stdout: &str) -> bool {
    ENVIRONMENTAL_PATTERNS
        .iter()
        .any(|p| stderr.contains(p) || stdout.contains(p))
}

/// Shell program and args for the current platform.
#[cfg(windows)]
fn shell_cmd(cmd: &str) -> (String, Vec<String>) {
    // Prefer PowerShell over cmd.exe for better Unix-ism support
    static SHELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let shell = SHELL.get_or_init(|| {
        if which_exists("pwsh") {
            "pwsh".into()
        } else if which_exists("powershell") {
            "powershell".into()
        } else {
            "cmd.exe".into()
        }
    });

    if shell == "cmd.exe" {
        (shell.clone(), vec!["/C".into(), cmd.into()])
    } else {
        (
            shell.clone(),
            vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
        )
    }
}

#[cfg(not(windows))]
fn shell_cmd(cmd: &str) -> (String, Vec<String>) {
    ("sh".into(), vec!["-c".into(), cmd.into()])
}

#[cfg(windows)]
fn which_exists(name: &str) -> bool {
    std::process::Command::new("where")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn check_cmd_succeeds(cmd: &str, timeout_sec: u64, cwd: &Path) -> CheckOutput {
    let start = Instant::now();
    let (shell, args) = shell_cmd(cmd);

    let result = Command::new(&shell)
        .args(&args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();

    match tokio::time::timeout(Duration::from_secs(timeout_sec), result).await {
        Ok(Ok(output)) if output.status.success() => CheckOutput::passed(start.elapsed()),
        Ok(Ok(output)) => {
            // GH-540: keep the tail of BOTH streams. Diagnostic conventions
            // split across them — `cargo test` ends stderr at "error: test
            // failed" while the failing test names land on stdout, and
            // link.exe puts "cannot open file '...'" at the very end of
            // stderr — so a tail of one stream reproduces the same useless
            // message the issue was filed with.
            let stderr = mask_secrets(&String::from_utf8_lossy(&output.stderr));
            let stdout = mask_secrets(&String::from_utf8_lossy(&output.stdout));
            let stderr_tail = output_tail(&stderr, OUTPUT_TAIL_CHARS);
            let stdout_tail = output_tail(&stdout, OUTPUT_TAIL_CHARS);
            let mut message = format!("exit {}", output.status.code().unwrap_or(-1));
            if !stderr_tail.trim().is_empty() {
                message.push_str(": ");
                message.push_str(stderr_tail.trim_end());
            }
            if !stdout_tail.trim().is_empty() {
                message.push_str("\n--- stdout (tail) ---\n");
                message.push_str(stdout_tail.trim_end());
            }
            let detail = message.trim().to_string();
            if is_environmental(stderr_tail, stdout_tail) {
                CheckOutput::failed_environmental(detail, start.elapsed())
            } else {
                CheckOutput::failed(detail, start.elapsed())
            }
        }
        Ok(Err(e)) => CheckOutput::failed(format!("spawn error: {e}"), start.elapsed()),
        Err(_) => CheckOutput::timed_out(
            format!("command timed out after {timeout_sec}s: {cmd}"),
            start.elapsed(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let out = check_cmd_succeeds("echo ok", 10, dir.path()).await;
        assert!(out.passed);
    }

    #[tokio::test]
    async fn false_fails() {
        let dir = tempfile::tempdir().unwrap();
        // Use a command that always fails
        #[cfg(not(windows))]
        let cmd = "false";
        #[cfg(windows)]
        let cmd = "exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("exit"));
    }

    #[tokio::test]
    async fn timeout_kills() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "sleep 60";
        #[cfg(windows)]
        // Keep the delay inside the selected shell so no child retains captured pipes.
        let cmd = if which_exists("pwsh") || which_exists("powershell") {
            "while ($true) { Start-Sleep -Milliseconds 100 }"
        } else {
            "for /L %i in (1,1,2147483647) do @rem"
        };
        let out = check_cmd_succeeds(cmd, 1, dir.path()).await;
        assert!(!out.passed);
        assert!(
            out.timed_out,
            "a killed command must be marked as a timeout (GH-529)"
        );
        assert!(out.detail.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn secrets_masked_in_output() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "echo 'key=sk-ant1234567890abcdefghij' >&2 && exit 1";
        #[cfg(windows)]
        let cmd = "echo key=sk-ant1234567890abcdefghij 1>&2 && exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        let detail = out.detail.unwrap();
        assert!(
            !detail.contains("sk-ant"),
            "secret should be masked: {detail}"
        );
    }

    /// GH-540: a failing command must keep the TAIL of BOTH streams.
    /// `cargo test` ends stderr at "error: test failed" while the failing
    /// test names land on stdout — the exact message this issue was filed
    /// with. Head-truncation of stderr alone reproduces it verbatim.
    #[tokio::test]
    async fn failure_keeps_tail_of_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        // >2000 chars of filler on stderr pushes the fatal line past any
        // head-truncation cut; the failure names go to stdout only.
        #[cfg(not(windows))]
        let cmd = concat!(
            "yes x | head -c 4000 1>&2 ; ",
            "echo 'error: test failed, to rerun pass `-p edda-conductor`' 1>&2 ; ",
            "echo 'failures:' ; ",
            "echo '    gh540::the_failing_test' ; ",
            "exit 1"
        );
        #[cfg(windows)]
        let cmd = concat!(
            "[Console]::Error.WriteLine(('x' * 4000)) ; ",
            "[Console]::Error.WriteLine('error: test failed, to rerun pass') ; ",
            "Write-Output 'failures:' ; ",
            "Write-Output '    gh540::the_failing_test' ; ",
            "exit 1"
        );
        let out = check_cmd_succeeds(cmd, 30, dir.path()).await;
        assert!(!out.passed);
        let detail = out.detail.unwrap();
        // doneWhen 1: the tail of stderr names the fatal line ...
        assert!(
            detail.contains("error: test failed"),
            "stderr tail must be kept, got: {detail}"
        );
        // ... and the stdout failure names are not dropped ...
        assert!(
            detail.contains("gh540::the_failing_test"),
            "stdout tail must be kept, got: {detail}"
        );
        // ... and the capture stays bounded.
        assert!(
            detail.chars().count() < 4600,
            "capture must stay bounded, got {} chars",
            detail.chars().count()
        );
    }

    /// GH-540: output_tail must be char-boundary safe (the old byte-slice
    /// head-truncation could panic on multibyte UTF-8).
    #[test]
    fn output_tail_is_char_boundary_safe() {
        let multibyte = "é中\u{1F600}".repeat(1000); // 3000 chars, 9000 bytes
        let tail = output_tail(&multibyte, 2000);
        assert_eq!(tail.chars().count(), 2000);
        // Panic-free slicing implies boundaries were respected.
        assert_eq!(output_tail("short", 2000), "short");
    }

    /// GH-540: output naming LNK1104 classifies the failure as environmental.
    #[tokio::test]
    async fn lnk1104_output_marks_environmental() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "echo \"LINK : fatal error LNK1104: cannot open file 'x.exe'\" 1>&2 ; exit 1";
        #[cfg(windows)]
        let cmd = "[Console]::Error.WriteLine('LINK : fatal error LNK1104: cannot open file ''x.exe''') ; exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        assert!(
            out.environmental,
            "LNK1104 must mark the failure environmental"
        );
        assert!(out.detail.unwrap().contains("LNK1104"));
    }

    /// A genuine agent-work failure (no environmental pattern) stays a plain
    /// failure, even when the output mentions exit codes and files.
    #[tokio::test]
    async fn genuine_failure_not_marked_environmental() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "echo 'test failed: assertion in foo.rs' 1>&2 ; exit 1";
        #[cfg(windows)]
        let cmd = "[Console]::Error.WriteLine('test failed: assertion in foo.rs') ; exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        assert!(!out.environmental);
    }

    /// GH-540 review round 1: a BARE `LNK1104` token in a non-linker context
    /// — e.g. a cargo test panicking on an assertion that discusses the
    /// LNK1104 message — must stay a plain product failure. Only the actual
    /// linker-fatal signature (`fatal error LNK1104`) classifies
    /// environmental; matching the bare code misclassified exactly this
    /// shape as a machine fault and granted it free retries.
    #[tokio::test]
    async fn bare_lnk1104_token_is_not_environmental() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "echo 'panicked: assertion failed: detail.contains(\"LNK1104\")' 1>&2 ; exit 1";
        #[cfg(windows)]
        let cmd =
            "[Console]::Error.WriteLine('panicked: assertion failed: detail.contains(''LNK1104'')') ; exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        let detail = out.detail.unwrap();
        assert!(
            detail.contains("LNK1104"),
            "token must reach the tail: {detail}"
        );
        assert!(
            !out.environmental,
            "a bare LNK1104 mention is not a linker fault"
        );
    }

    /// GH-558: a non-passing check result must record the exact shell line
    /// the harness executed, so a harness-side invocation difference is
    /// visible in the captured output (doneWhen 1). Before the fix the
    /// failure detail carried only the child's output — when cargo-fmt
    /// prints its usage block, there is no way to tell what argv actually
    /// reached it.
    #[tokio::test]
    async fn failure_detail_records_executed_shell_line() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let cmd = "exit 1";
        #[cfg(windows)]
        let cmd = "exit 1";
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        let detail = out.detail.unwrap();
        assert!(
            detail.contains("executed:"),
            "failure detail must record the executed shell line, got: {detail}"
        );
        assert!(
            detail.contains(cmd),
            "executed line must contain the literal command, got: {detail}"
        );
    }

    /// GH-558: the observed failure was cargo-fmt printing its help/usage
    /// block — which it does for ANY internal io failure, with the actual
    /// REASON line printed BEFORE the block. A tail-only capture keeps the
    /// block and drops the reason, turning a diagnosable failure into the
    /// mystery this issue was filed as. The capture must keep the HEAD of a
    /// stream too, not only the tail.
    #[tokio::test]
    async fn failure_keeps_head_of_stream_when_output_exceeds_tail() {
        let dir = tempfile::tempdir().unwrap();
        // Diagnostic at the START of stderr, then >2000 chars of filler: a
        // tail-only capture keeps only filler.
        #[cfg(not(windows))]
        let cmd = concat!(
            "echo 'fatal: the real reason is at the head' 1>&2 ; ",
            "yes x | head -c 4000 1>&2 ; ",
            "exit 1"
        );
        #[cfg(windows)]
        let cmd = concat!(
            "[Console]::Error.WriteLine('fatal: the real reason is at the head') ; ",
            "[Console]::Error.WriteLine(('x' * 4000)) ; ",
            "exit 1"
        );
        let out = check_cmd_succeeds(cmd, 10, dir.path()).await;
        assert!(!out.passed);
        let detail = out.detail.unwrap();
        assert!(
            detail.contains("the real reason is at the head"),
            "head of the stream must be kept, got: {detail}"
        );
        assert!(
            detail.chars().count() < 5200,
            "capture must stay bounded, got {} chars",
            detail.chars().count()
        );
    }

    /// GH-558 doneWhen 3: a multi-flag command must pass through the check
    /// runner from a REAL git worktree cwd (`.git` is a file, not a
    /// directory) — the cwd shape reported in the issue. Witness test: the
    /// harness passes the literal spec string regardless of cwd, verified to
    /// run green here both before and after the fix (the bug itself was not
    /// reproducible — see the PR for the full reproduction record).
    #[tokio::test]
    async fn multi_flag_cmd_succeeds_from_real_worktree_cwd() {
        let tmp = tempfile::tempdir().unwrap();

        // A tiny standalone cargo project, independent of this workspace.
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"gh558diag\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();

        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&proj)
                .output()
                .expect("git must be available");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        git(&["add", "-A"]);
        git(&["commit", "-m", "init"]);

        // The worktree: cwd shape whose .git is a FILE.
        let wt = tmp.path().join("proj-wt");
        git(&[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "-b",
            "gh558-witness",
        ]);
        assert!(
            wt.join(".git").is_file(),
            "worktree .git must be a file — that is the cwd shape from GH-558"
        );

        let engine = CheckEngine::new(wt);
        let checks = vec![CheckSpec::CmdSucceeds {
            cmd: "cargo fmt --all --check".into(),
            timeout_sec: 300,
        }];
        let result = engine.run_all(&checks, None).await;
        assert!(
            result.all_passed,
            "multi-flag command must pass from a worktree cwd: {:?}",
            result
                .results
                .iter()
                .map(|r| (&r.status, &r.detail))
                .collect::<Vec<_>>()
        );
    }
}
