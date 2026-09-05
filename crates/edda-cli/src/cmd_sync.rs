//! `edda sync` — pull shared decisions from group members, or import a
//! committed markdown mirror from another machine (GH-671).

use edda_ledger::sync::SyncSource;
use std::path::Path;

/// Build sync sources from registry group members.
fn sources_from_group(repo_root: &Path) -> Vec<SyncSource> {
    edda_store::registry::list_group_members(repo_root)
        .into_iter()
        .map(|entry| SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: std::path::PathBuf::from(&entry.path),
        })
        .collect()
}

/// Build sync sources from a specific project name in the registry.
fn sources_from_name(name: &str) -> Vec<SyncSource> {
    edda_store::registry::list_projects()
        .into_iter()
        .filter(|p| p.name == name)
        .map(|entry| SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: std::path::PathBuf::from(&entry.path),
        })
        .collect()
}

pub fn execute(
    repo_root: &Path,
    from: Option<&str>,
    from_mirror: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    if let Some(mirror) = from_mirror {
        return execute_from_mirror(repo_root, mirror, dry_run);
    }

    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let target_project_id = edda_store::project_id(repo_root);

    let sources = if let Some(name) = from {
        let sources = sources_from_name(name);
        if sources.is_empty() {
            anyhow::bail!("no registered project named '{name}'");
        }
        sources
    } else {
        let sources = sources_from_group(repo_root);
        if sources.is_empty() {
            let group = edda_store::registry::project_group(repo_root);
            if group.is_none() {
                anyhow::bail!("this project has no group. Use `edda group set <name>` first.");
            }
            println!("No group members found.");
            return Ok(());
        }
        sources
    };

    if dry_run {
        println!("Dry run: showing what would be imported.\n");
    }

    let result =
        edda_ledger::sync::sync_from_sources(&ledger, &sources, &target_project_id, dry_run)?;

    if !result.errors.is_empty() {
        eprintln!("Warnings ({}):", result.errors.len());
        for e in &result.errors {
            eprintln!("  {}: {}", e.project_name, e.error);
        }
        eprintln!();
    }

    if result.imported.is_empty() && result.conflicts.is_empty() {
        println!("Already up to date ({} skipped).", result.skipped);
        return Ok(());
    }

    if !result.imported.is_empty() {
        let verb = if dry_run { "Would import" } else { "Imported" };
        println!("{verb} {} decision(s):", result.imported.len());
        for d in &result.imported {
            println!("  {} = {} (from {})", d.key, d.value, d.source_project);
        }
    }

    if !result.conflicts.is_empty() {
        println!("\nConflicts ({}):", result.conflicts.len());
        for c in &result.conflicts {
            println!(
                "  {}: local={}, remote={} (from {})",
                c.key, c.local_value, c.remote_value, c.source_project
            );
        }
        if !dry_run {
            println!("  Conflicting decisions imported as inactive. Resolve manually.");
        }
    }

    if result.skipped > 0 {
        println!("\n{} already imported (skipped).", result.skipped);
    }

    Ok(())
}

/// `edda sync --from-mirror <dir>` — import decisions from a committed
/// markdown mirror (GH-671), e.g. `docs/ledger` checked out from git after
/// another machine ran `scripts/fleet/ledger-sync.sh`.
fn execute_from_mirror(repo_root: &Path, mirror: &str, dry_run: bool) -> anyhow::Result<()> {
    let mirror_dir = resolve_mirror_dir(repo_root, mirror)?;
    let ledger = edda_ledger::Ledger::open(repo_root)?;

    if dry_run {
        println!("Dry run: showing what would be imported.\n");
    }

    let source = edda_ledger::sync::MirrorSource { mirror_dir };
    let result = edda_ledger::sync::sync_from_mirror(&ledger, &source, dry_run)?;

    // Death visibility first: a stale (or unreadable) INDEX stamp warns
    // before any summary, never silently.
    if let Some(meta) = &result.mirror {
        if meta.freshness.is_stale() {
            println!("{}", stale_warning_line(meta));
            println!();
        }
    }

    if result.imported.is_empty() && result.conflicts.is_empty() {
        println!("Already up to date ({} skipped).", result.skipped);
        return Ok(());
    }

    if let Some(meta) = &result.mirror {
        let stamp = meta.freshness.exported_at.as_deref().unwrap_or("?");
        println!("Mirror source: {} (exported {stamp})", meta.source_name);
    }

    if !result.imported.is_empty() {
        let verb = if dry_run { "Would import" } else { "Imported" };
        println!("{verb} {} decision(s):", result.imported.len());
        for d in &result.imported {
            println!(
                "  {} = {} (from mirror:{})",
                d.key, d.value, d.source_project
            );
        }
    }

    if !result.conflicts.is_empty() {
        println!("\nConflicts ({}):", result.conflicts.len());
        for c in &result.conflicts {
            println!(
                "  {}: local={}, remote={} (from mirror:{})",
                c.key, c.local_value, c.remote_value, c.source_project
            );
        }
        if !dry_run {
            println!("  Conflicting decisions imported as inactive. Resolve manually.");
        }
    }

    if result.skipped > 0 {
        println!("\n{} already present (skipped).", result.skipped);
    }

    Ok(())
}

