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

use crate::agent_kind::{
    build_launcher, validate_dispatch_options, AgentKind, DispatchOptions, LauncherOptions,
};
use crate::cmd_conduct::{budget_warning_for_agent, cost_line, NO_USAGE_COST_TEXT};
use anyhow::{bail, Context, Result};
use clap::Args;
use edda_conductor::agent::launcher::{phase_session_id, AgentLauncher, PhaseResult};
use edda_conductor::plan::schema::Phase as PhaseSchema;
use tokio_util::sync::CancellationToken;

// ── CLI Schema ──

/// Arguments for `edda dispatch`.
#[derive(Args, Debug)]
pub struct DispatchArgs {
    /// Agent backend that runs the turn
    #[arg(long, value_enum)]
    pub agent: AgentKind,
    /// Path to a file containing the prompt (read verbatim). Required
    /// unless --list-models is given.
    #[arg(long, required_unless_present = "list_models")]
    pub prompt_file: Option<String>,
    /// Session id passed to the backend verbatim. Continuity semantics are
    /// per-backend, and all three persist conversations across invocations:
    /// pi delegates to its backend, and codex's session→thread map is
    /// persisted in the per-user edda store (GH-535), so for both the same
    /// id resumes the prior conversation. claude is the exception — a
    /// `--session-id` that already exists is refused ("Session ID <id> is
    /// already in use"), so a second turn on the same conversation adds
    /// `--resume` (GH-708). Generated and printed when omitted so the
    /// caller can reuse it on the next call.
    #[arg(long)]
    pub session_id: Option<String>,
    /// Continue the conversation `--session-id` names instead of starting a
    /// new one (claude only, `claude --resume <id>`; GH-708). pi and codex
    /// resume by repeating `--session-id` alone and refuse this flag rather
    /// than accept a switch that does nothing. Requires --session-id: there
    /// is nothing to resume without one.
    #[arg(long, requires = "session_id")]
    pub resume: bool,
    /// Working directory for the agent (default: current directory)
    #[arg(long)]
    pub cwd: Option<String>,
    /// Per-turn budget in USD (codex cannot enforce budgets)
    #[arg(long)]
    pub budget_usd: Option<f64>,
    /// Turn timeout in seconds (default: 1800, like a conduct phase)
    #[arg(long)]
    pub timeout_sec: Option<u64>,
    /// Permission mode for the claude backend (`claude --permission-mode`,
    /// default bypassPermissions). pi and codex have no permission-mode
    /// concept; an explicitly passed value is refused, never accepted and
    /// silently dropped (GH-574).
    #[arg(long)]
    pub permission_mode: Option<String>,
    /// Model selection passed to the backend verbatim (GH-574): pi gets
    /// `--model <pattern>` (e.g. `openai-codex/gpt-5.6-sol`), claude gets
    /// `--model`. codex has no verifiable selection path and refuses the
    /// flag. Run `--list-models` to look up valid patterns.
    #[arg(long)]
    pub model: Option<String>,
    /// Thinking level passed to pi verbatim via `--thinking`
    /// (off|minimal|low|medium|high|xhigh|max). claude and codex refuse it.
    #[arg(long)]
    pub thinking: Option<String>,
    /// Tool allowlist, comma-separated (GH-574): pi `--tools`, claude
    /// `--tools`. Both restrict capabilities — the listed tools are the
    /// only ones the backend can use — so e.g. `--tools read,grep,find,ls`
    /// makes the turn structurally read-only. claude's `--tools` covers
    /// only the built-in set, so the spawn also denies all unlisted MCP
    /// tools (`--disallowedTools "mcp__*"`); pi's has no MCP leak. codex
    /// refuses the flag.
    #[arg(long, value_delimiter = ',')]
    pub tools: Option<Vec<String>>,
    /// Tool denylist, comma-separated (GH-574): pi `--exclude-tools`, claude
    /// `--disallowedTools`. Structural enforcement, not prompt discipline:
    /// e.g. `--exclude-tools edit,write` removes file-modification tools
    /// from the agent entirely. codex refuses it.
    #[arg(long, value_delimiter = ',')]
    pub exclude_tools: Option<Vec<String>>,
    /// Session storage directory (pi only, `--session-dir`). claude and
    /// codex manage their own session storage and refuse the flag.
    #[arg(long)]
    pub session_dir: Option<String>,
    /// List available provider/model pairs for the backend and exit, instead
    /// of dispatching. Optional search term filters the listing (pi
    /// `--list-models [search]`); other backends refuse it. Requires --agent.
    /// The listing is text: combining it with --json is an explicit
    /// conflict error, because --json promises exactly one JSON object.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub list_models: Option<String>,
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
/// the agent's summary, the honest cost figure, the session id to reuse, and
/// the model story — what edda requested and what the backend reported
/// observing in-band (GH-574).
#[derive(Debug, Clone)]
pub struct DispatchOutput {
    pub outcome: Outcome,
    pub result_text: Option<String>,
    pub cost_usd: Option<f64>,
    pub session_id: String,
    pub error: Option<String>,
    /// The model edda actually passed to the backend, or the literal
    /// "inherited" when no --model was given (the backend's own default
    /// applies — visible instead of silently identical).
    pub model_requested: String,
    /// The model the backend reported in-band, or the literal "unknown"
    /// when it reported nothing. Observed, never inferred from config or
    /// session files (GH-574 honesty rule).
    pub model_observed: String,
    /// The session id the backend reported in-band, or "unknown" when it
    /// reported nothing. `session_id` above is what edda ASKED for; this is
    /// what the backend says it ran. Keeping them apart is the whole point:
    /// `claude --resume <id>` "starts a copy and says so when the session is
    /// already running", so echoing the requested id back would report every
    /// fork as a clean resume (GH-708).
    pub session_observed: String,
}

impl DispatchOutput {
    pub fn from_result(
        result: PhaseResult,
        session_id: String,
        model_requested: String,
        model_observed: String,
        session_observed: String,
    ) -> Self {
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
                model_requested,
                model_observed,
                session_observed,
            },
            PhaseResult::AgentCrash { error } => Self {
                outcome: Outcome::Crash,
                result_text: None,
                cost_usd: None,
                session_id,
                error: Some(error),
                model_requested,
                model_observed,
                session_observed,
            },
            PhaseResult::Timeout => Self {
                outcome: Outcome::Timeout,
                result_text: None,
                cost_usd: None,
                session_id,
                error: None,
                model_requested,
                model_observed,
                session_observed,
            },
            PhaseResult::MaxTurns { cost_usd } => Self {
                outcome: Outcome::MaxTurns,
                result_text: None,
                cost_usd,
                session_id,
                error: None,
                model_requested,
                model_observed,
                session_observed,
            },
            PhaseResult::BudgetExceeded { cost_usd } => Self {
                outcome: Outcome::BudgetExceeded,
                result_text: None,
                cost_usd,
                session_id,
                error: None,
                model_requested,
                model_observed,
                session_observed,
            },
        }
    }

    pub fn exit_code(&self) -> i32 {
        exit_code_for(self.outcome)
    }

    /// The `Cost:` line, reusing conduct's honest rendering: a total nobody
    /// measured renders as "n/a", never as a fabricated "$0.00". Here
    /// measured-ness is the `Option` itself — `Some(c)` means the backend
    /// reported that figure, including a genuine $0.00 (GH-533).
    fn cost_text(&self) -> String {
        self.cost_usd
            .map(|cost| cost_line(cost, true))
            .unwrap_or_else(|| NO_USAGE_COST_TEXT.to_owned())
    }

    /// Text-mode stdout: an `Outcome:` line whenever the turn did not
    /// finish, then result text, then `Cost:`, then the model story
    /// (GH-574), then the session story — `Session:` (asked for) and
    /// `Session observed:` (what the backend reported, GH-708).
    ///
    /// A failed turn used to render exactly like a successful one — same
    /// `Cost:`/`Session:` summary, nothing naming the failure (GH-669) — so
    /// a reader of stdout alone could not tell the two apart. The marker
    /// leads the output and is emitted only for non-`done` outcomes, so a
    /// successful turn's shape is unchanged.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        if self.outcome != Outcome::Done {
            out.push_str(&format!("Outcome: {}\n", self.outcome.as_str()));
        }
        if let Some(text) = &self.result_text {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push_str(&format!("Cost: {}\n", self.cost_text()));
        out.push_str(&format!("Model requested: {}\n", self.model_requested));
        out.push_str(&format!("Model observed: {}\n", self.model_observed));
        out.push_str(&format!("Session: {}\n", self.session_id));
        out.push_str(&format!("Session observed: {}\n", self.session_observed));
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
            "model_requested": self.model_requested,
            "model_observed": self.model_observed,
            "session_observed": self.session_observed,
        })
        .to_string()
    }
}

