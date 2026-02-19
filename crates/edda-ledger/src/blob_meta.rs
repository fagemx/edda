use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Blob classification for GC priority decisions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobClass {
    /// Outputs, design docs, patches, decision files — never auto-removed.
    Artifact,
    /// Decision dependencies, important snippets — removable past keep_days.
    DecisionEvidence,
    /// stdout/stderr, ls output, noise — removed first.
    #[default]
    TraceNoise,
}

impl BlobClass {
    /// GC priority: lower number = removed first. Artifact is never auto-removed.
    pub fn gc_priority(self) -> u8 {
        match self {
            BlobClass::TraceNoise => 0,
            BlobClass::DecisionEvidence => 1,
            BlobClass::Artifact => 2,
        }
    }
}

impl std::fmt::Display for BlobClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobClass::Artifact => write!(f, "artifact"),
            BlobClass::DecisionEvidence => write!(f, "decision_evidence"),
            BlobClass::TraceNoise => write!(f, "trace_noise"),
        }
    }
}

impl std::str::FromStr for BlobClass {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "artifact" => Ok(BlobClass::Artifact),
            "decision_evidence" => Ok(BlobClass::DecisionEvidence),
            "trace_noise" => Ok(BlobClass::TraceNoise),
            _ => anyhow::bail!(
                "invalid blob class: {s}. Expected: artifact, decision_evidence, trace_noise"
            ),
        }
    }
}

/// A record of a classification change for audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassChange {
    pub from: BlobClass,
    pub to: BlobClass,
    pub by: String,
    pub at: String,
}

/// Metadata for a single blob (stored outside the event hash chain).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobMetaEntry {
    #[serde(default)]
    pub class: BlobClass,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classified_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub class_history: Vec<ClassChange>,
}

impl Default for BlobMetaEntry {
    fn default() -> Self {
        Self {
            class: BlobClass::TraceNoise,
            pinned: false,
            classified_at: None,
            classified_by: None,
            class_history: Vec::new(),
        }
    }
}

/// In-memory map of blob hash → metadata.
pub type BlobMetaMap = HashMap<String, BlobMetaEntry>;

/// Load blob_meta.json. Returns empty map if file doesn't exist.
pub fn load_blob_meta(path: &Path) -> anyhow::Result<BlobMetaMap> {
    if !path.exists() {
        return Ok(BlobMetaMap::new());
    }
    let content = std::fs::read_to_string(path)?;
    let map: BlobMetaMap = serde_json::from_str(&content)?;
    Ok(map)
}

/// Save blob_meta.json atomically (write to tmp, then rename).
pub fn save_blob_meta(path: &Path, meta: &BlobMetaMap) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(meta)?;
    // Atomic write: tmp file → rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json.as_bytes())?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Get metadata for a blob hash, returning defaults if not present.
pub fn get_meta(meta: &BlobMetaMap, hash: &str) -> BlobMetaEntry {
    meta.get(hash).cloned().unwrap_or_default()
}

/// Set classification for a blob. Records history when reclassifying.
pub fn set_class(meta: &mut BlobMetaMap, hash: &str, class: BlobClass, by: &str) {
    let entry = meta.entry(hash.to_string()).or_default();
    let ts = now_rfc3339();

    // Record history only when actually changing class on an existing entry
    if entry.classified_at.is_some() && entry.class != class {
        entry.class_history.push(ClassChange {
            from: entry.class,
            to: class,
            by: by.to_string(),
            at: ts.clone(),
        });
    }

    entry.class = class;
    entry.classified_by = Some(by.to_string());
    entry.classified_at = Some(ts);
}

