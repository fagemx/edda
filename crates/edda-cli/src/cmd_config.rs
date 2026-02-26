use clap::Subcommand;
use std::path::Path;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Set a config value
    Set {
        /// Config key (e.g. skill_guide)
        key: String,
        /// Config value (true/false/number/string)
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// List all config values
    List,
}

// ── Dispatch ──

pub fn run(cmd: ConfigCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        ConfigCmd::Set { key, value } => set(repo_root, &key, &value),
        ConfigCmd::Get { key } => get(repo_root, &key),
        ConfigCmd::List => list(repo_root),
    }
}

// ── Command Implementations ──

/// Read config from `.edda/config.json`. Returns empty map if file doesn't exist.
fn read_config(path: &Path) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let content = std::fs::read_to_string(path)?;
    let val: serde_json::Value = serde_json::from_str(&content)?;
    match val {
        serde_json::Value::Object(map) => Ok(map),
        _ => Ok(serde_json::Map::new()),
    }
}

/// Write config to `.edda/config.json`.
fn write_config(
    path: &Path,
    config: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(&config)?;
    edda_store::write_atomic(path, json.as_bytes())
}

/// Parse a string value into an appropriate JSON value (bool/number/string).
fn parse_value(s: &str) -> serde_json::Value {
    match s {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => {
            if let Ok(n) = s.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::Value::String(s.to_string())
            }
        }
    }
}

/// `edda config set <key> <value>`
pub fn set(repo_root: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let mut config = read_config(&paths.config_json)?;
    config.insert(key.to_string(), parse_value(value));
    write_config(&paths.config_json, &config)?;
    println!("{key} = {value}");
    Ok(())
}

/// `edda config get <key>`
pub fn get(repo_root: &Path, key: &str) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let config = read_config(&paths.config_json)?;
    match config.get(key) {
        Some(val) => println!("{val}"),
        None => println!("(not set)"),
    }
    Ok(())
}

/// `edda config list`
pub fn list(repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let config = read_config(&paths.config_json)?;
    if config.is_empty() {
        println!("(no config set)");
    } else {
        for (k, v) in &config {
            println!("{k} = {v}");
        }
    }
    Ok(())
}
