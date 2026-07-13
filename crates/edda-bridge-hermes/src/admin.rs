//! Install / uninstall / doctor for the Hermes shell-hook bridge.
//!
//! Hermes reads shell hooks from the `hooks:` block of `~/.hermes/cli-config.yaml`
//! (see `hermes-agent/agent/shell_hooks.py::register_from_config`). We MERGE
//! into that file's existing `hooks:` structure — the file is the user's main
//! Hermes config and holds unrelated settings we must preserve.
//!
//! Consent workflow (from `shell_hooks.py::_prompt_and_record`):
//!   * First use of each `(event, command)` triggers a TTY prompt or is
//!     bypassed via `--accept-hooks` / `HERMES_ACCEPT_HOOKS=1` /
//!     `hooks_auto_accept: true`.
//!   * Approvals persist to `~/.hermes/shell-hooks-allowlist.json`.
//!   * `HERMES_SAFE_MODE=1` disables all shell hooks (agent troubleshooting).

use std::fs;
use std::path::{Path, PathBuf};

const HOOK_COMMAND: &str = "edda hook hermes";

/// Events we register on. Chosen from Hermes' VALID_HOOKS (see
/// `hermes_cli/plugins.py`) for those that map cleanly to edda's
/// bridge-claude machinery. `pre_verify` and `subagent_*` are candidates for
/// a follow-up commit.
const HOOK_EVENTS: &[&str] = &[
    "pre_llm_call",
    "pre_tool_call",
    "post_tool_call",
    "on_session_start",
    "on_session_end",
    "on_session_reset",
];

fn default_config_path() -> Option<PathBuf> {
    // Hermes' `get_hermes_home()` picks `~/.hermes` on POSIX and the platform
    // equivalent on Windows. The primary config filename is `cli-config.yaml`
    // (the codebase writes both cli-config.yaml and config.yaml in different
    // eras; cli-config.yaml is the current name).
    dirs::home_dir().map(|h| h.join(".hermes").join("cli-config.yaml"))
}

/// Write (or merge) the edda hook into Hermes' cli-config.yaml.
///
/// If a hook for `edda hook hermes` already exists under an event, we leave
/// it in place instead of appending a duplicate — install is idempotent.
///
/// Preserves all other keys in the file and all other user hooks.
pub fn install(target: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match target {
        Some(p) => p.to_path_buf(),
        None => default_config_path()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Load existing config (or start empty). Malformed YAML becomes empty
    // mapping — never crash the user's config.
    let mut root: serde_yaml::Value = if path.exists() {
        let raw = fs::read_to_string(&path)?;
        serde_yaml::from_str(&raw)
            .unwrap_or_else(|_| serde_yaml::Value::Mapping(Default::default()))
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };

    if !root.is_mapping() {
        root = serde_yaml::Value::Mapping(Default::default());
    }
    let root_map = root.as_mapping_mut().unwrap();

    // Get or create the top-level `hooks:` mapping.
    let hooks_key = serde_yaml::Value::String("hooks".into());
    if !root_map.contains_key(&hooks_key)
        || !root_map
            .get(&hooks_key)
            .map(|v| v.is_mapping())
            .unwrap_or(false)
    {
        root_map.insert(
            hooks_key.clone(),
            serde_yaml::Value::Mapping(Default::default()),
        );
    }
    let hooks_map = root_map
        .get_mut(&hooks_key)
        .unwrap()
        .as_mapping_mut()
        .unwrap();

    for event in HOOK_EVENTS {
        let event_key = serde_yaml::Value::String((*event).into());
        // Ensure it's a sequence (list of hook entries).
        if !hooks_map.contains_key(&event_key)
            || !hooks_map
                .get(&event_key)
                .map(|v| v.is_sequence())
                .unwrap_or(false)
        {
            hooks_map.insert(event_key.clone(), serde_yaml::Value::Sequence(Vec::new()));
        }
        let entries = hooks_map
            .get_mut(&event_key)
            .unwrap()
            .as_sequence_mut()
            .unwrap();

        let already_present = entries.iter().any(|entry| {
            entry
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("command".into())))
                .and_then(|c| c.as_str())
                .map(|c| c == HOOK_COMMAND)
                .unwrap_or(false)
        });
        if already_present {
            continue;
        }

        let mut entry_map = serde_yaml::Mapping::new();
        entry_map.insert(
            serde_yaml::Value::String("command".into()),
            serde_yaml::Value::String(HOOK_COMMAND.into()),
        );
        entries.push(serde_yaml::Value::Mapping(entry_map));
    }

    let pretty = serde_yaml::to_string(&root)?;
    fs::write(&path, pretty)?;

    println!("Installed edda Hermes hooks to {}", path.display());
    println!();
    println!("Events wired: {}", HOOK_EVENTS.join(", "));
    println!("Hook command: {HOOK_COMMAND}");
    println!();
    println!("Hermes will prompt on first use of each event. To skip prompting:");
    println!("  export HERMES_ACCEPT_HOOKS=1");
    println!("  # or add 'hooks_auto_accept: true' to cli-config.yaml");
    println!("  # or run 'hermes --accept-hooks chat'");
    Ok(path)
}

