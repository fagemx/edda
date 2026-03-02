//! `edda user` subcommand group — user-level aggregation commands.

use clap::Subcommand;
use edda_aggregate::aggregate::{self, DateRange};
use edda_aggregate::rollup;
use edda_store::registry;

#[derive(Subcommand)]
pub enum UserCmd {
    /// List all registered projects
    Projects {
        /// Remove stale entries (projects whose .edda/ no longer exists)
        #[arg(long)]
        prune: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Cross-repo overview (event/commit/decision counts)
    Overview {
        /// Only include events after this date (ISO 8601 prefix)
        #[arg(long)]
        after: Option<String>,
        /// Only include events before this date
        #[arg(long)]
        before: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List recent commits across all projects
    Commits {
        /// Only include commits after this date
        #[arg(long)]
        after: Option<String>,
        /// Only include commits before this date
        #[arg(long)]
        before: Option<String>,
        /// Maximum number of commits to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List active decisions across all projects
    Decisions {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Compute/show rollup statistics
    Rollup {
        /// Tool name (default: claude-code)
        #[arg(long, default_value = "claude-code")]
        tool: String,
        /// Recompute from scratch instead of incremental
        #[arg(long)]
        refresh: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// User-level config operations
    Config {
        #[command(subcommand)]
        cmd: UserConfigCmd,
    },
}

#[derive(Subcommand)]
pub enum UserConfigCmd {
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// Set a config value
    Set {
        /// Config key
        key: String,
        /// Config value (JSON-compatible)
        value: String,
    },
    /// List all config values
    List,
}

pub fn execute(cmd: UserCmd) -> anyhow::Result<()> {
    match cmd {
        UserCmd::Projects { prune, json } => execute_projects(prune, json),
        UserCmd::Overview { after, before, json } => execute_overview(after, before, json),
        UserCmd::Commits {
            after,
            before,
            limit,
            json,
        } => execute_commits(after, before, limit, json),
        UserCmd::Decisions { json } => execute_decisions(json),
        UserCmd::Rollup { tool, refresh, json } => execute_rollup(&tool, refresh, json),
        UserCmd::Config { cmd } => execute_config(cmd),
    }
}

fn execute_projects(prune: bool, json: bool) -> anyhow::Result<()> {
    if prune {
        let (_valid, stale) = registry::validate_projects();
        if stale.is_empty() {
            println!("No stale projects found.");
        } else {
            for entry in &stale {
                registry::unregister_project(&entry.project_id)?;
                println!("  Removed: {} ({})", entry.name, entry.path);
            }
            println!("{} stale project(s) removed.", stale.len());
        }
        return Ok(());
    }

    let projects = registry::list_projects();
    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(());
    }

    if projects.is_empty() {
        println!("No registered projects. Run `edda init` in a repository to register it.");
        return Ok(());
    }

    println!("Registered projects ({}):\n", projects.len());
    for p in &projects {
        let (valid, _) = registry::validate_projects();
        let status = if valid.iter().any(|v| v.project_id == p.project_id) {
            "ok"
        } else {
            "stale"
        };
        println!(
            "  {} [{}] {}\n    path: {}\n    registered: {}\n    last_seen: {}\n",
            p.name, status, p.project_id, p.path, p.registered_at, p.last_seen
        );
    }

    Ok(())
}

fn execute_overview(after: Option<String>, before: Option<String>, json: bool) -> anyhow::Result<()> {
    let projects = registry::list_projects();
    let range = DateRange { after, before };
    let result = aggregate::aggregate_overview(&projects, &range);

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("Cross-repo overview:");
    println!("  Projects:  {}", result.projects.len());
    println!("  Events:    {}", result.total_events);
    println!("  Commits:   {}", result.total_commits);
    println!("  Decisions: {}", result.total_decisions);

    if !result.projects.is_empty() {
        println!("\nPer-project breakdown:");
        for p in &result.projects {
            println!(
                "  {} — {} events, {} commits, {} decisions",
                p.name, p.event_count, p.commit_count, p.decision_count
            );
        }
    }

    Ok(())
}

fn execute_commits(
    after: Option<String>,
    before: Option<String>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let projects = registry::list_projects();
    let range = DateRange { after, before };
    let commits = aggregate::aggregate_commits(&projects, &range, limit);

    if json {
        println!("{}", serde_json::to_string_pretty(&commits)?);
        return Ok(());
    }

    if commits.is_empty() {
        println!("No commits found.");
        return Ok(());
    }

    println!("Recent commits across all projects:\n");
    for c in &commits {
        println!(
            "  [{}] {} — {} ({})",
            c.ts, c.project_name, c.title, c.branch
        );
    }

    Ok(())
}

fn execute_decisions(json: bool) -> anyhow::Result<()> {
    let projects = registry::list_projects();
    let decisions = aggregate::aggregate_decisions(&projects);

    if json {
        println!("{}", serde_json::to_string_pretty(&decisions)?);
        return Ok(());
    }

    if decisions.is_empty() {
        println!("No active decisions found.");
        return Ok(());
    }

    println!("Active decisions across all projects:\n");
    for d in &decisions {
        println!(
            "  [{}] {}.{}={} — {}",
            d.project_name, d.domain, d.key, d.value, d.reason
        );
    }

    Ok(())
}

fn execute_rollup(tool: &str, refresh: bool, json: bool) -> anyhow::Result<()> {
    let projects = registry::list_projects();

    let rollup_data = if refresh {
        let range = DateRange::default();
        let r = rollup::compute_rollup(&projects, &range, tool);
        rollup::save_rollup(&r)?;
        r
    } else {
        rollup::compute_rollup_incremental(&projects, tool)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&rollup_data)?);
        return Ok(());
    }

    println!("Rollup for '{}':", tool);
    println!("  Last updated: {}", rollup_data.last_updated);
    println!("  Daily entries: {}", rollup_data.daily.len());
    println!("  Weekly entries: {}", rollup_data.weekly.len());
    println!("  Monthly entries: {}", rollup_data.monthly.len());

    if !rollup_data.monthly.is_empty() {
        println!("\nMonthly summary:");
        for m in &rollup_data.monthly {
            println!("  {} — {} events, {} commits", m.month, m.events, m.commits);
        }
    }

    Ok(())
}

fn execute_config(cmd: UserConfigCmd) -> anyhow::Result<()> {
    use edda_store::user_config;

    match cmd {
        UserConfigCmd::Get { key } => {
            match user_config::get_user_config(&key) {
                Some(val) => println!("{}", serde_json::to_string_pretty(&val)?),
                None => println!("(not set)"),
            }
            Ok(())
        }
        UserConfigCmd::Set { key, value } => {
            let parsed: serde_json::Value = serde_json::from_str(&value)
                .unwrap_or_else(|_| serde_json::Value::String(value));
            user_config::set_user_config(&key, parsed)?;
            println!("Set {key}");
            Ok(())
        }
        UserConfigCmd::List => {
            let config = user_config::load_user_config();
            if config.is_empty() {
                println!("(no user-level config set)");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::Value::Object(config))?
                );
            }
            Ok(())
        }
    }
}
