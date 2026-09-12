use crate::control_authority::{
    authority_is_active, seal_local, verify_local, verify_manifest_proof, ACTION_ADJUDICATE,
    ACTION_COMPILE,
};
pub(super) use crate::control_events::{
    deterministic_event_id, make_control_intent_event, make_control_manifest_event,
    make_control_receipt_event, make_execution_brief_event, new_event_id, now_rfc3339,
};
use crate::control_projection_targets::{bound_pr_target, delivery_target, task_target};
use crate::Ledger;
use edda_core::event::finalize_event;
use edda_core::guided_execution::*;
use edda_core::Event;
use serde::Serialize;
use std::collections::BTreeMap;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
pub(super) const ACTION_TOKEN_TTL_SECONDS: i64 = 300;
#[derive(Debug, Clone)]
pub(super) struct ProjectedControl {
    pub(super) record: ControlManifestRecordV1,
    pub(super) status: ControlStatusV1,
    pub(super) authority_valid: bool,
    pub(super) intents: BTreeMap<String, (String, ControlIntentV1)>,
    pub(super) receipts: BTreeMap<String, (String, ControlReceiptV1)>,
    /// Start of the active review generation; delivery facts remain reusable.
    pub(super) generation_start_state_version: u64,
}
#[derive(Serialize)]
pub(super) struct TokenClaims<'a> {
    pub(super) control_id: &'a str,
    pub(super) manifest_digest: &'a str,
    pub(super) capability_id: &'a str,
    pub(super) observed_state_version: u64,
    pub(super) action_id: &'a str,
    pub(super) action_kind: ControlActionKindV1,
    pub(super) target: &'a ControlTargetV1,
    pub(super) expires_at: &'a str,
}

#[derive(Serialize)]
struct ActionIdentity<'a> {
    control_id: &'a str,
    manifest_digest: &'a str,
    observed_state_version: u64,
    action_kind: ControlActionKindV1,
    target: &'a ControlTargetV1,
}
pub(super) fn project_control(
    ledger: &Ledger,
    control_id: &str,
) -> anyhow::Result<ProjectedControl> {
    let events = ledger.iter_events()?;
    let mut current: Option<ControlManifestRecordV1> = None;
    let mut state = ControlStateV1::Prepared;
    let mut state_version = 0_u64;
    let mut authority_valid = true;
    let mut intents = BTreeMap::new();
    let mut receipts = BTreeMap::new();
    let mut generation_start_state_version = 1_u64;
    let mut last_receipt = None;
    for event in events {
        match event.event_type.as_str() {
            CONTROL_MANIFEST_EVENT_TYPE => {
                let record = parse_manifest_event(&event)?;
                if record.manifest.control_id != control_id {
                    continue;
                }
                if let Err(_error) = verify_manifest_record(ledger, &record) {
                    authority_valid = false;
                }
                if let Some(previous) = &current {
                    let adjudication = record.adjudication.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "replacement control manifest omits adjudication provenance"
                        )
                    })?;
                    anyhow::ensure!(
                        record.manifest.manifest_version == previous.manifest.manifest_version + 1
                            && adjudication.prior_state_version == state_version
                            && adjudication.prior_manifest_digest == previous.manifest_digest,
                        "control adjudication lineage is stale or discontinuous"
                    );
                    state_version += 1;
                    generation_start_state_version = state_version;
                    state = ControlStateV1::Prepared;
                } else {
                    anyhow::ensure!(
                        record.manifest.manifest_version == 1 && record.adjudication.is_none(),
                        "initial control manifest has invalid lineage"
                    );
                    state_version = 1;
                }
                current = Some(record);
            }
            CONTROL_INTENT_EVENT_TYPE => {
                let intent = parse_intent_event(&event)?;
                if intent.control_id != control_id {
                    continue;
                }
                if verify_intent(ledger, &intent).is_err() {
                    authority_valid = false;
                }
                intents.insert(intent.action_id.clone(), (event.event_id, intent));
            }
            CONTROL_RECEIPT_EVENT_TYPE => {
                let receipt = parse_receipt_event(&event)?;
                if receipt.control_id != control_id {
                    continue;
                }
                if verify_receipt(ledger, &receipt).is_err() {
                    authority_valid = false;
                    continue;
                }
                anyhow::ensure!(
                    receipt.observed_state_version == state_version,
                    "control receipt has a stale observed state version"
                );
                let Some((intent_event_id, intent)) = intents.get(&receipt.action_id) else {
                    anyhow::bail!("control receipt has no durable intent");
                };
                anyhow::ensure!(
                    &receipt.intent_event_id == intent_event_id
                        && receipt.action_kind == intent.action_kind
                        && receipt.target == intent.target
                        && receipt.action_token == intent.action_token,
                    "control receipt does not match its durable intent"
                );
                state_version += 1;
                state = receipt.next_state;
                last_receipt = Some(event.event_id.clone());
                receipts.insert(receipt.action_id.clone(), (event.event_id, receipt));
            }
            _ => {}
        }
    }
    let record = current.ok_or_else(|| anyhow::anyhow!("control manifest not found"))?;
    if verify_manifest_record(ledger, &record).is_err()
        || authority_is_active(ledger, &record.authority).is_err()
    {
        authority_valid = false;
    }
    let pending = intents
        .iter()
        .find(|(action_id, (_, intent))| {
            intent.observed_state_version >= generation_start_state_version
                && !receipts.contains_key(*action_id)
        })
        .map(|(_, (event_id, _))| event_id.clone());
    let next_wake = match state {
        ControlStateV1::Completed => "complete",
        ControlStateV1::NeedsDecision => "strong_adjudication",
        _ if !authority_valid => "control_authority_required",
        _ => "immediate",
    }
    .to_string();
    let status = ControlStatusV1 {
        control_id: control_id.to_string(),
        manifest_event_id: record.manifest_event_id.clone(),
        manifest_version: record.manifest.manifest_version,
        manifest_digest: record.manifest_digest.clone(),
        state_version,
        state,
        authority_valid,
        last_receipt_event_id: last_receipt,
        pending_intent_event_id: pending,
        next_wake,
    };
    Ok(ProjectedControl {
        record,
        status,
        authority_valid,
        intents,
        receipts,
        generation_start_state_version,
    })
}

