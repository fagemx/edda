//! Repository-level review settings (GH-763).
//!
//! One small file, `.edda/review/due.json`, holding only what an operator
//! actually needs to turn: how long a pushed head must settle before it is
//! worth a review, and which triggers are live at all. Absent means defaults,
//! which is the normal case — the file exists so a repository *can* differ,
//! not so every repository must carry one.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Settings for `edda review due`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DueConfig {
    /// How long a pushed head must sit unchanged before it is worth reviewing.
    ///
    /// Ten minutes by default. The cost it guards is real: a round-1 Opus
    /// review measured $1.28–$2.57 on #754, so a five-push burst reviewed
    /// per push costs about ten dollars for one tree.
    pub debounce_seconds: u64,
    /// Which triggers may fire. `draft` is not among them — refusing a draft
    /// is not a trigger and cannot be switched off.
    pub triggers: Vec<String>,
}

impl Default for DueConfig {
    fn default() -> Self {
        Self {
            debounce_seconds: 600,
            triggers: vec!["ready".into(), "response".into(), "push".into()],
        }
    }
}

impl DueConfig {
    pub(crate) fn enabled(&self, trigger: &str) -> bool {
        self.triggers.iter().any(|name| name == trigger)
    }

    /// Read `.edda/review/due.json`, or the defaults when it is absent.
    ///
    /// A file that exists but cannot be parsed is an error rather than a
    /// silent fall back to defaults: an operator who wrote a debounce and got
    /// the default one would be paying for a switch they believe they threw.
    pub(crate) fn load(repo: &Path) -> Result<Self> {
        let path = repo.join(".edda").join("review").join("due.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("read review config {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("read review config {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_no_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = DueConfig::load(dir.path()).expect("absent config is not an error");
        assert_eq!(config.debounce_seconds, 600);
        assert!(config.enabled("ready") && config.enabled("response") && config.enabled("push"));
        assert!(!config.enabled("draft"), "draft is not a trigger");
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_it_does_not_mention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let review = dir.path().join(".edda").join("review");
        std::fs::create_dir_all(&review).expect("config dir");
        std::fs::write(review.join("due.json"), r#"{"debounce_seconds": 30}"#).expect("write");
        let config = DueConfig::load(dir.path()).expect("load");
        assert_eq!(config.debounce_seconds, 30);
        assert!(config.enabled("push"), "triggers keep their default");
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_a_silent_default() {
        // An operator who wrote a debounce and silently got 600 would be
        // paying for a switch they believe they threw.
        let dir = tempfile::tempdir().expect("tempdir");
        let review = dir.path().join(".edda").join("review");
        std::fs::create_dir_all(&review).expect("config dir");
        std::fs::write(review.join("due.json"), "{ not json").expect("write");
        assert!(DueConfig::load(dir.path()).is_err());

        // A typo in a key is the same failure wearing a friendlier face.
        std::fs::write(review.join("due.json"), r#"{"debounce_second": 30}"#).expect("write");
        assert!(DueConfig::load(dir.path()).is_err());
    }
}
