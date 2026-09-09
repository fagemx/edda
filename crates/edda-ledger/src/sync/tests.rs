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
        cites: None,
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
        cites: None,
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
        cites: None,
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

/// Every caller-supplied field in its escaped form (GH-671 R5) — not only
/// Value and Reason. `\n` here is the two-character escape the export writes,
/// never a real line break; a real one would make the line below it a
/// *field* of the same decision, which is the whole defect.
const MIRROR_TOTAL_ESCAPE_FIXTURE: &str = concat!(
    "# Domain: `esc\\ndomain`\n\n",
    "## `esc.multi\\nline \\\\ key`\n\n",
    "- **Value**: `v`\n",
    "- **Reason**: r\n",
    "- **Branch/ts**: `main` · 2026-09-05T03:00:00Z\n",
    "- **Governance**: ratified by op\\nerator \\\\ x at 2026-09-05T04:00:00Z\n",
    "- **Scope**: local\\n- **Scope**: global\n",
    "- **Authority**: agent\\nhuman \\\\ x\n",
    "- **Reversibility**: hard\\\\ish\n",
    "- **Review after**: 2027-01-01\\ntrailing\n",
    "- **Village**: village-a\\\\one\\n- **Reversibility**: forged\n",
    "- **event_id**: `evt_01esc\\nid`\n",
);

/// The read half of the R5 pair: a field the export escaped must be unescaped
/// on import, or the row lands carrying literal `\n` and doubled backslashes.
///
/// The forged `- **Scope**:` / `- **Reversibility**:` text inside Scope and
/// Village is the point of the encoding: after unescaping it is *data* sitting
/// inside one column, never a second field line the parser obeyed.
#[test]
fn mirror_parse_unescapes_every_caller_supplied_field() {
    let parsed = parse_domain_markdown("file-stem", MIRROR_TOTAL_ESCAPE_FIXTURE).unwrap();
    assert_eq!(parsed.len(), 1, "one section, not one plus injected lines");
    let d = &parsed[0];

    assert_eq!(d.row.key, "esc.multi\nline \\ key");
    assert_eq!(d.row.domain, "esc\ndomain");
    assert_eq!(d.row.scope, "local\n- **Scope**: global");
    assert_eq!(d.row.authority, "agent\nhuman \\ x");
    assert_eq!(d.row.reversibility, "hard\\ish");
    assert_eq!(d.row.review_after.as_deref(), Some("2027-01-01\ntrailing"));
    assert_eq!(
        d.row.village_id.as_deref(),
        Some("village-a\\one\n- **Reversibility**: forged")
    );
    assert_eq!(d.row.event_id, "evt_01esc\nid");
    assert_eq!(d.ratified_by.as_deref(), Some("op\nerator \\ x"));
    // Machine-generated, never escaped: an RFC3339 stamp and a branch name
    // `validate_branch_name` restricts to [A-Za-z0-9._/-].
    assert_eq!(d.row.branch, "main");
    assert_eq!(d.ratified_at.as_deref(), Some("2026-09-05T04:00:00Z"));
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
    // a ratify event on the target, so standard derivation sees it. The
    // ratifier, however, is the MIRROR, not the name the markdown claimed —
    // that name is unauthenticated text and the import now runs unattended at
    // SessionStart. This assertion previously demanded "operator", which is
    // exactly the forgery `append_mirror_ratification` documents.
    let ratified = target.ratified_decisions_map().unwrap();
    assert!(
        ratified.contains_key(&lane.event_id),
        "imported ratification must bind"
    );
    assert_eq!(ratified[&lane.event_id].ratified_by, "mirror:4090");

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

// ── GH-1044: `backtick_list` / `unescape_field` backtick handling ──────
//
// `edda-ledger` cannot call the real `cmd_export::escape_field` (it lives in
// `edda-cli`, which depends on `edda-ledger`, not the reverse), so
// `mirror_escape` below is a deliberate, documented duplicate of it, used
// only to CONSTRUCT test input the same way the real writer would — every
// assertion is still against `backtick_list`/`unescape_field`, the read
// side. Building `rendered` with this helper (rather than hand-encoded
// string literals) keeps each test's intent checkable by inspection instead
// of by counting backslashes. The full write-then-read pipeline is
// exercised end-to-end by `export_import_round_trip_is_total_for_backticks`
// in `edda-cli::cmd_export`; these pin the read side alone, in isolation, so
// a failure here points at `backtick_list`/`unescape_field` directly rather
// than somewhere in the round trip.

/// Duplicate of `edda-cli::cmd_export::escape_field` — see the module
/// comment above for why this crate cannot call the original directly. Keep
/// in sync with it by hand; a divergence here would make these tests assert
/// against a writer the real code no longer has.
fn mirror_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('`', "\\`")
}