fn pending_intent_next(
    ledger: &Ledger,
    projected: &ProjectedControl,
) -> anyhow::Result<Option<ControlNextV1>> {
    let Some((_, intent)) = projected.intents.values().find(|(_, intent)| {
        intent.observed_state_version >= projected.generation_start_state_version
            && !projected.receipts.contains_key(&intent.action_id)
    }) else {
        return Ok(None);
    };
    let issued_at = OffsetDateTime::parse(&intent.created_at, &Rfc3339)
        .map_err(|_| anyhow::anyhow!("durable intent creation time is malformed"))?;
    let expires = issued_at + time::Duration::seconds(ACTION_TOKEN_TTL_SECONDS);
    let expires_at = expires.format(&Rfc3339)?;
    let claims = TokenClaims {
        control_id: &projected.status.control_id,
        manifest_digest: &projected.record.manifest_digest,
        capability_id: &projected.record.authority.capability_id,
        observed_state_version: intent.observed_state_version,
        action_id: &intent.action_id,
        action_kind: intent.action_kind,
        target: &intent.target,
        expires_at: &expires_at,
    };
    let mac = seal_local(ledger, &claims)?;
    let token = format!(
        "ctl1.{}.{}.{}.{}",
        intent.observed_state_version,
        expires.unix_timestamp(),
        intent.action_id,
        mac
    );
    let commitment = format!("sha256:{}", edda_core::hash::sha256_hex(token.as_bytes()));
    anyhow::ensure!(
        commitment == intent.action_token,
        "pending intent predates deterministic recovery-token support"
    );
    if expires <= OffsetDateTime::now_utc() {
        return Ok(Some(next_without_token(
            projected,
            ControlAvailabilityV1::NeedsDecision,
            ControlActionKindV1::NeedsDecision,
            "EXPIRED_PENDING_INTENT_REQUIRES_ADJUDICATION",
        )));
    }
    Ok(Some(ControlNextV1 {
        control_id: projected.status.control_id.clone(),
        state_version: projected.status.state_version,
        availability: ControlAvailabilityV1::Available,
        action_kind: intent.action_kind,
        target: intent.target.clone(),
        action_id: intent.action_id.clone(),
        token: Some(token),
        expires_at: Some(expires_at),
        reason_code: "PENDING_INTENT_RECOVERY_READY".into(),
    }))
}

