//! Shared agent-backend selection, used by both `edda conduct` (plan runs)
//! and `edda dispatch` (single-turn runs). The enum, its string form, the
//! transcript capability flag, and the launcher construct-and-probe factory
//! live here so neither command duplicates the other's backend list.

use anyhow::{bail, Result};
use edda_conductor::agent::codex_rpc::CodexLauncher;
use edda_conductor::agent::launcher::{AgentLauncher, ClaudeCodeLauncher};
use edda_conductor::agent::pi_rpc::PiRpcLauncher;
use std::path::PathBuf;

/// Which agent backend runs the plan's phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentKind {
    /// Claude Code via `claude -p` (default)
    Claude,
    /// pi coding agent via `pi --mode rpc`
    Pi,
    /// codex CLI via `codex app-server`
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Pi => "pi",
            AgentKind::Codex => "codex",
        }
    }

    /// Whether this backend tees per-phase transcripts to disk, which the
    /// tmux phase panes tail.
    pub fn writes_transcripts(self) -> bool {
        match self {
            AgentKind::Claude => true,
            AgentKind::Pi => false,
            AgentKind::Codex => false,
        }
    }

    /// Whether the backend can select a model via a flag edda controls
    /// (`pi --model`, `claude --model`). codex's app-server exposes no
    /// verifiable model-selection path, so declaring one there is refused,
    /// never ignored (GH-574).
    pub fn supports_model(self) -> bool {
        matches!(self, AgentKind::Claude | AgentKind::Pi)
    }

    /// Whether the backend has a thinking-level flag. Only pi does today.
    pub fn supports_thinking(self) -> bool {
        matches!(self, AgentKind::Pi)
    }

    /// Whether the backend takes tool allow/deny lists
    /// (`pi --tools/--exclude-tools`, `claude --allowedTools/--disallowedTools`).
    pub fn supports_tool_policy(self) -> bool {
        matches!(self, AgentKind::Claude | AgentKind::Pi)
    }

    /// Whether the backend takes a session-storage directory (`pi
    /// --session-dir`). claude and codex manage their own session storage.
    pub fn supports_session_dir(self) -> bool {
        matches!(self, AgentKind::Pi)
    }

    /// Whether the backend exposes a provider/model listing query
    /// (`pi --list-models`).
    pub fn supports_model_listing(self) -> bool {
        matches!(self, AgentKind::Pi)
    }
}

// ── Backend × option support matrix (GH-574) ──

/// The launcher-capability flags `edda dispatch` accepts, before they are
/// copied onto the synthetic phase.
#[derive(Debug, Default)]
pub(crate) struct DispatchOptions<'a> {
    pub model: Option<&'a str>,
    pub thinking: Option<&'a str>,
    pub tools: Option<&'a [String]>,
    pub exclude_tools: Option<&'a [String]>,
    pub session_dir: Option<&'a str>,
}

/// Reject unsupported backend/option combinations with an explicit error.
/// Silent acceptance would recreate the exact GH-574 failure mode: a flag
/// the caller believes is enforced, quietly doing nothing.
pub(crate) fn validate_dispatch_options(
    agent: AgentKind,
    options: &DispatchOptions<'_>,
) -> Result<()> {
    let unsupported = [
        (
            "--model",
            options.model.map(|m| format!("{m:?}")),
            agent.supports_model(),
        ),
        (
            "--thinking",
            options.thinking.map(|t| format!("{t:?}")),
            agent.supports_thinking(),
        ),
        (
            "--tools",
            options.tools.map(|t| format!("{t:?}")),
            agent.supports_tool_policy(),
        ),
        (
            "--exclude-tools",
            options.exclude_tools.map(|t| format!("{t:?}")),
            agent.supports_tool_policy(),
        ),
        (
            "--session-dir",
            options.session_dir.map(|d| format!("{d:?}")),
            agent.supports_session_dir(),
        ),
    ];
    let refused: Vec<String> = unsupported
        .into_iter()
        .filter_map(|(flag, value, supported)| match (value, supported) {
            (Some(v), false) => Some(format!("{flag} {v}")),
            _ => None,
        })
        .collect();
    if !refused.is_empty() {
        bail!(
            "agent \"{}\" does not support: {}. edda refuses to silently ignore \
             unsupported options — drop them or dispatch with a backend that supports them",
            agent.as_str(),
            refused.join(", ")
        );
    }
    Ok(())
}

// ── Launcher factory ──

/// Per-backend options for [`build_launcher`]: verbose streaming, the
/// transcript directory for the backend that tees transcripts, whether
/// codex thread persistence is enabled, and pi's session storage directory.
pub(crate) struct LauncherOptions {
    /// Stream live agent activity while the phase runs.
    pub verbose: bool,
    /// If set, claude captures raw agent stdout to
    /// `{transcript_dir}/{phase_id}-{session_id_prefix}.jsonl`.
    pub transcript_dir: Option<PathBuf>,
    /// Persist codex's session→thread map in the per-user edda store so a
    /// repeated `--session-id` resumes across invocations. Dispatch-scoped
    /// (GH-535 round 1): `edda dispatch` sets this, `edda conduct` must not
    /// — conduct session ids are deterministic per plan/phase/attempt and
    /// its behavior must stay byte-identical with the pre-persistence path.
    pub persistent_codex_threads: bool,
    /// pi `--session-dir` (GH-574). Other backends manage their own session
    /// storage; dispatch rejects the flag for them before reaching here.
    pub session_dir: Option<PathBuf>,
}