// ── Phase + launcher construction ──

/// The GH-574 launcher-capability options a dispatched turn carries on its
/// synthetic phase: model selection, thinking level, and tool policy.
/// Declared but unsupported backend combinations are rejected by
/// [`crate::agent_kind::validate_dispatch_options`] before this reaches a
/// launcher.
#[derive(Debug, Default, Clone)]
pub struct CapabilityOptions {
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
}

/// Build the synthetic single-turn phase, mapping flags exactly the way a
/// conduct phase's fields map: budget, timeout, permission mode, model,
/// thinking level, and tool policy land on the phase, and a missing timeout
/// falls back to the launcher's 1800 s default — the same default a
/// plan-level `timeout_sec` would supply.
pub fn build_phase(
    prompt: &str,
    budget_usd: Option<f64>,
    timeout_sec: Option<u64>,
    permission_mode: &str,
    capabilities: CapabilityOptions,
) -> PhaseSchema {
    let CapabilityOptions {
        model,
        thinking,
        tools,
        exclude_tools,
    } = capabilities;
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
        tools,
        exclude_tools,
        model,
        thinking,
        permission_mode: permission_mode.to_owned(),
        gate: None,
        gate_timeout_sec: None,
        on_reject: Default::default(),
        on_gate_timeout: Default::default(),
        // A dispatched single turn declares no owned write surface.
        owns: Vec::new(),
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

// The #534 round-2 startup warning for codex `--session-id` is gone
// (GH-535): codex's session→thread map is persisted in the per-user edda
// store, so an explicit id now resumes across dispatch invocations like
// claude and pi. The warning survives only in the launcher, for the one
// genuinely non-resumable case — a corrupt persisted map, which degrades
// to `thread/start`.

// ── Run ──

/// Execute `edda dispatch`.
///
/// The whole run happens in [`run_inner`] so the tokio runtime and the
/// launcher are dropped before the process exits: `std::process::exit`
/// skips destructors, and a future launcher holding a live child across a
/// non-zero outcome must not be orphaned.
pub fn run(args: DispatchArgs) -> Result<()> {
    let code = run_inner(args)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn run_inner(args: DispatchArgs) -> Result<i32> {
    // GH-574 honesty gate: refuse unsupported backend/option combinations
    // instead of accepting them and silently doing nothing. An explicitly
    // passed --permission-mode on a backend with no permission concept is
    // refused here too (GH-574 round 2, P1-2) — there is no clap default,
    // so an absent flag claims nothing and drops nothing. Round 3 (P1-2):
    // this gate runs before EVERY short-circuit, including --list-models —
    // listing mode must not accept and drop a permission contract either.
    validate_dispatch_options(
        args.agent,
        &DispatchOptions {
            model: args.model.as_deref(),
            thinking: args.thinking.as_deref(),
            tools: args.tools.as_deref(),
            exclude_tools: args.exclude_tools.as_deref(),
            session_dir: args.session_dir.as_deref(),
            permission_mode: args.permission_mode.as_deref(),
            resume: args.resume,
        },
    )?;

    // --list-models short-circuits dispatch: print the provider/model table
    // and exit 0 (GH-574 — callers look up patterns instead of guessing a
    // provider prefix).
    if let Some(search) = args.list_models.as_deref() {
        // GH-574 round 2 (P1-3): the listing is text, but --json promises
        // "exactly one object" on stdout. Refuse the combination instead of
        // printing a text table that breaks every JSON consumer.
        if args.json {
            bail!(
                "--json cannot be combined with --list-models: the model listing is \
                 text, and --json promises exactly one JSON object on stdout"
            );
        }
        if !args.agent.supports_model_listing() {
            bail!(
                "--list-models is only available for the pi backend; agent \"{}\" \
                 exposes no provider/model listing query",
                args.agent.as_str()
            );
        }
        let text = edda_conductor::agent::pi_rpc::list_models(None, Some(search))?;
        print!("{text}");
        return Ok(0);
    }

    let prompt_file = args
        .prompt_file
        .as_deref()
        .context("--prompt-file is required unless --list-models is given")?;
    let prompt = std::fs::read_to_string(prompt_file)
        .with_context(|| format!("--prompt-file not readable: {prompt_file}"))?;

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

    // Permission mode is a claude-only contract (refused above for the
    // other backends); an absent flag falls back to claude's default.
    let permission_mode = args
        .permission_mode
        .clone()
        .unwrap_or_else(|| "bypassPermissions".to_owned());

    let launcher = build_launcher(
        args.agent,
        LauncherOptions {
            verbose: false,
            transcript_dir: None,
            // Dispatch is the persistence scope (GH-535): a caller-chosen
            // --session-id must resume the conversation a previous dispatch
            // recorded.
            persistent_codex_threads: true,
            session_dir: args.session_dir.as_ref().map(std::path::PathBuf::from),
            resume: args.resume,
        },
    )?;
    let phase = build_phase(
        &prompt,
        args.budget_usd,
        args.timeout_sec,
        &permission_mode,
        CapabilityOptions {
            model: args.model,
            thinking: args.thinking,
            tools: args.tools,
            exclude_tools: args.exclude_tools,
        },
    );
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

    // GH-577: Ingest Pi session transcripts after the turn completes.
    // Observation surface cannot kill work surface (GH-566/GH-577): errors degrade to warnings.
    if args.agent == AgentKind::Pi {
        if let Err(e) = ingest_pi_session_post_dispatch(
            &cwd,
            &session_id,
            args.session_dir.as_deref().map(std::path::Path::new),
        ) {
            eprintln!("Warning: failed to ingest pi session: {e}");
        }
    }

    let code = output.exit_code();
    Ok(code)
}

/// Ingest Pi session transcripts after a dispatch turn and emit `#session_digest`.
pub(crate) fn ingest_pi_session_post_dispatch(
    cwd: &std::path::Path,
    session_id: &str,
    session_dir: Option<&std::path::Path>,
) -> Result<()> {
    let project_id = edda_store::project_id(cwd);
    let project_dir = edda_store::project_dir(&project_id);
    edda_store::ensure_dirs(&project_id)?;

    let session_file = match edda_transcript::find_pi_session_file(cwd, session_id, session_dir) {
        Some(f) => f,
        None => {
            eprintln!("Warning: pi session file not found for session {session_id}");
            return Ok(());
        }
    };

    let _stats =
        edda_transcript::ingest_pi_transcript_delta(&project_dir, session_id, cwd, &session_file)?;

    let cwd_str = cwd.to_string_lossy();
    edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, &cwd_str, true)?;

    Ok(())
}

/// One turn through the launcher with an empty plan context. Split out from
/// [`run`] so tests can drive it with MockLauncher or a recording stub.
///
/// GH-566/GH-569: the turn runs through the conductor runner's
/// `run_phase_with_heartbeat`, so a dispatched lane (any backend, no Claude
/// hooks) periodically refreshes the session heartbeat and is visible to
/// `edda peers` while it works. The write lives in the conductor runner;
/// dispatch stays stateless — the heartbeat is an observation surface, not a
/// control surface, and ages out through the normal staleness threshold.
pub async fn run_with_launcher(
    launcher: &dyn AgentLauncher,
    phase: &PhaseSchema,
    session_id: &str,
    cwd: &std::path::Path,
    cancel: CancellationToken,
) -> Result<DispatchOutput> {
    let result = {
        let hb = edda_conductor::runner::heartbeat::LaneHeartbeat {
            cwd: cwd.to_path_buf(),
            session_id: session_id.to_string(),
            plan: "dispatch".to_string(),
            phase: phase.id.clone(),
            attempt: 1,
        };
        edda_conductor::runner::heartbeat::run_phase_with_heartbeat(
            launcher,
            phase,
            &phase.prompt,
            "",
            cwd,
            &cancel,
            &hb,
        )
        .await?
    };
    // GH-574: requested comes from the phase edda built (what was actually
    // passed to the backend); observed is whatever the backend reported
    // in-band, "unknown" when it reported nothing.
    let model_requested = phase
        .model
        .clone()
        .unwrap_or_else(|| "inherited".to_owned());
    let model_observed = launcher
        .last_observed_model()
        .unwrap_or_else(|| "unknown".to_owned());
    let session_observed = launcher
        .last_observed_session()
        .unwrap_or_else(|| "unknown".to_owned());
    Ok(DispatchOutput::from_result(
        result,
        session_id.to_owned(),
        model_requested,
        model_observed,
        session_observed,
    ))
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

    #[test]
    fn dispatch_prompt_file_optional_when_listing_models() {
        let args = parse(&["edda", "--agent", "pi", "--list-models"]);
        assert!(args.prompt_file.is_none());
        assert_eq!(args.list_models.as_deref(), Some(""));
        let args = parse(&["edda", "--agent", "pi", "--list-models", "sol"]);
        assert_eq!(args.list_models.as_deref(), Some("sol"));
    }

    // ── GH-574: launcher-capability flags ──

    #[test]
    fn dispatch_parses_model_thinking_and_tool_flags() {
        let args = parse(&[
            "edda",
            "--agent",
            "pi",
            "--prompt-file",
            "p.txt",
            "--model",
            "openai-codex/gpt-5.6-sol",
            "--thinking",
            "high",
            "--tools",
            "read,grep",
            "--exclude-tools",
            "edit,write",
            "--session-dir",
            "/tmp/pi-sessions",
        ]);
        assert_eq!(args.model.as_deref(), Some("openai-codex/gpt-5.6-sol"));
        assert_eq!(args.thinking.as_deref(), Some("high"));
        assert_eq!(args.tools, Some(vec!["read".into(), "grep".into()]));
        assert_eq!(
            args.exclude_tools,
            Some(vec!["edit".into(), "write".into()])
        );
        assert_eq!(args.session_dir.as_deref(), Some("/tmp/pi-sessions"));
    }

    #[test]
    fn phase_carries_model_thinking_and_tool_policy() {
        let phase = build_phase(
            "p",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions {
                model: Some("anthropic/claude-opus-5".into()),
                thinking: Some("high".into()),
                tools: Some(vec!["read".into(), "grep".into()]),
                exclude_tools: Some(vec!["edit".into(), "write".into()]),
            },
        );
        assert_eq!(phase.model.as_deref(), Some("anthropic/claude-opus-5"));
        assert_eq!(phase.thinking.as_deref(), Some("high"));
        assert_eq!(phase.tools, Some(vec!["read".into(), "grep".into()]));
        assert_eq!(
            phase.exclude_tools,
            Some(vec!["edit".into(), "write".into()])
        );
    }

    #[test]
    fn model_report_defaults_to_inherited_and_unknown() {
        let out = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            },
            "s".into(),
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
        );
        let text = out.render_text();
        assert!(text.contains("Model requested: inherited"), "{text}");
        assert!(text.contains("Model observed: unknown"), "{text}");
        let value: serde_json::Value = serde_json::from_str(&out.to_json()).unwrap();
        assert_eq!(value["model_requested"], "inherited");
        assert_eq!(value["model_observed"], "unknown");
    }

    #[test]
    fn model_report_carries_requested_and_observed() {
        let out = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: Some(0.2),
                result_text: None,
            },
            "s".into(),
            "openai-codex/gpt-5.6-sol".into(),
            "openai-codex/gpt-5.6-sol".into(),
            "unknown".into(),
        );
        let text = out.render_text();
        assert!(
            text.contains("Model requested: openai-codex/gpt-5.6-sol"),
            "{text}"
        );
        assert!(
            text.contains("Model observed: openai-codex/gpt-5.6-sol"),
            "{text}"
        );
        let value: serde_json::Value = serde_json::from_str(&out.to_json()).unwrap();
        assert_eq!(value["model_requested"], "openai-codex/gpt-5.6-sol");
    }

    /// Incident counterexample (GH-574): the 2026-09-02 review degradation
    /// produced byte-identical stdout whether the review model was applied
    /// or silently dropped. After the fix, the requested-model line alone
    /// makes the two dispatches distinguishable.
    #[test]
    fn stdout_differs_between_a_dispatched_model_and_the_inherited_default() {
        let result = PhaseResult::AgentDone {
            cost_usd: Some(0.2),
            result_text: Some("review done".into()),
        };
        let with_model = DispatchOutput::from_result(
            result.clone(),
            "s".into(),
            "openai-codex/gpt-5.6-sol".into(),
            "unknown".into(),
            "unknown".into(),
        );
        let without_model = DispatchOutput::from_result(
            result,
            "s".into(),
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
        );
        assert_ne!(
            with_model.render_text(),
            without_model.render_text(),
            "dispatch stdout must differ between --model X and no --model"
        );
    }

    #[test]
    fn run_with_launcher_reports_inherited_when_no_model_declared() {
        let launcher = MockLauncher::new();
        launcher.set_results(
            "dispatch",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            }],
        );
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(run_with_launcher(
            &launcher,
            &phase,
            "s",
            Path::new("."),
            CancellationToken::new(),
        ));
        let out = out.unwrap();
        assert_eq!(out.model_requested, "inherited");
        // MockLauncher reports nothing in-band: unknown, honestly.
        assert_eq!(out.model_observed, "unknown");
    }

    #[test]
    fn run_with_launcher_reports_the_declared_model_as_requested() {
        let launcher = MockLauncher::new();
        launcher.set_results(
            "dispatch",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            }],
        );
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions {
                model: Some("openai-codex/gpt-5.6-sol".into()),
                thinking: None,
                tools: None,
                exclude_tools: None,
            },
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(run_with_launcher(
            &launcher,
            &phase,
            "s",
            Path::new("."),
            CancellationToken::new(),
        ));
        let out = out.unwrap();
        assert_eq!(out.model_requested, "openai-codex/gpt-5.6-sol");
    }

    /// The --list-models gate lives in run_inner ahead of any spawn, so the
    /// refusal is testable without a backend: a backend without a listing
    /// query is an explicit error, never a silent fallthrough.
    #[test]
    fn list_models_refuses_backends_without_a_listing_query() {
        let args = parse(&["edda", "--agent", "claude", "--list-models"]);
        let error = run_inner(args).expect_err("claude has no model listing");
        assert!(
            error
                .to_string()
                .contains("only available for the pi backend"),
            "{error}"
        );
    }

    /// GH-574 round 3 (P1-2): the --list-models short-circuit must not
    /// bypass the capability gate. An explicit --permission-mode on pi is
    /// refused even in listing mode, before any model query runs. Pre-fix,
    /// this combination reached the pi backend and exited 0 with no
    /// permission signal at all.
    #[test]
    fn list_models_does_not_short_circuit_capability_validation() {
        let args = parse(&[
            "edda",
            "--agent",
            "pi",
            "--list-models",
            "definitely-no-such-model-8f3a",
            "--permission-mode",
            "bypassPermissions",
        ]);
        let error = run_inner(args)
            .expect_err("an explicit permission-mode on pi must be refused in listing mode too");
        assert!(error.to_string().contains("--permission-mode"), "{error}");
    }

    /// GH-574 round 2 (P1-3): with --json, stdout must be exactly one JSON
    /// object. A text model listing printed at exit 0 breaks every JSON
    /// consumer mid-stream, so the combination must be an explicit conflict
    /// error — never a successful listing that silently ignores --json.
    #[test]
    fn list_models_with_json_is_refused_as_a_conflict() {
        let args = parse(&["edda", "--agent", "pi", "--list-models", "--json"]);
        let error = run_inner(args).expect_err("--json + --list-models must be refused");
        assert!(error.to_string().contains("--json"), "{error}");
    }

    // ── GH-669: a backend authentication failure is not a done turn ──

    #[test]
    fn backend_auth_failure_reports_crash_exit_1_and_non_null_error() {
        // The whole caller-facing contract for the observed failure: claude
        // answers a revoked/invalid OAuth token with a result message whose
        // `subtype` is "success" and whose `is_error` is true. Dispatch must
        // read that as a failed turn — exit 1, `outcome` not "done", `error`
        // a non-null string — instead of exit 0 with the reason parked in
        // `result_text`.
        use edda_conductor::agent::stream::{classify_result, MonitorResult, ResultInfo};
        let reason = "Failed to authenticate. API Error: 401 OAuth access token is invalid.";
        let monitor = MonitorResult {
            total_cost_usd: 0.0,
            result: Some(ResultInfo {
                subtype: "success".into(),
                total_cost_usd: Some(0.0),
                error: None,
                is_error: true,
                result_text: Some(reason.into()),
            }),
            result_text: Some(reason.into()),
            model: Some("claude-opus-5[1m]".into()),
            session_id: None,
        };
        let out = DispatchOutput::from_result(
            classify_result(&monitor, Some(1)),
            "0e92629d-1f2e-597e-90ec-662e206efcde".into(),
            "inherited".into(),
            "claude-opus-5[1m]".into(),
            "unknown".into(),
        );

        assert_eq!(out.exit_code(), 1, "{out:?}");
        let value: serde_json::Value = serde_json::from_str(&out.to_json()).unwrap();
        assert_ne!(value["outcome"].as_str(), Some("done"));
        assert_eq!(value["outcome"].as_str(), Some("crash"));
        assert!(
            value["error"].as_str().is_some_and(|e| e.contains("401")),
            "error must describe the failure, got {}",
            value["error"]
        );

        // And the human-readable turn must not read like a successful one.
        let text = out.render_text();
        assert!(text.contains("Outcome: crash"), "{text}");
        assert!(!text.contains("Cost: $0.00"), "{text}");
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
            let out = DispatchOutput::from_result(
                result,
                "s".into(),
                "inherited".into(),
                "unknown".into(),
                "unknown".into(),
            );
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
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
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
            let out = DispatchOutput::from_result(
                result,
                "sess-1".into(),
                "inherited".into(),
                "unknown".into(),
                "unknown".into(),
            );
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
                vec![
                    "cost_usd",
                    "error",
                    "model_observed",
                    "model_requested",
                    "outcome",
                    "result_text",
                    "session_id",
                    "session_observed"
                ]
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
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
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
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
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
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
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
        let out = DispatchOutput::from_result(
            PhaseResult::MaxTurns { cost_usd: None },
            "s".into(),
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
        );
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
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
        );
        assert!(out.render_text().contains("Session: my-session-42"));
        assert!(out.to_json().contains("my-session-42"));
    }

    /// GH-708: the requested id and the observed one are separate fields
    /// with separate sources, so a `--resume` that forked instead of
    /// continuing is visible. Echoing the request into `session_observed`
    /// would report every fork as a clean resume.
    #[test]
    fn requested_and_observed_session_ids_are_reported_separately() {
        let forked = DispatchOutput::from_result(
            PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            },
            "asked-for-42".into(),
            "inherited".into(),
            "unknown".into(),
            "actually-ran-99".into(),
        );
        let text = forked.render_text();
        assert!(text.contains("Session: asked-for-42"), "{text}");
        assert!(text.contains("Session observed: actually-ran-99"), "{text}");
        assert!(forked
            .to_json()
            .contains("\"session_observed\":\"actually-ran-99\""));
    }

    /// A backend that reports no session id renders "unknown", never the
    /// requested id — the same honesty rule `model_observed` follows.
    #[tokio::test]
    async fn a_silent_backend_reports_an_unknown_observed_session() {
        let launcher = MockLauncher::new();
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
        let out = run_with_launcher(
            &launcher,
            &phase,
            "asked-for-42",
            Path::new("."),
            CancellationToken::new(),
        )
        .await
        .expect("mock run");
        assert_eq!(out.session_id, "asked-for-42");
        assert_eq!(
            out.session_observed, "unknown",
            "a launcher that observed nothing must not echo the requested id"
        );
    }

    /// Records the session ids it receives. This proves verbatim delivery
    /// of the caller's id to the launcher within one process — it says
    /// nothing about cross-invocation continuity, which for codex does not
    /// hold (see `session_id_warning_for_agent`).
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
    async fn session_id_is_delivered_verbatim_on_every_call_within_one_process() {
        // Verbatim delivery only: repeating the same id inside one process
        // reaches the launcher unchanged. This test does not (and cannot)
        // prove cross-invocation continuity — for codex it does not hold.
        let recorder = RecordingLauncher {
            session_ids: Mutex::new(Vec::new()),
        };
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
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

    /// P0 regression (review round 1): the user-controlled `--session-id`
    /// reaches the heartbeat writer; a traversal id (`x\..\..\..\escaped`
    /// was the reviewed repro that wrote `store/projects/escaped.json`)
    /// must be contained by the store's path funnel, not escape the state
    /// directory.
    #[tokio::test]
    // isolated_store() takes the shared test-support lock, so the process-global
    // EDDA_STORE_ROOT stays ours for the whole test, including the await below —
    // that is the point, not an accidental hold. A private lock here is what
    // broke Windows CI (PR #588, round 2): this test relocated the root under a
    // lock no other test knew about, so a concurrent cmd_bridge test that held
    // the shared lock had its writes land in one store and its reads in another.
    #[allow(clippy::await_holding_lock)]
    async fn hostile_session_id_cannot_escape_the_state_directory() {
        let _store = crate::test_support::isolated_store();

        let launcher = RecordingLauncher {
            session_ids: Mutex::new(Vec::new()),
        };
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
        let sid = "x\\..\\..\\..\\escaped";
        run_with_launcher(
            &launcher,
            &phase,
            sid,
            Path::new("."),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let project_id = edda_store::project_id(Path::new("."));
        let project = edda_store::project_dir(&project_id);
        let state = project.join("state");
        assert!(
            edda_store::read_heartbeat(&project_id, sid).is_some(),
            "heartbeat written under the sanitized in-state-dir path"
        );
        // Nothing escaped to the project dir, the projects/ level or the
        // store root.
        assert!(!project.join("escaped.json").exists());
        assert!(!state.join("escaped.json").exists());
        if let Some(projects) = project.parent() {
            assert!(!projects.join("escaped.json").exists());
            if let Some(root) = projects.parent() {
                assert!(!root.join("escaped.json").exists());
            }
        }
        // _store (the IsolatedStore guard) restores EDDA_STORE_ROOT on drop.
    }

    // ── Synthetic phase parity with conduct ──

    #[test]
    fn phase_maps_budget_and_timeout_flags_like_conduct() {
        let phase = build_phase(
            "do the thing",
            Some(1.5),
            Some(60),
            "bypassPermissions",
            CapabilityOptions::default(),
        );
        assert_eq!(phase.id, "dispatch");
        assert_eq!(phase.prompt, "do the thing");
        assert_eq!(phase.budget_usd, Some(1.5));
        assert_eq!(phase.timeout_sec, Some(60));

        // Omitted flags stay None, exactly like a conduct phase without
        // them; the launcher then applies its 1800 s default.
        let phase = build_phase(
            "p",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
        assert_eq!(phase.budget_usd, None);
        assert_eq!(phase.timeout_sec, None);
        assert_eq!(phase.permission_mode, "bypassPermissions");
        assert!(phase.env.is_empty());
        assert!(phase.check.is_empty());
    }

    #[test]
    fn phase_carries_permission_mode_verbatim() {
        // The flag lands on the synthetic phase the way a conduct phase
        // carries its own permission_mode.
        let phase = build_phase("p", None, None, "acceptEdits", CapabilityOptions::default());
        assert_eq!(phase.permission_mode, "acceptEdits");
    }

    // ── --permission-mode flag ──

    #[test]
    fn dispatch_permission_mode_is_none_when_not_passed() {
        // No clap default: an absent flag claims nothing, so backends that
        // ignore permission modes drop nothing silently (GH-574 round 2).
        let args = parse(&["edda", "--agent", "claude", "--prompt-file", "p.txt"]);
        assert_eq!(args.permission_mode, None);
    }

    #[test]
    fn dispatch_parses_explicit_permission_mode() {
        let args = parse(&[
            "edda",
            "--agent",
            "claude",
            "--prompt-file",
            "p.txt",
            "--permission-mode",
            "default",
        ]);
        assert_eq!(args.permission_mode.as_deref(), Some("default"));
    }

    /// The P1-2 repro, at the run_inner level: an explicit --permission-mode
    /// on codex is refused before any launcher is built. (The cross-process
    /// form lives in tests/dispatch_permission_mode.rs; this in-process form
    /// needs no fake binary because the refusal fires first.)
    #[test]
    fn run_inner_refuses_permission_mode_on_backends_without_a_permission_concept() {
        let dir = std::env::temp_dir().join(format!(
            "edda-dispatch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let prompt = dir.join("prompt.txt");
        std::fs::write(&prompt, "p").expect("prompt written");
        let prompt = prompt.to_string_lossy().into_owned();
        let args = parse(&[
            "edda",
            "--agent",
            "codex",
            "--permission-mode",
            "bypassPermissions",
            "--prompt-file",
            &prompt,
        ]);
        let error = run_inner(args)
            .expect_err("codex has no permission-mode concept; the value must be refused");
        assert!(error.to_string().contains("--permission-mode"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── GH-708: --resume ──

    #[test]
    fn resume_requires_a_session_id_to_resume() {
        // Without an id there is nothing to continue, and claude would pick
        // "the most recent conversation in this directory" — which for a
        // review lane is whatever ran there last, not this PR's reviewer.
        assert!(
            TestCli::try_parse_from([
                "edda",
                "--agent",
                "claude",
                "--prompt-file",
                "p.txt",
                "--resume"
            ])
            .is_err(),
            "--resume without --session-id must not parse"
        );
        let args = parse(&[
            "edda",
            "--agent",
            "claude",
            "--prompt-file",
            "p.txt",
            "--session-id",
            "7a9c6b1e-0000-4708-8000-000000000001",
            "--resume",
        ]);
        assert!(args.resume);
        assert_eq!(
            args.session_id.as_deref(),
            Some("7a9c6b1e-0000-4708-8000-000000000001")
        );
    }

    #[test]
    fn resume_is_refused_on_backends_that_resume_by_session_id_alone() {
        for agent in ["pi", "codex"] {
            let error = validate_dispatch_options(
                match agent {
                    "pi" => AgentKind::Pi,
                    _ => AgentKind::Codex,
                },
                &DispatchOptions {
                    resume: true,
                    ..Default::default()
                },
            )
            .expect_err("only claude needs a distinct resume spelling");
            assert!(error.to_string().contains("--resume"), "{error}");
        }
        validate_dispatch_options(
            AgentKind::Claude,
            &DispatchOptions {
                resume: true,
                ..Default::default()
            },
        )
        .expect("claude supports --resume");
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
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
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
        let phase = build_phase(
            "prompt",
            None,
            None,
            "bypassPermissions",
            CapabilityOptions::default(),
        );
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

    #[test]
    fn ingest_pi_session_post_dispatch_missing_file_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let res = ingest_pi_session_post_dispatch(tmp.path(), "nonexistent-sess", None);
        assert!(
            res.is_ok(),
            "missing pi session file returns Ok(()) gracefully"
        );
    }

    #[test]
    fn ingest_pi_session_post_dispatch_wired_produces_digest() {
        let _store = crate::test_support::isolated_store();
        let tmp_ws = tempfile::tempdir().unwrap();
        let tmp_sessions = tempfile::tempdir().unwrap();

        let ws_path = tmp_ws.path();
        let ledger = edda_ledger::Ledger::open_or_init(ws_path).unwrap();

        let session_id = "test-wired-pi-577";
        let session_file = tmp_sessions
            .path()
            .join(format!("2026-09-02T12-00-00-000Z_{session_id}.jsonl"));
        let lines = vec![
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": session_id,
                "timestamp": "2026-09-02T12:00:00.000Z",
                "cwd": ws_path.to_string_lossy(),
            }),
            serde_json::json!({
                "type": "message",
                "id": "m1",
                "timestamp": "2026-09-02T12:00:05.000Z",
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": "Run tests" }]
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "m2",
                "timestamp": "2026-09-02T12:01:30.000Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "toolCall",
                            "id": "c1",
                            "name": "bash",
                            "arguments": { "command": "cargo check" }
                        }
                    ],
                    "model": "gpt-5.6-sol",
                    "usage": {
                        "input": 500,
                        "output": 100,
                        "cacheRead": 0,
                        "cacheWrite": 0,
                        "totalTokens": 600,
                        "cost": { "total": 0.02 }
                    }
                }
            }),
            serde_json::json!({
                "type": "message",
                "id": "m3",
                "timestamp": "2026-09-02T12:01:35.000Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "c1",
                    "toolName": "bash",
                    "content": [{ "type": "text", "text": "ok" }],
                    "isError": false
                }
            }),
        ];

        let mut content = String::new();
        for l in lines {
            content.push_str(&serde_json::to_string(&l).unwrap());
            content.push('\n');
        }
        std::fs::write(&session_file, content).unwrap();

        // Exercise the wired entry point directly
        let res = ingest_pi_session_post_dispatch(ws_path, session_id, Some(tmp_sessions.path()));
        assert!(
            res.is_ok(),
            "wired post-dispatch ingest should succeed: {:?}",
            res.err()
        );

        // Verify that #session_digest note was appended to ledger
        let events = ledger.iter_events().unwrap();
        let digests: Vec<_> = events
            .iter()
            .filter(|e| {
                e.payload
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .is_some_and(|arr| arr.iter().any(|tag| tag == "session_digest"))
            })
            .collect();
        assert_eq!(digests.len(), 1, "exactly one session_digest note emitted");
        let text = digests[0]
            .payload
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        assert!(
            text.contains("1 tool calls"),
            "text should contain '1 tool calls': {text}"
        );
        assert!(
            text.contains("Bash:1"),
            "text should contain 'Bash:1': {text}"
        );
        assert!(
            text.contains("gpt-5.6-sol"),
            "text should contain model: {text}"
        );
        assert!(text.contains("$0.02"), "text should contain cost: {text}");
    }
}
