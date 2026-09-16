//! Machine-local `node.json` configuration (`<store root>/node.json`).
//!
//! The file is the same carrier convention as `~/.edda/config.json`: plain
//! JSON, `deny_unknown_fields`, no new config dependency. A missing file means
//! the node is not configured — not a default that silently opens a door.
//!
//! Only the **name** of an environment variable holding a shared token ever
//! appears here. A literal `token` value is accepted for local test rigs only;
//! token values are never printed, logged, or carried on the wire.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// `node.json` top-level document.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeConfig {
    pub version: u32,
    pub node: NodeSection,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
}

/// This machine's own node identity and listener.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSection {
    /// This machine's `<machine>` label, `^[a-z0-9._-]{1,64}$`.
    pub alias: String,
    /// The Tailscale `100.x` IPv4 address to bind.
    pub bind: String,
    pub port: u16,
}

/// One peer node.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerConfig {
    pub alias: String,
    pub host: String,
    pub port: u16,
    /// Name of the environment variable holding the shared token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    /// Literal token. Local test rigs only; never used for the real two-machine
    /// proof, and never printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// `<store root>/node.json`.
pub fn node_config_path() -> PathBuf {
    edda_store::store_root().join("node.json")
}

/// Parse `path` as a [`NodeConfig`]. An unknown key fails closed with the key
/// named by serde. Semantic validation is separate ([`validate_config`]) so a
/// local test rig can opt into a non-tailnet bind explicitly.
pub fn load_node_config(path: &Path) -> Result<NodeConfig> {
    let bytes = std::fs::read(path).with_context(|| {
        format!(
            "read node config {} (run `edda node` setup first)",
            path.display()
        )
    })?;
    let config: NodeConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse node config {}", path.display()))?;
    Ok(config)
}

/// Validate a config with the production bind policy: the bind address and
/// every peer host must be a Tailscale `100.x` IPv4 address.
pub fn validate_config(cfg: &NodeConfig) -> Result<()> {
    validate_config_with_bind_policy(cfg, false)
}

/// Validate a config, allowing a non-tailnet bind/host only when the caller has
/// explicitly passed the test-only `--insecure-bind` flag.
pub fn validate_config_with_bind_policy(cfg: &NodeConfig, insecure_bind: bool) -> Result<()> {
    if cfg.version != 1 {
        bail!(
            "unsupported node config version {}: expected 1",
            cfg.version
        );
    }
    validate_machine_label(&cfg.node.alias).context("node.alias")?;
    if !insecure_bind && !is_tailnet_ipv4(&cfg.node.bind) {
        bail!(
            "node.bind '{}' is not a Tailscale 100.x IPv4 address; \
             pass --insecure-bind only for a local test rig",
            cfg.node.bind
        );
    }
    if cfg.node.port == 0 {
        bail!("node.port must be 1..=65535");
    }

    let mut seen: Vec<&str> = vec![cfg.node.alias.as_str()];
    for (index, peer) in cfg.peers.iter().enumerate() {
        validate_machine_label(&peer.alias).with_context(|| format!("peers[{index}].alias"))?;
        if seen.contains(&peer.alias.as_str()) {
            bail!(
                "peers[{index}].alias '{}' collides with node.alias or another peer",
                peer.alias
            );
        }
        seen.push(peer.alias.as_str());
        if !insecure_bind && !is_tailnet_ipv4(&peer.host) {
            bail!(
                "peers[{index}].host '{}' is not a Tailscale 100.x IPv4 address; \
                 pass --insecure-bind only for a local test rig",
                peer.host
            );
        }
        if peer.port == 0 {
            bail!("peers[{index}].port must be 1..=65535");
        }
        if let Some(name) = &peer.token_env {
            if name.trim().is_empty() {
                bail!("peers[{index}].tokenEnv must not be empty");
            }
        }
    }
    Ok(())
}

/// `true` when `addr` is a bare IPv4 address inside Tailscale's
/// `100.64.0.0/10` carrier-grade NAT range (first octet `100`, second
/// `64..=127`). Anything else — `0.0.0.0`, `127.0.0.1`, a LAN address, a
/// hostname, an IPv6 address — is not a tailnet bind.
pub fn is_tailnet_ipv4(addr: &str) -> bool {
    match addr.parse::<Ipv4Addr>() {
        Ok(ip) => {
            let octets = ip.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        Err(_) => false,
    }
}

/// Lowercase `[a-z0-9._-]`, 1..=64 bytes. The label is a machine `<machine>`
/// alias, never a session id or a host display name.
pub fn validate_machine_label(label: &str) -> Result<()> {
    if label.is_empty() || label.len() > 64 {
        bail!("machine label must be 1..=64 bytes");
    }
    if !label.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    }) {
        bail!("machine label '{label}' must match ^[a-z0-9._-]{{1,64}}$");
    }
    Ok(())
}

/// Resolve a peer's shared token: the environment variable named by
/// `tokenEnv` first, then a literal test-rig `token`. Returns `None` when the
/// named variable is unset or empty — an unset token is a 401-class failure,
/// never an open door. The value is never logged.
pub fn resolve_peer_token(peer: &PeerConfig) -> Option<String> {
    if let Some(name) = peer.token_env.as_deref() {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    peer.token.clone().filter(|value| !value.is_empty())
}