/// Construct and probe (`verify_available`) the launcher for `agent`.
///
/// This is the single construct-and-probe table: the backend list exists in
/// exactly one place, shared by `edda conduct run` (which threads
/// `--verbose` and a transcript dir for claude) and `edda dispatch` (which
/// uses defaults plus codex thread persistence). pi has no transcript tee
/// capability yet, so the transcript dir is claude-only — see
/// [`AgentKind::writes_transcripts`].
pub(crate) fn build_launcher(
    agent: AgentKind,
    options: LauncherOptions,
) -> Result<Box<dyn AgentLauncher>> {
    Ok(match agent {
        AgentKind::Claude => {
            let mut launcher = ClaudeCodeLauncher::new().with_verbose(options.verbose);
            launcher.transcript_dir = options.transcript_dir;
            launcher.verify_available()?;
            Box::new(launcher)
        }
        AgentKind::Pi => {
            let mut launcher = PiRpcLauncher::new().with_verbose(options.verbose);
            if let Some(dir) = options.session_dir {
                launcher = launcher.with_session_dir(dir);
            }
            launcher.verify_available()?;
            Box::new(launcher)
        }
        AgentKind::Codex => {
            let mut launcher = CodexLauncher::new().with_verbose(options.verbose);
            if options.persistent_codex_threads {
                launcher = launcher.with_persistent_threads();
            }
            launcher.verify_available()?;
            Box::new(launcher)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_claude_writes_transcripts() {
        // Drives the --tmux fallback: an agent without transcripts would
        // otherwise get panes tailing files nobody writes.
        assert!(AgentKind::Claude.writes_transcripts());
        assert!(!AgentKind::Pi.writes_transcripts());
        assert!(!AgentKind::Codex.writes_transcripts());
        assert_eq!(AgentKind::Claude.as_str(), "claude");
        assert_eq!(AgentKind::Pi.as_str(), "pi");
        assert_eq!(AgentKind::Codex.as_str(), "codex");
    }

    // ── GH-574 backend × option support matrix ──

    fn options_for<'a>(
        model: Option<&'a str>,
        thinking: Option<&'a str>,
        tools: Option<&'a [String]>,
        exclude_tools: Option<&'a [String]>,
        session_dir: Option<&'a str>,
    ) -> DispatchOptions<'a> {
        DispatchOptions {
            model,
            thinking,
            tools,
            exclude_tools,
            session_dir,
        }
    }

    #[test]
    fn support_matrix_matches_per_backend_reality() {
        use AgentKind::*;
        // model: claude + pi
        assert!(Claude.supports_model());
        assert!(Pi.supports_model());
        assert!(!Codex.supports_model());
        // thinking: pi only
        assert!(!Claude.supports_thinking());
        assert!(Pi.supports_thinking());
        assert!(!Codex.supports_thinking());
        // tool policy: claude + pi
        assert!(Claude.supports_tool_policy());
        assert!(Pi.supports_tool_policy());
        assert!(!Codex.supports_tool_policy());
        // session-dir and model listing: pi only
        assert!(Pi.supports_session_dir());
        assert!(!Claude.supports_session_dir());
        assert!(!Codex.supports_session_dir());
        assert!(Pi.supports_model_listing());
        assert!(!Claude.supports_model_listing());
        assert!(!Codex.supports_model_listing());
    }

    #[test]
    fn validation_accepts_every_supported_combination() {
        let tools = vec!["read".to_string()];
        for (agent, opts) in [
            (
                AgentKind::Claude,
                options_for(Some("m"), None, Some(&tools), Some(&tools), None),
            ),
            (
                AgentKind::Pi,
                options_for(
                    Some("m"),
                    Some("high"),
                    Some(&tools),
                    Some(&tools),
                    Some("d"),
                ),
            ),
            (AgentKind::Codex, options_for(None, None, None, None, None)),
        ] {
            validate_dispatch_options(agent, &opts)
                .unwrap_or_else(|e| panic!("{agent:?} combination should pass: {e}"));
        }
    }

    #[test]
    fn validation_refuses_unsupported_combinations_explicitly() {
        let tools = vec!["read".to_string()];
        let codex_all = options_for(
            Some("anthropic/claude-opus-5"),
            Some("high"),
            Some(&tools),
            Some(&tools),
            Some("dir"),
        );
        let error = validate_dispatch_options(AgentKind::Codex, &codex_all)
            .expect_err("codex supports none of these");
        let text = error.to_string();
        for flag in [
            "--model",
            "--thinking",
            "--tools",
            "--exclude-tools",
            "--session-dir",
        ] {
            assert!(text.contains(flag), "must name {flag}: {text}");
        }
        assert!(text.contains("silently ignore"), "{text}");

        // claude: thinking and session-dir only.
        let claude_partial = options_for(Some("m"), Some("high"), None, None, Some("d"));
        let error = validate_dispatch_options(AgentKind::Claude, &claude_partial)
            .expect_err("claude refuses thinking + session-dir");
        let text = error.to_string();
        assert!(
            text.contains("--thinking") && text.contains("--session-dir"),
            "{text}"
        );
        assert!(
            !text.contains("--model"),
            "supported flag must not be refused: {text}"
        );
    }
}
