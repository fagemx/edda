use clap::Subcommand;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Subcommand)]
pub enum PairCmd {
    /// Register a new device and generate a device token
    New {
        /// Device name (e.g. "iPhone", "tablet")
        #[arg(long)]
        name: String,
    },
    /// List all paired devices
    List,
    /// Revoke a specific device
    Revoke {
        /// Device name to revoke
        name: String,
    },
    /// Revoke all paired devices
    RevokeAll,
}

pub fn execute(cmd: PairCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        PairCmd::New { name } => execute_new(repo_root, &name),
        PairCmd::List => execute_list(repo_root),
        PairCmd::Revoke { name } => execute_revoke(repo_root, &name),
        PairCmd::RevokeAll => execute_revoke_all(repo_root),
    }
}

/// Generate a device token: `edda_dev_<64-hex-chars>`.
fn generate_device_token() -> String {
    // Use ULID bytes + timestamp for randomness without requiring `rand`
    let id1 = ulid::Ulid::new();
    let id2 = ulid::Ulid::new();
    let mut bytes = [0u8; 32];
    let b1 = id1.to_bytes();
    let b2 = id2.to_bytes();
    bytes[..16].copy_from_slice(&b1);
    bytes[16..].copy_from_slice(&b2);
    format!("edda_dev_{}", hex::encode(bytes))
}

/// Hash a raw token string with SHA-256 and return hex.
fn hash_token(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    hex::encode(hasher.finalize())
}

fn execute_new(repo_root: &Path, name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("device name cannot be empty");
    }

    let ledger = edda_ledger::Ledger::open(repo_root)?;

    // Check for duplicate name
    let existing = ledger.list_device_tokens()?;
    if existing
        .iter()
        .any(|t| t.device_name == name && t.revoked_at.is_none())
    {
        anyhow::bail!(
            "device '{}' is already paired. Revoke it first with `edda pair revoke {}`.",
            name,
            name
        );
    }

    let device_token = generate_device_token();
    let token_hash = hash_token(&device_token);

    let event_id = format!("evt_{}", ulid::Ulid::new());
    let branch = ledger.head_branch()?;
    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| anyhow::anyhow!("time format error: {e}"))?;

    let payload = serde_json::json!({
        "device_name": name,
        "paired_from_ip": "localhost",
        "token_hash_prefix": &token_hash[..8],
    });

    let parent_hash = ledger.last_event_hash()?;
    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts: ts.clone(),
        event_type: "device_pair".to_string(),
        branch,
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    ledger.insert_device_token(&edda_ledger::DeviceTokenRow {
        token_hash,
        device_name: name.to_string(),
        paired_at: ts,
        paired_from_ip: "localhost".to_string(),
        revoked_at: None,
        pair_event_id: event_id,
        revoke_event_id: None,
    })?;

    eprintln!("Device paired: {name}");
    eprintln!();
    println!("Device token (save this — it will not be shown again):");
    println!();
    println!("  {device_token}");
    println!();
    eprintln!("Use this token in the Authorization header:");
    eprintln!("  Authorization: Bearer {device_token}");

    Ok(())
}

fn execute_list(repo_root: &Path) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let tokens = ledger.list_device_tokens()?;

    if tokens.is_empty() {
        eprintln!("No paired devices.");
        return Ok(());
    }

    println!(
        "{:<20} {:<10} {:<28} FROM IP",
        "DEVICE", "STATUS", "PAIRED AT"
    );
    println!("{}", "-".repeat(80));

    for t in &tokens {
        let status = if t.revoked_at.is_some() {
            "revoked"
        } else {
            "active"
        };
        println!(
            "{:<20} {:<10} {:<28} {}",
            t.device_name, status, t.paired_at, t.paired_from_ip
        );
    }

    let active_count = tokens.iter().filter(|t| t.revoked_at.is_none()).count();
    let total = tokens.len();
    eprintln!("\n{active_count} active, {total} total");

    Ok(())
}

fn execute_revoke(repo_root: &Path, name: &str) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)?;

    let event_id = format!("evt_{}", ulid::Ulid::new());
    let branch = ledger.head_branch()?;
    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| anyhow::anyhow!("time format error: {e}"))?;

    let payload = serde_json::json!({ "device_name": name });
    let parent_hash = ledger.last_event_hash()?;

    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts,
        event_type: "device_revoke".to_string(),
        branch,
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    let revoked = ledger.revoke_device_token(name, &event_id)?;
    if revoked {
        eprintln!("Revoked device: {name}");
    } else {
        eprintln!("No active device token found for '{name}'");
    }

    Ok(())
}

fn execute_revoke_all(repo_root: &Path) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)?;

    let event_id = format!("evt_{}", ulid::Ulid::new());
    let branch = ledger.head_branch()?;
    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| anyhow::anyhow!("time format error: {e}"))?;

    let payload = serde_json::json!({ "revoke_all": true });
    let parent_hash = ledger.last_event_hash()?;

    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts,
        event_type: "device_revoke".to_string(),
        branch,
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    let count = ledger.revoke_all_device_tokens(&event_id)?;
    eprintln!("Revoked {count} device(s).");

    Ok(())
}