/// Remove edda entries from Hermes hooks (preserving all other keys).
pub fn uninstall(target: Option<&Path>) -> anyhow::Result<()> {
    let path = match target {
        Some(p) => p.to_path_buf(),
        None => default_config_path()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };
    if !path.exists() {
        println!("No Hermes config at {}", path.display());
        return Ok(());
    }

    let raw = fs::read_to_string(&path)?;
    let mut root: serde_yaml::Value = match serde_yaml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            println!(
                "Hermes config at {} is not valid YAML; leaving untouched",
                path.display()
            );
            return Ok(());
        }
    };

    if let Some(hooks) = root
        .as_mapping_mut()
        .and_then(|m| m.get_mut(serde_yaml::Value::String("hooks".into())))
    {
        if let Some(hooks_map) = hooks.as_mapping_mut() {
            let event_names: Vec<serde_yaml::Value> = hooks_map.keys().cloned().collect();
            for event_key in event_names {
                if let Some(seq) = hooks_map
                    .get_mut(&event_key)
                    .and_then(|v| v.as_sequence_mut())
                {
                    seq.retain(|entry| {
                        entry
                            .as_mapping()
                            .and_then(|m| m.get(serde_yaml::Value::String("command".into())))
                            .and_then(|c| c.as_str())
                            .map(|c| c != HOOK_COMMAND)
                            .unwrap_or(true)
                    });
                }
            }
            // Drop empty sequences so the config stays tidy.
            let empty_keys: Vec<serde_yaml::Value> = hooks_map
                .iter()
                .filter(|(_, v)| v.as_sequence().map(|s| s.is_empty()).unwrap_or(false))
                .map(|(k, _)| k.clone())
                .collect();
            for k in empty_keys {
                hooks_map.remove(&k);
            }
        }
    }

    let pretty = serde_yaml::to_string(&root)?;
    fs::write(&path, pretty)?;
    println!("Removed edda hooks from {}", path.display());
    Ok(())
}

/// Report Hermes bridge health.
pub fn doctor() -> anyhow::Result<()> {
    let edda_in_path = which_edda();
    println!(
        "[{}] edda in PATH: {}",
        if edda_in_path.is_some() { "OK" } else { "WARN" },
        edda_in_path.as_deref().unwrap_or("not found")
    );

    let config_path = default_config_path();
    let has_hook = config_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|raw| raw.contains(HOOK_COMMAND))
        .unwrap_or(false);
    println!(
        "[{}] Hermes hooks installed: {}",
        if has_hook { "OK" } else { "WARN" },
        config_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".into())
    );

    // Check consent status. Even if hooks are installed, they don't fire
    // until approved once via TTY/env/config.
    let allowlist_path =
        dirs::home_dir().map(|h| h.join(".hermes").join("shell-hooks-allowlist.json"));
    let approved_here = allowlist_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|raw| raw.contains(HOOK_COMMAND))
        .unwrap_or(false);
    let auto_accept_env = std::env::var("HERMES_ACCEPT_HOOKS")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    let consent_ok = approved_here || auto_accept_env;
    println!(
        "[{}] Hermes consent: {}",
        if consent_ok { "OK" } else { "INFO" },
        if approved_here {
            "allowlisted".to_string()
        } else if auto_accept_env {
            "HERMES_ACCEPT_HOOKS=1 in env".to_string()
        } else {
            "not yet approved — first Hermes run will TTY-prompt, or set HERMES_ACCEPT_HOOKS=1"
                .to_string()
        }
    );

    let store_root = edda_store::store_root();
    println!(
        "[{}] store root: {}",
        if store_root.exists() { "OK" } else { "WARN" },
        store_root.display()
    );
    Ok(())
}

