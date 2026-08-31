//! `edda dispatch` — a single-turn agent dispatch decoupled from the plan
//! loop (issue #526). No plan file, no DAG, no state machine: read a prompt
//! from a file, run it through the selected launcher, print the outcome, and
//! exit with a code that reflects the outcome class. Loop control stays with
//! the caller.
//!
//! Exit codes by outcome class:
//! - 0 = agent done
//! - 1 = agent crash or any other failure (including pre-dispatch errors)
//! - 2 = timeout
//! - 3 = budget exceeded
//! - 4 = max turns

use crate::agent_kind::AgentKind;
use crate::cmd_conduct::{budget_warning_for_agent, cost_line};
use anyhow::{Context, Result};
use clap::Args;
use edda_conductor::agent::codex_rpc::CodexLauncher;
use edda_conductor::agent::launcher::PhaseResult;
use edda_conductor::agent::launcher::{phase_session_id, AgentLauncher, ClaudeCodeLauncher};
use edda_conductor::agent::pi_rpc::PiRpcLauncher;
use edda_conductor::plan::schema::Phase as PhaseSchema;
use tokio_util::sync::CancellationToken;

// ── CLI Schema ──

/// Arguments for `edda dispatch`.
#[derive(Args, Debug)]
pub struct DispatchArgs {
    /// Agent backend that runs the turn
    #[arg(long, value_enum)]
    pub agent: AgentKind,
    /// Path to a file containing the prompt (read verbatim)
    #[arg(long)]
    pub prompt_file: String,
    /// Session id passed to the backend verbatim (create-or-continue is the
    /// backend's concern). Generated and printed when omitted so the caller
    /// can reuse it on the next call.
    #[arg(long)]
    pub session_id: Option<String>,
    /// Working directory for the agent (default: current directory)
    #[arg(long)]
    pub cwd: Option<String>,
    /// Per-turn budget in USD (codex cannot enforce budgets)
    #[arg(long)]
    pub budget_usd: Option<f64>,
    /// Turn timeout in seconds (default: 1800, like a conduct phase)
    #[arg(long)]
    pub timeout_sec: Option<u64>,
    /// Print one JSON object to stdout instead of text lines
    #[arg(long)]
    pub json: bool,
}

// ── Outcome model ──

/// The outcome class of a dispatched turn, in the wire vocabulary used by
/// both the `--json` output and the exit-code table in the long help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Crash,
    Timeout,
    MaxTurns,
    BudgetExceeded,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Crash => "crash",
            Outcome::Timeout => "timeout",
            Outcome::MaxTurns => "max_turns",
            Outcome::BudgetExceeded => "budget_exceeded",
        }
    }
}

/// Exit code for an outcome class, per the help-text table:
/// 0 done, 1 crash/other, 2 timeout, 3 budget exceeded, 4 max turns.
pub fn exit_code_for(outcome: Outcome) -> i32 {
    match outcome {
        Outcome::Done => 0,
        Outcome::Crash => 1,
        Outcome::Timeout => 2,
        Outcome::BudgetExceeded => 3,
        Outcome::MaxTurns => 4,
    }
}

/// Everything the caller needs after one dispatched turn: the outcome class,
/// the agent's summary, the honest cost figure, and the session id to reuse.
#[derive(Debug, Clone)]
pub struct DispatchOutput {
    pub outcome: Outcome,
    pub result_text: Option<String>,
    pub cost_usd: Option<f64>,
    pub session_id: String,
    pub error: Option<String>,
}

