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
//! Paths are recorded as globs (`edda claim --paths "crates/foo/*"`), so what
//! gets probed is the deepest wildcard-free ancestor — the area the decision
//! governs — not the pattern itself, which resolves to nothing. See
//! [`probe_target`]. Literal paths are probed exactly as written.
//!
//! **What a glob can and cannot detect (GH-405).** Probing the directory means a
//! glob-scoped decision detects entries being *added or removed*, but not a file
//! inside being *edited* — a directory's mtime does not move when its contents
//! are rewritten in place. So for globs the contract above is coarser than it
//! reads: the most common way a decision goes stale is invisible.
//!
//! That is a deliberate trade, and it sits with this module's stated preference
//! for false-negatives over false-positives: before, a glob resolved to nothing
//! and every such decision was reported `missing`, which trains readers to
//! ignore the hint and destroys it for the paths that really are stale. A hint
//! that rarely fires beats one that always lies. Detecting edits properly means
//! enumerating the glob's matches and taking their newest mtime, which needs an
//! [`FsOracle`] that can walk a directory rather than only probe one path.
//!
//! Vocabulary alignment:
//! - `fresh`: the probed path exists and its mtime is at or before the decision ts.
//! - `stale_modified`: it exists but its mtime is strictly after decision ts.
//! - `missing`: it does not resolve on disk (repo-relative or absolute).
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

/// What to actually probe on disk for a recorded path pattern.
///
/// Scopes are recorded as globs — `edda claim --paths "crates/foo/*"` is the
/// documented form — and a glob cannot be probed literally, because nothing is
/// named `*`. Doing so reported every such decision as `Missing` regardless of
/// the truth (GH-405), which trains readers to ignore the hint and so destroys
/// it for genuinely stale paths.
///
/// The deepest wildcard-free ancestor is what the hint is really asserting:
/// "the area this decision governs still exists". `crates/foo/*` probes
/// `crates/foo`; `a/*/c.rs` probes `a`. A pattern with no wildcard is returned
/// unchanged, so literal paths keep their exact previous behaviour — including
/// per-file mtime.
fn probe_target(pattern: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in pattern.components() {
        let text = comp.as_os_str().to_string_lossy();
        if text.contains('*') || text.contains('?') || text.contains('[') {
            break;
        }
        out.push(comp);
    }
    // An all-wildcard pattern (`*`) leaves nothing to probe; keep it literal so
    // it resolves against the repo root rather than silently becoming "".
    if out.as_os_str().is_empty() {
        return pattern.to_path_buf();
    }
    out
}