#[allow(clippy::too_many_lines)]
fn next_action(
    ledger: &Ledger,
    projected: &ProjectedControl,
) -> anyhow::Result<(
    ControlActionKindV1,
    ControlTargetV1,
    ControlAvailabilityV1,
    &'static str,
)> {
    let status = &projected.status;
    if !projected.record.manifest.return_for_decision.is_empty() {
        return Ok((
            ControlActionKindV1::NeedsDecision,
            ControlTargetV1::Control {
                control_id: status.control_id.clone(),
            },
            ControlAvailabilityV1::Available,
            "DECLARED_DECISION_REQUIRED",
        ));
    }
    if projected.record.manifest.completion_condition
        == ControlCompletionConditionV1::LocalPreparationOnly
    {
        return Ok((
            ControlActionKindV1::Complete,
            ControlTargetV1::Control {
                control_id: status.control_id.clone(),
            },
            ControlAvailabilityV1::Available,
            "LOCAL_COMPLETION_READY",
        ));
    }
    if projected.record.manifest.tasks.is_empty() {
        return match projected.record.manifest.completion_condition {
            ControlCompletionConditionV1::LocalPreparationOnly => Ok((
                ControlActionKindV1::Complete,
                ControlTargetV1::Control {
                    control_id: status.control_id.clone(),
                },
                ControlAvailabilityV1::Available,
                "ACTION_READY",
            )),
            ControlCompletionConditionV1::VerificationSucceeded
                if projected.record.manifest.merge_policy.pr_number.is_none() =>
            {
                Ok((
                    ControlActionKindV1::RequestVerification,
                    ControlTargetV1::Control {
                        control_id: status.control_id.clone(),
                    },
                    ControlAvailabilityV1::ControlUnavailable,
                    "PR_HEAD_BINDING_REQUIRED",
                ))
            }
            ControlCompletionConditionV1::VerificationSucceeded => {
                let target = bound_pr_target(projected)?;
                let claimed = projected.receipts.values().any(|(_, receipt)| {
                    receipt.observed_state_version >= projected.generation_start_state_version
                        && receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::ClaimVerification
                });
                if !claimed {
                    return Ok((
                        ControlActionKindV1::ClaimVerification,
                        target,
                        ControlAvailabilityV1::Available,
                        "VERIFICATION_CLAIM_READY",
                    ));
                }
                let requested = projected.receipts.values().any(|(_, receipt)| {
                    receipt.observed_state_version >= projected.generation_start_state_version
                        && receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::RequestVerification
                });
                Ok(if requested {
                    (
                        ControlActionKindV1::WaitForWorkers,
                        target,
                        ControlAvailabilityV1::ControlUnavailable,
                        "REVIEW_RESULT_PENDING",
                    )
                } else {
                    (
                        ControlActionKindV1::RequestVerification,
                        target,
                        ControlAvailabilityV1::Available,
                        "VERIFICATION_REQUEST_READY",
                    )
                })
            }
            ControlCompletionConditionV1::DelegatedMergeSucceeded => {
                let target = bound_pr_target(projected)?;
                let claimed = projected.receipts.values().any(|(_, receipt)| {
                    receipt.observed_state_version >= projected.generation_start_state_version
                        && receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::ClaimVerification
                });
                if !claimed {
                    return Ok((
                        ControlActionKindV1::ClaimVerification,
                        target,
                        ControlAvailabilityV1::Available,
                        "VERIFICATION_CLAIM_READY",
                    ));
                }
                let requested = projected.receipts.values().any(|(_, receipt)| {
                    receipt.observed_state_version >= projected.generation_start_state_version
                        && receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::RequestVerification
                });
                Ok(if requested {
                    (
                        ControlActionKindV1::MergeDelegated,
                        target,
                        ControlAvailabilityV1::Available,
                        "DELEGATED_MERGE_READY",
                    )
                } else {
                    (
                        ControlActionKindV1::RequestVerification,
                        target,
                        ControlAvailabilityV1::Available,
                        "VERIFICATION_REQUEST_READY",
                    )
                })
            }
        };
    }
    for task in &projected.record.manifest.tasks {
        if task.local_only {
            continue;
        }
        let admitted = projected.receipts.values().any(|(_, receipt)| {
            receipt.next_state != ControlStateV1::NeedsDecision
                && receipt.action_kind == ControlActionKindV1::AdmitAndClaimIssue
                && matches!(
                    &receipt.target,
                    ControlTargetV1::Task { task_key, .. } if task_key == &task.task_key
                )
        });
        if !admitted {
            return Ok((
                ControlActionKindV1::AdmitAndClaimIssue,
                task_target(ledger, task)?,
                ControlAvailabilityV1::Available,
                "ISSUE_ADMISSION_READY",
            ));
        }
    }

    let views = ledger.task_views()?;
    let active = views
        .iter()
        .filter(|view| {
            view.status == crate::TaskStatus::Running
                && projected
                    .record
                    .manifest
                    .tasks
                    .iter()
                    .any(|task| task.task_id == Some(view.task_id))
        })
        .count();
    let mut waiting_target = None;
    for task in &projected.record.manifest.tasks {
        let target = task_target(ledger, task)?;
        let dependency_pending = task.depends_on.iter().any(|dependency_key| {
            projected
                .record
                .manifest
                .tasks
                .iter()
                .find(|candidate| &candidate.task_key == dependency_key)
                .and_then(|dependency| dependency.task_id)
                .and_then(|id| views.iter().find(|view| view.task_id == id))
                .is_none_or(|view| view.status != crate::TaskStatus::Done)
        });
        if dependency_pending {
            waiting_target.get_or_insert(target);
            continue;
        }
        let Some(task_id) = task.task_id else {
            return Ok((
                ControlActionKindV1::ControlError,
                target,
                ControlAvailabilityV1::NeedsDecision,
                "TASK_RAIL_BINDING_REQUIRED",
            ));
        };
        let Some(view) = views.iter().find(|view| view.task_id == task_id) else {
            return Ok((
                ControlActionKindV1::ControlError,
                task_target(ledger, task)?,
                ControlAvailabilityV1::NeedsDecision,
                "TASK_RAIL_SUBJECT_MISSING",
            ));
        };
        match view.status {
            crate::TaskStatus::Ready | crate::TaskStatus::Failed => {
                if active >= projected.record.manifest.capacity.max_workers as usize {
                    waiting_target.get_or_insert(target);
                    continue;
                }
                return Ok((
                    ControlActionKindV1::PrepareAttempt,
                    target,
                    ControlAvailabilityV1::Available,
                    "ATTEMPT_PREPARATION_READY",
                ));
            }
            crate::TaskStatus::Blocked => {
                waiting_target.get_or_insert(target);
                continue;
            }
            crate::TaskStatus::Running => {
                let prepared = projected.receipts.values().any(|(_, receipt)| {
                    receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::PrepareAttempt
                        && receipt.target == target
                });
                if !prepared {
                    return Ok((
                        ControlActionKindV1::PrepareAttempt,
                        target,
                        ControlAvailabilityV1::Available,
                        "ATTEMPT_PREPARATION_RECOVERY_READY",
                    ));
                }
                let dispatched = projected.receipts.values().any(|(_, receipt)| {
                    receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::DispatchTask
                        && receipt.target == target
                });
                if dispatched {
                    waiting_target.get_or_insert(target);
                    continue;
                }
                return Ok((
                    ControlActionKindV1::DispatchTask,
                    target,
                    ControlAvailabilityV1::Available,
                    "TASK_DISPATCH_READY",
                ));
            }
            crate::TaskStatus::Done => {
                if task.required_delivery == DeliveryRequirementV1::LocalOnly {
                    if !task.local_only {
                        let prepared_and_dispatched = [
                            ControlActionKindV1::PrepareAttempt,
                            ControlActionKindV1::DispatchTask,
                        ]
                        .into_iter()
                        .all(|action| {
                            projected.receipts.values().any(|(_, receipt)| {
                                receipt.next_state != ControlStateV1::NeedsDecision
                                    && receipt.action_kind == action
                                    && receipt.target == target
                            })
                        });
                        if !prepared_and_dispatched {
                            return Ok((
                                ControlActionKindV1::ControlError,
                                target,
                                ControlAvailabilityV1::NeedsDecision,
                                "CONTROLLED_TASK_EXECUTION_RECEIPTS_MISSING",
                            ));
                        }
                    }
                    continue;
                }
                let Some(delivery) = delivery_target(ledger, task)? else {
                    return Ok((
                        ControlActionKindV1::ControlError,
                        target,
                        ControlAvailabilityV1::NeedsDecision,
                        "DELIVERY_RECEIPT_MISSING",
                    ));
                };
                let already_bound = projected.receipts.values().any(|(_, receipt)| {
                    receipt.next_state != ControlStateV1::NeedsDecision
                        && receipt.action_kind == ControlActionKindV1::BindDelivery
                        && receipt.target == delivery
                });
                if already_bound {
                    continue;
                }
                return Ok((
                    ControlActionKindV1::BindDelivery,
                    delivery,
                    ControlAvailabilityV1::Available,
                    "DELIVERY_BINDING_READY",
                ));
            }
        }
    }
    if let Some(target) = waiting_target {
        return Ok((
            ControlActionKindV1::WaitForWorkers,
            target,
            ControlAvailabilityV1::ControlUnavailable,
            "WORK_RECEIPT_PENDING",
        ));
    }

    let pr_target = || bound_pr_target(projected);
    if projected.record.manifest.completion_condition
        != ControlCompletionConditionV1::LocalPreparationOnly
    {
        let claim = projected.receipts.values().any(|(_, receipt)| {
            receipt.observed_state_version >= projected.generation_start_state_version
                && receipt.next_state != ControlStateV1::NeedsDecision
                && receipt.action_kind == ControlActionKindV1::ClaimVerification
        });
        if !claim {
            return Ok((
                ControlActionKindV1::ClaimVerification,
                pr_target()?,
                ControlAvailabilityV1::Available,
                "VERIFICATION_CLAIM_READY",
            ));
        }
        let requested = projected.receipts.values().any(|(_, receipt)| {
            receipt.observed_state_version >= projected.generation_start_state_version
                && receipt.next_state != ControlStateV1::NeedsDecision
                && receipt.action_kind == ControlActionKindV1::RequestVerification
        });
        if !requested {
            return Ok((
                ControlActionKindV1::RequestVerification,
                pr_target()?,
                ControlAvailabilityV1::Available,
                "VERIFICATION_REQUEST_READY",
            ));
        }
        if !projected.record.manifest.merge_policy.required {
            return Ok((
                ControlActionKindV1::WaitForWorkers,
                pr_target()?,
                ControlAvailabilityV1::ControlUnavailable,
                "REVIEW_RESULT_PENDING",
            ));
        }
    }
    if projected.record.manifest.merge_policy.required {
        return Ok((
            ControlActionKindV1::MergeDelegated,
            pr_target()?,
            ControlAvailabilityV1::Available,
            "DELEGATED_MERGE_READY",
        ));
    }
    Ok((
        ControlActionKindV1::Complete,
        ControlTargetV1::Control {
            control_id: status.control_id.clone(),
        },
        ControlAvailabilityV1::Available,
        "CONTROL_COMPLETE",
    ))
}

