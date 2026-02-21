use crate::check::{mask_secrets, CheckOutput};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;

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
            let stderr = String::from_utf8_lossy(&output.stderr);
            let masked = mask_secrets(&stderr);
            let truncated = if masked.len() > 2000 {
                format!("{}...", &masked[..2000])
            } else {
                masked.to_string()
            };
            CheckOutput::failed(
                format!(
                    "exit {}: {}",
                    output.status.code().unwrap_or(-1),
                    truncated.trim()
                ),
                start.elapsed(),
            )
        }
        Ok(Err(e)) => CheckOutput::failed(format!("spawn error: {e}"), start.elapsed()),
        Err(_) => CheckOutput::failed(
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
        let cmd = "ping -n 60 127.0.0.1";
        let out = check_cmd_succeeds(cmd, 1, dir.path()).await;
        assert!(!out.passed);
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
}