/// `["a", "b", ...]` → the exact `` `a`, `b` `` markdown `backtick_list`
/// reads, each item escaped and wrapped the way `render_domain` does.
fn mirror_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|it| format!("`{}`", mirror_escape(it)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn backtick_list_ordinary_items_unchanged() {
    // Regression: no backticks involved, must split exactly as before.
    assert_eq!(
        backtick_list(&mirror_list(&["sqlite", "postgres"])),
        vec!["sqlite".to_string(), "postgres".to_string()]
    );
    assert_eq!(
        backtick_list(&mirror_list(&["solo"])),
        vec!["solo".to_string()]
    );
    assert_eq!(backtick_list(""), Vec::<String>::new());
}

#[test]
fn backtick_list_does_not_split_on_embedded_separator_sequence() {
    // GH-1044, the exact reported defect: a single tag whose value is
    // `a`, `b` — i.e. it contains the raw four-byte sequence "`, `" that
    // `backtick_list` splits list items on. One item in, one item out.
    let item = "a`, `b";
    assert_eq!(backtick_list(&mirror_list(&[item])), vec![item.to_string()]);
}

#[test]
fn backtick_list_recovers_leading_and_trailing_backtick_items() {
    // GH-1044's secondary hole: once backticks are escaped, an item whose
    // escaped form is adjacent to the list's own wrapping delimiter must not
    // have that delimiter's greedy removal eat the escaped backtick's bare
    // half too.
    assert_eq!(
        backtick_list(&mirror_list(&["`leading"])),
        vec!["`leading".to_string()]
    );
    assert_eq!(
        backtick_list(&mirror_list(&["trailing`"])),
        vec!["trailing`".to_string()]
    );
}

#[test]
fn backtick_list_multi_item_mixes_edge_and_embedded_backticks() {
    // Four items in one list, each exercising a different edge of the
    // GH-1044 hole: embedded separator-lookalike, leading backtick, trailing
    // backtick, and one plain item as a control.
    let items = ["a`, `b", "`leading", "trailing`", "plain"];
    assert_eq!(
        backtick_list(&mirror_list(&items)),
        items.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn backtick_list_item_ending_in_backtick_comma_space_does_not_corrupt_the_next_item() {
    // GH-1044 Round 1 P0 (re-derived independently from the review, not
    // copied): escaping a content backtick makes it distinguishable *to
    // `unescape_field`* — it is preceded by a backslash — but does nothing
    // for a plain `split`, which matches four literal bytes and never looks
    // at what precedes them. An item whose value ends in exactly backtick,
    // comma, space renders (escaped) as `` a\`,  `` — so the escaped
    // backtick's own bare half sits immediately before that same item's
    // trailing ", " and the wrapper's closing backtick, and a leftmost,
    // non-overlapping `split("`, `")` matches one byte too early, starting
    // at the *content* backtick instead of the wrapper one.
    //
    // Two items, `` a`, `` (a, backtick, comma, space) and `b` — rendered:
    //
    //   ` a \ ` ,  ` , ` b `
    //   0 1 2 3 4 5 6 7 8 9 10 11        (12 bytes)
    //         ^wrap-open(3=esc.bt)  ^wrap-close(6)   ^wrap-open(9)
    //
    // After the outer strip, `inner = a \ ` , ␣ ` , ␣ ` b`. The leftmost
    // `` `,  ` `` match starts at inner[2] (the escaped content backtick),
    // consuming inner[2..6] — the item's own ", " *and* the wrapper's
    // closing backtick — before the genuine delimiter at inner[5..9] is
    // ever reached. Pre-fix this produced `["a\\", ", `b"]`: item 1 loses
    // its trailing comma and gains a dangling backslash, item 2 gains a
    // leading ", `" stolen from item 1. Both values land corrupted in the
    // ledger import payload — silently, since `backtick_list` cannot fail.
    //
    // The expected value below is `"a`,"`, not `"a`, "`: `backtick_list`'s
    // own `.trim()` drops the trailing space regardless of this fix — a
    // separate, pre-existing whitespace-loss limitation tracked outside
    // GH-1044 (see the reviewer's FOLLOW-UP ISSUE on PR #1108 Round 1), not
    // something this test conflates with the split corruption above.
    let items = ["a`, ", "b"];
    assert_eq!(
        backtick_list(&mirror_list(&items)),
        vec!["a`,".to_string(), "b".to_string()]
    );
}

// The two tests below read a mirror line shaped the way a mirror committed
// before PR #1017 (`3306c1a`, 2026-09-07) actually looks on disk: that
// writer applied no escaping at all
// (`3306c1a^:crates/edda-cli/src/cmd_export.rs:150`,
// `.map(|p| format!("`{}`", p))`). Hand-constructed as literal string
// slices below, NOT through `mirror_list`/`mirror_escape` — those two
// helpers duplicate the *current* writer, which is exactly the writer this
// input predates. GH-1044 Round 2 (re-derived independently, not copied
// from the review): see `unescape_field`'s doc comment for the full
// raw-era-vs-escaped-era argument these two tests pin.

#[test]
fn backtick_list_reads_a_raw_pre_1017_item_whose_backslash_is_not_at_the_trailing_edge() {
    // Item 1's raw value is `a\b` — a backslash, but not immediately before
    // the item's own closing wrapper backtick. The escape-aware split still
    // finds the genuine `` `,  ` `` delimiter correctly here, and this
    // particular value even round-trips byte-for-byte (`\b` is not a
    // recognised escape target, so `unescape_field`'s fallback arm passes
    // it through unchanged). Contrast with the odd-trailing-run case below,
    // which this does NOT hold for.
    let raw_legacy_line = "`a\\b`, `c`";
    assert_eq!(
        backtick_list(raw_legacy_line),
        vec!["a\\b".to_string(), "c".to_string()]
    );
}

#[test]
fn backtick_list_merges_a_raw_pre_1017_item_ending_in_an_odd_backslash_run() {
    // Known, accepted limitation (GH-1044 Round 2 finding F6) — pinned here
    // so a future change to this function changes this behavior on
    // purpose, not by accident. Item 1's raw value is `a\` (a, backslash):
    // the writer rendered the two items as the 9-byte line `` `a\`, `b` ``.
    // After the outer strip, `inner = a \ ` , ␣ ` b`. The escape-aware walk
    // hits the backslash at inner[1] and consumes inner[2] — the item's own
    // *genuine* closing wrapper — as if it were escaped content, so that
    // backtick is never tested as delimiter material. No further delimiter
    // exists in the rest of the line, so both items come back as one,
    // silently (`split_unescaped_backtick_comma` has no error channel).
    //
    // This is not fixable by a cleverer scan: a raw-era single item whose
    // value is literally `` a`, `b `` renders to the identical bytes
    // `` `a`, `b` `` that a *current* two-item list `["a", "b"]` also
    // renders to — no decoder operating on bytes alone can be correct for
    // both origins of that string. Recovering it needs a mirror
    // format/version marker (GH-1113), out of GH-1044's own stated scope
    // (mirror directory layout / `INDEX.md`).
    let raw_legacy_line = "`a\\`, `b`";
    assert_eq!(backtick_list(raw_legacy_line), vec!["a`, `b".to_string()]);
}

#[test]
fn unescape_field_inverts_escaped_backtick() {
    for original in ["a`b", "`lead", "trail`", "``double``"] {
        assert_eq!(
            unescape_field(&mirror_escape(original)),
            original,
            "round trip for {original:?}"
        );
    }
    // Backslash-first ordering (matches `escape_field`): a real backslash
    // immediately before a real backtick — `mirror_escape` doubles the
    // backslash before it escapes the backtick — must round-trip whole, not
    // be misread as one already-escaped unit that swallows the backtick.
    let original = "a\\`b"; // a, backslash, backtick, b
    assert_eq!(unescape_field(&mirror_escape(original)), original);
}

#[test]
fn unescape_field_unknown_escape_passes_through_unchanged() {
    // Back-compat (doneWhen #3): a mirror written before GH-1044 never
    // produced `` \` `` (escape_field did not escape backticks), so this arm
    // only ever fires on mirrors written by the fixed writer. An unrelated
    // unknown escape must still pass through unchanged, exactly as before.
    assert_eq!(unescape_field("\\p"), "\\p");
}