pub(super) fn has_expired_pending_intent(projected: &ProjectedControl) -> anyhow::Result<bool> {
    projected
        .intents
        .values()
        .filter(|(_, intent)| {
            intent.observed_state_version >= projected.generation_start_state_version
                && !projected.receipts.contains_key(&intent.action_id)
        })
        .try_fold(false, |expired, (_, intent)| {
            let issued_at = OffsetDateTime::parse(&intent.created_at, &Rfc3339)
                .map_err(|_| anyhow::anyhow!("durable intent creation time is malformed"))?;
            Ok(expired
                || issued_at + time::Duration::seconds(ACTION_TOKEN_TTL_SECONDS)
                    <= OffsetDateTime::now_utc())
        })
}

pub(super) fn next_for_projected(
    ledger: &Ledger,
    projected: &ProjectedControl,
) -> anyhow::Result<ControlNextV1> {
    let status = &projected.status;
    if let Some(pending) = pending_intent_next(ledger, projected)? {
        return Ok(pending);
    }
    if status.state == ControlStateV1::Completed {
        return Ok(next_without_token(
            projected,
            ControlAvailabilityV1::Complete,
            ControlActionKindV1::Complete,
            "CONTROL_COMPLETE",
        ));
    }
    if status.state == ControlStateV1::NeedsDecision {
        return Ok(next_without_token(
            projected,
            ControlAvailabilityV1::NeedsDecision,
            ControlActionKindV1::NeedsDecision,
            "STRONG_ADJUDICATION_REQUIRED",
        ));
    }
    let (action, target, availability, reason) = next_action(ledger, projected)?;
    let action_id = action_id(projected, action, &target)?;
    if action == ControlActionKindV1::MergeDelegated {
        return Ok(ControlNextV1 {
            control_id: status.control_id.clone(),
            state_version: status.state_version,
            availability: ControlAvailabilityV1::ControlUnavailable,
            action_kind: action,
            target,
            action_id,
            token: None,
            expires_at: None,
            reason_code: "DELEGATED_MERGE_ATOMIC_BASE_PRECONDITION_UNAVAILABLE".into(),
        });
    }
    if !action.is_applicable() {
        return Ok(ControlNextV1 {
            control_id: status.control_id.clone(),
            state_version: status.state_version,
            availability,
            action_kind: action,
            target,
            action_id,
            token: None,
            expires_at: None,
            reason_code: reason.into(),
        });
    }
    let expires_unix = OffsetDateTime::now_utc().unix_timestamp() + ACTION_TOKEN_TTL_SECONDS;
    let expires = OffsetDateTime::from_unix_timestamp(expires_unix)?;
    let expires_at = expires.format(&Rfc3339)?;
    let claims = TokenClaims {
        control_id: &status.control_id,
        manifest_digest: &projected.record.manifest_digest,
        capability_id: &projected.record.authority.capability_id,
        observed_state_version: status.state_version,
        action_id: &action_id,
        action_kind: action,
        target: &target,
        expires_at: &expires_at,
    };
    let mac = seal_local(ledger, &claims)?;
    let token = format!(
        "ctl1.{}.{}.{}.{}",
        status.state_version,
        expires.unix_timestamp(),
        action_id,
        mac
    );
    Ok(ControlNextV1 {
        control_id: status.control_id.clone(),
        state_version: status.state_version,
        availability,
        action_kind: action,
        target,
        action_id,
        token: Some(token),
        expires_at: Some(expires_at),
        reason_code: reason.into(),
    })
}