/// Whether a recorded path is a glob (has a wildcard) rather than a literal
/// file. Same characters `probe_target` splits on, so the two agree on what
/// "glob" means.
fn is_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// The later of two optional rfc3339 timestamps. Unparseable values are ignored
/// rather than treated as newest, keeping the module's false-negative bias: a
/// timestamp we cannot read must not manufacture staleness.
fn newer_of(a: Option<String>, b: Option<String>) -> Option<String> {
    let parse = |s: &str| OffsetDateTime::parse(s, &Rfc3339).ok();
    match (a, b) {
        (Some(x), Some(y)) => match (parse(&x), parse(&y)) {
            (Some(dx), Some(dy)) => Some(if dy > dx { y } else { x }),
            (Some(_), None) => Some(x),
            (None, Some(_)) => Some(y),
            (None, None) => Some(x),
        },
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Most entries scanned when finding the newest mtime under a glob's directory.
///
/// This runs per glob-scoped decision per `edda ask`, on the interactive path,
/// so the walk is bounded: a directory with more entries than this reports the
/// newest among the first `MAX_GLOB_ENTRIES` it yields. Normal source
/// directories are far smaller, so the cap only ever bites a pathological tree —
/// where a bounded, possibly-incomplete answer beats a slow `ask`.
const MAX_GLOB_ENTRIES: usize = 512;

/// Filesystem oracle abstracts real fs so tests can be deterministic.
pub trait FsOracle {
    /// Return (exists, mtime_rfc3339 when available).
    fn probe(&self, path: &Path) -> (bool, Option<String>);

    /// Newest mtime (rfc3339) among the direct entries of `dir`, or `None` if it
    /// is not a readable directory.
    ///
    /// This is what lets a glob detect an *edit*: a directory's own mtime moves
    /// when entries are added or removed, but not when a file inside is rewritten
    /// in place — so the freshest child, not the directory, is the signal
    /// (GH-424). Direct entries only (one level); see [`check_paths_staleness`]
    /// for how `**` subtree scopes are handled.
    fn newest_mtime_under(&self, _dir: &Path) -> Option<String> {
        None
    }
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

    fn newest_mtime_under(&self, dir: &Path) -> Option<String> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut newest: Option<OffsetDateTime> = None;
        for entry in rd.flatten().take(MAX_GLOB_ENTRIES) {
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let ts = OffsetDateTime::from(modified);
            newest = Some(newest.map_or(ts, |n| n.max(ts)));
        }
        newest.and_then(|t| t.format(&Rfc3339).ok())
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
        // Reduce a glob to the directory it names before resolving; the reported
        // path stays the pattern the decision actually recorded (GH-405).
        let pattern = probe_target(Path::new(rel));
        let resolved: PathBuf = {
            let p = pattern.as_path();
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

        let (exists, dir_mtime) = fs.probe(&resolved);
        if !exists {
            any_stale = true;
            out.push(PathStaleness {
                path: rel.clone(),
                status: PathStatus::Missing,
                touched_at: dir_mtime,
            });
            continue;
        }

        // A glob's directory-mtime moves on add/remove but not on an in-place
        // edit, so the freshest *entry* is the real signal — take the newer of
        // the directory and its newest child (GH-424). A literal path names one
        // file and uses its own mtime, untouched. Only direct children are
        // walked, so a `**` subtree scope still misses edits nested deeper than
        // one level; that is a narrower promise than `**` implies, but a fully
        // kept one for the common `*` case, and it never lies.
        let touched_at = if is_glob(rel) {
            newer_of(dir_mtime, fs.newest_mtime_under(&resolved))
        } else {
            dir_mtime
        };

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
        // dir key → newest child mtime rfc3339 (drives newest_mtime_under)
        newest: RefCell<HashMap<String, Option<String>>>,
    }
    impl MockFs {
        fn new() -> Self {
            Self {
                entries: RefCell::new(HashMap::new()),
                newest: RefCell::new(HashMap::new()),
            }
        }
        fn set(&self, path: &str, exists: bool, mtime: Option<&str>) {
            self.entries
                .borrow_mut()
                .insert(path.to_string(), (exists, mtime.map(String::from)));
        }
        /// Record the newest mtime among a directory's children, as if a file
        /// inside it had been edited at that time.
        fn set_newest_under(&self, dir: &str, mtime: Option<&str>) {
            self.newest
                .borrow_mut()
                .insert(dir.to_string(), mtime.map(String::from));
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
        fn newest_mtime_under(&self, dir: &Path) -> Option<String> {
            let key = dir.to_string_lossy().replace('\\', "/");
            self.newest.borrow().get(&key).cloned().flatten()
        }
    }

    #[test]
    fn empty_paths_returns_none() {
        let fs = MockFs::new();
        let out = check_paths_staleness(&[], "2026-07-01T00:00:00Z", None, &fs);
        assert!(out.is_none());
    }

    /// GH-405: `edda claim --paths "crates/foo/*"` is the documented way to
    /// record a decision's scope, so globs are the norm rather than an edge case.
    /// Probing one literally can only ever fail — nothing is named `*` — so every
    /// such decision carried a false `(Missing)`, which trains readers to ignore
    /// the hint and kills it for genuinely stale paths.
    /// The point of GH-424: a file *edited* inside a glob scope, after the
    /// decision, must report `StaleModified` — even though the directory's own
    /// mtime did not move, because an in-place rewrite does not touch a dir's
    /// mtime. Before this, the directory looked untouched and the edit was
    /// invisible.
    #[test]
    fn a_glob_detects_a_file_edited_inside_it() {
        let fs = MockFs::new();
        // The directory exists and its own mtime PREDATES the decision — no
        // entries were added or removed, so a directory-mtime check alone would
        // call this Fresh.
        fs.set("/repo/crates/foo", true, Some("2026-07-01T00:00:00Z"));
        // But a file inside was rewritten AFTER the decision.
        fs.set_newest_under("/repo/crates/foo", Some("2026-07-10T00:00:00Z"));

        let out = check_paths_staleness(
            &["crates/foo/*".to_string()],
            "2026-07-05T00:00:00Z", // decision recorded between the two
            Some(Path::new("/repo")),
            &fs,
        )
        .unwrap();

        assert!(
            out.is_stale,
            "an edit inside the glob, after the decision, must be stale: {out:?}"
        );
        assert_eq!(out.paths[0].status, PathStatus::StaleModified);
    }

    /// The MockFs tests prove the staleness *logic*; this proves `StdFs`
    /// actually reads a real directory — a `read_dir` that returned nothing
    /// would leave the whole feature inert while every MockFs test stayed green.
    /// Asserts presence, not an exact time, so there is no clock race.
    #[test]
    fn stdfs_newest_mtime_reads_a_real_directory() {
        let dir = std::env::temp_dir().join(format!("edda_glob_stdfs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), b"x").unwrap();

        let fs = StdFs;
        assert!(
            fs.newest_mtime_under(&dir).is_some(),
            "a real directory with an entry must yield a newest mtime"
        );
        assert!(
            fs.newest_mtime_under(&dir.join("missing")).is_none(),
            "a path that is not a readable directory yields None"
        );
        assert!(
            fs.newest_mtime_under(&dir.join("a.rs")).is_none(),
            "a plain file is not a directory to walk"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A literal path (no wildcard) must not enumerate — it names one file, and
    /// its own mtime is the whole answer. Guards against the glob path leaking
    /// into the literal case.
    #[test]
    fn a_literal_path_uses_its_own_mtime_not_its_siblings() {
        let fs = MockFs::new();
        // The named file is old; a sibling in the same dir is new. A literal
        // scope must report Fresh — the sibling is not in scope.
        fs.set("/repo/src/main.rs", true, Some("2026-07-01T00:00:00Z"));
        fs.set_newest_under("/repo/src", Some("2026-07-10T00:00:00Z"));

        let out = check_paths_staleness(
            &["src/main.rs".to_string()],
            "2026-07-05T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        )
        .unwrap();

        assert_eq!(
            out.paths[0].status,
            PathStatus::Fresh,
            "a literal path must ignore its siblings' edits: {out:?}"
        );
    }

    #[test]
    fn a_glob_is_checked_against_the_directory_it_names_not_literally() {
        let fs = MockFs::new();
        // The directory the glob names exists and predates the decision. Note
        // nothing is registered for the literal "*" path — that is the point.
        fs.set("/repo/crates/foo", true, Some("2026-06-01T00:00:00Z"));

        let out = check_paths_staleness(
            &["crates/foo/*".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        )
        .unwrap();

        assert_eq!(
            out.paths[0].status,
            PathStatus::Fresh,
            "a glob over an existing, untouched directory is not missing"
        );
        assert!(!out.is_stale);
    }

    /// The same defect, and the form the issue actually observed: an absolute
    /// glob into a sibling repo that is present on this machine.
    #[test]
    fn an_absolute_glob_into_another_repo_that_exists_is_not_missing() {
        // The pattern must match the platform. `Path::is_absolute` only counts a
        // drive letter as absolute on Windows — on Unix `C:/x` is a *relative*
        // path, so a hardcoded drive letter would quietly exercise the repo-root
        // branch instead of the absolute one, testing nothing it claims to. CI
        // caught this; a Windows-only run cannot.
        #[cfg(windows)]
        let (dir, pattern) = ("C:/ai_agent/edda/crates", "C:/ai_agent/edda/crates/*");
        #[cfg(not(windows))]
        let (dir, pattern) = ("/ai_agent/edda/crates", "/ai_agent/edda/crates/*");

        let fs = MockFs::new();
        fs.set(dir, true, Some("2026-06-01T00:00:00Z"));

        let out = check_paths_staleness(
            &[pattern.to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/some/other/repo")),
            &fs,
        )
        .unwrap();

        assert_eq!(out.paths[0].status, PathStatus::Fresh);
    }

    /// The signal must survive the fix: a real deletion still warns.
    #[test]
    fn a_glob_whose_directory_is_gone_still_reports_missing() {
        let fs = MockFs::new(); // nothing exists

        let out = check_paths_staleness(
            &["crates/deleted/*".to_string()],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        )
        .unwrap();

        assert_eq!(out.paths[0].status, PathStatus::Missing);
        assert!(out.is_stale);
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert!(out.is_stale, "missing counts as stale");
        assert_eq!(out.paths[0].status, PathStatus::Missing);
    }

    #[test]
    fn absolute_path_bypasses_repo_root() {
        let fs = MockFs::new();
        // Use OS-appropriate absolute path so Path::is_absolute agrees.
        let abs = if cfg!(windows) {
            "C:/opt/config.json"
        } else {
            "/opt/config.json"
        };
        fs.set(abs, true, Some("2026-07-05T00:00:00Z"));
        let out =
            check_paths_staleness(&[abs.to_string()], "2026-07-01T00:00:00Z", None, &fs).unwrap();
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
        )
        .unwrap();
        assert!(
            !out.is_stale,
            "unknown does not flip is_stale (F9-shaped restraint)"
        );
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
        )
        .unwrap();
        assert!(!out.is_stale);
        assert_eq!(out.paths[0].status, PathStatus::Unknown);
    }

    #[test]
    fn mixed_bag_is_stale_when_any_path_stale() {
        let fs = MockFs::new();
        fs.set("/repo/fresh.rs", true, Some("2026-06-01T00:00:00Z"));
        fs.set("/repo/modified.rs", true, Some("2026-07-05T00:00:00Z"));
        let out = check_paths_staleness(
            &[
                "fresh.rs".to_string(),
                "modified.rs".to_string(),
                "deleted.rs".to_string(),
            ],
            "2026-07-01T00:00:00Z",
            Some(Path::new("/repo")),
            &fs,
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(
            out.paths[0].touched_at.as_deref(),
            Some("2026-07-05T10:00:00Z")
        );
    }
}
