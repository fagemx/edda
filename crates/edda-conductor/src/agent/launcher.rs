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

    /// The model the backend reported **in-band** during the most recent
    /// turn on this launcher, if any. Observation, not inference: a
    /// launcher reports only what the backend itself carried in its
    /// protocol stream (claude stream-json `system/init`, pi RPC
    /// `get_state`); `None` means the backend reported nothing and callers
    /// must render `"unknown"`, never a guess from config or session files
    /// (GH-574 honesty rule).
    fn last_observed_model(&self) -> Option<String> {
        None
    }
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
    /// In-band model report from the most recent turn (stream-json
    /// `system/init`), for [`AgentLauncher::last_observed_model`].
    observed_model: std::sync::Mutex<Option<String>>,
}

impl Default for ClaudeCodeLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeLauncher {
    /// Assemble the `claude -p` command line for one phase. Split out from
    /// [`AgentLauncher::run_phase`] so tests can assert the exact spawn
    /// arguments without launching a real backend (GH-574).
    fn build_command(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
    ) -> tokio::process::Command {
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

        // Optional: model selection, carried verbatim (GH-574)
        if let Some(model) = &phase.model {
            cmd.arg("--model").arg(model);
        }

        // Optional: tool allowlist / denylist. The allowlist must be the
        // capability-restricting flag: claude's `--tools` sets "the list of
        // available tools from the built-in set", while `--allowedTools` is
        // only a permission-prompt rule and leaves Write/Edit/Bash reachable
        // under bypassPermissions — the fake allowlist GH-574 round 2
        // (P1-1) called out. `--disallowedTools` is a genuine deny rule.
        if let Some(tools) = &phase.tools {
            cmd.arg("--tools").arg(tools.join(","));
        }
        if let Some(tools) = &phase.exclude_tools {
            cmd.arg("--disallowedTools").arg(tools.join(","));
        }

        // Merge plan-level + phase-level env
        for (k, v) in &phase.env {
            cmd.env(k, v);
        }
        cmd
    }

    pub fn new() -> Self {
        Self {
            claude_bin: PathBuf::from("claude"),
            verbose: false,
            transcript_dir: None,
            observed_model: std::sync::Mutex::new(None),
        }
    }

    pub fn with_bin(claude_bin: PathBuf) -> Self {
        Self {
            claude_bin,
            verbose: false,
            transcript_dir: None,
            observed_model: std::sync::Mutex::new(None),
        }
    }

