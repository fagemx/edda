// Tests for the cross-project sync engine and the committed markdown
// mirror import (GH-671). Split from sync.rs for the GH-779 file-length
// ratchet, mirroring sqlite_store/tests.rs.
use super::*;
use crate::ledger::{init_branches_json, init_head, init_workspace};
use crate::EddaPaths;
use edda_core::types::DecisionScope;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn setup_workspace() -> (std::path::PathBuf, Ledger) {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("edda_sync_test_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let paths = EddaPaths::discover(&tmp);
    init_workspace(&paths).unwrap();
    init_head(&paths, "main").unwrap();
    init_branches_json(&paths, "main").unwrap();
    let ledger = Ledger::open(&tmp).unwrap();
    (tmp, ledger)
}

fn write_shared_decision(ledger: &Ledger, key: &str, value: &str, reason: &str) {
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: Some(reason.to_string()),
        scope: Some(DecisionScope::Shared),
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
    ledger.append_event(&event).unwrap();
}

fn write_local_decision(ledger: &Ledger, key: &str, value: &str) {
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: None,
        scope: None,
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
    ledger.append_event(&event).unwrap();
}

#[test]
fn sync_empty_sources() {
    let (tmp, ledger) = setup_workspace();
    let result = sync_from_sources(&ledger, &[], "target_proj", false).unwrap();
    assert!(result.imported.is_empty());
    assert_eq!(result.skipped, 0);
    assert!(result.conflicts.is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn sync_imports_shared_decision() {
    let (tmp_src, src_ledger) = setup_workspace();
    let (tmp_tgt, tgt_ledger) = setup_workspace();

    write_shared_decision(&src_ledger, "api.version", "v3", "breaking change");

    let sources = vec![SyncSource {
        project_id: "source_proj".to_string(),
        project_name: "source".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    let result = sync_from_sources(&tgt_ledger, &sources, "target_proj", false).unwrap();
    assert_eq!(result.imported.len(), 1);
    assert_eq!(result.imported[0].key, "api.version");
    assert_eq!(result.imported[0].value, "v3");

    // Verify it was written to the ledger (use raw rows to check source_project_id)
    let decisions = tgt_ledger
        .sqlite
        .active_decisions(None, None, None, None, None)
        .unwrap();
    assert!(decisions.iter().any(|d| d.key == "api.version"
        && d.value == "v3"
        && d.source_project_id.as_deref() == Some("source_proj")));

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_skips_already_imported() {
    let (tmp_src, src_ledger) = setup_workspace();
    let (tmp_tgt, tgt_ledger) = setup_workspace();

    write_shared_decision(&src_ledger, "db.engine", "pg", "fast");

    let sources = vec![SyncSource {
        project_id: "src2".to_string(),
        project_name: "source2".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    // First sync
    let r1 = sync_from_sources(&tgt_ledger, &sources, "target_proj", false).unwrap();
    assert_eq!(r1.imported.len(), 1);

    // Second sync should skip
    let r2 = sync_from_sources(&tgt_ledger, &sources, "target_proj", false).unwrap();
    assert_eq!(r2.imported.len(), 0);
    assert_eq!(r2.skipped, 1);

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_detects_conflict() {
    let (tmp_src, src_ledger) = setup_workspace();
    let (tmp_tgt, tgt_ledger) = setup_workspace();

    // Local decision
    write_local_decision(&tgt_ledger, "api.version", "v2");

    // Remote shared decision with different value
    write_shared_decision(&src_ledger, "api.version", "v3", "breaking");

    let sources = vec![SyncSource {
        project_id: "src3".to_string(),
        project_name: "source3".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    let result = sync_from_sources(&tgt_ledger, &sources, "target_proj", false).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].local_value, "v2");
    assert_eq!(result.conflicts[0].remote_value, "v3");
    // Imported but as inactive (conflict)
    assert_eq!(result.imported.len(), 1);

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_preserves_governance_metadata() {
    let (tmp_src, source) = setup_workspace();
    let (tmp_tgt, target) = setup_workspace();
    let payload = edda_core::types::DecisionPayload {
        key: "security.auth".to_string(),
        value: "passkey".to_string(),
        reason: Some("phishing resistance".to_string()),
        scope: Some(DecisionScope::Shared),
        authority: Some("human".to_string()),
        affected_paths: Some(vec!["crates/auth/**".to_string()]),
        tags: Some(vec!["security".to_string(), "identity".to_string()]),
        review_after: Some("2027-01-01".to_string()),
        reversibility: Some("hard".to_string()),
        village_id: Some("village-alpha".to_string()),
    };
    let event = edda_core::event::new_decision_event("main", None, "system", &payload).unwrap();
    source.append_event(&event).unwrap();
    let sources = vec![SyncSource {
        project_id: "source_meta".to_string(),
        project_name: "source-meta".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    sync_from_sources(&target, &sources, "target", false).unwrap();
    let imported = target
        .sqlite
        .find_active_decision("main", "security.auth")
        .unwrap()
        .unwrap();

    assert_eq!(imported.authority, "human");
    assert_eq!(imported.affected_paths, r#"["crates/auth/**"]"#);
    assert_eq!(imported.tags, r#"["security","identity"]"#);
    assert_eq!(imported.review_after.as_deref(), Some("2027-01-01"));
    assert_eq!(imported.reversibility, "hard");
    assert_eq!(imported.village_id.as_deref(), Some("village-alpha"));
    assert_eq!(imported.scope, "shared");
    assert_eq!(imported.source_project_id.as_deref(), Some("source_meta"));
    assert_eq!(
        imported.source_event_id.as_deref(),
        Some(event.event_id.as_str())
    );

    let governed = target
        .query_by_paths(&["crates/auth/src/lib.rs"], Some("main"), None)
        .unwrap();
    assert_eq!(governed.len(), 1);
    assert_eq!(governed[0].key, "security.auth");

    let import_event = target.get_event(&imported.event_id).unwrap().unwrap();
    assert_eq!(import_event.refs.events, vec![event.event_id.clone()]);
    assert_eq!(import_event.refs.provenance.len(), 1);
    assert_eq!(
        import_event.refs.provenance[0].rel,
        edda_core::types::rel::IMPORTED_FROM
    );
    assert_eq!(import_event.refs.provenance[0].target, event.event_id);

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_keeps_one_active_decision_across_remote_sources() {
    let (tmp_a, ledger_a) = setup_workspace();
    let (tmp_b, ledger_b) = setup_workspace();
    let (tmp_tgt, target) = setup_workspace();
    write_shared_decision(&ledger_a, "api.version", "v3", "source a");
    write_shared_decision(&ledger_b, "api.version", "v4", "source b");

    let sources = vec![
        SyncSource {
            project_id: "source_a".to_string(),
            project_name: "source-a".to_string(),
            ledger_path: tmp_a.clone(),
        },
        SyncSource {
            project_id: "source_b".to_string(),
            project_name: "source-b".to_string(),
            ledger_path: tmp_b.clone(),
        },
    ];

    let result = sync_from_sources(&target, &sources, "target", false).unwrap();
    let active = target
        .sqlite
        .active_decisions(None, Some("api.version"), None, None, None)
        .unwrap();

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].value, "v3");

    let _ = std::fs::remove_dir_all(&tmp_a);
    let _ = std::fs::remove_dir_all(&tmp_b);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_replaces_same_value_import_without_duplicate_active_rows() {
    let (tmp_a, ledger_a) = setup_workspace();
    let (tmp_b, ledger_b) = setup_workspace();
    let (tmp_tgt, target) = setup_workspace();
    write_shared_decision(&ledger_a, "api.version", "v3", "source a");
    write_shared_decision(&ledger_b, "api.version", "v3", "source b");
    let sources = vec![
        SyncSource {
            project_id: "source_a".to_string(),
            project_name: "source-a".to_string(),
            ledger_path: tmp_a.clone(),
        },
        SyncSource {
            project_id: "source_b".to_string(),
            project_name: "source-b".to_string(),
            ledger_path: tmp_b.clone(),
        },
    ];

    let result = sync_from_sources(&target, &sources, "target", false).unwrap();
    let active = target
        .sqlite
        .active_decisions(None, Some("api.version"), None, None, None)
        .unwrap();

    assert!(result.conflicts.is_empty());
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].source_project_id.as_deref(), Some("source_b"));

    let _ = std::fs::remove_dir_all(&tmp_a);
    let _ = std::fs::remove_dir_all(&tmp_b);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_dry_run_does_not_write() {
    let (tmp_src, src_ledger) = setup_workspace();
    let (tmp_tgt, tgt_ledger) = setup_workspace();

    write_shared_decision(&src_ledger, "auth.method", "JWT", "stateless");

    let sources = vec![SyncSource {
        project_id: "src4".to_string(),
        project_name: "source4".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    let result = sync_from_sources(&tgt_ledger, &sources, "target_proj", true).unwrap();
    assert_eq!(result.imported.len(), 1);

    // Should not have written anything
    let decisions = tgt_ledger.active_decisions(None, None, None, None).unwrap();
    assert!(decisions.is_empty());

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn sync_ignores_local_scope_decisions() {
    let (tmp_src, src_ledger) = setup_workspace();
    let (tmp_tgt, tgt_ledger) = setup_workspace();

    // Write a local-scope decision to source
    write_local_decision(&src_ledger, "internal.key", "val");

    let sources = vec![SyncSource {
        project_id: "src5".to_string(),
        project_name: "source5".to_string(),
        ledger_path: tmp_src.clone(),
    }];

    let result = sync_from_sources(&tgt_ledger, &sources, "target_proj", false).unwrap();
    assert!(result.imported.is_empty());

    let _ = std::fs::remove_dir_all(&tmp_src);
    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

// ── Committed markdown mirror (GH-671) ────────────────────────────

/// Mirror markdown in the exact shape `edda export md` writes (plus the
/// GH-671 fields import needs). Used to test the parser and the import
/// engine independently of the renderer.
const MIRROR_FIXTURE: &str = concat!(
        "<!-- edda-ledger-export v1 — GENERATED FILE, DO NOT EDIT — SQLite ledger is authoritative -->\n",
        "# Domain: `fleet`\n\n",
        "2 active decision(s), sorted by key.\n\n",
        "## `fleet.lane-profile`\n\n",
        "- **Value**: `agent-actor-is-the-profile`\n",
        "- **Reason**: operator ruling (six points): (1) model; (2) thinking; (3) tools; (4) budget; (5) permission mode; (6) session dir.\n",
        "- **Branch/ts**: `main` · 2026-09-05T03:00:00Z\n",
        "- **Governance**: ratified by operator at 2026-09-05T04:00:00Z\n",
        "- **Scope**: local\n",
        "- **Authority**: agent\n",
        "- **Reversibility**: hard\n",
        "- **Review after**: 2027-01-01\n",
        "- **Village**: village-alpha\n",
        "- **event_id**: `evt_01lane`\n\n",
        "## `fleet.merge-authority`\n\n",
        "- **Value**: `controller-merges-on-current-head-lgtm`\n",
        "- **Reason**: operator ruling 2026-09-02.\n",
        "- **Branch/ts**: `main` · 2026-09-05T03:10:00Z\n",
        "- **Governance**: unratified (agent)\n",
        "- **Scope**: local\n",
        "- **Authority**: agent\n",
        "- **Affected paths**: `scripts/fleet/**`\n",
        "- **event_id**: `evt_01merge`\n",
    );

const MIRROR_ESCAPED_FIXTURE: &str = concat!(
    "<!-- edda-ledger-export v1 — GENERATED FILE, DO NOT EDIT -->\n",
    "# Domain: `esc`\n\n",
    "1 active decision(s), sorted by key.\n\n",
    "## `esc.value`\n\n",
    "- **Value**: `line one\\nline two \u{2014} with \\\\ backslash`\n",
    "- **Reason**: reason with\\nnewline and \\\\ backslash\n",
    "- **Branch/ts**: `main` · 2026-09-05T03:00:00Z\n",
    "- **Governance**: unratified (agent)\n",
    "- **Scope**: local\n",
    "- **Authority**: agent\n",
    "- **event_id**: `evt_01esc`\n",
);

fn write_mirror_tree(dir: &std::path::Path, index_body: &str) -> std::path::PathBuf {
    let decisions = dir.join("decisions");
    std::fs::create_dir_all(&decisions).unwrap();
    std::fs::write(dir.join("INDEX.md"), index_body).unwrap();
    std::fs::write(decisions.join("fleet.md"), MIRROR_FIXTURE).unwrap();
    dir.to_path_buf()
}

fn index_body_with_stamp(stamp: &str, machine: &str) -> String {
    format!(
            "- **Exported at**: {stamp}\n- **Exporting machine**: {machine}\n- **Total decisions**: 2\n"
        )
}

#[test]
fn mirror_parse_extracts_every_field_verbatim() {
    let parsed = parse_domain_markdown("fleet", MIRROR_FIXTURE).unwrap();
    assert_eq!(parsed.len(), 2);

    let lane = &parsed[0];
    assert_eq!(lane.row.key, "fleet.lane-profile");
    assert_eq!(lane.row.value, "agent-actor-is-the-profile");
    assert_eq!(
            lane.row.reason,
            "operator ruling (six points): (1) model; (2) thinking; (3) tools; (4) budget; (5) permission mode; (6) session dir."
        );
    assert_eq!(lane.row.domain, "fleet");
    assert_eq!(lane.row.branch, "main");
    assert_eq!(lane.row.event_id, "evt_01lane");
    assert_eq!(lane.row.scope, "local");
    assert_eq!(lane.row.authority, "agent");
    assert_eq!(lane.row.reversibility, "hard");
    assert_eq!(lane.row.review_after.as_deref(), Some("2027-01-01"));
    assert_eq!(lane.row.village_id.as_deref(), Some("village-alpha"));
    assert_eq!(lane.ratified_by.as_deref(), Some("operator"));
    assert_eq!(lane.ratified_at.as_deref(), Some("2026-09-05T04:00:00Z"));

    let merge = &parsed[1];
    assert_eq!(merge.row.key, "fleet.merge-authority");
    assert_eq!(merge.row.value, "controller-merges-on-current-head-lgtm");
    assert_eq!(merge.row.authority, "agent");
    assert_eq!(merge.ratified_by, None);
    let paths: Vec<String> = serde_json::from_str(&merge.row.affected_paths).unwrap();
    assert_eq!(paths, vec!["scripts/fleet/**".to_string()]);
}

#[test]
fn mirror_parse_unescapes_value_and_reason() {
    let parsed = parse_domain_markdown("esc", MIRROR_ESCAPED_FIXTURE).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].row.value,
        "line one\nline two — with \\ backslash"
    );
    assert_eq!(
        parsed[0].row.reason,
        "reason with\nnewline and \\ backslash"
    );
}

#[test]
fn mirror_parse_defaults_when_optional_lines_absent() {
    // A pre-GH-671 mirror (no Scope/Authority/Reversibility lines) must
    // still import: conservative defaults, never a hard failure.
    let text = concat!(
        "# Domain: `db`\n\n",
        "## `db.engine`\n\n",
        "- **Value**: `sqlite`\n",
        "- **Reason**: embedded\n",
        "- **Branch/ts**: `main` · 2026-09-05T03:00:00Z\n",
        "- **Governance**: unratified (agent)\n",
        "- **event_id**: `evt_01db`\n",
    );
    let parsed = parse_domain_markdown("db", text).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].row.scope, "local");
    assert_eq!(parsed[0].row.authority, "agent");
    assert_eq!(parsed[0].row.reversibility, "medium");
    assert_eq!(parsed[0].row.review_after, None);
    assert_eq!(parsed[0].row.village_id, None);
}

#[test]
fn mirror_index_meta_reads_stamp_and_machine_ignoring_gloss_lines() {
    let body = concat!(
        "- **Exported at**: 2026-09-05T03:00:00Z\n",
        "- **Exporting machine**: 4090\n",
        "- **Total decisions**: 2\n",
        // A hand-added gloss line must never be mistaken for decision data:
        "- **Gloss**: fleet.lane-profile=actor-is-profile\n",
    );
    let meta = parse_index_meta(body);
    assert_eq!(meta.exported_at.as_deref(), Some("2026-09-05T03:00:00Z"));
    assert_eq!(meta.machine.as_deref(), Some("4090"));
}

#[test]
fn mirror_freshness_stale_when_stamp_older_than_threshold() {
    let old = (time::OffsetDateTime::now_utc() - time::Duration::hours(25))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let meta = parse_index_meta(&index_body_with_stamp(&old, "4090"));
    let f = mirror_freshness(&meta, DEFAULT_MIRROR_STALE_HOURS);
    assert!(f.is_stale(), "25h-old stamp must read stale at 24h default");
    assert_eq!(f.machine.as_deref(), Some("4090"));
}

#[test]
fn mirror_freshness_fresh_when_recent_or_threshold_larger() {
    let recent = (time::OffsetDateTime::now_utc() - time::Duration::hours(2))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let meta = parse_index_meta(&index_body_with_stamp(&recent, "4090"));
    assert!(!mirror_freshness(&meta, DEFAULT_MIRROR_STALE_HOURS).is_stale());
    // A 25h stamp is only stale relative to the threshold, never absolutely:
    let old = (time::OffsetDateTime::now_utc() - time::Duration::hours(25))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let meta_old = parse_index_meta(&index_body_with_stamp(&old, "4090"));
    assert!(!mirror_freshness(&meta_old, 48).is_stale());
}

#[test]
fn mirror_freshness_stale_when_stamp_missing_or_unparseable() {
    // Unknown freshness must be visible (death visibility), not silent.
    let meta = parse_index_meta("- **Total decisions**: 2\n");
    let f = mirror_freshness(&meta, DEFAULT_MIRROR_STALE_HOURS);
    assert!(f.is_stale());
    assert!(f.age_hours.is_none());

    let meta_bad = parse_index_meta("- **Exported at**: not-a-timestamp\n");
    assert!(mirror_freshness(&meta_bad, DEFAULT_MIRROR_STALE_HOURS).is_stale());
}

#[test]
fn mirror_import_round_trip_then_dedups_on_second_run() {
    let (tmp_tgt, target) = setup_workspace();
    let mirror = write_mirror_tree(
        &tmp_tgt.join("_mirror"),
        &index_body_with_stamp(
            &time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            "4090",
        ),
    );
    let source = MirrorSource { mirror_dir: mirror };

    let r1 = sync_from_mirror(&target, &source, false).unwrap();
    assert_eq!(r1.imported.len(), 2, "both fixture decisions import");
    assert!(r1.conflicts.is_empty());
    assert!(r1.mirror.is_some(), "mirror meta recorded on the result");

    // Values arrive verbatim, never minted or paraphrased.
    let lane = target
        .sqlite
        .find_active_decision("main", "fleet.lane-profile")
        .unwrap()
        .expect("lane-profile imported");
    assert_eq!(lane.value, "agent-actor-is-the-profile");
    assert_eq!(lane.source_project_id.as_deref(), Some("mirror:4090"));
    assert_eq!(lane.source_event_id.as_deref(), Some("evt_01lane"));

    // Ratified/unratified is preserved: the mirror's ratification becomes
    // a ratify event on the target, so standard derivation sees it.
    let ratified = target.ratified_decisions_map().unwrap();
    assert!(
        ratified.contains_key(&lane.event_id),
        "imported ratification must bind"
    );
    assert_eq!(ratified[&lane.event_id].ratified_by, "operator");

    let merge = target
        .sqlite
        .find_active_decision("main", "fleet.merge-authority")
        .unwrap()
        .expect("merge-authority imported");
    assert_eq!(merge.value, "controller-merges-on-current-head-lgtm");

    // Second run: everything already imported → skipped, nothing new.
    let r2 = sync_from_mirror(&target, &source, false).unwrap();
    assert_eq!(r2.imported.len(), 0);
    assert_eq!(r2.skipped, 2);

    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn mirror_conflict_imports_inactive_394_rule() {
    let (tmp_tgt, target) = setup_workspace();
    // Local value already active for the key.
    write_local_decision(&target, "fleet.lane-profile", "actor-is-profile");

    let mirror = write_mirror_tree(
        &tmp_tgt.join("_mirror"),
        &index_body_with_stamp(
            &time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            "4090",
        ),
    );
    let source = MirrorSource { mirror_dir: mirror };

    let r = sync_from_mirror(&target, &source, false).unwrap();
    // merge-authority imports clean; lane-profile conflicts (#394).
    assert_eq!(r.imported.len(), 2);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].key, "fleet.lane-profile");
    assert_eq!(r.conflicts[0].local_value, "actor-is-profile");
    assert_eq!(r.conflicts[0].remote_value, "agent-actor-is-the-profile");

    // The local active value is never overwritten by the mirror.
    let active = target
        .find_active_decision("main", "fleet.lane-profile")
        .unwrap()
        .unwrap();
    assert_eq!(active.value, "actor-is-profile");

    // The remote value is present, but inactive.
    let timeline = target
        .sqlite
        .decision_timeline("fleet.lane-profile", None, None)
        .unwrap();
    assert!(timeline
        .iter()
        .any(|d| d.value == "agent-actor-is-the-profile" && !d.is_active));

    let _ = std::fs::remove_dir_all(&tmp_tgt);
}

#[test]
fn mirror_dry_run_writes_nothing() {
    let (tmp_tgt, target) = setup_workspace();
    let mirror = write_mirror_tree(
        &tmp_tgt.join("_mirror"),
        &index_body_with_stamp(
            &time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            "4090",
        ),
    );
    let source = MirrorSource { mirror_dir: mirror };

    let r = sync_from_mirror(&target, &source, true).unwrap();
    assert_eq!(r.imported.len(), 2);
    let decisions = target.active_decisions(None, None, None, None).unwrap();
    assert!(decisions.is_empty(), "dry run must not write");

    let _ = std::fs::remove_dir_all(&tmp_tgt);
}
