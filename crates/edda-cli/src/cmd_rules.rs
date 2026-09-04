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
    /// Revoke a rule by ID (marks it Dead with a reason)
    Revoke {
        /// Rule ID (rule_*)
        id: String,
        /// Why the rule is being revoked
        #[arg(long)]
        reason: String,
    },
}

pub fn execute(cmd: RulesCmd, repo_root: &Path) -> anyhow::Result<()> {
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
                    "{:<24} {:<12} {:<10} {:<5} {:<5} {:<5} TRIGGER → ACTION",
                    "ID", "STATUS", "CATEGORY", "HITS", "SHOWS", "TTL"
                );
                println!("{}", "-".repeat(88));
                for rule in &rules {
                    let revoked = rule
                        .revoked_reason
                        .as_deref()
                        .map(|r| format!("  [revoked: {r}]"))
                        .unwrap_or_default();
                    println!(
                        "{:<24} {:<12} {:<10} {:<5} {:<5} {:<5} {} → {}{}",
                        rule.id,
                        rule.status,
                        rule.category,
                        rule.hits,
                        rule.shows,
                        rule.ttl_days,
                        rule.trigger,
                        rule.action,
                        revoked,
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
            println!("  Active: {} → {}", before.active, after.active);
            println!("  Dormant: {} → {}", before.dormant, after.dormant);
            println!("  Dead: {} → {}", before.dead, after.dead);
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

        RulesCmd::Revoke { id, reason } => {
            let mut store = edda_postmortem::RulesStore::load_project(&project_id);
            if store.revoke_rule(&id, reason.clone()) {
                store.save_project(&project_id)?;
                println!("Rule {id} revoked: {reason}");
            } else {
                anyhow::bail!("Rule not found: {id}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::isolated_store;
    use edda_postmortem::{RuleCategory, RuleStatus, RulesStore};

    /// Seed a project rules store with one live and one dead rule, pointed
    /// at the isolated store root. Returns the live rule's ID.
    fn seed_store(repo_root: &Path) -> String {
        let project_id = edda_store::project_id(repo_root);
        let mut store = RulesStore::default();
        let live_id = store.propose_rule(
            "command_failure:npm".to_string(),
            "Check npm install output".to_string(),
            None,
            RuleCategory::Workflow,
            "test-session".to_string(),
            None,
        );
        let dead_id = store.propose_rule(
            "command_failure:legacy".to_string(),
            "Legacy rule".to_string(),
            None,
            RuleCategory::Workflow,
            "test-session".to_string(),
            None,
        );
        assert!(store.revoke_rule(&dead_id, "seed revoke".to_string()));
        store
            .save(&RulesStore::project_rules_path(&project_id))
            .expect("seed rules.json");
        live_id
    }

    fn load_store(repo_root: &Path) -> RulesStore {
        RulesStore::load_project(&edda_store::project_id(repo_root))
    }

    #[test]
    fn revoke_marks_rule_dead_with_reason() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        let live_id = seed_store(repo.path());

        execute(
            RulesCmd::Revoke {
                id: live_id.clone(),
                reason: "operator revocation".to_string(),
            },
            repo.path(),
        )
        .expect("revoke should succeed");

        let store = load_store(repo.path());
        let rule = store.get(&live_id).expect("rule still present");
        assert_eq!(rule.status, RuleStatus::Dead);
        assert_eq!(rule.revoked_reason.as_deref(), Some("operator revocation"));
    }

    #[test]
    fn revoke_unknown_id_errors() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        seed_store(repo.path());

        let result = execute(
            RulesCmd::Revoke {
                id: "rule_does_not_exist".to_string(),
                reason: "nope".to_string(),
            },
            repo.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn list_reports_rules_without_error() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        let live_id = seed_store(repo.path());

        // Human-readable listing (exercises the ID/SHOWS columns)...
        execute(
            RulesCmd::List {
                all: true,
                json: false,
            },
            repo.path(),
        )
        .expect("list should succeed");
        // ...and JSON listing.
        execute(
            RulesCmd::List {
                all: false,
                json: true,
            },
            repo.path(),
        )
        .expect("json list should succeed");

        // The store itself is untouched by listing.
        let store = load_store(repo.path());
        assert!(store.get(&live_id).is_some());
    }

    #[test]
    fn list_empty_store_is_ok() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        execute(
            RulesCmd::List {
                all: false,
                json: false,
            },
            repo.path(),
        )
        .expect("list on empty store should succeed");
    }

    #[test]
    fn gc_removes_only_dead_rules() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        let live_id = seed_store(repo.path());

        execute(RulesCmd::Gc, repo.path()).expect("gc should succeed");

        let store = load_store(repo.path());
        assert_eq!(store.rules.len(), 1, "only the dead rule is removed");
        assert_eq!(store.rules[0].id, live_id);
        assert_eq!(store.rules[0].status, RuleStatus::Proposed);
    }

    #[test]
    fn decay_persists_after_execution() {
        let _store = isolated_store();
        let repo = tempfile::tempdir().expect("repo tempdir");
        seed_store(repo.path());

        execute(RulesCmd::Decay, repo.path()).expect("decay should succeed");

        let store = load_store(repo.path());
        assert!(store.last_decay_run.is_some(), "decay run recorded");
    }
}
