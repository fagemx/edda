//! Tool tier governance policy — T0..T4 classification and query.
//!
//! Defines tool risk tiers and approval requirements. The runtime source of
//! truth is `tool_tiers.yaml` in the `.edda/` directory; changes are also
//! recorded as decision events in the ledger for audit.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

// ── ToolTier enum ──

/// Risk tier for a tool (T0 = safest, T4 = forbidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ToolTier {
    T0,
    T1,
    T2,
    T3,
    T4,
}

impl fmt::Display for ToolTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolTier::T0 => write!(f, "T0"),
            ToolTier::T1 => write!(f, "T1"),
            ToolTier::T2 => write!(f, "T2"),
            ToolTier::T3 => write!(f, "T3"),
            ToolTier::T4 => write!(f, "T4"),
        }
    }
}

impl FromStr for ToolTier {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "T0" => Ok(ToolTier::T0),
            "T1" => Ok(ToolTier::T1),
            "T2" => Ok(ToolTier::T2),
            "T3" => Ok(ToolTier::T3),
            "T4" => Ok(ToolTier::T4),
            other => anyhow::bail!("invalid tool tier: '{other}' (expected T0..T4)"),
        }
    }
}

// ── Approval requirement ──

/// What approval is needed before executing a tool at this tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalRequirement {
    /// No approval needed.
    None,
    /// Execute immediately but log for post-review.
    Lazy,
    /// Must get explicit approval before execution.
    Required,
    /// Cannot execute under any circumstance.
    Blocked,
}

impl fmt::Display for ApprovalRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApprovalRequirement::None => write!(f, "none"),
            ApprovalRequirement::Lazy => write!(f, "lazy"),
            ApprovalRequirement::Required => write!(f, "required"),
            ApprovalRequirement::Blocked => write!(f, "blocked"),
        }
    }
}

// ── Tier definition ──

/// Metadata for a single tier level (stored in YAML).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierDef {
    pub description: String,
    pub approval: ApprovalRequirement,
}

// ── Config (loaded from tool_tiers.yaml) ──

/// Full tool-tier policy loaded from `.edda/tool_tiers.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTierConfig {
    pub version: u32,
    pub default_tier: ToolTier,
    pub tiers: BTreeMap<ToolTier, TierDef>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolTier>,
}

// ── Query result ──

/// Result of resolving a tool's tier — returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTierResult {
    pub tool: String,
    pub tier: ToolTier,
    pub approval: ApprovalRequirement,
    pub description: String,
}

// ── Entry for list output ──

/// A single tool -> tier entry for list display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTierEntry {
    pub tool: String,
    pub tier: ToolTier,
    pub approval: ApprovalRequirement,
}

// ── Default config ──

/// Build a sensible default `ToolTierConfig` (no tools mapped yet).
pub fn default_tool_tier_config() -> ToolTierConfig {
    let mut tiers = BTreeMap::new();
    tiers.insert(
        ToolTier::T0,
        TierDef {
            description: "Safe \u{2014} read-only, no side effects".to_string(),
            approval: ApprovalRequirement::None,
        },
    );
    tiers.insert(
        ToolTier::T1,
        TierDef {
            description: "Standard \u{2014} normal operations".to_string(),
            approval: ApprovalRequirement::None,
        },
    );
    tiers.insert(
        ToolTier::T2,
        TierDef {
            description: "Elevated \u{2014} significant changes".to_string(),
            approval: ApprovalRequirement::Lazy,
        },
    );
    tiers.insert(
        ToolTier::T3,
        TierDef {
            description: "Dangerous \u{2014} destructive or irreversible".to_string(),
            approval: ApprovalRequirement::Required,
        },
    );
    tiers.insert(
        ToolTier::T4,
        TierDef {
            description: "Forbidden \u{2014} never allowed".to_string(),
            approval: ApprovalRequirement::Blocked,
        },
    );

    ToolTierConfig {
        version: 1,
        default_tier: ToolTier::T1,
        tiers,
        tools: BTreeMap::new(),
    }
}

// ── Query logic ──

/// Resolve a tool's tier from the config, falling back to `default_tier`.
pub fn resolve_tool_tier(config: &ToolTierConfig, tool_name: &str) -> ToolTierResult {
    let tier = config
        .tools
        .get(tool_name)
        .copied()
        .unwrap_or(config.default_tier);

    let tier_def = config.tiers.get(&tier);
    let (approval, description) = match tier_def {
        Some(def) => (def.approval, def.description.clone()),
        // Fallback for missing tier definition (shouldn't happen with defaults)
        None => (approval_for_tier(tier), format!("Tier {tier}")),
    };

    ToolTierResult {
        tool: tool_name.to_string(),
        tier,
        approval,
        description,
    }
}

/// Derive the canonical approval requirement from a tier level.
pub fn approval_for_tier(tier: ToolTier) -> ApprovalRequirement {
    match tier {
        ToolTier::T0 | ToolTier::T1 => ApprovalRequirement::None,
        ToolTier::T2 => ApprovalRequirement::Lazy,
        ToolTier::T3 => ApprovalRequirement::Required,
        ToolTier::T4 => ApprovalRequirement::Blocked,
    }
}

