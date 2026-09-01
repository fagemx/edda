use crate::check::{mask_secrets, output_tail, CheckOutput};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;

/// Per-stream capture cap (GH-540): keep the TAIL, where fatal diagnostics
/// live (LNK1104's payload, `error: test failed`).
const OUTPUT_TAIL_CHARS: usize = 2000;

/// Output substrings that identify a machine-layer build failure (GH-540).
/// LNK1104's payload — the file the linker could not open — is the single
/// token distinguishing a held .exe from a concurrent cargo or an antivirus
/// handle, so the capture must reach the tail of the stream that names it.
/// (Content classification, unlike the GH-529 timeout: a linker fault has no
/// constructor-level signal, the output text is the only evidence.)
const ENVIRONMENTAL_PATTERNS: &[&str] = &["LNK1104"];

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
}
