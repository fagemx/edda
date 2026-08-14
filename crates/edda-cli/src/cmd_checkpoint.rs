use clap::Args;
use edda_core::event::{new_checkpoint_event, CheckpointPayload, RejectedHypothesis};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

#[derive(Args, Debug)]
pub struct CheckpointArgs {
    /// Current hypotheses (repeatable)
    #[arg(long = "hypothesis")]
    pub hypotheses: Vec<String>,
    /// Rejected hypothesis and reason, separated by `|` (repeatable)
    #[arg(long = "rejected", value_name = "HYPOTHESIS|REASON")]
    pub rejected: Vec<String>,
    /// Open questions (repeatable)
    #[arg(long = "open")]
    pub open: Vec<String>,
    /// Next checkpoint action
    #[arg(long)]
    pub next: String,
    /// Author role
    #[arg(long, default_value = "agent")]
    pub role: String,
}

pub fn execute(repo_root: &Path, args: CheckpointArgs) -> anyhow::Result<()> {
    let rejected = args
        .rejected
        .into_iter()
        .map(|value| {
            let (hypothesis, reason) = value
                .split_once('|')
                .ok_or_else(|| anyhow::anyhow!("--rejected must use HYPOTHESIS|REASON"))?;
            if hypothesis.trim().is_empty() || reason.trim().is_empty() {
                anyhow::bail!("--rejected requires both a hypothesis and a reason");
            }
            Ok(RejectedHypothesis {
                hypothesis: hypothesis.trim().to_string(),
                reason: reason.trim().to_string(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let checkpoint = CheckpointPayload {
        hypotheses: args.hypotheses,
        rejected,
        open: args.open,
        next: args.next,
    };
    let event = new_checkpoint_event(&branch, parent_hash.as_deref(), &args.role, &checkpoint)?;
    ledger.append_event(&event)?;

    println!("Wrote CHECKPOINT {}", event.event_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejected_value_uses_one_separator() {
        let (hypothesis, reason) = "db corruption|integrity check passes"
            .split_once('|')
            .unwrap();
        assert_eq!(hypothesis, "db corruption");
        assert_eq!(reason, "integrity check passes");
    }
}