impl DispatchOutput {
    pub fn from_result(result: PhaseResult, session_id: String) -> Self {
        match result {
            PhaseResult::AgentDone {
                cost_usd,
                result_text,
            } => Self {
                outcome: Outcome::Done,
                result_text,
                cost_usd,
                session_id,
                error: None,
            },
            PhaseResult::AgentCrash { error } => Self {
                outcome: Outcome::Crash,
                result_text: None,
                cost_usd: None,
                session_id,
                error: Some(error),
            },
            PhaseResult::Timeout => Self {
                outcome: Outcome::Timeout,
                result_text: None,
                cost_usd: None,
                session_id,
                error: None,
            },
            PhaseResult::MaxTurns { cost_usd } => Self {
                outcome: Outcome::MaxTurns,
                result_text: None,
                cost_usd,
                session_id,
                error: None,
            },
            PhaseResult::BudgetExceeded { cost_usd } => Self {
                outcome: Outcome::BudgetExceeded,
                result_text: None,
                cost_usd,
                session_id,
                error: None,
            },
        }
    }

    pub fn exit_code(&self) -> i32 {
        exit_code_for(self.outcome)
    }

    /// The `Cost:` line, reusing conduct's honest rendering: a total nobody
    /// measured renders as "n/a", never as a fabricated "$0.00".
    fn cost_text(&self) -> String {
        self.cost_usd
            .map(cost_line)
            .unwrap_or_else(|| "n/a (no usage data reported)".to_owned())
    }

    /// Text-mode stdout: result text, then `Cost:`, then `Session:`.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        if let Some(text) = &self.result_text {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push_str(&format!("Cost: {}\n", self.cost_text()));
        out.push_str(&format!("Session: {}\n", self.session_id));
        out
    }

    /// One JSON object: the whole machine-facing contract of the verb.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "outcome": self.outcome.as_str(),
            "result_text": self.result_text,
            "cost_usd": self.cost_usd,
            "session_id": self.session_id,
            "error": self.error,
        })
        .to_string()
    }
}

// ── Phase + launcher construction ──

/// Build the synthetic single-turn phase, mapping flags exactly the way a
/// conduct phase's fields map: budget and timeout land on the phase, and a
/// missing timeout falls back to the launcher's 1800 s default — the same
/// default a plan-level `timeout_sec` would supply.
pub fn build_phase(prompt: &str, budget_usd: Option<f64>, timeout_sec: Option<u64>) -> PhaseSchema {
    PhaseSchema {
        id: "dispatch".to_owned(),
        prompt: prompt.to_owned(),
        cwd: None,
        depends_on: Vec::new(),
        check: Vec::new(),
        max_attempts: None,
        timeout_sec,
        on_fail: None,
        context: None,
        env: std::collections::HashMap::new(),
        budget_usd,
        allowed_tools: None,
        permission_mode: "bypassPermissions".to_owned(),
    }
}

