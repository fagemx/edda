//! Decision-code staleness detection (Foundry q334 EDDA-STALENESS1).
//!
//! Contract: for a decision that names one or more `affected_paths`, check
//! whether those files have been modified since the decision was recorded.
//! Modified paths get a `stale` flag so a future agent doesn't cite a decision
//! about code that no longer looks the way it did at decide time.
//!
//! Deterministic (mtime-based). Ledger stays untouched — staleness is a
//! query-time derivation, not a mutation. Best-effort: unreadable repo /
//! missing file returns "unknown" rather than false-positive stale.
//!
//! Vocabulary alignment:
//! - `fresh`: the path exists and its mtime is at or before the decision ts.
//! - `stale_modified`: file exists but its mtime is strictly after decision ts.
//! - `missing`: path does not resolve on disk (repo-relative or absolute).
//! - `unknown`: repo root not supplied, or path attributes unreadable.

use crate::DecisionHit;
use serde::Serialize;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStatus {
    Fresh,
    StaleModified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct PathStaleness {
    pub path: String,
    pub status: PathStatus,
    /// mtime as ISO 8601 (RFC 3339) when known; otherwise absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionStaleness {
    /// Overall stale flag: any path stale_modified OR missing.
    pub is_stale: bool,
    pub paths: Vec<PathStaleness>,
}

/// Filesystem oracle abstracts real fs so tests can be deterministic.
pub trait FsOracle {
    /// Return (exists, mtime_rfc3339 when available).
    fn probe(&self, path: &Path) -> (bool, Option<String>);
}

pub struct StdFs;
impl FsOracle for StdFs {
    fn probe(&self, path: &Path) -> (bool, Option<String>) {
        let Ok(meta) = std::fs::metadata(path) else {
            return (false, None);
        };
        let Ok(modified) = meta.modified() else {
            return (true, None);
        };
        // system_time → OffsetDateTime → rfc3339
        let ts = OffsetDateTime::from(modified);
        let rendered = ts.format(&Rfc3339).ok();
        (true, rendered)
    }
}

/// Check staleness of `affected_paths` for a single decision. `repo_root`
/// resolves relative paths; absolute paths are used as-is. Returns None when
/// paths is empty (no staleness concept applies).
pub fn check_paths_staleness<F: FsOracle>(
    affected_paths: &[String],
    decision_ts: &str,
    repo_root: Option<&Path>,
    fs: &F,
) -> Option<DecisionStaleness> {
    if affected_paths.is_empty() {
        return None;
    }
    let decision_dt = OffsetDateTime::parse(decision_ts, &Rfc3339).ok();
    let mut out = Vec::with_capacity(affected_paths.len());
    let mut any_stale = false;
    for rel in affected_paths {
        let resolved: PathBuf = {
            let p = Path::new(rel);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                match repo_root {
                    Some(root) => root.join(p),
                    None => {
                        out.push(PathStaleness {
                            path: rel.clone(),
                            status: PathStatus::Unknown,
                            touched_at: None,
                        });
                        continue;
                    }
                }
            }
        };

        let (exists, touched_at) = fs.probe(&resolved);
        if !exists {
            any_stale = true;
            out.push(PathStaleness {
                path: rel.clone(),
                status: PathStatus::Missing,
                touched_at,
            });
            continue;
        }

        let status = match (&touched_at, &decision_dt) {
            (Some(t), Some(dt)) => match OffsetDateTime::parse(t, &Rfc3339) {
                Ok(mtime) => {
                    if mtime > *dt {
                        any_stale = true;
                        PathStatus::StaleModified
                    } else {
                        PathStatus::Fresh
                    }
                }
                Err(_) => PathStatus::Unknown,
            },
            _ => PathStatus::Unknown,
        };
        out.push(PathStaleness {
            path: rel.clone(),
            status,
            touched_at,
        });
    }
    Some(DecisionStaleness {
        is_stale: any_stale,
        paths: out,
    })
}

