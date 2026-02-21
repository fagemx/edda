use crate::blob_meta::BlobClass;
use crate::paths::EddaPaths;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Why a blob was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteReason {
    /// Removed by age-based GC retention
    Retention,
    /// Removed by quota enforcement
    Quota,
    /// Removed by purge-archive
    PurgeArchive,
    /// Manually removed by user
    Manual,
}

impl std::fmt::Display for DeleteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeleteReason::Retention => write!(f, "retention"),
            DeleteReason::Quota => write!(f, "quota"),
            DeleteReason::PurgeArchive => write!(f, "purge_archive"),
            DeleteReason::Manual => write!(f, "manual"),
        }
    }
}

/// Record of a deleted blob for auditability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    pub blob_hash: String,
    pub deleted_at: String,
    pub reason: DeleteReason,
    pub last_known_class: BlobClass,
    pub was_pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Append a tombstone record to tombstones.jsonl.
pub fn append_tombstone(paths: &EddaPaths, tombstone: &Tombstone) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(tombstone)?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.tombstones_jsonl)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Read all tombstones from tombstones.jsonl. Returns empty vec if file doesn't exist.
pub fn list_tombstones(paths: &EddaPaths) -> anyhow::Result<Vec<Tombstone>> {
    if !paths.tombstones_jsonl.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&paths.tombstones_jsonl)?;
    let mut tombstones = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let t: Tombstone = serde_json::from_str(line)?;
        tombstones.push(t);
    }
    Ok(tombstones)
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

/// Create a tombstone record with current timestamp.
pub fn make_tombstone(
    blob_hash: &str,
    reason: DeleteReason,
    class: BlobClass,
    pinned: bool,
    size_bytes: Option<u64>,
) -> Tombstone {
    Tombstone {
        blob_hash: blob_hash.to_string(),
        deleted_at: now_rfc3339(),
        reason,
        last_known_class: class,
        was_pinned: pinned,
        size_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_round_trip() {
        let tmp = std::env::temp_dir().join(format!("edda_tomb_rt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        paths.ensure_layout().unwrap();

        let t1 = make_tombstone(
            "abc123",
            DeleteReason::Retention,
            BlobClass::TraceNoise,
            false,
            Some(1024),
        );
        let t2 = make_tombstone(
            "def456",
            DeleteReason::Quota,
            BlobClass::DecisionEvidence,
            false,
            Some(2048),
        );

        append_tombstone(&paths, &t1).unwrap();
        append_tombstone(&paths, &t2).unwrap();

        let loaded = list_tombstones(&paths).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].blob_hash, "abc123");
        assert_eq!(loaded[0].reason, DeleteReason::Retention);
        assert_eq!(loaded[0].last_known_class, BlobClass::TraceNoise);
        assert_eq!(loaded[0].size_bytes, Some(1024));
        assert_eq!(loaded[1].blob_hash, "def456");
        assert_eq!(loaded[1].reason, DeleteReason::Quota);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tombstone_missing_file_returns_empty() {
        let paths = EddaPaths::discover("/nonexistent/path");
        let result = list_tombstones(&paths).unwrap();
        assert!(result.is_empty());
    }
}
