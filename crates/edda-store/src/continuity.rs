use crate::{lock_file, project_id, store_root, write_atomic};
use edda_core::secret_guard::redact;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PORTABLE_REPO_CONFIG_KEY: &str = "portable_repo_key";
const MAX_GIT_OUTPUT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRepositoryIdentity {
    pub portable_repo_id: Option<String>,
    pub display_hint: Option<String>,
    pub local_only_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableAliasResolution {
    pub portable_repo_ids: Vec<String>,
    pub ambiguous: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableAliasRegistryV1 {
    version: u8,
    repositories: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

pub fn derive_portable_repository_identity(
    checkout: &Path,
    project_config: &Path,
) -> anyhow::Result<PortableRepositoryIdentity> {
    if let Some(key) = configured_key(project_config)? {
        return Ok(PortableRepositoryIdentity {
            portable_repo_id: Some(hash_identity(&format!("key:{key}"))),
            display_hint: None,
            local_only_reason: None,
        });
    }

    let remotes = git_lines(checkout, &["remote"]).unwrap_or_default();
    let remote_name = if remotes.iter().any(|name| name == "origin") {
        Some("origin".to_string())
    } else if remotes.len() == 1 {
        remotes.first().cloned()
    } else {
        None
    };
    let Some(remote_name) = remote_name else {
        return Ok(local_only(
            "no configured repository key or unique Git remote",
        ));
    };
    let Some(raw_remote) = git_text(checkout, &["remote", "get-url", &remote_name]) else {
        return Ok(local_only("Git remote identity is unavailable"));
    };
    let Some(canonical) = canonical_remote_identity(raw_remote.trim()) else {
        return Ok(local_only("Git remote is not a portable network identity"));
    };
    Ok(PortableRepositoryIdentity {
        portable_repo_id: Some(hash_identity(&format!("remote:{canonical}"))),
        display_hint: Some(canonical),
        local_only_reason: None,
    })
}

pub fn record_portable_alias(portable_repo_id: &str, checkout: &Path) -> anyhow::Result<()> {
    edda_core::continuity::validate_portable_repo_id(portable_repo_id)?;
    let clone_path = checkout
        .canonicalize()
        .unwrap_or_else(|_| checkout.to_path_buf())
        .to_string_lossy()
        .to_string();
    edda_core::continuity::validate_raw_secrets(
        clone_path.as_bytes(),
        "portable repository clone path",
    )?;
    let local_project_id = project_id(checkout);
    let _lock = lock_file(&alias_lock_path())?;
    let mut registry = load_aliases()?;
    let version_changed = registry.version != 1;
    registry.version = 1;
    let inserted = registry
        .repositories
        .entry(portable_repo_id.to_string())
        .or_default()
        .entry(local_project_id)
        .or_default()
        .insert(clone_path);
    if !version_changed && !inserted {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(&registry)?;
    edda_core::continuity::validate_raw_secrets(&bytes, "portable repository alias registry")?;
    write_atomic(&alias_path(), &bytes)
}

pub fn resolve_portable_aliases(checkout: &Path) -> anyhow::Result<PortableAliasResolution> {
    let registry = load_aliases()?;
    let local_project_id = project_id(checkout);
    let portable_repo_ids = registry
        .repositories
        .iter()
        .filter(|(_, locals)| locals.contains_key(&local_project_id))
        .map(|(portable_repo_id, _)| portable_repo_id.clone())
        .collect::<Vec<_>>();
    Ok(PortableAliasResolution {
        ambiguous: portable_repo_ids.len() > 1,
        portable_repo_ids,
    })
}

fn configured_key(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() > 64 * 1024 {
        anyhow::bail!("project config exceeds the continuity read bound");
    }
    edda_core::continuity::validate_raw_secrets(&bytes, "project config")?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("project config has an invalid JSON schema"))?;
    let Some(value) = value.get(PORTABLE_REPO_CONFIG_KEY) else {
        return Ok(None);
    };
    let key = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{PORTABLE_REPO_CONFIG_KEY} must be a string"))?;
    if key.trim().is_empty() || key.chars().count() > 256 {
        anyhow::bail!("{PORTABLE_REPO_CONFIG_KEY} must contain 1-256 characters");
    }
    if let Some(hit) = redact(key).1.first() {
        anyhow::bail!(
            "secret content refused in field config.{PORTABLE_REPO_CONFIG_KEY} (kind: {})",
            hit.kind
        );
    }
    Ok(Some(key.to_string()))
}

fn canonical_remote_identity(raw: &str) -> Option<String> {
    if raw.contains('\0') || raw.chars().count() > 4_096 {
        return None;
    }
    if let Some((scheme, rest)) = raw.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ssh" | "git"
        ) {
            return None;
        }
        let rest = rest.split(['?', '#']).next()?;
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?.to_ascii_lowercase();
        if host.is_empty() || path.is_empty() {
            return None;
        }
        return normalize_host_path(&host, path);
    }

    let (authority, path) = raw.split_once(':')?;
    if authority.len() == 1 && authority.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if authority.contains('/') || authority.contains('\\') {
        return None;
    }
    let host = authority.rsplit('@').next()?.to_ascii_lowercase();
    normalize_host_path(&host, path)
}

fn normalize_host_path(host: &str, path: &str) -> Option<String> {
    let mut path = path.trim_matches('/').to_string();
    if path.to_ascii_lowercase().ends_with(".git") {
        path.truncate(path.len() - 4);
    }
    let safe = !host.is_empty()
        && !path.is_empty()
        && !host.contains(['/', '\\', '@'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."));
    safe.then(|| format!("{host}/{path}"))
}

fn hash_identity(identity: &str) -> String {
    format!("repo_{}", edda_core::hash::sha256_hex(identity.as_bytes()))
}

fn local_only(reason: &str) -> PortableRepositoryIdentity {
    PortableRepositoryIdentity {
        portable_repo_id: None,
        display_hint: None,
        local_only_reason: Some(reason.to_string()),
    }
}

fn git_lines(checkout: &Path, args: &[&str]) -> Option<Vec<String>> {
    git_text(checkout, args).map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn git_text(checkout: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take((MAX_GIT_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_GIT_OUTPUT {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    if !child.wait().ok()?.success() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn alias_path() -> PathBuf {
    store_root().join("portable_repositories.json")
}

fn alias_lock_path() -> PathBuf {
    store_root().join("portable_repositories.lock")
}

fn load_aliases() -> anyhow::Result<PortableAliasRegistryV1> {
    let path = alias_path();
    if !path.is_file() {
        return Ok(PortableAliasRegistryV1::default());
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() > 1024 * 1024 {
        anyhow::bail!("portable repository alias registry exceeds its size bound");
    }
    edda_core::continuity::validate_raw_secrets(&bytes, "portable repository alias registry")?;
    let registry: PortableAliasRegistryV1 = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("portable repository alias registry has an invalid JSON schema")
    })?;
    if registry.version != 1 {
        anyhow::bail!("unsupported portable repository alias registry version");
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_alias_lookup_is_read_only_and_reports_ambiguity() {
        let store = crate::test_support::isolated_store_root().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let first = format!("repo_{}", "a".repeat(64));
        let second = format!("repo_{}", "b".repeat(64));
        record_portable_alias(&first, checkout.path()).unwrap();
        record_portable_alias(&second, checkout.path()).unwrap();
        let before = std::fs::read(store.path().join("portable_repositories.json")).unwrap();

        let resolution = resolve_portable_aliases(checkout.path()).unwrap();

        assert_eq!(resolution.portable_repo_ids, vec![first, second]);
        assert!(resolution.ambiguous);
        assert_eq!(
            std::fs::read(store.path().join("portable_repositories.json")).unwrap(),
            before
        );
    }

    #[test]
    fn suspicious_clone_path_is_rejected_before_registry_mutation() {
        let store = crate::test_support::isolated_store_root().unwrap();
        let safe_checkout = tempfile::tempdir().unwrap();
        let portable_repo_id = format!("repo_{}", "a".repeat(64));
        record_portable_alias(&portable_repo_id, safe_checkout.path()).unwrap();
        let registry_path = store.path().join("portable_repositories.json");
        let before = std::fs::read(&registry_path).unwrap();

        let parent = tempfile::tempdir().unwrap();
        let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
        let suspicious_checkout = parent.path().join(secret);
        std::fs::create_dir(&suspicious_checkout).unwrap();
        let error = record_portable_alias(&portable_repo_id, &suspicious_checkout)
            .expect_err("secret-shaped clone paths must be refused");

        assert!(error
            .to_string()
            .contains("secret content refused in portable repository clone path"));
        assert!(!error.to_string().contains(secret));
        assert_eq!(std::fs::read(&registry_path).unwrap(), before);
        assert!(resolve_portable_aliases(safe_checkout.path())
            .unwrap()
            .portable_repo_ids
            .contains(&portable_repo_id));
    }

    #[test]
    fn remote_normalization_strips_credentials_and_dot_git() {
        assert_eq!(
            canonical_remote_identity("https://alice:token@example.COM/org/repo.git"),
            Some("example.com/org/repo".to_string())
        );
        assert_eq!(
            canonical_remote_identity("git@example.com:org/repo.git"),
            Some("example.com/org/repo".to_string())
        );
    }

    #[test]
    fn local_and_traversing_remotes_are_not_portable() {
        assert_eq!(canonical_remote_identity("C:/repo"), None);
        assert_eq!(
            canonical_remote_identity("https://example.com/org/../repo.git"),
            None
        );
    }

    #[test]
    fn configured_key_is_stable_across_clone_paths() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let config = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(config.path(), r#"{"portable_repo_key":"org/project"}"#).unwrap();
        let a = derive_portable_repository_identity(first.path(), config.path()).unwrap();
        let b = derive_portable_repository_identity(second.path(), config.path()).unwrap();
        assert_eq!(a.portable_repo_id, b.portable_repo_id);
        assert!(a.portable_repo_id.unwrap().starts_with("repo_"));
    }
}
