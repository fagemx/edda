//! Per-target ACP spawn table for the GH-800 task runner.
//!
//! One row per ACP agent the rail can drive. An entry carries only what the
//! runner needs at spawn time: the endpoint program and arguments, plus —
//! where the target documents one — a read-only argument set used when the
//! lane runs as verifier. Targets without a documented read-only switch get
//! none here and never a guessed flag: the in-band
//! [`crate::runner::acp::AcpPermissionPolicy`] remains the enforcement
//! boundary, so protocol-level denial does not depend on spawn flags.
//!
//! Evidence state per entry (no success is claimed from `--help` alone):
//!
//! | Target | Endpoint | Verified |
//! |---|---|---|
//! | grok | `grok agent stdio` | help + flag surface probed 2026-09-04 on this workstation (`--sandbox <PROFILE>`, `--deny`, `--disallowed-tools` all present); `grok agent stdio --help` succeeds |
//! | kilo | `kilo acp` | not on PATH (`where kilo` fails); kept for package-local drill attempts |
//! | pi | `pi-acp` | not on PATH (`where pi-acp` fails; unrelated `pi` launcher is not the ACP entry) |
//! | claude | `npx --yes @agentclientprotocol/claude-agent-acp` | TASK_RAIL_V1 §4.1 registry; npx probes on this workstation produced no output within 60 s (unsupported evidence, not a drill) |

use crate::runner::acp::AcpEndpoint;

/// ACP targets the runner knows how to spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpTarget {
    /// Grok Build (`grok agent stdio`, upstream `agent-client-protocol` 0.10.4).
    Grok,
    /// Kilo Code CLI (OpenCode-derived `kilo acp`).
    Kilo,
    /// pi (`pi-acp`).
    Pi,
    /// Claude Code via the ACP adapter (`@agentclientprotocol/claude-agent-acp`).
    Claude,
}

impl AcpTarget {
    /// Parse a target key from `task.created` `agent_kind` suffixes such as
    /// `grok-acp` / `acp:grok`. The suffix is lowercase and ASCII.
    pub fn parse(key: &str) -> Option<Self> {
        let key = key
            .strip_suffix("-acp")
            .or_else(|| key.strip_prefix("acp:"))
            .unwrap_or(key);
        match key {
            "grok" => Some(Self::Grok),
            "kilo" => Some(Self::Kilo),
            "pi" => Some(Self::Pi),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }

    /// Canonical key used in task receipts and drill transcripts.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Kilo => "kilo",
            Self::Pi => "pi",
            Self::Claude => "claude",
        }
    }

    /// Spawn endpoint for this target. `verifier` launches documented
    /// read-only flags in addition to the in-band policy; a verifier lane
    /// must never rely on the target's goodwill alone.
    pub fn endpoint(&self, verifier: bool) -> AcpEndpoint {
        let (program, mut args): (PathBuf, Vec<String>) = match self {
            Self::Grok => (
                PathBuf::from("grok"),
                ["agent", "stdio"].into_iter().map(String::from).collect(),
            ),
            Self::Kilo => (PathBuf::from("kilo"), vec!["acp".to_string()]),
            Self::Pi => (PathBuf::from("pi-acp"), Vec::new()),
            Self::Claude => (
                PathBuf::from("npx"),
                ["--yes", "@agentclientprotocol/claude-agent-acp"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
        };
        if verifier {
            // Read-only flags go before the subcommand: grok parses global
            // options ahead of `[COMMAND]` (`grok [OPTIONS] [PROMPT]
            // [COMMAND]`).
            let readonly = self.readonly_args();
            let mut prefixed: Vec<String> =
                readonly.iter().map(|flag| (*flag).to_string()).collect();
            prefixed.append(&mut args);
            args = prefixed;
        }
        AcpEndpoint { program, args }
    }

    /// Documented read-only switch, if the target ships one. Grok exposes a
    /// global `--sandbox <PROFILE>` on this workstation's v1.0.13 binary;
    /// the other targets have no documented ACP-layer read-only flag, so the
    /// protocol policy is the only gate for them.
    fn readonly_args(&self) -> &'static [&'static str] {
        match self {
            Self::Grok => &["--sandbox", "read-only"],
            Self::Kilo | Self::Pi | Self::Claude => &[],
        }
    }
}

use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agent_kind_suffixes_and_prefixes() {
        assert_eq!(AcpTarget::parse("grok-acp"), Some(AcpTarget::Grok));
        assert_eq!(AcpTarget::parse("acp:kilo"), Some(AcpTarget::Kilo));
        assert_eq!(AcpTarget::parse("claude"), Some(AcpTarget::Claude));
        assert_eq!(AcpTarget::parse("codex-acp"), None);
        assert_eq!(AcpTarget::parse(""), None);
    }

    #[test]
    fn grok_worker_endpoint_is_bare_subcommand() {
        let endpoint = AcpTarget::Grok.endpoint(false);
        assert_eq!(endpoint.program, PathBuf::from("grok"));
        assert_eq!(endpoint.args, vec!["agent", "stdio"]);
    }

    #[test]
    fn grok_verifier_endpoint_prepends_read_only_sandbox() {
        let endpoint = AcpTarget::Grok.endpoint(true);
        assert_eq!(
            endpoint.args,
            vec!["--sandbox", "read-only", "agent", "stdio"]
        );
    }

    #[test]
    fn targets_without_documented_readonly_flags_get_none() {
        // No invented flags: policy enforcement is in-band for these.
        assert!(AcpTarget::Kilo.endpoint(true).args == vec!["acp".to_string()]);
        assert!(AcpTarget::Pi.endpoint(true).args.is_empty());
        let claude = AcpTarget::Claude.endpoint(true);
        assert_eq!(
            claude.args,
            vec![
                "--yes".to_string(),
                "@agentclientprotocol/claude-agent-acp".to_string()
            ]
        );
    }

    #[test]
    fn keys_round_trip() {
        for target in [
            AcpTarget::Grok,
            AcpTarget::Kilo,
            AcpTarget::Pi,
            AcpTarget::Claude,
        ] {
            assert_eq!(AcpTarget::parse(target.key()), Some(target));
        }
    }
}