/// Resolve the mirror directory: absolute as-is, otherwise relative to the
/// workspace root (the brief's repo-relative form, `docs/ledger`).
fn resolve_mirror_dir(repo_root: &Path, mirror: &str) -> anyhow::Result<std::path::PathBuf> {
    let candidate = std::path::Path::new(mirror);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repo_root.join(candidate)
    };
    if !resolved.join("INDEX.md").is_file() {
        anyhow::bail!(
            "no mirror INDEX.md at {} — run `edda export md --out <dir>` on the source machine first (docs/ledger is the fleet default)",
            resolved.display()
        );
    }
    Ok(resolved)
}

/// The visible stale signal (GH-671): names threshold, stamp and machine so
/// a reader can tell how dead the mirror is and where to re-export.
fn stale_warning_line(meta: &edda_ledger::sync::MirrorImportMeta) -> String {
    let f = &meta.freshness;
    let age = match f.age_hours {
        Some(h) => format!(
            "INDEX stamp is {h:.1}h old (threshold {}h)",
            f.threshold_hours
        ),
        None => format!(
            "INDEX stamp missing or unreadable (threshold {}h)",
            f.threshold_hours
        ),
    };
    format!(
        "⚠ STALE MIRROR: {age} — exported {} by {}. Decisions may be out of date; re-export on the source machine (scripts/fleet/ledger-sync.sh).",
        f.exported_at.as_deref().unwrap_or("?"),
        f.machine.as_deref().unwrap_or("?"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_ledger::Ledger;
    use std::fs;

    /// The verbatim value and six-point reason the binding carrier demands:
    /// `fleet.lane-profile` quotes the operator ruling, never the
    /// `actor-is-profile` gloss from the design doc.
    const LANE_PROFILE_VALUE: &str = "agent-actor-is-the-profile";
    const LANE_PROFILE_REASON: &str = "operator ruling 2026-09-02 (six-row table): (1) model — per-actor default execution model; (2) thinking — per-actor thinking level; (3) tools — per-actor tool policy; (4) budget — per-actor spend cap; (5) permission mode — per-actor; (6) session dir — per-actor. The agent actor IS the profile: no fourth config surface.";
    const GLOSS: &str = "actor-is-profile";

    fn decide_event(ledger: &Ledger, key: &str, value: &str, reason: &str) {
        let branch = ledger.head_branch().unwrap();
        let parent = ledger.last_event_hash().unwrap();
        let dp = edda_core::types::DecisionPayload {
            key: key.to_string(),
            value: value.to_string(),
            reason: Some(reason.to_string()),
            scope: None,
            authority: Some("agent".to_string()),
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let ev = edda_core::event::new_decision_event(&branch, parent.as_deref(), "worker-1", &dp)
            .unwrap();
        ledger.append_event(&ev).unwrap();
    }

    fn ratify_event(ledger: &Ledger, key: &str, by: &str) {
        let branch = ledger.head_branch().unwrap();
        let parent = ledger.last_event_hash().unwrap();
        let ev =
            edda_core::event::new_decision_ratify_event(&branch, parent.as_deref(), key, by, None)
                .unwrap();
        ledger.append_event(&ev).unwrap();
    }

    /// The acceptance round trip (GH-671): two tempdirs as two machines.
    /// Machine A records the three acceptance keys and exports the committed
    /// mirror; machine B (empty ledger) imports it.
    #[test]
    fn mirror_round_trip_preserves_verbatim_value_reason_governance_and_source_machine() {
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("machine-a");
        let b_root = dir.path().join("machine-b");
        fs::create_dir_all(&a_root).unwrap();
        fs::create_dir_all(&b_root).unwrap();

        let a = Ledger::open_or_init(&a_root).unwrap();
        decide_event(
            &a,
            "fleet.lane-profile",
            LANE_PROFILE_VALUE,
            LANE_PROFILE_REASON,
        );
        let original_lane_event_id = a
            .find_active_decision("main", "fleet.lane-profile")
            .unwrap()
            .unwrap()
            .event_id;
        decide_event(
            &a,
            "fleet.merge-authority",
            "controller-merges-on-current-head-lgtm",
            "operator ruling 2026-09-02: the controller may merge once review posts LGTM with P0=0/P1=0, CI is green, and the reviewed SHA still equals the PR head.",
        );
        decide_event(
            &a,
            "coord.session-identity",
            "label-and-machine-explicit",
            "session identity (label@machine, collision warning) is #685 scope; the committed mirror carries the decision, not the implementation.",
        );
        ratify_event(&a, "fleet.lane-profile", "operator");
        drop(a);

        // What the trigger runs: export to the git-tracked mirror directory.
        let mirror = a_root.join("docs").join("ledger");
        crate::cmd_export::execute(&a_root, &mirror, false, Some("4090")).unwrap();

        // A hand-added INDEX gloss must never mint a decision value.
        let index_path = mirror.join("INDEX.md");
        let mut index = fs::read_to_string(&index_path).unwrap();
        index.push_str(&format!("\n- **Gloss**: fleet.lane-profile={GLOSS}\n"));
        fs::write(&index_path, index).unwrap();

        // Machine B: empty ledger, imports from the checked-out mirror.
        Ledger::open_or_init(&b_root).unwrap();
        execute(&b_root, None, Some(mirror.to_str().unwrap()), false).unwrap();

        let b = Ledger::open(&b_root).unwrap();

        // 1. Verbatim value + six-point reason survive the round trip.
        let lane = b
            .find_active_decision("main", "fleet.lane-profile")
            .unwrap()
            .expect("lane-profile visible on B");
        assert_eq!(
            lane.value, LANE_PROFILE_VALUE,
            "verbatim value, not a gloss"
        );
        assert_eq!(lane.value, LANE_PROFILE_VALUE);
        assert_ne!(lane.value, GLOSS);
        assert_eq!(
            lane.reason, LANE_PROFILE_REASON,
            "reason quoted, never paraphrased"
        );

        // 2. Original actor + source machine recorded on the import event.
        assert_eq!(lane.authority, "agent");
        let imports = b.iter_events_by_type("decision_import").unwrap();
        let lane_import = imports
            .iter()
            .find(|e| e.payload["decision"]["key"] == "fleet.lane-profile")
            .expect("lane-profile import event on B");
        assert_eq!(lane_import.payload["source_project_id"], "mirror:4090");
        assert_eq!(
            lane_import.payload["source_event_id"],
            original_lane_event_id.as_str(),
            "provenance points at A's original decision event"
        );

        // 3. Ratified/unratified preserved through standard derivation.
        let ratified = b.ratified_decisions_map().unwrap();
        assert!(ratified.contains_key(&lane.event_id));
        assert_eq!(ratified[&lane.event_id].ratified_by, "operator");

        // 4. The other acceptance keys round trip too.
        assert!(b
            .find_active_decision("main", "fleet.merge-authority")
            .unwrap()
            .is_some());
        assert!(b
            .find_active_decision("main", "coord.session-identity")
            .unwrap()
            .is_some());

        // 5. `edda ask` sees the decision with governance attached.
        let opts = edda_ask::AskOptions {
            limit: 10,
            ..Default::default()
        };
        let result = edda_ask::ask(&b, "lane-profile", &opts, None).unwrap();
        let hit = result
            .decisions
            .iter()
            .find(|d| d.key == "fleet.lane-profile")
            .expect("ask must see the imported decision");
        assert_eq!(hit.value, LANE_PROFILE_VALUE);
        assert_eq!(hit.governance.status, "ratified");
        assert_eq!(hit.governance.ratified_by.as_deref(), Some("operator"));
    }

    /// #394 through the mirror: same key, different value → merge, never
    /// overwrite. B's local value stays active; A's value imports inactive.
    #[test]
    fn mirror_import_conflict_imports_inactive_local_value_stays_active() {
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("machine-a");
        let b_root = dir.path().join("machine-b");
        fs::create_dir_all(&a_root).unwrap();
        fs::create_dir_all(&b_root).unwrap();

        let a = Ledger::open_or_init(&a_root).unwrap();
        decide_event(
            &a,
            "fleet.lane-profile",
            LANE_PROFILE_VALUE,
            LANE_PROFILE_REASON,
        );
        drop(a);
        let mirror = a_root.join("docs").join("ledger");
        crate::cmd_export::execute(&a_root, &mirror, false, Some("4090")).unwrap();

        // B already holds the gloss as its local active value.
        let b = Ledger::open_or_init(&b_root).unwrap();
        decide_event(
            &b,
            "fleet.lane-profile",
            GLOSS,
            "mirrored from #613: actor 即 profile（#593 設計中）",
        );
        drop(b);

        execute(&b_root, None, Some(mirror.to_str().unwrap()), false).unwrap();

        let b = Ledger::open(&b_root).unwrap();
        let active = b
            .find_active_decision("main", "fleet.lane-profile")
            .unwrap()
            .unwrap();
        assert_eq!(
            active.value, GLOSS,
            "merge, do not overwrite: local value stays active"
        );

        let timeline = b
            .decision_timeline("fleet.lane-profile", None, None)
            .unwrap();
        assert!(
            timeline
                .iter()
                .any(|d| d.value == LANE_PROFILE_VALUE && d.status == "superseded"),
            "remote value imported inactive (#394)"
        );
    }

    /// A machine importing its own mirror must be a no-op: the original
    /// events are already local, so nothing re-imports over them.
    #[test]
    fn mirror_self_import_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("machine-a");
        fs::create_dir_all(&a_root).unwrap();

        let a = Ledger::open_or_init(&a_root).unwrap();
        decide_event(
            &a,
            "fleet.lane-profile",
            LANE_PROFILE_VALUE,
            LANE_PROFILE_REASON,
        );
        let original_event_id = a
            .find_active_decision("main", "fleet.lane-profile")
            .unwrap()
            .unwrap()
            .event_id;
        drop(a);
        let mirror = a_root.join("docs").join("ledger");
        crate::cmd_export::execute(&a_root, &mirror, false, Some("4090")).unwrap();

        execute(&a_root, None, Some(mirror.to_str().unwrap()), false).unwrap();

        let a = Ledger::open(&a_root).unwrap();
        let timeline = a
            .decision_timeline("fleet.lane-profile", None, None)
            .unwrap();
        assert_eq!(timeline.len(), 1, "no duplicate decision rows");
        assert_eq!(
            timeline[0].event_id, original_event_id,
            "original row untouched by self-import"
        );
        assert!(a.iter_events_by_type("decision_import").unwrap().is_empty());
    }

    /// The stale signal must be a visible line naming threshold, stamp and
    /// machine — death visibility, not a silent best-effort read.
    #[test]
    fn stale_mirror_warning_line_names_threshold_stamp_and_machine() {
        let line = stale_warning_line(&edda_ledger::sync::MirrorImportMeta {
            source_id: "mirror:4090".into(),
            source_name: "4090".into(),
            freshness: edda_ledger::sync::MirrorFreshness {
                exported_at: Some("2026-09-04T03:00:00Z".into()),
                machine: Some("4090".into()),
                age_hours: Some(30.5),
                threshold_hours: edda_ledger::sync::DEFAULT_MIRROR_STALE_HOURS,
            },
        });
        assert!(line.contains("STALE"), "{line}");
        assert!(line.contains("24"), "threshold visible: {line}");
        assert!(line.contains("4090"), "machine visible: {line}");
        assert!(
            line.contains("2026-09-04T03:00:00Z"),
            "stamp visible: {line}"
        );
    }
}