/// Convenience: annotate an in-memory DecisionHit list with staleness using
/// the real filesystem. Silent no-op when repo_root is None.
pub fn annotate_hits(
    hits: &mut [DecisionHit],
    hits_paths: &[Vec<String>],
    repo_root: Option<&Path>,
) {
    let fs = StdFs;
    for (hit, paths) in hits.iter_mut().zip(hits_paths.iter()) {
        hit.staleness = check_paths_staleness(paths, &hit.ts, repo_root, &fs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct MockFs {
        // path key → (exists, mtime rfc3339)
        entries: RefCell<HashMap<String, (bool, Option<String>)>>,
    }
    impl MockFs {
        fn new() -> Self {
            Self { entries: RefCell::new(HashMap::new()) }
        }
        fn set(&self, path: &str, exists: bool, mtime: Option<&str>) {
            self.entries
                .borrow_mut()
                .insert(path.to_string(), (exists, mtime.map(String::from)));
        }
    }
    impl FsOracle for MockFs {
        fn probe(&self, path: &Path) -> (bool, Option<String>) {
            let key = path.to_string_lossy().replace('\\', "/");
            self.entries
                .borrow()
                .get(&key)
                .cloned()
                .unwrap_or((false, None))
        }
    }

    #[test]
    fn empty_paths_returns_none() {
        let fs = MockFs::new();
        let out = check_paths_staleness(&[], "2026-07-01T00:00:00Z", None, &fs);
        assert!(out.is_none());
    }

    #[test]
    fn path_modified_after_decision_is_stale_modified() {
        let fs = MockFs::new();
        fs.set("/repo/src/foo.rs", true, Some("2026-07-05T10:00:00Z"));
        let out = check_paths_staleness(
            &["src/foo.rs".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert!(out.is_stale);
        assert_eq!(out.paths[0].status, PathStatus::StaleModified);
    }

    #[test]
    fn path_untouched_since_decision_is_fresh() {
        let fs = MockFs::new();
        fs.set("/repo/src/bar.rs", true, Some("2026-06-01T00:00:00Z"));
        let out = check_paths_staleness(
            &["src/bar.rs".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert!(!out.is_stale);
        assert_eq!(out.paths[0].status, PathStatus::Fresh);
    }

    #[test]
    fn missing_path_is_stale_missing() {
        let fs = MockFs::new();
        // no entry ⇒ !exists
        let out = check_paths_staleness(
            &["src/deleted.rs".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert!(out.is_stale, "missing counts as stale");
        assert_eq!(out.paths[0].status, PathStatus::Missing);
    }

    #[test]
    fn absolute_path_bypasses_repo_root() {
        let fs = MockFs::new();
        // Use OS-appropriate absolute path so Path::is_absolute agrees.
        let abs = if cfg!(windows) { "C:/opt/config.json" } else { "/opt/config.json" };
        fs.set(abs, true, Some("2026-07-05T00:00:00Z"));
        let out = check_paths_staleness(
            &[abs.to_string()],
            "2026-07-01T00:00:00Z",
            None,
            &fs,
        ).unwrap();
        assert_eq!(out.paths[0].status, PathStatus::StaleModified);
    }

    #[test]
    fn no_repo_root_and_relative_path_is_unknown_not_missing() {
        let fs = MockFs::new();
        let out = check_paths_staleness(
            &["src/foo.rs".to_string()],
            "2026-07-01T00:00:00Z",
            None,
            &fs,
        ).unwrap();
        assert!(!out.is_stale, "unknown does not flip is_stale (F9-shaped restraint)");
        assert_eq!(out.paths[0].status, PathStatus::Unknown);
    }

    #[test]
    fn unparseable_decision_ts_marks_paths_unknown() {
        let fs = MockFs::new();
        fs.set("/repo/src/foo.rs", true, Some("2026-07-05T10:00:00Z"));
        let out = check_paths_staleness(
            &["src/foo.rs".to_string()],
            "not-a-date",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert!(!out.is_stale);
        assert_eq!(out.paths[0].status, PathStatus::Unknown);
    }

    #[test]
    fn mixed_bag_is_stale_when_any_path_stale() {
        let fs = MockFs::new();
        fs.set("/repo/fresh.rs", true, Some("2026-06-01T00:00:00Z"));
        fs.set("/repo/modified.rs", true, Some("2026-07-05T00:00:00Z"));
        let out = check_paths_staleness(
            &["fresh.rs".to_string(), "modified.rs".to_string(), "deleted.rs".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert!(out.is_stale);
        assert_eq!(out.paths[0].status, PathStatus::Fresh);
        assert_eq!(out.paths[1].status, PathStatus::StaleModified);
        assert_eq!(out.paths[2].status, PathStatus::Missing);
    }

    #[test]
    fn touched_at_carried_through_when_available() {
        let fs = MockFs::new();
        fs.set("/repo/a.rs", true, Some("2026-07-05T10:00:00Z"));
        let out = check_paths_staleness(
            &["a.rs".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        ).unwrap();
        assert_eq!(out.paths[0].touched_at.as_deref(), Some("2026-07-05T10:00:00Z"));
    }
}
