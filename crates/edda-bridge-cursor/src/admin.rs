use std::fs;
use std::path::{Path, PathBuf};

const HOOK_COMMAND: &str = "edda hook cursor";
const HOOK_EVENTS: &[&str] = &[
    "sessionStart",
    "beforeSubmitPrompt",
    "preToolUse",
    "postToolUse",
    "preCompact",
    "sessionEnd",
    "stop",
    "subagentStart",
    "subagentStop",
];

fn default_hooks_path() -> anyhow::Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".cursor").join("hooks.json"))
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))
}

pub fn install(target: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match target {
        Some(path) => path.to_path_buf(),
        None => default_hooks_path()?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut config = if path.exists() {
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({})
    };
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Cursor hooks config must be a JSON object"))?;
    root.entry("version".to_string())
        .or_insert(serde_json::json!(1));
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Cursor hooks field must be a JSON object"))?;
    for event in HOOK_EVENTS {
        let entries = hooks
            .entry((*event).to_string())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("Cursor hook event {event} must be an array"))?;
        let installed = entries.iter().any(|entry| {
            entry.get("command").and_then(serde_json::Value::as_str) == Some(HOOK_COMMAND)
        });
        if !installed {
            entries.push(serde_json::json!({"command": HOOK_COMMAND}));
        }
    }
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    println!("Installed edda Cursor hooks to {}", path.display());
    Ok(path)
}

pub fn uninstall(target: Option<&Path>) -> anyhow::Result<()> {
    let path = match target {
        Some(path) => path.to_path_buf(),
        None => default_hooks_path()?,
    };
    if !path.exists() {
        println!("No Cursor hooks config at {}", path.display());
        return Ok(());
    }

    let mut config: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    if let Some(hooks) = config
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    {
        for entries in hooks.values_mut() {
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| {
                    entry.get("command").and_then(serde_json::Value::as_str) != Some(HOOK_COMMAND)
                });
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|entries| !entries.is_empty()));
    }

    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    println!("Removed edda Cursor hooks from {}", path.display());
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct HookHealth {
    configured_events: usize,
    expected_events: usize,
}

fn inspect_hooks(path: &Path) -> anyhow::Result<HookHealth> {
    if !path.exists() {
        return Ok(HookHealth {
            configured_events: 0,
            expected_events: HOOK_EVENTS.len(),
        });
    }
    let config: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let hooks = config.get("hooks").and_then(serde_json::Value::as_object);
    let configured_events = HOOK_EVENTS
        .iter()
        .filter(|event| {
            hooks
                .and_then(|hooks| hooks.get(**event))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry.get("command").and_then(serde_json::Value::as_str)
                            == Some(HOOK_COMMAND)
                    })
                })
        })
        .count();
    Ok(HookHealth {
        configured_events,
        expected_events: HOOK_EVENTS.len(),
    })
}

pub fn doctor() -> anyhow::Result<()> {
    let edda = which_edda();
    println!(
        "[{}] edda in PATH: {}",
        if edda.is_some() { "OK" } else { "WARN" },
        edda.as_deref().unwrap_or("not found")
    );

    let path = default_hooks_path()?;
    let health = inspect_hooks(&path)?;
    let hooks_ok = health.configured_events == health.expected_events;
    println!(
        "[{}] Cursor hooks installed: {}/{} events ({})",
        if hooks_ok { "OK" } else { "WARN" },
        health.configured_events,
        health.expected_events,
        path.display()
    );

    let store_root = edda_store::store_root();
    let store_writable = store_is_writable(&store_root);
    println!(
        "[{}] store writable: {}",
        if store_writable { "OK" } else { "WARN" },
        store_root.display()
    );

    if claude_hook_detected() {
        println!(
            "[WARN] Claude edda hooks also detected; disable Cursor third-party hook import to avoid duplicate injection"
        );
    }
    Ok(())
}

fn which_edda() -> Option<String> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let executable = if cfg!(windows) { "edda.exe" } else { "edda" };
    std::env::var("PATH")
        .unwrap_or_default()
        .split(separator)
        .map(|directory| Path::new(directory).join(executable))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

fn store_is_writable(store_root: &Path) -> bool {
    if fs::create_dir_all(store_root).is_err() {
        return false;
    }
    let probe = store_root.join(format!(".doctor-write-{}", std::process::id()));
    if fs::write(&probe, b"ok").is_err() {
        return false;
    }
    fs::remove_file(probe).is_ok()
}

fn claude_hook_detected() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    [
        home.join(".claude").join("settings.json"),
        home.join(".claude").join("settings.local.json"),
    ]
    .iter()
    .filter_map(|path| fs::read_to_string(path).ok())
    .any(|settings| settings.contains("edda hook claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks_path(temp: &tempfile::TempDir) -> std::path::PathBuf {
        temp.path().join(".cursor").join("hooks.json")
    }

    #[test]
    fn install_creates_native_cursor_hooks_for_all_events() {
        let temp = tempfile::tempdir().unwrap();
        let path = hooks_path(&temp);

        let installed = install(Some(&path)).unwrap();

        assert_eq!(installed, path);
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(config["version"], 1);
        for event in HOOK_EVENTS {
            let entries = config["hooks"][event].as_array().unwrap();
            assert_eq!(entries.len(), 1, "event {event} should have one hook");
            assert_eq!(entries[0]["command"], HOOK_COMMAND);
        }
    }

    #[test]
    fn install_preserves_existing_cursor_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let path = hooks_path(&temp);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"hooks":{"preToolUse":[{"command":"other-tool","matcher":"Shell"}]},"custom":true}"#,
        )
        .unwrap();

        install(Some(&path)).unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(config["custom"], true);
        let entries = config["hooks"]["preToolUse"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["command"] == "other-tool"));
        assert!(entries.iter().any(|entry| entry["command"] == HOOK_COMMAND));
    }

    #[test]
    fn install_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let path = hooks_path(&temp);

        install(Some(&path)).unwrap();
        install(Some(&path)).unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for event in HOOK_EVENTS {
            let count = config["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["command"] == HOOK_COMMAND)
                .count();
            assert_eq!(count, 1, "event {event} should not duplicate edda");
        }
    }

    #[test]
    fn uninstall_removes_only_edda_entries() {
        let temp = tempfile::tempdir().unwrap();
        let path = hooks_path(&temp);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"hooks":{"sessionStart":[{"command":"other-tool"}]}}"#,
        )
        .unwrap();
        install(Some(&path)).unwrap();

        uninstall(Some(&path)).unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("other-tool"));
        assert!(!serialized.contains(HOOK_COMMAND));
    }

    #[test]
    fn hook_health_counts_configured_edda_events() {
        let temp = tempfile::tempdir().unwrap();
        let path = hooks_path(&temp);
        install(Some(&path)).unwrap();

        let health = inspect_hooks(&path).unwrap();

        assert_eq!(health.configured_events, HOOK_EVENTS.len());
        assert_eq!(health.expected_events, HOOK_EVENTS.len());
    }
}
