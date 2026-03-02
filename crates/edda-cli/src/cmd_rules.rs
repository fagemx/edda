//! CLI subcommand: `edda rules` — manage learned rules from L3 post-mortem.

use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum RulesCmd {
    /// List all rules (default: alive only)
    List {
        /// Show all rules including dead/superseded
        #[arg(long)]
        all: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show a specific rule by ID
    Show {
        /// Rule ID (rule_*)
        id: String,
    },
    /// Run decay cycle on all rules
    Decay,
    /// Show rules store statistics
    Stats,
    /// Garbage-collect dead rules
    Gc,
}

pub fn run(cmd: RulesCmd, repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    match cmd {
        RulesCmd::List { all, json } => {
            let store = edda_postmortem::RulesStore::load_project(&project_id);
            let rules: Vec<_> = if all {
                store.rules.iter().collect()
            } else {
                store.alive_rules()
            };

            if rules.is_empty() {
                if !json {
                    println!("No rules found.");
                }
                return Ok(());
            }

            if json {
                for rule in &rules {
                    println!("{}", serde_json::to_string(rule)?);
                }
            } else {
                println!(
                    "{:<12} {:<10} {:<12} {:<5} {}",
                    "STATUS", "CATEGORY", "HITS", "TTL", "TRIGGER → ACTION"
                );
                println!("{}", "-".repeat(70));
                for rule in &rules {
                    println!(
                        "{:<12} {:<10} {:<12} {:<5} {} → {}",
                        rule.status,
                        rule.category,
                        rule.hits,
                        rule.ttl_days,
                        rule.trigger,
                        rule.action,
                    );
                }
                println!("\n{} rules shown.", rules.len());
            }
        }

        RulesCmd::Show { id } => {
            let store = edda_postmortem::RulesStore::load_project(&project_id);
            match store.get(&id) {
                Some(rule) => {
                    println!("{}", serde_json::to_string_pretty(rule)?);
                }
                None => {
                    anyhow::bail!("Rule not found: {id}");
                }
            }
        }

        RulesCmd::Decay => {
            let mut store = edda_postmortem::RulesStore::load_project(&project_id);
            let before = store.stats();
            store.run_decay_cycle();
            let after = store.stats();
            store.save_project(&project_id)?;
            println!("Decay cycle complete.");
            println!(
                "  Active: {} → {}",
                before.active, after.active
            );
            println!(
                "  Dormant: {} → {}",
                before.dormant, after.dormant
            );
            println!(
                "  Dead: {} → {}",
                before.dead, after.dead
            );
        }

        RulesCmd::Stats => {
            let store = edda_postmortem::RulesStore::load_project(&project_id);
            let stats = store.stats();
            println!("Rules store statistics:");
            println!("  Total:      {}", stats.total);
            println!("  Proposed:   {}", stats.proposed);
            println!("  Active:     {}", stats.active);
            println!("  Dormant:    {}", stats.dormant);
            println!("  Settled:    {}", stats.settled);
            println!("  Dead:       {}", stats.dead);
            println!("  Superseded: {}", stats.superseded);
            if let Some(ref last) = store.last_decay_run {
                println!("  Last decay: {}", last);
            }
        }

        RulesCmd::Gc => {
            let mut store = edda_postmortem::RulesStore::load_project(&project_id);
            let removed = store.gc_dead_rules();
            store.save_project(&project_id)?;
            println!("Removed {removed} dead rules.");
        }
    }

    Ok(())
}
