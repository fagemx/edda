use std::path::{Path, PathBuf};

/// All well-known paths under `.edda/`.
#[derive(Debug, Clone)]
pub struct EddaPaths {
    pub root: PathBuf,
    pub edda_dir: PathBuf,
    pub ledger_dir: PathBuf,
    pub events_jsonl: PathBuf,
    pub blobs_dir: PathBuf,
    pub branches_dir: PathBuf,
    pub refs_dir: PathBuf,
    pub head_file: PathBuf,
    pub branches_json: PathBuf,
    pub drafts_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub lock_file: PathBuf,
    pub config_json: PathBuf,
    pub patterns_dir: PathBuf,
    pub blob_meta_json: PathBuf,
    pub tombstones_jsonl: PathBuf,
    pub archive_dir: PathBuf,
    pub archive_blobs_dir: PathBuf,
}

impl EddaPaths {
    /// Derive all paths from a repo root. Pure computation, no I/O.
    pub fn discover(repo_root: impl Into<PathBuf>) -> Self {
        let root = repo_root.into();
        let edda_dir = root.join(".edda");
        let ledger_dir = edda_dir.join("ledger");
        let refs_dir = edda_dir.join("refs");
        let archive_dir = edda_dir.join("archive");
        Self {
            events_jsonl: ledger_dir.join("events.jsonl"),
            blobs_dir: ledger_dir.join("blobs"),
            blob_meta_json: ledger_dir.join("blob_meta.json"),
            tombstones_jsonl: ledger_dir.join("tombstones.jsonl"),
            branches_dir: edda_dir.join("branches"),
            head_file: refs_dir.join("HEAD"),
            branches_json: refs_dir.join("branches.json"),
            drafts_dir: edda_dir.join("drafts"),
            cache_dir: edda_dir.join("cache"),
            lock_file: edda_dir.join("LOCK"),
            config_json: edda_dir.join("config.json"),
            patterns_dir: edda_dir.join("patterns"),
            archive_blobs_dir: archive_dir.join("blobs"),
            archive_dir,
            ledger_dir,
            refs_dir,
            edda_dir,
            root,
        }
    }

    /// Create all required directories. Idempotent.
    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        for dir in [
            &self.ledger_dir,
            &self.blobs_dir,
            &self.branches_dir,
            &self.refs_dir,
            &self.drafts_dir,
            &self.cache_dir,
            &self.patterns_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Check whether `.edda/` exists.
    pub fn is_initialized(&self) -> bool {
        self.edda_dir.is_dir()
    }

    /// Resolve a branch directory under `.edda/branches/<name>/`.
    pub fn branch_dir(&self, name: &str) -> PathBuf {
        self.branches_dir.join(name)
    }
}

impl EddaPaths {
    /// Walk up from `start` looking for a directory containing `.edda/`.
    /// Returns `None` if not found.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut cur = start.to_path_buf();
        loop {
            if cur.join(".edda").is_dir() {
                return Some(cur);
            }
            if !cur.pop() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_builds_correct_paths() {
        let p = EddaPaths::discover("/tmp/repo");
        assert_eq!(p.edda_dir, PathBuf::from("/tmp/repo/.edda"));
        assert_eq!(
            p.events_jsonl,
            PathBuf::from("/tmp/repo/.edda/ledger/events.jsonl")
        );
        assert_eq!(p.blobs_dir, PathBuf::from("/tmp/repo/.edda/ledger/blobs"));
        assert_eq!(p.head_file, PathBuf::from("/tmp/repo/.edda/refs/HEAD"));
        assert_eq!(p.lock_file, PathBuf::from("/tmp/repo/.edda/LOCK"));
        assert_eq!(p.patterns_dir, PathBuf::from("/tmp/repo/.edda/patterns"));
        assert_eq!(
            p.blob_meta_json,
            PathBuf::from("/tmp/repo/.edda/ledger/blob_meta.json")
        );
        assert_eq!(
            p.tombstones_jsonl,
            PathBuf::from("/tmp/repo/.edda/ledger/tombstones.jsonl")
        );
        assert_eq!(p.archive_dir, PathBuf::from("/tmp/repo/.edda/archive"));
        assert_eq!(
            p.archive_blobs_dir,
            PathBuf::from("/tmp/repo/.edda/archive/blobs")
        );
    }

    #[test]
    fn ensure_layout_creates_dirs() {
        let tmp = std::env::temp_dir().join(format!("edda_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();
        assert!(p.ledger_dir.is_dir());
        assert!(p.blobs_dir.is_dir());
        assert!(p.branches_dir.is_dir());
        assert!(p.refs_dir.is_dir());
        assert!(p.drafts_dir.is_dir());
        assert!(p.cache_dir.is_dir());
        assert!(p.patterns_dir.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
