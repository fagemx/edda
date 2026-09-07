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
/// markdown mirror (GH-671), e.g. `docs/decisions` checked out from git after
/// another machine ran `scripts/fleet/ratify-merged.sh`.
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
/// workspace root (the brief's repo-relative form, `docs/decisions`).
fn resolve_mirror_dir(repo_root: &Path, mirror: &str) -> anyhow::Result<std::path::PathBuf> {
    let candidate = std::path::Path::new(mirror);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repo_root.join(candidate)
    };
    if !resolved.join("INDEX.md").is_file() {
        anyhow::bail!(
            "no mirror INDEX.md at {} — run `edda export md --out <dir>` on the source machine first (docs/decisions is the fleet default)",
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
        "⚠ STALE MIRROR: {age} — exported {} by {}. Decisions may be out of date; re-export on the source machine (scripts/fleet/ratify-merged.sh).",
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
        decide_event_cites(ledger, key, value, reason, None);
    }

    fn decide_event_cites(
        ledger: &Ledger,
        key: &str,
        value: &str,
        reason: &str,
        cites: Option<Vec<String>>,
    ) {
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
            cites,
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
        let mirror = a_root.join("docs").join("decisions");
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
        let mirror = a_root.join("docs").join("decisions");
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
        let mirror = a_root.join("docs").join("decisions");
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
    /// GH-761 through the mirror: the citation chain is the authority a
    /// decision rests on, and `cites` lives in the event payload rather than
    /// a projected column (`decision.cites=event-payload-not-sqlite-column`).
    /// A mirror that drops it lies by omission about why the decision binds,
    /// which is exactly what `quote-never-paraphrase` forbids.
    #[test]
    fn mirror_round_trip_carries_the_citation_chain() {
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("machine-a");
        let b_root = dir.path().join("machine-b");
        fs::create_dir_all(&a_root).unwrap();
        fs::create_dir_all(&b_root).unwrap();

        let a = Ledger::open_or_init(&a_root).unwrap();
        decide_event_cites(
            &a,
            "fleet.merge-authority",
            "controller-merges-on-current-head-lgtm",
            "operator ruling 2026-09-02",
            Some(vec![
                "operator:2026-09-02".to_string(),
                "issue:#671".to_string(),
            ]),
        );
        // Paired presence control: a decision with no citations must round
        // trip with no `Cites` line, so the assertion below is testing the
        // carrier and not the absence of the feature.
        decide_event(
            &a,
            "coord.session-identity",
            "label-and-machine-explicit",
            "identity half is #685 scope",
        );
        drop(a);

        let mirror = a_root.join("docs").join("decisions");
        crate::cmd_export::execute(&a_root, &mirror, false, Some("4090")).unwrap();

        let fleet_md = fs::read_to_string(mirror.join("decisions").join("fleet.md")).unwrap();
        assert!(
            fleet_md.contains("- **Cites**: `operator:2026-09-02`, `issue:#671`"),
            "citations are exported: {fleet_md}"
        );
        let coord_md = fs::read_to_string(mirror.join("decisions").join("coord.md")).unwrap();
        assert!(
            !coord_md.contains("**Cites**"),
            "an uncited decision gains no Cites line: {coord_md}"
        );

        Ledger::open_or_init(&b_root).unwrap();
        execute(&b_root, None, Some(mirror.to_str().unwrap()), false).unwrap();

        // `edda ratify --by-rule` reads citations from
        // `payload["decision"]["cites"]`, keyed on the row event_id — which
        // for an import is the import event own id. So this is the shape the
        // rule actually consumes on B, not a private encoding.
        let b = Ledger::open(&b_root).unwrap();
        let row = b
            .find_active_decision("main", "fleet.merge-authority")
            .unwrap()
            .expect("merge-authority visible on B");
        let import = b
            .get_event(&row.event_id)
            .unwrap()
            .expect("row resolves to its import event");
        let cites: Vec<&str> = import.payload["decision"]["cites"]
            .as_array()
            .expect("cites survive the mirror")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(cites, vec!["operator:2026-09-02", "issue:#671"]);

        let uncited_row = b
            .find_active_decision("main", "coord.session-identity")
            .unwrap()
            .expect("session-identity visible on B");
        let uncited = b.get_event(&uncited_row.event_id).unwrap().unwrap();
        assert!(
            uncited.payload["decision"]["cites"].is_null(),
            "no citations in, no citations out"
        );
    }

    /// GH-671 doneWhen 2, freshness clause: the stale signal has to survive
    /// the import command that printed it. A decision that arrived over a dead
    /// mirror must read as dead on B, not as one decided here this morning.
    #[test]
    fn ask_marks_a_decision_that_rode_a_stale_mirror() {
        let fresh = "2026-09-07T03:00:00Z";
        let stale_hit = ask_hit_after_import_stamped(None);
        let fresh_hit = ask_hit_after_import_stamped(Some(fresh));

        // Paired presence control: the same round trip differing only in the
        // stamp, so a passing assertion cannot be the annotator never running.
        let stale = stale_hit.mirror.expect("mirror provenance recorded");
        assert!(stale.is_stale, "an unreadable stamp reads as dead");
        assert_eq!(stale.machine, "4090", "the source machine is named");

        let fresh_origin = fresh_hit.mirror.expect("mirror provenance recorded");
        assert_eq!(fresh_origin.exported_at.as_deref(), Some(fresh));
    }

    /// Run the whole A-export-B-import-ask path with the mirror
    /// `Exported at` stamp rewritten, and return the annotated hit.
    fn ask_hit_after_import_stamped(stamp: Option<&str>) -> edda_ask::DecisionHit {
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

        let mirror = a_root.join("docs").join("decisions");
        crate::cmd_export::execute(&a_root, &mirror, false, Some("4090")).unwrap();

        // Age the mirror by rewriting its stamp — the same edit a checkout
        // that nobody re-exported presents days later. `None` drops the line
        // entirely, the unreadable case.
        let index_path = mirror.join("INDEX.md");
        let index = fs::read_to_string(&index_path).unwrap();
        let rewritten: Vec<String> = index
            .lines()
            .filter_map(|l| {
                if l.starts_with("- **Exported at**:") {
                    stamp.map(|s| format!("- **Exported at**: {s}"))
                } else {
                    Some(l.to_string())
                }
            })
            .collect();
        fs::write(&index_path, rewritten.join("\n")).unwrap();

        Ledger::open_or_init(&b_root).unwrap();
        execute(&b_root, None, Some(mirror.to_str().unwrap()), false).unwrap();

        let b = Ledger::open(&b_root).unwrap();
        let opts = edda_ask::AskOptions {
            limit: 10,
            ..Default::default()
        };
        let mut result = edda_ask::ask(&b, "fleet.lane-profile", &opts, None).unwrap();
        let origins = edda_ask::mirror::origins_for_hits(&b, &result.decisions);
        edda_ask::mirror::annotate_hits(&mut result.decisions, &origins);
        result
            .decisions
            .into_iter()
            .find(|d| d.key == "fleet.lane-profile")
            .expect("ask sees the imported decision")
    }

    /// A locally-decided row carries no mirror provenance — the reason the
    /// marker stays meaningful instead of decorating every decision.
    #[test]
    fn ask_does_not_mark_a_locally_decided_row() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("solo");
        fs::create_dir_all(&root).unwrap();
        let l = Ledger::open_or_init(&root).unwrap();
        decide_event(&l, "db.engine", "sqlite", "embedded, zero-config");

        let opts = edda_ask::AskOptions {
            limit: 10,
            ..Default::default()
        };
        let mut result = edda_ask::ask(&l, "db.engine", &opts, None).unwrap();
        let origins = edda_ask::mirror::origins_for_hits(&l, &result.decisions);
        edda_ask::mirror::annotate_hits(&mut result.decisions, &origins);
        let hit = result
            .decisions
            .iter()
            .find(|d| d.key == "db.engine")
            .expect("ask sees the local decision");
        assert!(hit.mirror.is_none());
        assert!(!edda_ask::format_human(&result).contains("stale-mirror"));
    }
}
