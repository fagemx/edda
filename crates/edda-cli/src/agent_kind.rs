//! Shared agent-backend selection, used by both `edda conduct` (plan runs)
//! and `edda dispatch` (single-turn runs). The enum, its string form, and
//! the transcript capability flag live here so neither command duplicates
//! the other's backend list.

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
