use crate::check::CheckOutput;
use crate::plan::schema::CheckSpec;
use crate::state::machine::{CheckResult, CheckStatus, ErrorInfo, ErrorType};
use std::path::PathBuf;

/// Result of running all checks for a phase.
#[derive(Debug)]
pub struct CheckRunResult {
    pub all_passed: bool,
    pub results: Vec<CheckResult>,
    pub error: Option<ErrorInfo>,
}

/// Check engine: runs check specs against the filesystem.
pub struct CheckEngine {
    cwd: PathBuf,
}

impl CheckEngine {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    /// Run all checks in order. Short-circuit on first failure.
    pub async fn run_all(
        &self,
        checks: &[CheckSpec],
        phase_started_at: Option<&str>,
    ) -> CheckRunResult {
        let mut results = Vec::new();

        for (i, spec) in checks.iter().enumerate() {
            let output = self.run_one(spec, phase_started_at).await;
            let status = if output.passed {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed
            };

            results.push(CheckResult {
                check_type: spec.type_name().to_string(),
                status,
                detail: output.detail.clone(),
                duration_ms: output.duration.as_millis() as u64,
            });

            if !output.passed {
                // GH-529: a harness-side timeout must not be classified as a
                // retryable check failure — a retry cannot change the outcome.
                let (error_type, retryable) = if output.timed_out {
                    (ErrorType::Timeout, false)
                } else {
                    (ErrorType::CheckFailed, spec.is_retryable())
                };
                // Mark remaining as Waiting
                for check in &checks[(i + 1)..] {
                    results.push(CheckResult {
                        check_type: check.type_name().to_string(),
                        status: CheckStatus::Waiting,
                        detail: None,
                        duration_ms: 0,
                    });
                }
                return CheckRunResult {
                    all_passed: false,
                    results,
                    error: Some(ErrorInfo {
                        error_type,
                        message: output
                            .detail
                            .unwrap_or_else(|| format!("check {} failed", spec.type_name())),
                        retryable,
                        check_index: Some(i),
                        timestamp: now_rfc3339(),
                    }),
                };
            }
        }

        CheckRunResult {
            all_passed: true,
            results,
            error: None,
        }
    }

    /// Run a single check spec.
    async fn run_one(&self, spec: &CheckSpec, phase_started_at: Option<&str>) -> CheckOutput {
        match spec {
            CheckSpec::FileExists { path } => {
                crate::check::file_exists::check_file_exists(path, &self.cwd)
            }
            CheckSpec::CmdSucceeds { cmd, timeout_sec } => {
                crate::check::cmd_succeeds::check_cmd_succeeds(cmd, *timeout_sec, &self.cwd).await
            }
            CheckSpec::FileContains { path, pattern } => {
                crate::check::file_contains::check_file_contains(path, pattern, &self.cwd)
            }
            CheckSpec::GitClean { allow_untracked } => {
                crate::check::git_clean::check_git_clean(*allow_untracked, &self.cwd).await
            }
            CheckSpec::EddaEvent { event_type, after } => {
                let after_val = after.as_deref().map(|a| {
                    if a == "$phase_start" {
                        phase_started_at.unwrap_or("")
                    } else {
                        a
                    }
                });
                crate::check::edda_event::check_edda_event(event_type, after_val, &self.cwd).await
            }
            CheckSpec::WaitUntil {
                check,
                interval_sec,
                timeout_sec,
                backoff,
            } => {
                crate::check::wait_until::check_wait_until(
                    check,
                    *interval_sec,
                    *timeout_sec,
                    *backoff,
                    &self.cwd,
                    phase_started_at,
                )
                .await
            }
        }
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::CheckSpec;

    #[tokio::test]
    async fn empty_checks_pass() {
        let dir = tempfile::tempdir().unwrap();
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let result = engine.run_all(&[], None).await;
        assert!(result.all_passed);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn file_exists_check_passes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let engine = CheckEngine::new(dir.path().to_path_buf());
        let checks = vec![CheckSpec::FileExists {
            path: "test.txt".into(),
        }];
        let result = engine.run_all(&checks, None).await;
        assert!(result.all_passed);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].status, CheckStatus::Passed);
    }

    #[tokio::test]
    async fn file_exists_check_fails() {
        let dir = tempfile::tempdir().unwrap();
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let checks = vec![CheckSpec::FileExists {
            path: "nonexistent.txt".into(),
        }];
        let result = engine.run_all(&checks, None).await;
        assert!(!result.all_passed);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn short_circuit_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let engine = CheckEngine::new(dir.path().to_path_buf());

        let checks = vec![
            CheckSpec::FileExists {
                path: "nonexistent.txt".into(),
            },
            CheckSpec::FileExists {
                path: "also-missing.txt".into(),
            },
        ];
        let result = engine.run_all(&checks, None).await;
        assert!(!result.all_passed);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].status, CheckStatus::Failed);
        assert_eq!(result.results[1].status, CheckStatus::Waiting); // short-circuited
    }

    /// GH-529: a timed-out cmd_succeeds must classify as `ErrorType::Timeout`
    /// with `retryable: false`, distinct from a genuine check failure.
    #[tokio::test]
    async fn cmd_timeout_classifies_as_timeout_not_check_failed() {
        let dir = tempfile::tempdir().unwrap();
        let engine = CheckEngine::new(dir.path().to_path_buf());

        // A command that never finishes inside its 1s budget.
        #[cfg(windows)]
        let cmd = "while ($true) { Start-Sleep -Milliseconds 100 }";
        #[cfg(not(windows))]
        let cmd = "sleep 30";
        let checks = vec![CheckSpec::CmdSucceeds {
            cmd: cmd.into(),
            timeout_sec: 1,
        }];

        let result = engine.run_all(&checks, None).await;
        assert!(!result.all_passed);
        let err = result.error.expect("timeout must carry an error");
        assert_eq!(err.error_type, ErrorType::Timeout);
        assert!(
            !err.retryable,
            "retrying a timeout cannot change the outcome"
        );
        assert!(err.message.contains("timed out"), "got: {}", err.message);
        assert_eq!(result.results[0].status, CheckStatus::Failed);
    }

    /// GH-529: a genuine command failure (non-zero exit) must STILL classify
    /// as `ErrorType::CheckFailed` and stay retryable.
    #[tokio::test]
    async fn cmd_failure_stays_retryable_check_failed() {
        let dir = tempfile::tempdir().unwrap();
        let engine = CheckEngine::new(dir.path().to_path_buf());

        #[cfg(windows)]
        let cmd = "exit 1";
        #[cfg(not(windows))]
        let cmd = "false";
        let checks = vec![CheckSpec::CmdSucceeds {
            cmd: cmd.into(),
            timeout_sec: 10,
        }];

        let result = engine.run_all(&checks, None).await;
        assert!(!result.all_passed);
        let err = result.error.expect("failure must carry an error");
        assert_eq!(err.error_type, ErrorType::CheckFailed);
        assert!(err.retryable);
    }
}
