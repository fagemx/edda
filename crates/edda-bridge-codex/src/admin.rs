//! Install / uninstall / doctor for the Codex bridge.
//!
//! Codex reads hooks from `~/.codex/hooks.json` (user-level) or
//! `<repo>/.codex/hooks.json` (project-level). We install to user-level by
//! default so every project a user opens under Codex gets edda hooks
//! automatically — the same push-mode experience Claude Code users have.
//!
//! Hook config shape (from https://developers.openai.com/codex/hooks):
//! ```json
//! {
//!   "hooks": {
//!     "SessionStart": [
//!       { "hooks": [{ "type": "command", "command": "edda hook codex" }] }
//!     ],
//!     "PreToolUse": [
//!       { "matcher": "Bash", "hooks": [{ "type": "command", "command": "edda hook codex" }] }
//!     ]
//!   }
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

const HOOK_COMMAND: &str = "edda hook codex";

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PreCompact",
    "SessionEnd",
    "Stop",
];

fn default_hooks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("hooks.json"))
}

/// Write (or merge) edda hooks into the Codex hooks config.
///
/// If a config already exists, we merge — respecting other users' hooks and
/// only adding our own `edda hook codex` handler under each event. Idempotent.
pub fn install(target: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match target {
        Some(p) => p.to_path_buf(),
        None => default_hooks_path()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut config: serde_json::Value = if path.exists() {
        let raw = fs::read_to_string(&path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !config.is_object() {
        config = serde_json::json!({});
    }
    let hooks_obj = config
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_obj.is_object() {
        *hooks_obj = serde_json::json!({});
    }
    let hooks_map = hooks_obj.as_object_mut().unwrap();

    for event in HOOK_EVENTS {
        let entries = hooks_map
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        if !entries.is_array() {
            *entries = serde_json::json!([]);
        }
        let array = entries.as_array_mut().unwrap();

        let already_present = array.iter().any(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c == HOOK_COMMAND)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        if !already_present {
            array.push(serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": HOOK_COMMAND
                }]
            }));
        }
    }

    let pretty = serde_json::to_string_pretty(&config)?;
    fs::write(&path, pretty)?;

    println!("Installed edda Codex hooks to {}", path.display());
    println!();
    println!("Events wired: {}", HOOK_EVENTS.join(", "));
    println!("Hook command: {HOOK_COMMAND}");
    Ok(path)
}

/// Remove edda hooks from the Codex config (preserving other users' entries).
pub fn uninstall(target: Option<&Path>) -> anyhow::Result<()> {
    let path = match target {
        Some(p) => p.to_path_buf(),
        None => default_hooks_path()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };
    if !path.exists() {
        println!("No hooks config at {}", path.display());
        return Ok(());
    }

    let raw = fs::read_to_string(&path)?;
    let mut config: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            println!("Hooks config at {} is not valid JSON; leaving untouched", path.display());
            return Ok(());
        }
    };

    if let Some(hooks_map) = config
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
    {
        for entries in hooks_map.values_mut() {
            if let Some(array) = entries.as_array_mut() {
                array.retain(|group| {
                    let mut is_edda = false;
                    if let Some(hooks) = group.get("hooks").and_then(|h| h.as_array()) {
                        is_edda = hooks.iter().all(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c == HOOK_COMMAND)
                                .unwrap_or(false)
                        }) && !hooks.is_empty();
                    }
                    !is_edda
                });
            }
        }
        // Drop empty event arrays entirely.
        hooks_map.retain(|_, v| {
            v.as_array().map(|a| !a.is_empty()).unwrap_or(true)
        });
    }

    let pretty = serde_json::to_string_pretty(&config)?;
    fs::write(&path, pretty)?;
    println!("Removed edda hooks from {}", path.display());
    Ok(())
}

/// Print a health report for the Codex bridge.
pub fn doctor() -> anyhow::Result<()> {
    let edda_in_path = which_edda();
    println!(
        "[{}] edda in PATH: {}",
        if edda_in_path.is_some() { "OK" } else { "WARN" },
        edda_in_path.as_deref().unwrap_or("not found")
    );

    let hooks_path = default_hooks_path();
    let has_hooks = hooks_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|raw| raw.contains(HOOK_COMMAND))
        .unwrap_or(false);
    println!(
        "[{}] Codex hooks installed: {}",
        if has_hooks { "OK" } else { "WARN" },
        hooks_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".into())
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
        tmp.path().join(".codex").join("hooks.json")
    }

    #[test]
    fn install_creates_hooks_file_with_all_events() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        install(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = v.get("hooks").unwrap().as_object().unwrap();
        for event in HOOK_EVENTS {
            let entries = hooks.get(*event).unwrap().as_array().unwrap();
            assert!(!entries.is_empty(), "event {event} should have entries");
            let cmd = entries[0]["hooks"][0]["command"].as_str().unwrap();
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
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let ptu = v["hooks"]["PreToolUse"].as_array().unwrap();
        // Only one edda group per event even after two installs.
        let edda_groups = ptu
            .iter()
            .filter(|g| {
                g["hooks"]
                    .as_array()
                    .map(|hs| {
                        hs.iter()
                            .any(|h| h["command"].as_str() == Some(HOOK_COMMAND))
                    })
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(edda_groups, 1);
    }

    #[test]
    fn install_preserves_other_users_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"other-tool"}]}]}}"#,
        )
        .unwrap();
        install(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("other-tool"));
        assert!(raw.contains(HOOK_COMMAND));
    }

    #[test]
    fn uninstall_removes_only_edda_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = install_target(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"other-tool"}]}]}}"#,
        )
        .unwrap();
        install(Some(&path)).unwrap();
        uninstall(Some(&path)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("other-tool"));
        assert!(!raw.contains(HOOK_COMMAND));
    }

    #[test]
    fn uninstall_on_missing_file_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        uninstall(Some(&tmp.path().join("nonexistent.json"))).unwrap();
    }
}
