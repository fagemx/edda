//! The **import** trigger for the committed cross-machine mirror (GH-671).
//!
//! `fleet.ledger-sync-trigger` splits the carrier in two: export runs at wave
//! close from the post-merge step (`scripts/fleet/ratify-merged.sh`), and
//! import runs here, at SessionStart. Without this half the mirror is a file
//! somebody has to remember to `edda sync --from-mirror` after every pull —
//! which is the remember-to-run failure the issue exists to remove.
//!
//! Four properties the trigger has to hold, in the order they bite:
//!
//! 1. **In-process, never a subprocess.** SessionStart runs on every session
//!    of every project; a spawn costs ~2.7 s on this workstation, and paying
//!    it to discover "nothing to import" is the cost that would get this
//!    trigger switched off.
//! 2. **No-op unless the mirror moved.** The stamp last imported is recorded
//!    in `state/mirror_import.json`; an unchanged stamp returns before the
//!    ledger is opened, so the steady-state cost is one `is_file` plus two
//!    small reads. A project with no `docs/decisions/INDEX.md` — every
//!    single-machine project — pays only the `is_file`.
//! 3. **Never fails or delays session start.** Every fallible step degrades to
//!    `None`. A broken mirror must not cost anyone a session; the import is
//!    convenience, and the manual `edda sync --from-mirror` still reports the
//!    real error.
//! 4. **Visible.** An import that happened silently is indistinguishable from
//!    one that did not, so the caller injects the returned line into the pack.

use edda_ledger::sync::{sync_from_mirror, MirrorSource};
use std::path::{Path, PathBuf};

/// Repo-relative mirror directory, fixed by the ratified
/// `ledger.cross-machine-projection` clause (1): the mirror is committed under
/// `docs/decisions/`.
const MIRROR_DIR: &str = "docs/decisions";

/// Import the committed mirror if it moved since the last session, and return
/// the line describing what happened. `None` means nothing to say: no mirror
/// in this repo, an unchanged stamp, or a failure that must stay silent.
pub fn import_on_session_start(cwd: &str, project_id: &str) -> Option<String> {
    let root = edda_ledger::EddaPaths::find_root(Path::new(cwd))?;
    let mirror_dir = root.join(MIRROR_DIR);
    let index = mirror_dir.join("INDEX.md");
    // The single-machine exit, and the reason this costs nothing in projects
    // that never adopted the mirror.
    if !index.is_file() {
        return None;
    }

    let stamp = read_stamp(&index)?;
    let state = state_path(project_id);
    if read_state(&state).as_deref() == Some(stamp.as_str()) {
        return None;
    }

    let ledger = edda_ledger::Ledger::open(&root).ok()?;
    let result = sync_from_mirror(&ledger, &MirrorSource { mirror_dir }, false).ok()?;

    // Recorded only after a successful import: a failed run must retry next
    // session rather than mark the stamp seen and go quiet forever.
    write_state(&state, &stamp);

    render_line(&result, &stamp)
}

