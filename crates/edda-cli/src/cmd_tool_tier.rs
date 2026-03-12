use clap::Subcommand;
use edda_core::tool_tier::{
    default_tool_tier_config, load_tool_tiers_from_dir, resolve_tool_tier, save_tool_tiers_to_dir,
    ToolTier, ToolTierEntry,
};
use std::path::Path;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum ToolTierCmd {
    /// Query a tool's tier and approval requirement
    Get {
        /// Tool name (e.g. "bash", "Write", "rm")
        tool: String,
    },
    /// Set a tool's tier (also records a decision event)
    Set {
        /// Tool name
        tool: String,
        /// Tier level (T0..T4)
        tier: ToolTier,
        /// Reason for this classification
        #[arg(long)]
        reason: Option<String>,
    },
    /// List all tool-to-tier mappings
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate default tool_tiers.yaml
    Init,
}

pub fn run(cmd: ToolTierCmd, repo_root: &Path) -> anyhow::Result<()> {
    let edda_dir = repo_root.join(".edda");

    match cmd {
        ToolTierCmd::Get { tool } => {
            let config = load_tool_tiers_from_dir(&edda_dir)?;
            let result = resolve_tool_tier(&config, &tool);
            let json = serde_json::to_string_pretty(&result)?;
            println!("{json}");
        }
        ToolTierCmd::Set {
            tool,
            tier,
            reason,
        } => {
            // Load, update, save
            let mut config = load_tool_tiers_from_dir(&edda_dir)?;
            config.tools.insert(tool.clone(), tier);
            save_tool_tiers_to_dir(&edda_dir, &config)?;

            // Record as decision event for audit trail
            let decision_str = format!("tool_governance.{tool}={tier}");
            let reason_str = reason.unwrap_or_else(|| format!("set tool tier: {tool} -> {tier}"));

            // Use the same decide path as `edda decide`
            super::cmd_bridge::decide(repo_root, &decision_str, Some(&reason_str), &[], None)?;

            println!("Set {tool} = {tier}");
        }
        ToolTierCmd::List { json } => {
            let config = load_tool_tiers_from_dir(&edda_dir)?;
            let entries: Vec<ToolTierEntry> = config
                .tools
                .iter()
                .map(|(tool, &tier)| {
                    let result = resolve_tool_tier(&config, tool);
                    ToolTierEntry {
                        tool: tool.clone(),
                        tier,
                        approval: result.approval,
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("No tool tier mappings configured.");
                println!(
                    "Default tier: {} (applied to all unmapped tools)",
                    config.default_tier
                );
            } else {
                println!("{:<20} {:<6} APPROVAL", "TOOL", "TIER");
                println!("{}", "-".repeat(44));
                for entry in &entries {
                    println!("{:<20} {:<6} {}", entry.tool, entry.tier, entry.approval);
                }
                println!();
                println!("Default tier: {}", config.default_tier);
            }
        }
        ToolTierCmd::Init => {
            let path = edda_dir.join("tool_tiers.yaml");
            if path.exists() {
                println!("tool_tiers.yaml already exists at {}", path.display());
            } else {
                let config = default_tool_tier_config();
                save_tool_tiers_to_dir(&edda_dir, &config)?;
                println!("Generated {}", path.display());
            }
        }
    }

    Ok(())
}