// ── YAML file I/O ──

const TOOL_TIERS_FILE: &str = "tool_tiers.yaml";

/// Load `tool_tiers.yaml` from the `.edda/` directory.
/// Returns a default config if the file doesn't exist.
pub fn load_tool_tiers_from_dir(edda_dir: &Path) -> anyhow::Result<ToolTierConfig> {
    let path = edda_dir.join(TOOL_TIERS_FILE);
    if !path.exists() {
        return Ok(default_tool_tier_config());
    }
    let content = std::fs::read(&path)?;
    let config: ToolTierConfig = serde_yaml::from_slice(&content)?;
    Ok(config)
}

/// Save `tool_tiers.yaml` to the `.edda/` directory.
pub fn save_tool_tiers_to_dir(edda_dir: &Path, config: &ToolTierConfig) -> anyhow::Result<()> {
    let path = edda_dir.join(TOOL_TIERS_FILE);
    let yaml = serde_yaml::to_string(config)?;
    std::fs::write(&path, yaml.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_tool() {
        let mut config = default_tool_tier_config();
        config.tools.insert("bash".to_string(), ToolTier::T1);
        config.tools.insert("rm".to_string(), ToolTier::T3);

        let result = resolve_tool_tier(&config, "bash");
        assert_eq!(result.tier, ToolTier::T1);
        assert_eq!(result.approval, ApprovalRequirement::None);

        let result = resolve_tool_tier(&config, "rm");
        assert_eq!(result.tier, ToolTier::T3);
        assert_eq!(result.approval, ApprovalRequirement::Required);
    }

    #[test]
    fn resolve_unknown_tool_uses_default() {
        let config = default_tool_tier_config();
        let result = resolve_tool_tier(&config, "unknown_tool");
        assert_eq!(result.tier, ToolTier::T1); // default_tier
        assert_eq!(result.approval, ApprovalRequirement::None);
        assert_eq!(result.tool, "unknown_tool");
    }

    #[test]
    fn all_tiers_have_correct_approval() {
        assert_eq!(approval_for_tier(ToolTier::T0), ApprovalRequirement::None);
        assert_eq!(approval_for_tier(ToolTier::T1), ApprovalRequirement::None);
        assert_eq!(approval_for_tier(ToolTier::T2), ApprovalRequirement::Lazy);
        assert_eq!(
            approval_for_tier(ToolTier::T3),
            ApprovalRequirement::Required
        );
        assert_eq!(
            approval_for_tier(ToolTier::T4),
            ApprovalRequirement::Blocked
        );
    }

    #[test]
    fn serde_roundtrip() {
        let mut config = default_tool_tier_config();
        config.tools.insert("bash".to_string(), ToolTier::T1);
        config.tools.insert("Write".to_string(), ToolTier::T2);

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ToolTierConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.default_tier, ToolTier::T1);
        assert_eq!(parsed.tools.get("bash"), Some(&ToolTier::T1));
        assert_eq!(parsed.tools.get("Write"), Some(&ToolTier::T2));
        assert_eq!(parsed.tiers.len(), 5);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let tmp = std::env::temp_dir().join(format!(
            "edda_tool_tier_missing_test_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let config = load_tool_tiers_from_dir(&tmp).unwrap();
        assert_eq!(config.default_tier, ToolTier::T1);
        assert!(config.tools.is_empty());
        assert_eq!(config.tiers.len(), 5);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp =
            std::env::temp_dir().join(format!("edda_tool_tier_io_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut config = default_tool_tier_config();
        config.tools.insert("curl".to_string(), ToolTier::T2);

        save_tool_tiers_to_dir(&tmp, &config).unwrap();
        let loaded = load_tool_tiers_from_dir(&tmp).unwrap();

        assert_eq!(loaded.tools.get("curl"), Some(&ToolTier::T2));
        assert_eq!(loaded.default_tier, ToolTier::T1);
        assert_eq!(loaded.tiers.len(), 5);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tool_tier_display_and_parse() {
        for tier in [
            ToolTier::T0,
            ToolTier::T1,
            ToolTier::T2,
            ToolTier::T3,
            ToolTier::T4,
        ] {
            let s = tier.to_string();
            let parsed: ToolTier = s.parse().unwrap();
            assert_eq!(parsed, tier);
        }
        // case-insensitive
        assert_eq!("t2".parse::<ToolTier>().unwrap(), ToolTier::T2);
        // invalid
        assert!("T5".parse::<ToolTier>().is_err());
    }

    #[test]
    fn resolve_with_t4_forbidden() {
        let mut config = default_tool_tier_config();
        config
            .tools
            .insert("git_push_force".to_string(), ToolTier::T4);

        let result = resolve_tool_tier(&config, "git_push_force");
        assert_eq!(result.tier, ToolTier::T4);
        assert_eq!(result.approval, ApprovalRequirement::Blocked);
    }
}
