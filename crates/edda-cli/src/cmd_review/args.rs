use crate::agent_kind::AgentKind;
use clap::Args;
use std::path::PathBuf;

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
    /// Explicit untrusted supporting context (UTF-8, maximum 32 KiB)
    #[arg(long)]
    pub context_file: Option<PathBuf>,
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
            context_file: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        review: ReviewArgs,
    }

    #[test]
    fn context_file_is_optional_and_preserves_the_selected_path() {
        let absent = Harness::try_parse_from(["test"]).expect("legacy args");
        assert!(absent.review.context_file.is_none());
        let present = Harness::try_parse_from(["test", "--context-file", "facts/review.md"])
            .expect("context args");
        assert_eq!(
            present.review.context_file.as_deref(),
            Some(std::path::Path::new("facts/review.md"))
        );
    }
}
