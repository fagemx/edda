use crate::check::CheckOutput;
use crate::plan::schema::{BackoffStrategy, CheckSpec};
use std::path::Path;
use std::time::{Duration, Instant};

/// Polling check: repeatedly run inner check until it passes or timeout.
pub async fn check_wait_until(
    inner: &CheckSpec,
    interval_sec: u64,
    timeout_sec: u64,
    backoff: BackoffStrategy,
    cwd: &Path,
    phase_started_at: Option<&str>,
) -> CheckOutput {
    let start = Instant::now();
    let deadline = start + Duration::from_secs(timeout_sec);
    let mut attempt = 0u32;

    loop {
        attempt += 1;
        let result = run_inner(inner, cwd, phase_started_at).await;
        if result.passed {
            return CheckOutput::passed_with_detail(
                format!("passed after {attempt} attempts"),
                start.elapsed(),
            );
        }

        if Instant::now() >= deadline {
            return CheckOutput::failed(
                format!(
                    "timed out after {}s ({attempt} attempts). Last: {}",
                    timeout_sec,
                    result.detail.as_deref().unwrap_or("(no detail)")
                ),
                start.elapsed(),
            );
        }

        let delay = compute_backoff(interval_sec, attempt, backoff);
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(delay.min(remaining)).await;
    }
}

fn compute_backoff(base_sec: u64, attempt: u32, strategy: BackoffStrategy) -> Duration {
    let secs = match strategy {
        BackoffStrategy::None => base_sec,
        BackoffStrategy::Linear => base_sec * attempt as u64,
        BackoffStrategy::Exponential => base_sec * 2u64.saturating_pow(attempt.saturating_sub(1)),
    };
    // Cap at 5 minutes
    Duration::from_secs(secs.min(300))
}

async fn run_inner(spec: &CheckSpec, cwd: &Path, phase_started_at: Option<&str>) -> CheckOutput {
    match spec {
        CheckSpec::FileExists { path } => crate::check::file_exists::check_file_exists(path, cwd),
        CheckSpec::CmdSucceeds { cmd, timeout_sec } => {
            crate::check::cmd_succeeds::check_cmd_succeeds(cmd, *timeout_sec, cwd).await
        }
        CheckSpec::FileContains { path, pattern } => {
            crate::check::file_contains::check_file_contains(path, pattern, cwd)
        }
        CheckSpec::GitClean { allow_untracked } => {
            crate::check::git_clean::check_git_clean(*allow_untracked, cwd).await
        }
        CheckSpec::EddaEvent { event_type, after } => {
            let after_val = after.as_deref().map(|a| {
                if a == "$phase_start" {
                    phase_started_at.unwrap_or("")
                } else {
                    a
                }
            });
            crate::check::edda_event::check_edda_event(event_type, after_val, cwd).await
        }
        CheckSpec::WaitUntil { .. } => {
            // Nested wait_until is rejected at parse time, but handle gracefully
            CheckOutput::failed("nested wait_until is not supported".into(), Duration::ZERO)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_none() {
        assert_eq!(
            compute_backoff(5, 1, BackoffStrategy::None),
            Duration::from_secs(5)
        );
        assert_eq!(
            compute_backoff(5, 3, BackoffStrategy::None),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn backoff_linear() {
        assert_eq!(
            compute_backoff(5, 1, BackoffStrategy::Linear),
            Duration::from_secs(5)
        );
        assert_eq!(
            compute_backoff(5, 2, BackoffStrategy::Linear),
            Duration::from_secs(10)
        );
        assert_eq!(
            compute_backoff(5, 3, BackoffStrategy::Linear),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn backoff_exponential() {
        assert_eq!(
            compute_backoff(5, 1, BackoffStrategy::Exponential),
            Duration::from_secs(5)
        );
        assert_eq!(
            compute_backoff(5, 2, BackoffStrategy::Exponential),
            Duration::from_secs(10)
        );
        assert_eq!(
            compute_backoff(5, 3, BackoffStrategy::Exponential),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn backoff_capped() {
        // Should cap at 300s
        assert_eq!(
            compute_backoff(100, 10, BackoffStrategy::Exponential),
            Duration::from_secs(300)
        );
    }

    #[tokio::test]
    async fn wait_until_immediate_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ready.txt"), "ok").unwrap();

        let out = check_wait_until(
            &CheckSpec::FileExists {
                path: "ready.txt".into(),
            },
            1,
            5,
            BackoffStrategy::None,
            dir.path(),
            None,
        )
        .await;

        assert!(out.passed);
        assert!(out.detail.unwrap().contains("1 attempts"));
    }

    #[tokio::test]
    async fn wait_until_delayed_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delayed.txt");

        // Spawn a task that creates the file after 500ms
        let p = path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            std::fs::write(p, "ok").unwrap();
        });

        let out = check_wait_until(
            &CheckSpec::FileExists {
                path: "delayed.txt".into(),
            },
            1, // 1s interval (but will poll quickly enough)
            5, // 5s timeout
            BackoffStrategy::None,
            dir.path(),
            None,
        )
        .await;

        assert!(out.passed);
    }

    #[tokio::test]
    async fn wait_until_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let out = check_wait_until(
            &CheckSpec::FileExists {
                path: "never.txt".into(),
            },
            1, // 1s interval
            2, // 2s timeout
            BackoffStrategy::None,
            dir.path(),
            None,
        )
        .await;

        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("timed out"));
    }
}
