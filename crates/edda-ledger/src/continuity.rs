use crate::{Ledger, WorkspaceLock};
use edda_core::continuity::{
    make_bundle, new_imported_capsule_event, new_local_capsule_event, new_local_only_capsule_event,
    parse_capsule_record, validate_bundle, CapsuleGitV1, CapsuleRecordV1, CapsuleRepositoryV1,
    ContextCapsuleV1, ContextReferencesV1, ContextSourceV1, ContextStateV1,
    PortableCapsuleBundleV1, RejectedContextHypothesisV1, TruncationNoticeV1,
    CONTINUITY_EVENT_TYPE,
};
use edda_core::Event;
use serde::Serialize;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct CapsuleEntryV1 {
    pub local_event_id: String,
    pub origin_event_id: String,
    pub capsule: ContextCapsuleV1,
    pub imported: bool,
    pub legacy_partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDisposition {
    Imported,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResultV1 {
    pub disposition: ImportDisposition,
    pub entry: CapsuleEntryV1,
}

#[derive(Debug, thiserror::Error)]
#[error("exact continuity read-back failed for event {event_id}")]
pub struct ContinuityReadbackError {
    pub event_id: String,
    pub capsule_id: String,
    #[source]
    source: anyhow::Error,
}

impl Ledger {
    pub fn append_continuity_capsule(
        &self,
        capsule: &ContextCapsuleV1,
    ) -> anyhow::Result<CapsuleEntryV1> {
        let _lock = acquire_lock_with_retry(&self.paths)?;
        if let Some(entry) = existing_local_capsule(self, capsule)? {
            return Ok(entry);
        }
        let branch = self.head_branch()?;
        let parent_hash = self.last_event_hash()?;
        let event = if capsule.repository.portable_repo_id.is_some() {
            new_local_capsule_event(&branch, parent_hash.as_deref(), capsule)?
        } else {
            new_local_only_capsule_event(&branch, parent_hash.as_deref(), capsule)?
        };
        self.append_event(&event)?;
        verified_readback(self, &event, &capsule.capsule_id)
    }

    pub fn import_continuity_bundle(
        &self,
        bundle: &PortableCapsuleBundleV1,
    ) -> anyhow::Result<ImportResultV1> {
        validate_bundle(bundle)?;
        let _lock = acquire_lock_with_retry(&self.paths)?;
        for event in self.iter_events()? {
            if event.event_type == CONTINUITY_EVENT_TYPE {
                let record = parse_capsule_record(&event)?;
                let same_capsule = record.origin.capsule_id == bundle.origin_capsule_id;
                let same_event = record.origin.event_id == bundle.origin_event_id;
                if !same_capsule && !same_event {
                    continue;
                }
                let exact = same_capsule
                    && same_event
                    && record.origin.portable_repo_id.as_deref() == Some(&bundle.portable_repo_id)
                    && record.capsule_sha256 == bundle.capsule_sha256
                    && record.capsule_bytes_hex == bundle.capsule_bytes_hex;
                if exact {
                    return Ok(ImportResultV1 {
                        disposition: ImportDisposition::Skipped,
                        entry: entry_from_record(&event, record),
                    });
                }
                anyhow::bail!("continuity integrity conflict for origin capsule or event identity");
            }
            if event.event_type == "checkpoint"
                && (project_legacy_checkpoint(&event).capsule.capsule_id
                    == bundle.origin_capsule_id
                    || event.event_id == bundle.origin_event_id)
            {
                anyhow::bail!("continuity integrity conflict with a legacy checkpoint identity");
            }
        }

        let branch = self.head_branch()?;
        let parent_hash = self.last_event_hash()?;
        let event = new_imported_capsule_event(&branch, parent_hash.as_deref(), bundle)?;
        self.append_event(&event)?;
        Ok(ImportResultV1 {
            disposition: ImportDisposition::Imported,
            entry: verified_readback(self, &event, &bundle.origin_capsule_id)?,
        })
    }

    pub fn continuity_capsules(&self) -> anyhow::Result<Vec<CapsuleEntryV1>> {
        let mut entries = Vec::new();
        for event in self.iter_events()? {
            if event.event_type == CONTINUITY_EVENT_TYPE {
                let record = parse_capsule_record(&event)?;
                entries.push(entry_from_record(&event, record));
            } else if event.event_type == "checkpoint" {
                entries.push(project_legacy_checkpoint(&event));
            }
        }
        entries.reverse();
        Ok(entries)
    }

    pub fn continuity_capsule(&self, capsule_id: &str) -> anyhow::Result<Option<CapsuleEntryV1>> {
        Ok(self
            .continuity_capsules()?
            .into_iter()
            .find(|entry| entry.capsule.capsule_id == capsule_id))
    }

    pub fn export_continuity_bundle(
        &self,
        capsule_id: &str,
    ) -> anyhow::Result<PortableCapsuleBundleV1> {
        let entry = self
            .continuity_capsule(capsule_id)?
            .ok_or_else(|| anyhow::anyhow!("continuity capsule not found: {capsule_id}"))?;
        if entry.legacy_partial {
            anyhow::bail!("legacy checkpoint capsules cannot be exported");
        }
        let event = self
            .get_event(&entry.local_event_id)?
            .ok_or_else(|| anyhow::anyhow!("continuity capsule event disappeared during export"))?;
        make_bundle(&parse_capsule_record(&event)?)
    }
}

fn existing_local_capsule(
    ledger: &Ledger,
    capsule: &ContextCapsuleV1,
) -> anyhow::Result<Option<CapsuleEntryV1>> {
    let mut exact = None;
    for event in ledger.iter_events()? {
        let entry = if event.event_type == CONTINUITY_EVENT_TYPE {
            let record = parse_capsule_record(&event)?;
            if record.origin.capsule_id != capsule.capsule_id {
                continue;
            }
            entry_from_record(&event, record)
        } else if event.event_type == "checkpoint" {
            let entry = project_legacy_checkpoint(&event);
            if entry.capsule.capsule_id != capsule.capsule_id {
                continue;
            }
            entry
        } else {
            continue;
        };
        if entry.legacy_partial || entry.capsule != *capsule {
            anyhow::bail!("continuity integrity conflict for local capsule identity");
        }
        exact = Some(entry);
    }
    Ok(exact)
}

fn verified_readback(
    ledger: &Ledger,
    event: &Event,
    capsule_id: &str,
) -> anyhow::Result<CapsuleEntryV1> {
    exact_readback(ledger, event).map_err(|source| {
        ContinuityReadbackError {
            event_id: event.event_id.clone(),
            capsule_id: capsule_id.to_string(),
            source,
        }
        .into()
    })
}

fn exact_readback(ledger: &Ledger, written: &Event) -> anyhow::Result<CapsuleEntryV1> {
    let read = ledger
        .get_event(&written.event_id)?
        .ok_or_else(|| anyhow::anyhow!("exact continuity read-back failed"))?;
    if read.hash != written.hash || read.payload != written.payload {
        anyhow::bail!("exact continuity read-back did not match appended event");
    }
    let record = parse_capsule_record(&read)?;
    Ok(entry_from_record(&read, record))
}

fn entry_from_record(event: &Event, record: CapsuleRecordV1) -> CapsuleEntryV1 {
    CapsuleEntryV1 {
        local_event_id: event.event_id.clone(),
        origin_event_id: record.origin.event_id,
        capsule: record.capsule,
        imported: record.imported,
        legacy_partial: false,
    }
}

fn project_legacy_checkpoint(event: &Event) -> CapsuleEntryV1 {
    let mut truncation = Vec::new();
    let rejected = event
        .payload
        .get("rejected")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .take(32)
                .enumerate()
                .filter_map(|(index, value)| {
                    Some(RejectedContextHypothesisV1 {
                        hypothesis: legacy_text(
                            value.get("hypothesis")?.as_str()?,
                            1_000,
                            &format!("state.rejected[{index}].hypothesis"),
                            &mut truncation,
                        ),
                        reason: legacy_text(
                            value.get("reason")?.as_str()?,
                            1_000,
                            &format!("state.rejected[{index}].reason"),
                            &mut truncation,
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let capsule = ContextCapsuleV1 {
        capsule_version: 1,
        capsule_id: format!(
            "cap_legacy{}",
            &edda_core::hash::sha256_hex(event.event_id.as_bytes())[..32]
        ),
        created_at: if time::OffsetDateTime::parse(
            &event.ts,
            &time::format_description::well_known::Rfc3339,
        )
        .is_ok()
        {
            event.ts.clone()
        } else {
            "1970-01-01T00:00:00Z".into()
        },
        source: ContextSourceV1 {
            machine_alias: None,
            actor: event
                .payload
                .get("role")
                .and_then(serde_json::Value::as_str)
                .map(|value| legacy_text(value, 160, "source.actor", &mut truncation)),
        },
        repository: CapsuleRepositoryV1 {
            portable_repo_id: None,
            display_hint: None,
            local_only_reason: Some("legacy checkpoint has no portable repository identity".into()),
        },
        state: ContextStateV1 {
            title: "Legacy checkpoint".into(),
            summary: "unknown".into(),
            goal: "unknown".into(),
            current: "unknown".into(),
            hypotheses: legacy_list(event, "hypotheses", &mut truncation),
            rejected,
            open_questions: legacy_list(event, "open", &mut truncation),
            next_action: legacy_text(
                event
                    .payload
                    .get("next")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("unknown"),
                4_000,
                "state.next_action",
                &mut truncation,
            ),
        },
        // The event branch is an Edda ledger branch, not evidence of the Git
        // branch that existed when this legacy checkpoint was written.
        git: CapsuleGitV1::default(),
        references: ContextReferencesV1 {
            event_ids: if event.event_id.starts_with("evt_") {
                vec![event.event_id.clone()]
            } else {
                Vec::new()
            },
            ..ContextReferencesV1::default()
        },
        truncation,
    };
    CapsuleEntryV1 {
        local_event_id: event.event_id.clone(),
        origin_event_id: event.event_id.clone(),
        capsule,
        imported: false,
        legacy_partial: true,
    }
}

fn legacy_list(event: &Event, key: &str, truncation: &mut Vec<TruncationNoticeV1>) -> Vec<String> {
    let Some(values) = event.payload.get(key).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    if values.len() > 32 {
        truncation.push(TruncationNoticeV1 {
            field: format!("state.{key}"),
            omitted_chars: 0,
            omitted_items: values.len() - 32,
        });
    }
    values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .take(32)
        .enumerate()
        .map(|(index, value)| {
            legacy_text(value, 1_000, &format!("state.{key}[{index}]"), truncation)
        })
        .collect()
}

fn legacy_text(
    value: &str,
    max_chars: usize,
    field: &str,
    truncation: &mut Vec<TruncationNoticeV1>,
) -> String {
    let redacted = edda_core::secret_guard::redact(value).0;
    let count = redacted.chars().count();
    if count > max_chars {
        truncation.push(TruncationNoticeV1 {
            field: field.to_string(),
            omitted_chars: count - max_chars,
            omitted_items: 0,
        });
    }
    redacted.chars().take(max_chars).collect()
}

fn acquire_lock_with_retry(paths: &crate::EddaPaths) -> anyhow::Result<WorkspaceLock> {
    let mut last_error = None;
    for _ in 0..50 {
        match WorkspaceLock::acquire(paths) {
            Ok(lock) => return Ok(lock),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("workspace lock unavailable")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portable_capsule() -> ContextCapsuleV1 {
        use edda_core::continuity::{build_capsule, ContextCapsuleInputV1};
        build_capsule(
            serde_json::from_value::<ContextCapsuleInputV1>(serde_json::json!({
                "capsule_version": 1,
                "state": {"next_action": "continue"}
            }))
            .unwrap(),
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: Some(format!("repo_{}", "a".repeat(64))),
                display_hint: None,
                local_only_reason: None,
            },
            CapsuleGitV1::default(),
        )
        .unwrap()
    }

    #[test]
    fn local_capsule_identity_is_deduped_or_rejected_under_append_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_or_init(tmp.path()).unwrap();
        let capsule = portable_capsule();
        let first = ledger.append_continuity_capsule(&capsule).unwrap();
        let duplicate = ledger.append_continuity_capsule(&capsule).unwrap();
        assert_eq!(duplicate.local_event_id, first.local_event_id);
        assert_eq!(ledger.count_events().unwrap(), 1);

        let shown = ledger
            .continuity_capsule(&capsule.capsule_id)
            .unwrap()
            .unwrap();
        let exported = ledger
            .export_continuity_bundle(&capsule.capsule_id)
            .unwrap();
        assert_eq!(shown.local_event_id, first.local_event_id);
        assert_eq!(exported.origin_event_id, first.local_event_id);

        let mut conflicting = capsule;
        conflicting.state.next_action = "different".into();
        assert!(ledger.append_continuity_capsule(&conflicting).is_err());
        assert_eq!(ledger.count_events().unwrap(), 1);
    }

    #[test]
    fn conflicting_capsule_identity_is_refused_without_append() {
        use edda_core::continuity::{
            build_capsule, canonical_capsule_bytes, capsule_digest, make_bundle, CapsuleOriginV1,
            CapsuleRecordV1, ContextCapsuleInputV1, DataAuthority,
        };
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_or_init(tmp.path()).unwrap();
        let repository = CapsuleRepositoryV1 {
            portable_repo_id: Some(format!("repo_{}", "a".repeat(64))),
            display_hint: Some("example.com/team/repo".into()),
            local_only_reason: None,
        };
        let input = || {
            serde_json::from_value::<ContextCapsuleInputV1>(serde_json::json!({
                "capsule_version": 1,
                "state": {"next_action": "continue"}
            }))
            .unwrap()
        };
        let original = build_capsule(
            input(),
            ContextSourceV1::default(),
            repository.clone(),
            CapsuleGitV1::default(),
        )
        .unwrap();
        ledger.append_continuity_capsule(&original).unwrap();
        let mut conflicting = build_capsule(
            input(),
            ContextSourceV1::default(),
            repository,
            CapsuleGitV1::default(),
        )
        .unwrap();
        conflicting.capsule_id.clone_from(&original.capsule_id);
        let bytes = canonical_capsule_bytes(&conflicting).unwrap();
        let bundle = make_bundle(&CapsuleRecordV1 {
            record_version: 1,
            data_authority: DataAuthority::DataOnly,
            origin: CapsuleOriginV1 {
                capsule_id: conflicting.capsule_id.clone(),
                event_id: format!("evt_{}", "2".repeat(26)),
                portable_repo_id: conflicting.repository.portable_repo_id.clone(),
            },
            capsule_sha256: capsule_digest(&bytes),
            capsule_bytes_hex: hex::encode(bytes),
            capsule: conflicting,
            imported: false,
        })
        .unwrap();
        assert!(ledger.import_continuity_bundle(&bundle).is_err());
        assert_eq!(ledger.count_events().unwrap(), 1);
    }
}