/// The `- **Exported at**:` stamp, which is the mirror's identity for
/// no-op purposes: `edda export md` rewrites it on every run.
fn read_stamp(index: &Path) -> Option<String> {
    let text = std::fs::read_to_string(index).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("- **Exported at**:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn state_path(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("mirror_import.json")
}

fn read_state(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("exported_at")?.as_str().map(str::to_string)
}

fn write_state(path: &Path, stamp: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = serde_json::json!({ "exported_at": stamp }).to_string();
    // This file is a no-op cache, not a record: the decisions themselves are
    // already in the ledger by the time this runs. Losing it costs one
    // redundant import next session, which dedups to nothing, while
    // propagating would fail a SessionStart over a note-to-self and break
    // property 3 above.
    // swallow-ok: a lost cache write costs one redundant no-op import.
    let _ = std::fs::write(path, body);
}

/// What the session is told. An import with nothing new stays silent — the
/// stamp moved but carried no decision this ledger did not already have, and
/// a line saying "imported 0" every morning is how a signal stops being read.
fn render_line(result: &edda_ledger::sync::SyncResult, stamp: &str) -> Option<String> {
    if result.imported.is_empty() && result.conflicts.is_empty() {
        return None;
    }
    let machine = result
        .mirror
        .as_ref()
        .map(|m| m.source_name.clone())
        .unwrap_or_else(|| "?".to_string());
    let mut line = format!(
        "## Cross-machine mirror\n\nImported {} decision(s) from `{machine}` (mirror exported {stamp}).",
        result.imported.len()
    );
    if !result.conflicts.is_empty() {
        // #394 merge-never-overwrite: a conflicting value landed inactive, so
        // it is invisible to `edda ask` until someone resolves it. Saying so
        // here is the only place a session learns it needs resolving.
        line.push_str(&format!(
            " {} conflicted with a local decision and were imported **inactive** — resolve with `edda ask <key>`.",
            result.conflicts.len()
        ));
    }
    if result
        .mirror
        .as_ref()
        .is_some_and(|m| m.freshness.is_stale())
    {
        line.push_str(" ⚠ The mirror itself is stale; re-export on the source machine.");
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_ledger::sync::{MirrorFreshness, MirrorImportMeta, SyncResult};

    fn meta(exported_at: Option<&str>, age_hours: Option<f64>) -> MirrorImportMeta {
        MirrorImportMeta {
            source_id: "mirror:4090".to_string(),
            source_name: "4090".to_string(),
            freshness: MirrorFreshness {
                exported_at: exported_at.map(str::to_string),
                machine: Some("4090".to_string()),
                age_hours,
                threshold_hours: 24,
            },
        }
    }

    fn imported(key: &str) -> edda_ledger::sync::ImportedDecision {
        edda_ledger::sync::ImportedDecision {
            key: key.to_string(),
            value: "v".to_string(),
            source_project: "4090".to_string(),
            source_event_id: "evt_x".to_string(),
        }
    }

    #[test]
    fn reads_the_index_stamp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = dir.path().join("INDEX.md");
        std::fs::write(
            &index,
            "# Index\n\n- **Exported at**: 2026-09-07T03:00:00Z\n- **Exporting machine**: 4090\n",
        )
        .expect("write");
        assert_eq!(
            read_stamp(&index).as_deref(),
            Some("2026-09-07T03:00:00Z"),
            "the stamp is what makes an unchanged mirror a no-op"
        );
    }

    #[test]
    fn a_mirror_with_no_stamp_reads_as_no_stamp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = dir.path().join("INDEX.md");
        std::fs::write(&index, "# Index\n\nno stamp here\n").expect("write");
        assert!(read_stamp(&index).is_none());
        // An empty value is not a stamp either — it would compare equal to
        // itself forever and wedge the no-op check on.
        let empty = dir.path().join("EMPTY.md");
        std::fs::write(&empty, "- **Exported at**:   \n").expect("write");
        assert!(read_stamp(&empty).is_none());
    }

    #[test]
    fn state_round_trips_and_a_missing_file_is_not_a_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state").join("mirror_import.json");
        assert_eq!(read_state(&path), None, "never imported ⇒ never a no-op");
        write_state(&path, "2026-09-07T03:00:00Z");
        assert_eq!(read_state(&path).as_deref(), Some("2026-09-07T03:00:00Z"));
        write_state(&path, "2026-09-07T09:00:00Z");
        assert_eq!(read_state(&path).as_deref(), Some("2026-09-07T09:00:00Z"));
    }

    #[test]
    fn a_stamp_that_moved_but_imported_nothing_stays_silent() {
        // The common case after a pull that only touched unrelated decisions:
        // the stamp is new, the import is a no-op. Announcing it every session
        // is how the line stops being read.
        let mut r = SyncResult {
            skipped: 12,
            ..Default::default()
        };
        r.mirror = Some(meta(Some("2026-09-07T03:00:00Z"), Some(1.0)));
        assert_eq!(render_line(&r, "2026-09-07T03:00:00Z"), None);
    }

    #[test]
    fn an_import_names_its_count_machine_and_stamp() {
        let mut r = SyncResult::default();
        r.imported.push(imported("db.engine"));
        r.imported.push(imported("auth.method"));
        r.mirror = Some(meta(Some("2026-09-07T03:00:00Z"), Some(1.0)));
        let line = render_line(&r, "2026-09-07T03:00:00Z").expect("import is visible");
        assert!(line.contains("Imported 2 decision(s)"), "{line}");
        assert!(line.contains("4090"), "{line}");
        assert!(line.contains("2026-09-07T03:00:00Z"), "{line}");
        assert!(
            !line.contains("inactive"),
            "no conflicts ⇒ no conflict text"
        );
        assert!(!line.contains("stale"), "fresh mirror ⇒ no stale text");
    }

    #[test]
    fn conflicts_are_named_because_they_import_invisible() {
        let mut r = SyncResult::default();
        r.imported.push(imported("db.engine"));
        r.conflicts.push(edda_ledger::sync::ConflictInfo {
            key: "db.engine".to_string(),
            local_value: "sqlite".to_string(),
            remote_value: "postgres".to_string(),
            source_project: "4090".to_string(),
        });
        r.mirror = Some(meta(Some("2026-09-07T03:00:00Z"), Some(1.0)));
        let line = render_line(&r, "2026-09-07T03:00:00Z").expect("import is visible");
        assert!(line.contains("1 conflicted"), "{line}");
        assert!(line.contains("inactive"), "{line}");
    }

    #[test]
    fn a_stale_mirror_says_so_even_when_the_import_worked() {
        let mut r = SyncResult::default();
        r.imported.push(imported("db.engine"));
        // Unreadable age ⇒ stale, same rule as the import-time warning.
        r.mirror = Some(meta(None, None));
        let line = render_line(&r, "?").expect("import is visible");
        assert!(line.contains("stale"), "{line}");
    }

    /// Write the smallest mirror the importer accepts. Full export/import
    /// fidelity is proved against the real `edda export md` writer in
    /// `edda-cli`'s `cmd_sync` tests; what this exercises is the glue —
    /// resolve root, read stamp, open ledger, import, record state.
    fn write_mirror(dir: &Path, stamp: &str, key: &str, value: &str, event_id: &str) {
        let decisions = dir.join("decisions");
        std::fs::create_dir_all(&decisions).expect("mirror dir");
        std::fs::write(
            dir.join("INDEX.md"),
            format!("# Ledger\n\n- **Exported at**: {stamp}\n- **Exporting machine**: 4090\n"),
        )
        .expect("index");
        let domain = key.split('.').next().expect("domain");
        std::fs::write(
            decisions.join(format!("{domain}.md")),
            format!(
                "# Domain: `{domain}`\n\n## `{key}`\n\n\
                 - **Value**: `{value}`\n\
                 - **Reason**: recorded on the source machine\n\
                 - **Branch/ts**: `main` · 2026-09-07T02:00:00Z\n\
                 - **Governance**: unratified (agent)\n\
                 - **Scope**: local\n\
                 - **Authority**: agent\n\
                 - **Reversibility**: medium\n\
                 - **event_id**: `{event_id}`\n"
            ),
        )
        .expect("domain file");
    }

    /// The wiring the trigger exists for: SessionStart pulls what another
    /// machine decided, and says so — then stays out of the way.
    #[test]
    fn session_start_imports_the_mirror_once_and_is_a_no_op_after() {
        let _store = crate::isolated_store();
        let project_id = "mirror_import_session_start";
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        edda_ledger::Ledger::open_or_init(&repo).expect("ledger");
        let cwd = repo.to_str().expect("utf-8 path");

        // A project with no mirror is the single-machine case, and must cost
        // nothing and say nothing.
        assert_eq!(import_on_session_start(cwd, project_id), None);

        write_mirror(
            &repo.join("docs").join("decisions"),
            "2026-09-07T02:00:00Z",
            "fleet.merge-authority",
            "controller-merges-on-current-head-lgtm",
            "evt_from_machine_a",
        );

        let line = import_on_session_start(cwd, project_id).expect("first session imports");
        assert!(line.contains("Imported 1 decision(s)"), "{line}");
        assert!(line.contains("4090"), "the source machine is named: {line}");

        // The decision is actually in this ledger, not merely announced.
        let ledger = edda_ledger::Ledger::open(&repo).expect("ledger");
        let row = ledger
            .find_active_decision("main", "fleet.merge-authority")
            .expect("query")
            .expect("imported decision is visible");
        assert_eq!(row.value, "controller-merges-on-current-head-lgtm");

        // Second session, unchanged stamp: silent, and no second import.
        assert_eq!(
            import_on_session_start(cwd, project_id),
            None,
            "an unchanged stamp must not re-announce or re-import"
        );
        assert_eq!(
            ledger
                .iter_events_by_type("decision_import")
                .expect("events")
                .len(),
            1
        );
    }

    /// A mirror the importer cannot read must cost the session nothing —
    /// property 3. Before this, a malformed domain file would have propagated
    /// out of `sync_from_mirror` and up through SessionStart.
    #[test]
    fn a_broken_mirror_does_not_fail_session_start() {
        let _store = crate::isolated_store();
        let project_id = "mirror_import_broken";
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        edda_ledger::Ledger::open_or_init(&repo).expect("ledger");

        let mirror = repo.join("docs").join("decisions");
        let decisions = mirror.join("decisions");
        std::fs::create_dir_all(&decisions).expect("mirror dir");
        std::fs::write(
            mirror.join("INDEX.md"),
            "# Ledger\n\n- **Exported at**: 2026-09-07T02:00:00Z\n",
        )
        .expect("index");
        // A decision section with no `event_id` line — `finish_mirror_decision`
        // rejects the whole run.
        std::fs::write(
            decisions.join("fleet.md"),
            "# Domain: `fleet`\n\n## `fleet.broken`\n\n- **Value**: `x`\n",
        )
        .expect("domain file");

        assert_eq!(
            import_on_session_start(repo.to_str().expect("utf-8 path"), project_id),
            None,
            "a broken mirror is silence, never a failed session start"
        );
    }
}
