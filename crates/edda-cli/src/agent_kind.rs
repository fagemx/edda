//! Shared agent-backend selection, used by both `edda conduct` (plan runs)
//! and `edda dispatch` (single-turn runs). The enum, its string form, the
//! transcript capability flag, and the launcher construct-and-probe factory
//! live here so neither command duplicates the other's backend list.

use anyhow::Result;
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
}

// ── Launcher factory ──

/// Per-backend options for [`build_launcher`]: verbose streaming, the
/// transcript directory for the backend that tees transcripts, and whether
/// codex thread persistence is enabled.
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
            let launcher = PiRpcLauncher::new().with_verbose(options.verbose);
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
}