/// A caller-provided session id is used verbatim (create-or-continue
/// semantics belong to the backend); an omitted one is generated as a fresh
/// UUID so backends that require UUID session ids accept it.
pub fn generate_session_id() -> String {
    let unique = format!(
        "dispatch-{}-{}",
        std::process::id(),
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    phase_session_id("dispatch", &unique).to_string()
}

/// Construct the launcher exactly the way `edda conduct run` does,
/// including the `verify_available()` probe before any dispatch happens.
fn build_launcher(agent: AgentKind) -> Result<Box<dyn AgentLauncher>> {
    Ok(match agent {
        AgentKind::Claude => {
            let launcher = ClaudeCodeLauncher::new();
            launcher.verify_available()?;
            Box::new(launcher)
        }
        AgentKind::Pi => {
            let launcher = PiRpcLauncher::new();
            launcher.verify_available()?;
            Box::new(launcher)
        }
        AgentKind::Codex => {
            let launcher = CodexLauncher::new();
            launcher.verify_available()?;
            Box::new(launcher)
        }
    })
}

// ── Run ──

/// Execute `edda dispatch`.
pub fn run(args: DispatchArgs) -> Result<()> {
    let prompt = std::fs::read_to_string(&args.prompt_file)
        .with_context(|| format!("--prompt-file not readable: {}", args.prompt_file))?;

    let session_id = args.session_id.clone().unwrap_or_else(generate_session_id);

    let cwd = match args.cwd.as_deref() {
        Some(dir) => {
            let path = std::path::PathBuf::from(dir);
            if path.is_relative() {
                std::env::current_dir()?.join(path)
            } else {
                path
            }
        }
        None => std::env::current_dir()?,
    };

    if let Some(warning) = budget_warning_for_agent(args.agent, args.budget_usd.is_some()) {
        eprintln!("{warning}");
    }

    let launcher = build_launcher(args.agent)?;
    let phase = build_phase(&prompt, args.budget_usd, args.timeout_sec);
    let cancel = CancellationToken::new();

    let rt = tokio::runtime::Runtime::new()?;
    let output = rt.block_on(run_with_launcher(
        launcher.as_ref(),
        &phase,
        &session_id,
        &cwd,
        cancel,
    ))?;

    if args.json {
        println!("{}", output.to_json());
    } else {
        print!("{}", output.render_text());
        if let Some(error) = &output.error {
            eprintln!("Error: {error}");
        }
    }

    let code = output.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// One turn through the launcher with an empty plan context. Split out from
/// [`run`] so tests can drive it with MockLauncher or a recording stub.
pub async fn run_with_launcher(
    launcher: &dyn AgentLauncher,
    phase: &PhaseSchema,
    session_id: &str,
    cwd: &std::path::Path,
    cancel: CancellationToken,
) -> Result<DispatchOutput> {
    let result = launcher
        .run_phase(phase, &phase.prompt, "", session_id, cwd, cancel)
        .await?;
    Ok(DispatchOutput::from_result(result, session_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clap::Parser;
    use edda_conductor::agent::launcher::MockLauncher;
    use std::path::Path;
    use std::sync::Mutex;

    /// Minimal parser harness: `DispatchArgs` is an `Args` struct, so it
    /// needs a root command to be parsed standalone.
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: DispatchArgs,
    }

    fn parse(args: &[&str]) -> DispatchArgs {
        TestCli::try_parse_from(args)
            .expect("args should parse")
            .args
    }

    // ── CLI parse tests ──

    #[test]
    fn dispatch_accepts_each_agent() {
        for (value, expected) in [
            ("claude", AgentKind::Claude),
            ("pi", AgentKind::Pi),
            ("codex", AgentKind::Codex),
        ] {
            let args = parse(&["edda", "--agent", value, "--prompt-file", "p.txt"]);
            assert_eq!(args.agent, expected, "agent {value}");
        }
    }

    #[test]
    fn dispatch_rejects_unknown_agent_and_lists_all_three() {
        let error =
            match TestCli::try_parse_from(["edda", "--agent", "gpt", "--prompt-file", "p.txt"]) {
                Err(error) => error,
                Ok(_) => panic!("unknown agent must be rejected"),
            };
        let text = error.to_string();
        for expected in ["claude", "pi", "codex"] {
            assert!(
                text.contains(expected),
                "error should list the valid agents, missing {expected:?}: {text}"
            );
        }
    }

    #[test]
    fn dispatch_accepts_json_flag() {
        let args = parse(&["edda", "--agent", "pi", "--prompt-file", "p.txt", "--json"]);
        assert!(args.json);
        let args = parse(&["edda", "--agent", "pi", "--prompt-file", "p.txt"]);
        assert!(!args.json);
    }

    #[test]
    fn dispatch_requires_prompt_file() {
        let error = match TestCli::try_parse_from(["edda", "--agent", "pi"]) {
            Err(error) => error,
            Ok(_) => panic!("missing --prompt-file must be an error"),
        };
        assert!(
            error.to_string().contains("prompt-file"),
            "error should mention --prompt-file: {error}"
        );
    }

    // ── Outcome → exit-code mapping (all five PhaseResult variants) ──

    #[test]
    fn exit_codes_map_all_outcome_classes() {
        // One assertion per PhaseResult variant: the mapping is exercised
        // exactly as a real launcher result would reach it.
        let cases = vec![
            (
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                0,
            ),
            (
                PhaseResult::AgentCrash {
                    error: "boom".into(),
                },
                1,
            ),
            (PhaseResult::Timeout, 2),
            (PhaseResult::BudgetExceeded { cost_usd: None }, 3),
            (PhaseResult::MaxTurns { cost_usd: None }, 4),
        ];
        for (result, expected) in cases {
            let out = DispatchOutput::from_result(result, "s".into());
            assert_eq!(
                exit_code_for(out.outcome),
                expected,
                "for {:?}",
                out.outcome
            );
            assert_eq!(out.exit_code(), expected, "for {:?}", out.outcome);
        }
    }

    #[test]
    fn dispatch_output_exit_code_matches_phase_result() {
        let out = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: Some(0.5),
                result_text: Some("ok".into()),
            },
            "s".into(),
        );
        assert_eq!(out.exit_code(), 0);
        assert_eq!(out.outcome, Outcome::Done);
    }

    // ── JSON serialization shape ──

    #[test]
    fn json_shape_pins_field_names_for_each_outcome() {
        let cases = vec![
            (
                PhaseResult::AgentDone {
                    cost_usd: Some(1.25),
                    result_text: Some("did it".into()),
                },
                "done",
            ),
            (
                PhaseResult::AgentCrash {
                    error: "boom".into(),
                },
                "crash",
            ),
            (PhaseResult::Timeout, "timeout"),
            (PhaseResult::MaxTurns { cost_usd: None }, "max_turns"),
            (
                PhaseResult::BudgetExceeded { cost_usd: None },
                "budget_exceeded",
            ),
        ];
        for (result, expected_outcome) in cases {
            let out = DispatchOutput::from_result(result, "sess-1".into());
            let value: serde_json::Value =
                serde_json::from_str(&out.to_json()).expect("json parses");
            assert_eq!(value["outcome"].as_str(), Some(expected_outcome));
            assert!(value["result_text"].is_null() || value["result_text"].is_string());
            assert!(value["cost_usd"].is_null() || value["cost_usd"].is_number());
            assert_eq!(value["session_id"].as_str(), Some("sess-1"));
            assert!(value["error"].is_null() || value["error"].is_string());
            let mut keys: Vec<&str> = value
                .as_object()
                .expect("json object")
                .keys()
                .map(|k| k.as_str())
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec!["cost_usd", "error", "outcome", "result_text", "session_id"]
            );
        }
    }

    #[test]
    fn json_carries_crash_error_and_done_fields() {
        let crash = DispatchOutput::from_result(
            PhaseResult::AgentCrash {
                error: "exit 3".into(),
            },
            "s".into(),
        );
        let value: serde_json::Value = serde_json::from_str(&crash.to_json()).unwrap();
        assert_eq!(value["outcome"].as_str(), Some("crash"));
        assert_eq!(value["error"].as_str(), Some("exit 3"));
        assert!(value["result_text"].is_null());
        assert!(value["cost_usd"].is_null());

        let done = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: Some(0.1),
                result_text: None,
            },
            "s".into(),
        );
        let value: serde_json::Value = serde_json::from_str(&done.to_json()).expect("json parses");
        assert_eq!(value["outcome"].as_str(), Some("done"));
        assert!(value["result_text"].is_null());
        assert_eq!(value["cost_usd"].as_f64(), Some(0.1));
        assert!(value["error"].is_null());
    }

    // ── Text rendering ──

    #[test]
    fn text_render_prints_result_cost_then_session() {
        let out = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: Some("done deal".into()),
            },
            "abc-123".into(),
        );
        let text = out.render_text();
        assert!(text.contains("done deal\n"), "{text}");
        assert!(text.contains("Cost: $0.42"), "{text}");
        assert!(text.contains("Session: abc-123"), "{text}");
        // Order: result before Cost before Session.
        let result_pos = text.find("done deal").unwrap();
        let cost_pos = text.find("Cost:").unwrap();
        let session_pos = text.find("Session:").unwrap();
        assert!(result_pos < cost_pos && cost_pos < session_pos);
    }

    #[test]
    fn text_render_reports_na_when_no_usage() {
        let out = DispatchOutput::from_result(PhaseResult::MaxTurns { cost_usd: None }, "s".into());
        let text = out.render_text();
        assert!(
            text.contains("Cost: n/a (no usage data reported)"),
            "{text}"
        );
    }

    // ── Session id ──

    #[test]
    fn generated_session_id_is_non_empty_and_unique() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert!(!a.is_empty());
        assert!(!b.is_empty());
        assert_ne!(a, b, "consecutive generated ids should differ");
    }

    #[test]
    fn session_id_appears_in_both_output_modes() {
        let out = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            },
            "my-session-42".into(),
        );
        assert!(out.render_text().contains("Session: my-session-42"));
        assert!(out.to_json().contains("my-session-42"));
    }

    /// Records the session ids it receives — proves the caller's id reaches
    /// the launcher verbatim on every call.
    struct RecordingLauncher {
        session_ids: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AgentLauncher for RecordingLauncher {
        async fn run_phase(
            &self,
            _phase: &PhaseSchema,
            _prompt: &str,
            _plan_context: &str,
            session_id: &str,
            _cwd: &Path,
            _cancel: CancellationToken,
        ) -> Result<PhaseResult> {
            self.session_ids.lock().unwrap().push(session_id.to_owned());
            Ok(PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            })
        }
    }

    #[tokio::test]
    async fn same_session_id_reaches_launcher_verbatim_on_every_call() {
        let recorder = RecordingLauncher {
            session_ids: Mutex::new(Vec::new()),
        };
        let phase = build_phase("prompt", None, None);
        let cancel = CancellationToken::new();

        run_with_launcher(
            &recorder,
            &phase,
            "fixed-id",
            Path::new("."),
            cancel.clone(),
        )
        .await
        .unwrap();
        run_with_launcher(&recorder, &phase, "fixed-id", Path::new("."), cancel)
            .await
            .unwrap();

        let recorded = recorder.session_ids.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0], "fixed-id");
        assert_eq!(recorded[1], "fixed-id");
    }

    // ── Synthetic phase parity with conduct ──

    #[test]
    fn phase_maps_budget_and_timeout_flags_like_conduct() {
        let phase = build_phase("do the thing", Some(1.5), Some(60));
        assert_eq!(phase.id, "dispatch");
        assert_eq!(phase.prompt, "do the thing");
        assert_eq!(phase.budget_usd, Some(1.5));
        assert_eq!(phase.timeout_sec, Some(60));

        // Omitted flags stay None, exactly like a conduct phase without
        // them; the launcher then applies its 1800 s default.
        let phase = build_phase("p", None, None);
        assert_eq!(phase.budget_usd, None);
        assert_eq!(phase.timeout_sec, None);
        assert_eq!(phase.permission_mode, "bypassPermissions");
        assert!(phase.env.is_empty());
        assert!(phase.check.is_empty());
    }

    // ── End-to-end-ish run through MockLauncher ──

    #[tokio::test]
    async fn mock_done_run_produces_done_output() {
        let launcher = MockLauncher::new();
        launcher.set_results(
            "dispatch",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.75),
                result_text: Some("(mock) finished".into()),
            }],
        );
        let phase = build_phase("prompt", None, None);
        let out = run_with_launcher(
            &launcher,
            &phase,
            "sess-x",
            Path::new("."),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.outcome, Outcome::Done);
        assert_eq!(out.exit_code(), 0);
        assert_eq!(out.result_text.as_deref(), Some("(mock) finished"));
        assert_eq!(out.cost_usd, Some(0.75));
        assert_eq!(out.session_id, "sess-x");
    }

    #[tokio::test]
    async fn mock_timeout_maps_to_exit_code_2() {
        let launcher = MockLauncher::new();
        launcher.set_results("dispatch", vec![PhaseResult::Timeout]);
        let phase = build_phase("prompt", None, None);
        let out = run_with_launcher(
            &launcher,
            &phase,
            "sess-y",
            Path::new("."),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.outcome, Outcome::Timeout);
        assert_eq!(out.exit_code(), 2);
        assert!(out.render_text().contains("Session: sess-y"));
    }
}