/// Set pinned status for a blob.
pub fn set_pinned(meta: &mut BlobMetaMap, hash: &str, pinned: bool) {
    let entry = meta.entry(hash.to_string()).or_default();
    entry.pinned = pinned;
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_meta_round_trip() {
        let tmp = std::env::temp_dir().join(format!("edda_bmeta_rt_{}", std::process::id()));
        let path = tmp.join("blob_meta.json");
        let _ = std::fs::remove_dir_all(&tmp);

        let mut meta = BlobMetaMap::new();
        set_class(&mut meta, "abc123", BlobClass::Artifact, "user");
        set_pinned(&mut meta, "abc123", true);
        set_class(&mut meta, "def456", BlobClass::TraceNoise, "auto");

        save_blob_meta(&path, &meta).unwrap();
        let loaded = load_blob_meta(&path).unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["abc123"].class, BlobClass::Artifact);
        assert!(loaded["abc123"].pinned);
        assert_eq!(loaded["def456"].class, BlobClass::TraceNoise);
        assert!(!loaded["def456"].pinned);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_meta_defaults() {
        let meta = BlobMetaMap::new();
        let entry = get_meta(&meta, "nonexistent");
        assert_eq!(entry.class, BlobClass::TraceNoise);
        assert!(!entry.pinned);
    }

    #[test]
    fn blob_meta_missing_file_returns_empty() {
        let loaded = load_blob_meta(std::path::Path::new("/nonexistent/blob_meta.json")).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn blob_class_from_str() {
        assert_eq!("artifact".parse::<BlobClass>().unwrap(), BlobClass::Artifact);
        assert_eq!(
            "decision_evidence".parse::<BlobClass>().unwrap(),
            BlobClass::DecisionEvidence
        );
        assert_eq!(
            "trace_noise".parse::<BlobClass>().unwrap(),
            BlobClass::TraceNoise
        );
        assert!("invalid".parse::<BlobClass>().is_err());
    }

    #[test]
    fn blob_class_display() {
        assert_eq!(BlobClass::Artifact.to_string(), "artifact");
        assert_eq!(BlobClass::DecisionEvidence.to_string(), "decision_evidence");
        assert_eq!(BlobClass::TraceNoise.to_string(), "trace_noise");
    }

    #[test]
    fn gc_priority_order() {
        assert!(BlobClass::TraceNoise.gc_priority() < BlobClass::DecisionEvidence.gc_priority());
        assert!(
            BlobClass::DecisionEvidence.gc_priority() < BlobClass::Artifact.gc_priority()
        );
    }

    #[test]
    fn set_class_updates_timestamp() {
        let mut meta = BlobMetaMap::new();
        set_class(&mut meta, "abc", BlobClass::Artifact, "test");
        let entry = &meta["abc"];
        assert!(entry.classified_at.is_some());
        assert_eq!(entry.classified_by.as_deref(), Some("test"));
    }

    #[test]
    fn reclassify_records_history() {
        let mut meta = BlobMetaMap::new();

        // First classification — no history (initial set)
        set_class(&mut meta, "abc", BlobClass::TraceNoise, "auto");
        assert!(meta["abc"].class_history.is_empty());

        // Reclassify to artifact — should record history
        set_class(&mut meta, "abc", BlobClass::Artifact, "user");
        assert_eq!(meta["abc"].class, BlobClass::Artifact);
        assert_eq!(meta["abc"].class_history.len(), 1);
        assert_eq!(meta["abc"].class_history[0].from, BlobClass::TraceNoise);
        assert_eq!(meta["abc"].class_history[0].to, BlobClass::Artifact);
        assert_eq!(meta["abc"].class_history[0].by, "user");

        // Reclassify again — second history entry
        set_class(&mut meta, "abc", BlobClass::DecisionEvidence, "admin");
        assert_eq!(meta["abc"].class_history.len(), 2);
        assert_eq!(meta["abc"].class_history[1].from, BlobClass::Artifact);
        assert_eq!(meta["abc"].class_history[1].to, BlobClass::DecisionEvidence);

        // Same class again — no new history
        set_class(&mut meta, "abc", BlobClass::DecisionEvidence, "admin");
        assert_eq!(meta["abc"].class_history.len(), 2);
    }

    #[test]
    fn reclassify_history_survives_round_trip() {
        let tmp = std::env::temp_dir().join(format!("edda_bmeta_hist_{}", std::process::id()));
        let path = tmp.join("blob_meta.json");
        let _ = std::fs::remove_dir_all(&tmp);

        let mut meta = BlobMetaMap::new();
        set_class(&mut meta, "abc", BlobClass::TraceNoise, "auto");
        set_class(&mut meta, "abc", BlobClass::Artifact, "user");

        save_blob_meta(&path, &meta).unwrap();
        let loaded = load_blob_meta(&path).unwrap();

        assert_eq!(loaded["abc"].class_history.len(), 1);
        assert_eq!(loaded["abc"].class_history[0].from, BlobClass::TraceNoise);
        assert_eq!(loaded["abc"].class_history[0].to, BlobClass::Artifact);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
