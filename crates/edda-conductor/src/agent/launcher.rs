use crate::agent::stream::{classify_result, StreamMonitor};
use crate::plan::schema::Phase;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Result of running an agent for a phase.
#[derive(Debug, Clone)]
pub enum PhaseResult {
    AgentDone {
        cost_usd: Option<f64>,
        /// The agent's final summary text (from stream-json result message).
        result_text: Option<String>,
    },
    AgentCrash {
        error: String,
    },
    Timeout,
    MaxTurns {
        cost_usd: Option<f64>,
    },
    BudgetExceeded {
        cost_usd: Option<f64>,
    },
}

/// Trait for launching AI agents. Implemented by MockLauncher (tests)
/// and ClaudeCodeLauncher (real, Wave 4).
#[async_trait::async_trait]
pub trait AgentLauncher: Send + Sync {
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult>;
}

/// Fixed namespace UUID for conductor sessions.
const CONDUCTOR_NS: Uuid = Uuid::from_bytes([
    0xed, 0xda, 0xc0, 0x5d, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);

/// Deterministic session ID per plan+phase+attempt.
/// Each attempt gets a unique session to avoid "session already in use" errors.
pub fn phase_session_id(plan_name: &str, phase_id: &str) -> Uuid {
    phase_session_id_attempt(plan_name, phase_id, 1)
}

/// Deterministic session ID with attempt number.
pub fn phase_session_id_attempt(plan_name: &str, phase_id: &str, attempt: u32) -> Uuid {
    Uuid::new_v5(
        &CONDUCTOR_NS,
        format!("{plan_name}-{phase_id}-{attempt}").as_bytes(),
    )
}

/// Launches real Claude Code processes via `claude -p`.
pub struct ClaudeCodeLauncher {
    pub claude_bin: PathBuf,
    pub verbose: bool,
    /// If set, raw agent stdout is captured to `{transcript_dir}/{phase_id}-{session_id_prefix}.jsonl`.
    pub transcript_dir: Option<PathBuf>,
}

impl Default for ClaudeCodeLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeLauncher {
    pub fn new() -> Self {
        Self {
            claude_bin: PathBuf::from("claude"),
            verbose: false,
            transcript_dir: None,
        }
    }

    pub fn with_bin(claude_bin: PathBuf) -> Self {
        Self {
            claude_bin,
            verbose: false,
            transcript_dir: None,
        }
    }

    /// Enable verbose mode: print live agent activity (tool calls, file writes, etc.)
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Check that the Claude CLI binary is reachable.
    pub fn verify_available(&self) -> Result<()> {
        let status = std::process::Command::new(&self.claude_bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => anyhow::bail!(
                "Claude CLI not found (looked for {:?}).\n\
                 Install: npm install -g @anthropic-ai/claude-code",
                self.claude_bin
            ),
        }
    }
}

#[async_trait::async_trait]
impl AgentLauncher for ClaudeCodeLauncher {
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
        let mut cmd = tokio::process::Command::new(&self.claude_bin);
        cmd.arg("-p")
            .arg(prompt)
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--session-id")
            .arg(session_id)
            .arg("--permission-mode")
            .arg(&phase.permission_mode)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Allow nesting — remove markers that prevent Claude Code from spawning
            .env_remove("CLAUDE_CODE")
            .env_remove("CLAUDECODE")
            // Tell edda hooks to use conductor-optimized injection
            .env("EDDA_CONDUCTOR_MODE", "1")
            // Propagate session_id so agent-spawned `edda decide` etc. can resolve identity
            .env("EDDA_SESSION_ID", session_id);

        // Optional: per-phase budget
        if let Some(budget) = phase.budget_usd {
            cmd.arg("--max-budget-usd").arg(budget.to_string());
        }

        // Optional: plan context as system prompt
        if !plan_context.is_empty() {
            cmd.arg("--append-system-prompt").arg(plan_context);
        }

        // Optional: allowed tools
        if let Some(tools) = &phase.allowed_tools {
            cmd.arg("--allowedTools").arg(tools.join(","));
        }

        // Merge plan-level + phase-level env
        for (k, v) in &phase.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to capture stdout"))?;

        let tee_path = self.transcript_dir.as_ref().map(|dir| {
            let sid_prefix = &session_id[..session_id.len().min(8)];
            dir.join(format!("{}-{sid_prefix}.jsonl", phase.id))
        });
        let mut monitor = StreamMonitor::new(stdout)
            .with_verbose(self.verbose)
            .with_tee(tee_path);
        let timeout_sec = phase.timeout_sec.unwrap_or(1800);

        tokio::select! {
            result = monitor.run() => {
                let monitor_result = result?;
                let exit = child.wait().await?;
                Ok(classify_result(&monitor_result, exit.code()))
            }
            _ = tokio::time::sleep(Duration::from_secs(timeout_sec)) => {
                child.kill().await.ok();
                Ok(PhaseResult::Timeout)
            }
            _ = cancel.cancelled() => {
                child.kill().await.ok();
                Ok(PhaseResult::AgentCrash { error: "conductor shutdown".into() })
            }
        }
    }
}