pub(super) fn next_without_token(
    projected: &ProjectedControl,
    availability: ControlAvailabilityV1,
    action: ControlActionKindV1,
    reason: &str,
) -> ControlNextV1 {
    let target = ControlTargetV1::Control {
        control_id: projected.status.control_id.clone(),
    };
    let action_id =
        action_id(projected, action, &target).unwrap_or_else(|_| "action_unavailable".into());
    ControlNextV1 {
        control_id: projected.status.control_id.clone(),
        state_version: projected.status.state_version,
        availability,
        action_kind: action,
        target,
        action_id,
        token: None,
        expires_at: None,
        reason_code: reason.into(),
    }
}

pub(super) fn action_id(
    projected: &ProjectedControl,
    action: ControlActionKindV1,
    target: &ControlTargetV1,
) -> anyhow::Result<String> {
    let bytes = edda_core::canon::canonical_json_bytes(&serde_json::to_value(ActionIdentity {
        control_id: &projected.status.control_id,
        manifest_digest: &projected.record.manifest_digest,
        observed_state_version: projected.status.state_version,
        action_kind: action,
        target,
    })?)?;
    Ok(format!("action_{}", edda_core::hash::sha256_hex(&bytes)))
}

pub(super) fn parse_token(token: &str) -> anyhow::Result<(u64, String, String, String)> {
    let parts: Vec<_> = token.split('.').collect();
    anyhow::ensure!(
        parts.len() == 5 && parts[0] == "ctl1",
        "opaque action token is malformed"
    );
    let version = parts[1]
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("opaque action token is malformed"))?;
    let unix = parts[2]
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("opaque action token is malformed"))?;
    anyhow::ensure!(
        parts[3].starts_with("action_") && parts[3].len() == 71,
        "opaque action token action binding is malformed"
    );
    anyhow::ensure!(
        parts[4].len() == 64
            && parts[4]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "opaque action token seal is malformed"
    );
    let expires_at = OffsetDateTime::from_unix_timestamp(unix)?.format(&Rfc3339)?;
    Ok((
        version,
        expires_at,
        parts[3].to_string(),
        parts[4].to_string(),
    ))
}

