//! User-level config at `~/.edda/config.json`.
//!
//! Provides get/set access to a simple JSON key-value store.

use crate::{store_root, write_atomic};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Path to the user-level config file.
pub fn user_config_path() -> PathBuf {
    store_root().join("config.json")
}

/// Load user config from disk. Returns empty map if file doesn't exist.
pub fn load_user_config() -> Map<String, Value> {
    let path = user_config_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

/// Save user config to disk atomically.
pub fn save_user_config(config: &Map<String, Value>) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(&Value::Object(config.clone()))?;
    write_atomic(&user_config_path(), json.as_bytes())
}

/// Get a single config value by key.
pub fn get_user_config(key: &str) -> Option<Value> {
    let config = load_user_config();
    config.get(key).cloned()
}

/// Set a single config value. Creates the file if it doesn't exist.
pub fn set_user_config(key: &str, value: Value) -> anyhow::Result<()> {
    let mut config = load_user_config();
    config.insert(key.to_string(), value);
    save_user_config(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a closure with `EDDA_STORE_ROOT` pointing to an isolated tempdir.
    fn with_isolated_store(f: impl FnOnce()) {
        let _guard = crate::ENV_STORE_LOCK.lock().unwrap();
        let store = tempfile::tempdir().unwrap();
        std::env::set_var("EDDA_STORE_ROOT", store.path());
        f();
        std::env::remove_var("EDDA_STORE_ROOT");
    }

    #[test]
    fn load_returns_empty_when_no_file() {
        with_isolated_store(|| {
            let config = load_user_config();
            assert!(config.is_empty());
        });
    }

    #[test]
    fn set_get_and_overwrite_roundtrip() {
        with_isolated_store(|| {
            set_user_config("test_key", Value::String("hello".into())).unwrap();
            let val = get_user_config("test_key");
            assert_eq!(val, Some(Value::String("hello".into())));

            // Overwrite
            set_user_config("test_key", Value::Number(42.into())).unwrap();
            let val = get_user_config("test_key");
            assert_eq!(val, Some(Value::Number(42.into())));
        });
    }
}
