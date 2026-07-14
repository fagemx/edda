//! Cross-project decision sync engine.
//!
//! Pull-based: the target project pulls shared decisions from source projects'
//! ledgers and creates `decision_import` events with provenance links.
//!
//! This module only accepts pre-resolved data — callers (L4: cli, serve)
//! are responsible for resolving project IDs and source paths via `edda-store`.

use crate::sqlite_store::ImportParams;
use crate::Ledger;
use edda_core::decision::extract_domain;
use edda_core::event::finalize_event;
use edda_core::types::{Event, Provenance, Refs, SCHEMA_VERSION};

/// A source project to sync from.
pub struct SyncSource {
    pub project_id: String,
    pub project_name: String,
    pub ledger_path: std::path::PathBuf,
}

/// A single imported decision record.
#[derive(Debug, Clone)]
pub struct ImportedDecision {
    pub key: String,
    pub value: String,
    pub source_project: String,
    pub source_event_id: String,
}

/// A conflict detected during sync.
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    pub key: String,
    pub local_value: String,
    pub remote_value: String,
    pub source_project: String,
}

/// An error that occurred while syncing from a specific source.
#[derive(Debug, Clone)]
pub struct SourceError {
    pub project_name: String,
    pub error: String,
}

/// Result of a sync operation.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub imported: Vec<ImportedDecision>,
    pub skipped: usize,
    pub conflicts: Vec<ConflictInfo>,
    pub errors: Vec<SourceError>,
}

/// Sync shared decisions from source projects into the target ledger.
///
/// `target_project_id` is the pre-resolved project ID for the target ledger
/// (callers compute this via `edda_store::project_id`).
///
/// For each source project:
/// 1. Open the source ledger and query shared/global decisions
/// 2. Skip decisions already imported (by source_project_id + source_event_id)
/// 3. For new decisions: create `decision_import` event with provenance
/// 4. For conflicts (same key, different value): import as inactive
///
/// Source-level failures (cannot open ledger or query decisions) are collected
/// in `SyncResult::errors` rather than silently swallowed.
pub fn sync_from_sources(
    target: &Ledger,
    sources: &[SyncSource],
    target_project_id: &str,
    dry_run: bool,
) -> anyhow::Result<SyncResult> {
    let branch = target.head_branch()?;
    let mut result = SyncResult::default();

    for source in sources {
        // Don't sync from self
        if source.project_id == target_project_id {
            continue;
        }

        let source_ledger = match Ledger::open(&source.ledger_path) {
            Ok(l) => l,
            Err(e) => {
                result.errors.push(SourceError {
                    project_name: source.project_name.clone(),
                    error: format!("failed to open ledger: {e}"),
                });
                continue;
            }
        };

        // Use internal SqliteStore to get raw DecisionRow (sync needs scope,
        // source_project_id fields not exposed via DecisionView).
        let shared = match source_ledger.sqlite.shared_decisions() {
            Ok(d) => d,
            Err(e) => {
                result.errors.push(SourceError {
                    project_name: source.project_name.clone(),
                    error: format!("failed to query shared decisions: {e}"),
                });
                continue;
            }
        };

        for decision in &shared {
            // Skip if already imported
            if target.is_already_imported(&source.project_id, &decision.event_id)? {
                result.skipped += 1;
                continue;
            }

            // Any differing active value is a conflict, regardless of whether
            // the current winner originated locally or from another source.
            let current = target.sqlite.find_active_decision(&branch, &decision.key)?;
            let is_conflict = current
                .as_ref()
                .map(|active| active.value != decision.value)
                .unwrap_or(false);

            if is_conflict {
                result.conflicts.push(ConflictInfo {
                    key: decision.key.clone(),
                    local_value: current
                        .as_ref()
                        .map(|active| active.value.clone())
                        .unwrap_or_default(),
                    remote_value: decision.value.clone(),
                    source_project: source.project_name.clone(),
                });
            }

            if dry_run {
                result.imported.push(ImportedDecision {
                    key: decision.key.clone(),
                    value: decision.value.clone(),
                    source_project: source.project_name.clone(),
                    source_event_id: decision.event_id.clone(),
                });
                continue;
            }

            // Create the import event
            let parent_hash = target.last_event_hash()?;
            let import_active = !is_conflict;

            let mut event = make_import_event(
                &branch,
                parent_hash.as_deref(),
                decision,
                &source.project_id,
                &source.project_name,
            )?;
            finalize_event(&mut event)?;

            let domain = extract_domain(&decision.key);
            target.insert_imported_decision(ImportParams {
                event: &event,
                key: &decision.key,
                value: &decision.value,
                reason: &decision.reason,
                domain: &domain,
                scope: &decision.scope,
                source_project_id: &source.project_id,
                source_event_id: &decision.event_id,
                is_active: import_active,
                authority: &decision.authority,
                affected_paths: &decision.affected_paths,
                tags: &decision.tags,
                review_after: decision.review_after.as_deref(),
                reversibility: &decision.reversibility,
                village_id: decision.village_id.as_deref(),
            })?;

            result.imported.push(ImportedDecision {
                key: decision.key.clone(),
                value: decision.value.clone(),
                source_project: source.project_name.clone(),
                source_event_id: decision.event_id.clone(),
            });
        }
    }

    Ok(result)
}

fn make_import_event(
    branch: &str,
    parent_hash: Option<&str>,
    decision: &crate::sqlite_store::DecisionRow,
    source_project_id: &str,
    source_project_name: &str,
) -> anyhow::Result<Event> {
    let affected_paths: serde_json::Value = serde_json::from_str(&decision.affected_paths)?;
    let decision_tags: serde_json::Value = serde_json::from_str(&decision.tags)?;
    let payload = serde_json::json!({
        "role": "system",
        "text": format!(
            "[sync] imported {key}={value} from {source}",
            key = decision.key,
            value = decision.value,
            source = source_project_name,
        ),
        "tags": ["decision", "decision_import"],
        "decision": {
            "key": decision.key,
            "value": decision.value,
            "reason": decision.reason,
            "scope": decision.scope,
            "authority": decision.authority,
            "affected_paths": affected_paths,
            "tags": decision_tags,
            "review_after": decision.review_after,
            "reversibility": decision.reversibility,
            "village_id": decision.village_id,
        },
        "source_project_id": source_project_id,
        "source_project_name": source_project_name,
        "source_event_id": decision.event_id,
    });

    let provenance = vec![Provenance {
        target: decision.event_id.clone(),
        rel: edda_core::types::rel::IMPORTED_FROM.to_string(),
        note: Some(format!("project:{source_project_name}")),
    }];

    let event = Event {
        event_id: format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase()),
        ts: time_now_rfc3339(),
        event_type: "decision_import".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            blobs: Vec::new(),
            events: vec![decision.event_id.clone()],
            provenance,
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    Ok(event)
}

fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
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
}