/// Mock launcher for testing. Pops results on each call per phase ID.
/// If no results configured (or exhausted), returns AgentDone.
pub struct MockLauncher {
    results: std::sync::Mutex<std::collections::HashMap<String, Vec<PhaseResult>>>,
}

impl Default for MockLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl MockLauncher {
    pub fn new() -> Self {
        Self {
            results: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn set_results(&self, phase_id: &str, results: Vec<PhaseResult>) {
        self.results
            .lock()
            .unwrap()
            .insert(phase_id.to_string(), results);
    }
}

#[async_trait::async_trait]
impl AgentLauncher for MockLauncher {
    async fn run_phase(
        &self,
        phase: &Phase,
        _prompt: &str,
        _plan_context: &str,
        _session_id: &str,
        _cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
        if cancel.is_cancelled() {
            return Ok(PhaseResult::AgentCrash {
                error: "cancelled".into(),
            });
        }

        let mut map = self.results.lock().unwrap();
        if let Some(vec) = map.get_mut(&phase.id) {
            if !vec.is_empty() {
                return Ok(vec.remove(0));
            }
        }
        Ok(PhaseResult::AgentDone {
            cost_usd: Some(0.10),
            result_text: Some("(mock) phase completed".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;

    #[test]
    fn session_id_deterministic() {
        let id1 = phase_session_id("my-plan", "build");
        let id2 = phase_session_id("my-plan", "build");
        assert_eq!(id1, id2);
    }

    #[test]
    fn session_id_differs_per_phase() {
        let id1 = phase_session_id("plan", "a");
        let id2 = phase_session_id("plan", "b");
        assert_ne!(id1, id2);
    }

    #[test]
    fn session_id_is_valid_uuid() {
        let id = phase_session_id("test", "phase");
        // UUID v5 has version nibble = 5
        assert_eq!(id.get_version_num(), 5);
    }

    #[tokio::test]
    async fn mock_returns_default() {
        let launcher = MockLauncher::new();
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let cancel = CancellationToken::new();
        let result = launcher
            .run_phase(&plan.phases[0], "prompt", "", "sid", Path::new("."), cancel)
            .await
            .unwrap();
        assert!(matches!(result, PhaseResult::AgentDone { .. }));
    }

    #[tokio::test]
    async fn mock_returns_configured() {
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentCrash {
                error: "boom".into(),
            }],
        );
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let cancel = CancellationToken::new();
        let result = launcher
            .run_phase(&plan.phases[0], "prompt", "", "sid", Path::new("."), cancel)
            .await
            .unwrap();
        assert!(matches!(result, PhaseResult::AgentCrash { .. }));
    }

    #[tokio::test]
    async fn mock_pops_sequential_results() {
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentCrash {
                    error: "first".into(),
                },
                PhaseResult::AgentDone {
                    cost_usd: Some(1.0),
                    result_text: None,
                },
            ],
        );
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let cancel = CancellationToken::new();

        let r1 = launcher
            .run_phase(&plan.phases[0], "", "", "s", Path::new("."), cancel.clone())
            .await
            .unwrap();
        assert!(matches!(r1, PhaseResult::AgentCrash { .. }));

        let r2 = launcher
            .run_phase(&plan.phases[0], "", "", "s", Path::new("."), cancel)
            .await
            .unwrap();
        assert!(
            matches!(r2, PhaseResult::AgentDone { cost_usd: Some(c), .. } if (c - 1.0).abs() < 0.01)
        );
    }

    #[tokio::test]
    async fn mock_respects_cancel() {
        let launcher = MockLauncher::new();
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = launcher
            .run_phase(&plan.phases[0], "", "", "s", Path::new("."), cancel)
            .await
            .unwrap();
        assert!(matches!(result, PhaseResult::AgentCrash { .. }));
    }
}