    fn record_observed_model(&self, model: Option<String>) {
        if let Ok(mut slot) = self.observed_model.lock() {
            *slot = model;
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
        if phase.thinking.is_some() {
            // GH-574 honesty: claude's CLI exposes no thinking-level flag,
            // so a phase that declares one would otherwise be silently
            // ignored — exactly the failure mode this flag exists to end.
            anyhow::bail!(
                "claude does not support a thinking-level flag; remove `thinking: {}` from phase {:?} or dispatch with --agent pi",
                phase.thinking.as_deref().unwrap_or(""),
                phase.id
            );
        }
        let mut cmd = self.build_command(phase, prompt, plan_context, session_id, cwd);

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

        let result = tokio::select! {
            result = monitor.run() => {
                let monitor_result = result?;
                // In-band model observation: whatever the backend itself
                // reported, or nothing. Never inferred from config.
                self.record_observed_model(monitor_result.model.clone());
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
        };
        result
    }

    fn last_observed_model(&self) -> Option<String> {
        self.observed_model
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

/// One recorded launcher call (for tests asserting session continuity etc.).
#[derive(Debug, Clone)]
pub struct LauncherCall {
    pub phase_id: String,
    pub session_id: String,
    pub prompt: String,
}

/// Mock launcher for testing. Pops results on each call per phase ID.
/// If no results configured (or exhausted), returns AgentDone.
/// Records every call so tests can assert session ids and prompts.
pub struct MockLauncher {
    results: std::sync::Mutex<std::collections::HashMap<String, Vec<PhaseResult>>>,
    calls: std::sync::Mutex<Vec<LauncherCall>>,
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
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn set_results(&self, phase_id: &str, results: Vec<PhaseResult>) {
        self.results
            .lock()
            .unwrap()
            .insert(phase_id.to_string(), results);
    }

    /// Every recorded call, in launch order.
    pub fn calls(&self) -> Vec<LauncherCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Recorded calls for one phase, in launch order.
    pub fn calls_for(&self, phase_id: &str) -> Vec<LauncherCall> {
        self.calls()
            .into_iter()
            .filter(|c| c.phase_id == phase_id)
            .collect()
    }

    /// How many times this phase launched an agent turn.
    pub fn call_count(&self, phase_id: &str) -> u32 {
        self.calls_for(phase_id).len() as u32
    }
}

#[async_trait::async_trait]
impl AgentLauncher for MockLauncher {
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        _plan_context: &str,
        session_id: &str,
        _cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
        if cancel.is_cancelled() {
            return Ok(PhaseResult::AgentCrash {
                error: "cancelled".into(),
            });
        }

        self.calls.lock().unwrap().push(LauncherCall {
            phase_id: phase.id.clone(),
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
        });

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

    // ── GH-574: phase-declared launcher capabilities reach the spawn line ──

    fn args_of(cmd: &tokio::process::Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn claude_command_for(yaml: &str) -> tokio::process::Command {
        let launcher = ClaudeCodeLauncher::with_bin(PathBuf::from("claude"));
        let phase = phase_from_yaml(yaml);
        launcher.build_command(&phase, "do the task", "", "sess-1", Path::new("."))
    }

    fn phase_from_yaml(yaml: &str) -> Phase {
        parse_plan(&format!("name: t\nphases:\n{yaml}"))
            .expect("test plan parses")
            .phases
            .remove(0)
    }

    #[test]
    fn claude_phase_model_reaches_the_spawn_line() {
        let args = args_of(&claude_command_for(
            "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n",
        ));
        let model_pos = args
            .iter()
            .position(|a| a == "--model")
            .expect("--model must appear in the claude spawn command line");
        assert_eq!(args[model_pos + 1], "anthropic/claude-opus-5");
    }

    #[test]
    fn claude_without_model_spawns_no_model_flag() {
        let args = args_of(&claude_command_for("  - id: a\n    prompt: x\n"));
        assert!(
            !args.contains(&"--model".to_string()),
            "no phase model must mean no --model flag: {args:?}"
        );
    }

    #[test]
    fn claude_tool_policy_reaches_the_spawn_line() {
        // GH-574 round 2 (P1-1): claude's `--allowedTools` is a
        // permission-prompt rule, not a capability restriction — under
        // bypassPermissions Write/Edit/Bash stay reachable. The
        // capability-restricting flag is `--tools` ("Specify the list of
        // available tools from the built-in set"), so the phase allowlist
        // must spawn `--tools` and must never spawn `--allowedTools` while
        // claiming a structural allowlist.
        let args = args_of(&claude_command_for(
            "  - id: a\n    prompt: x\n    tools: [Read, Grep]\n    exclude_tools: [Write, Edit]\n",
        ));
        let allow = args
            .iter()
            .position(|a| a == "--tools")
            .expect("--tools must appear in the claude spawn command line");
        assert_eq!(args[allow + 1], "Read,Grep");
        assert!(
            !args.contains(&"--allowedTools".to_string()),
            "--allowedTools does not restrict capabilities; it must not be spawned: {args:?}"
        );
        let deny = args
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("--disallowedTools");
        assert_eq!(args[deny + 1], "Write,Edit");
    }

    /// GH-574: claude's CLI exposes no thinking-level flag, so a phase that
    /// declares one must be refused, never silently ignored. The refusal
    /// fires before any spawn, so a bare launcher suffices.
    #[tokio::test]
    async fn claude_refuses_phase_declared_thinking() {
        let launcher = ClaudeCodeLauncher::with_bin(PathBuf::from("claude"));
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    thinking: high\n");
        let error = launcher
            .run_phase(
                &phase,
                "p",
                "",
                "s",
                Path::new("."),
                CancellationToken::new(),
            )
            .await
            .expect_err("claude must refuse a declared thinking level");
        assert!(error.to_string().contains("does not support"), "{error}");
    }

    /// GH-574: the in-band observation starts empty and is only filled by
    /// what the backend itself reports — never from config or session files.
    #[test]
    fn claude_observed_model_starts_unknown() {
        let launcher = ClaudeCodeLauncher::with_bin(PathBuf::from("claude"));
        assert_eq!(launcher.last_observed_model(), None);
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