fn which_edda() -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exe = if cfg!(windows) { "edda.exe" } else { "edda" };
    for dir in path_var.split(sep) {
        let candidate = Path::new(dir).join(exe);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_target(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join(".hermes").join("cli-config.yaml")
    }

    #[test]
    fn install_creates_yaml_with_all_events() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        install(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        let hooks = &v["hooks"];
        for event in HOOK_EVENTS {
            let entries = hooks[*event].as_sequence().unwrap();
            assert!(!entries.is_empty(), "event {event} should have entries");
            let cmd = entries[0]["command"].as_str().unwrap();
            assert_eq!(cmd, HOOK_COMMAND);
        }
    }

    #[test]
    fn install_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        install(Some(&path)).unwrap();
        install(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        let ptc = v["hooks"]["pre_tool_call"].as_sequence().unwrap();
        let edda_count = ptc
            .iter()
            .filter(|e| e["command"].as_str() == Some(HOOK_COMMAND))
            .count();
        assert_eq!(edda_count, 1);
    }

    #[test]
    fn install_preserves_other_hooks_and_top_level_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"model: claude-sonnet
providers:
  anthropic:
    api_key: fake
hooks:
  pre_tool_call:
    - matcher: terminal
      command: /home/user/my-guard.sh
      timeout: 30
hooks_auto_accept: false
"#,
        )
        .unwrap();
        install(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        // Other top-level keys preserved.
        assert_eq!(v["model"].as_str().unwrap(), "claude-sonnet");
        assert!(!v["hooks_auto_accept"].as_bool().unwrap());
        assert_eq!(
            v["providers"]["anthropic"]["api_key"].as_str().unwrap(),
            "fake"
        );
        // User's own hook preserved AND edda hook added.
        let ptc = v["hooks"]["pre_tool_call"].as_sequence().unwrap();
        let has_user = ptc
            .iter()
            .any(|e| e["command"].as_str() == Some("/home/user/my-guard.sh"));
        let has_edda = ptc
            .iter()
            .any(|e| e["command"].as_str() == Some(HOOK_COMMAND));
        assert!(has_user, "user hook must be preserved");
        assert!(has_edda, "edda hook must be added");
    }

    #[test]
    fn uninstall_removes_only_edda_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"model: gpt-4
hooks:
  pre_tool_call:
    - command: other-tool
"#,
        )
        .unwrap();
        install(Some(&path)).unwrap();
        uninstall(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("other-tool"));
        assert!(!raw.contains(HOOK_COMMAND));
        assert!(raw.contains("gpt-4"));
    }

    #[test]
    fn uninstall_drops_empty_event_sequences() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        install(Some(&path)).unwrap();
        uninstall(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        // hooks: exists but is empty; individual event keys should be gone.
        for event in HOOK_EVENTS {
            assert!(
                v["hooks"].get(*event).map(|v| v.is_null()).unwrap_or(true),
                "event {event} sequence should be dropped after uninstall"
            );
        }
    }

    #[test]
    fn uninstall_on_missing_file_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        uninstall(Some(&tmp.path().join("nonexistent.yaml"))).unwrap();
    }

    #[test]
    fn install_on_malformed_yaml_starts_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is: not: valid: yaml: content:").unwrap();
        install(Some(&path)).unwrap();
        // Should have written a valid new config with our hooks.
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        assert!(v["hooks"]["pre_llm_call"].is_sequence());
    }
}