pub(super) fn stale_apply(
    projected: &ProjectedControl,
    action_id: String,
    result: &str,
) -> ControlApplyV1 {
    ControlApplyV1 {
        applied: false,
        stale: true,
        control_id: projected.status.control_id.clone(),
        action_id,
        intent_event_id: None,
        receipt_event_id: None,
        state_version: projected.status.state_version,
        state: projected.status.state,
        result: result.into(),
    }
}

pub(super) fn parse_manifest_event(event: &Event) -> anyhow::Result<ControlManifestRecordV1> {
    anyhow::ensure!(
        event.event_type == CONTROL_MANIFEST_EVENT_TYPE,
        "event is not a control manifest"
    );
    let record: ControlManifestRecordV1 = serde_json::from_value(
        event
            .payload
            .get("control_manifest")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("control manifest event omits its record"))?,
    )
    .map_err(|_| anyhow::anyhow!("control manifest event has an unsupported schema"))?;
    anyhow::ensure!(
        record.record_version == CONTROL_MANIFEST_RECORD_VERSION
            && record.manifest_event_id == event.event_id,
        "control manifest event identity mismatch"
    );
    let mut verified = event.clone();
    finalize_event(&mut verified)?;
    anyhow::ensure!(
        verified.hash == event.hash && verified.digests == event.digests,
        "control manifest event integrity mismatch"
    );
    let canonical = canonical_manifest_bytes(&record.manifest)?;
    anyhow::ensure!(
        hex::decode(&record.canonical_bytes_hex)? == canonical
            && edda_core::hash::sha256_hex(&canonical) == record.manifest_digest,
        "control manifest canonical bytes or digest mismatch"
    );
    Ok(record)
}

