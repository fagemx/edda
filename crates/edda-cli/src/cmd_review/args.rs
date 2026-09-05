use crate::agent_kind::AgentKind;
use clap::Args;

#[derive(Debug, Args)]
pub struct ReviewArgs {
    /// Comparison base; defaults to origin/HEAD, origin/main/master, main/master
    #[arg(long)]
    pub base: Option<String>,
    /// Committed subject to review
    #[arg(long, default_value = "HEAD")]
    pub head: String,
    /// Resolve head, base and closing issue from this GitHub PR
    #[arg(long)]
    pub pr: Option<u64>,
    /// Acceptance specification: path or #issue
    #[arg(long)]
    pub spec: Option<String>,
    /// Trust an issue's verify commands for opt-in gate execution
    #[arg(long)]
    pub trust_spec: bool,
    /// Declare a trusted gate command (repeatable)
    #[arg(long = "gate")]
    pub gates: Vec<String>,
    /// Require verifiably different author and reviewer models
    #[arg(long)]
    pub require_model_diversity: bool,
    #[arg(long, value_enum, default_value = "pi")]
    pub agent: AgentKind,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub thinking: Option<String>,
    /// Reviewer UUID; an author session is refused
    #[arg(long)]
    pub session_id: Option<String>,
    /// Continue the prior ledger-recorded reviewer conversation
    #[arg(long)]
    pub resume: bool,
    #[arg(long, default_value_t = 900)]
    pub timeout_sec: u64,
    #[arg(long)]
    pub budget_usd: Option<f64>,
    /// Execute declared trusted gates; otherwise read existing evidence only
    #[arg(long)]
    pub run_gates: bool,
    #[arg(long, default_value_t = 300)]
    pub max_ran_sec: u64,
    #[arg(long)]
    pub keep_worktree: bool,
    /// Print the unstable review_verdict/0 payload plus event_id
    #[arg(long)]
    pub json: bool,
}

#[cfg(test)]
impl Default for ReviewArgs {
    fn default() -> Self {
        Self {
            base: None,
            head: "HEAD".into(),
            pr: None,
            spec: None,
            trust_spec: false,
            gates: vec![],
            require_model_diversity: false,
            agent: AgentKind::Pi,
            model: None,
            thinking: None,
            session_id: None,
            resume: false,
            timeout_sec: 900,
            budget_usd: None,
            run_gates: false,
            max_ran_sec: 300,
            keep_worktree: false,
            json: false,
        }
    }
}
