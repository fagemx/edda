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

            // Check for local conflict (use raw row to access source_project_id)
            let local = target.sqlite.find_active_decision(&branch, &decision.key)?;
            let is_conflict = local
                .as_ref()
                .map(|l| l.value != decision.value && l.source_project_id.is_none())
                .unwrap_or(false);

            if is_conflict {
                result.conflicts.push(ConflictInfo {
                    key: decision.key.clone(),
                    local_value: local.as_ref().map(|l| l.value.clone()).unwrap_or_default(),
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