pub(super) fn verify_manifest_record(
    ledger: &Ledger,
    record: &ControlManifestRecordV1,
) -> anyhow::Result<()> {
    validate_control_manifest(&record.manifest)?;
    anyhow::ensure!(
        record.authority.portable_repo_id == record.manifest.basis.portable_repo_id
            && record.authority.github_repository == record.manifest.basis.github_repository,
        "control manifest repository authority mismatch"
    );
    let expected_action = if record.adjudication.is_some() {
        ACTION_ADJUDICATE
    } else {
        ACTION_COMPILE
    };
    anyhow::ensure!(
        record.authority.permitted_action == expected_action,
        "control manifest used the wrong authority action"
    );
    verify_manifest_proof(
        ledger,
        &record.authority,
        &record.manifest_event_id,
        &record.manifest_digest,
        record.manifest.manifest_version,
        record.adjudication.as_ref(),
    )
}

pub(super) fn parse_intent_event(event: &Event) -> anyhow::Result<ControlIntentV1> {
    serde_json::from_value(
        event
            .payload
            .get("control_intent")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("control intent event omits its record"))?,
    )
    .map_err(|_| anyhow::anyhow!("control intent event has an unsupported schema"))
}

pub(super) fn parse_receipt_event(event: &Event) -> anyhow::Result<ControlReceiptV1> {
    serde_json::from_value(
        event
            .payload
            .get("control_receipt")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("control receipt event omits its record"))?,
    )
    .map_err(|_| anyhow::anyhow!("control receipt event has an unsupported schema"))
}

pub(super) fn intent_without_seal(intent: &ControlIntentV1) -> anyhow::Result<serde_json::Value> {
    without_seal(intent)
}
pub(super) fn receipt_without_seal(
    receipt: &ControlReceiptV1,
) -> anyhow::Result<serde_json::Value> {
    without_seal(receipt)
}
pub(super) fn without_seal<T: Serialize>(value: &T) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(value)?;
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("sealed control record is not an object"))?
        .remove("seal");
    Ok(value)
}
pub(super) fn verify_intent(ledger: &Ledger, intent: &ControlIntentV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        intent.intent_version == CONTROL_INTENT_VERSION && intent.action_kind.is_applicable(),
        "unsupported control intent"
    );
    validate_authorization_commitment(&intent.action_token)?;
    verify_local(ledger, &intent_without_seal(intent)?, &intent.seal)
}
pub(super) fn verify_receipt(ledger: &Ledger, receipt: &ControlReceiptV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        receipt.receipt_version == CONTROL_RECEIPT_VERSION && receipt.action_kind.is_applicable(),
        "unsupported control receipt"
    );
    validate_authorization_commitment(&receipt.action_token)?;
    crate::control_review_artifact::validate_external_identity(
        receipt.external_identity.as_deref(),
    )?;
    verify_local(ledger, &receipt_without_seal(receipt)?, &receipt.seal)
}

fn validate_authorization_commitment(value: &str) -> anyhow::Result<()> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow::anyhow!("control authorization commitment is malformed"))?;
    anyhow::ensure!(
        digest.len() == 64
            && digest
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
        "control authorization commitment is malformed"
    );
    Ok(())
}
