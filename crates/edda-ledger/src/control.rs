use crate::control_authority::{
    authorize_command, provision_local_merge_capability, seal_local, seal_manifest_proof,
    verify_local, verify_local_merge_capability, ControlMergeCapabilityProvision,
    VerifiedControlMergeCapabilityV1, ACTION_ADJUDICATE, ACTION_COMPILE,
};
use crate::control_projection::*;
use crate::{AcceptedExecutionBriefV1, Ledger, WorkspaceLock};
use edda_core::guided_execution::*;
use edda_core::{Event, Refs};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct CompiledControlV1 {
    pub manifest_event_id: String,
    pub manifest_digest: String,
    pub manifest: ControlManifestV1,
    pub briefs: Vec<AcceptedExecutionBriefV1>,
}

/// Product-owned input to one external control adapter. The durable intent
/// named here has already been appended before the adapter is called.
#[derive(Debug, Clone)]
pub struct ControlEffectRequestV1 {
    pub manifest: ControlManifestV1,
    pub manifest_digest: String,
    pub authority: ControlAuthorityProofV1,
    pub intent_event_id: String,
    pub intent: ControlIntentV1,
    pub recovering: bool,
}

/// Structured result returned by a product adapter. Free-form backend output
/// is evidence only and never selects a transition.
#[derive(Debug, Clone)]
pub struct ControlEffectResultV1 {
    /// The external operation has a durable adoptable handle but no terminal
    /// structured result yet. The intent remains pending and no receipt/state
    /// transition is invented.
    pub durable_pending: bool,
    pub product_result: String,
    pub dispatch_handle: Option<String>,
    pub event_ids: Vec<String>,
    pub external_identity: Option<String>,
    pub cost_microusd: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub ephemeral_artifact_digest: Option<String>,
    pub next_state: ControlStateV1,
    pub next_wake: String,
}

impl ControlEffectResultV1 {
    pub fn applied(
        action: ControlActionKindV1,
        product_result: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let next_state = next_state_for_action(action)?;
        Ok(Self {
            durable_pending: false,
            product_result: product_result.into(),
            dispatch_handle: None,
            event_ids: Vec::new(),
            external_identity: None,
            cost_microusd: None,
            elapsed_ms: None,
            ephemeral_artifact_digest: None,
            next_state,
            next_wake: if next_state == ControlStateV1::Completed {
                "complete".into()
            } else {
                "immediate".into()
            },
        })
    }

    pub fn durable_pending(
        product_result: impl Into<String>,
        dispatch_handle: String,
        next_wake: impl Into<String>,
    ) -> Self {
        Self {
            durable_pending: true,
            product_result: product_result.into(),
            dispatch_handle: Some(dispatch_handle),
            event_ids: Vec::new(),
            external_identity: None,
            cost_microusd: None,
            elapsed_ms: None,
            ephemeral_artifact_digest: None,
            next_state: ControlStateV1::Verifying,
            next_wake: next_wake.into(),
        }
    }

    pub fn needs_decision(reason_code: impl Into<String>) -> Self {
        Self {
            durable_pending: false,
            product_result: reason_code.into(),
            dispatch_handle: None,
            event_ids: Vec::new(),
            external_identity: None,
            cost_microusd: None,
            elapsed_ms: None,
            ephemeral_artifact_digest: None,
            next_state: ControlStateV1::NeedsDecision,
            next_wake: "strong_adjudication".into(),
        }
    }
}

struct PresentedActionToken {
    observed_state_version: u64,
    expires_at: String,
    expiry: OffsetDateTime,
    action_id: String,
    mac: String,
    commitment: String,
}

impl PresentedActionToken {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let (observed_state_version, expires_at, action_id, mac) = parse_token(raw)?;
        let expiry = OffsetDateTime::parse(&expires_at, &Rfc3339)
            .map_err(|_| anyhow::anyhow!("action token expiry is malformed"))?;
        Ok(Self {
            observed_state_version,
            expires_at,
            expiry,
            action_id,
            mac,
            commitment: format!("sha256:{}", edda_core::hash::sha256_hex(raw.as_bytes())),
        })
    }
}

struct AuthorizedLocalTransition {
    action_id: String,
    action_kind: ControlActionKindV1,
    target: ControlTargetV1,
    intent_event_id: String,
    intent: ControlIntentV1,
}

fn verify_presented_token(
    ledger: &Ledger,
    projected: &ProjectedControl,
    control_id: &str,
    token: &PresentedActionToken,
    action_kind: ControlActionKindV1,
    target: &ControlTargetV1,
) -> anyhow::Result<()> {
    let claims = TokenClaims {
        control_id,
        manifest_digest: &projected.record.manifest_digest,
        capability_id: &projected.record.authority.capability_id,
        observed_state_version: token.observed_state_version,
        action_id: &token.action_id,
        action_kind,
        target,
        expires_at: &token.expires_at,
    };
    verify_local(ledger, &claims, &token.mac)
}

fn parse_live_action_token(raw_token: &str) -> anyhow::Result<PresentedActionToken> {
    let token = PresentedActionToken::parse(raw_token)?;
    anyhow::ensure!(
        token.expiry > OffsetDateTime::now_utc(),
        "expired action token refused"
    );
    Ok(token)
}

fn validate_effect_result(
    action: ControlActionKindV1,
    result: &ControlEffectResultV1,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !result.product_result.trim().is_empty() && result.product_result.len() <= 1_000,
        "control effect result has an invalid product result"
    );
    anyhow::ensure!(
        !result.next_wake.trim().is_empty() && result.next_wake.len() <= 1_000,
        "control effect result has an invalid wake condition"
    );
    let expected = next_state_for_action(action)?;
    let verification_terminal = action == ControlActionKindV1::RequestVerification
        && matches!(
            result.next_state,
            ControlStateV1::Completed | ControlStateV1::MergeReady
        );
    anyhow::ensure!(
        result.next_state == expected
            || result.next_state == ControlStateV1::NeedsDecision
            || verification_terminal,
        "control effect result selected an invalid transition"
    );
    anyhow::ensure!(
        !result.durable_pending
            || (action == ControlActionKindV1::RequestVerification
                && result.dispatch_handle.is_some()
                && result.next_state == ControlStateV1::Verifying),
        "only an adoptable verification dispatch may remain durably pending"
    );
    anyhow::ensure!(
        action != ControlActionKindV1::DispatchTask
            || result.dispatch_handle.is_some()
            || result.next_state == ControlStateV1::NeedsDecision,
        "successful dispatch result omits its durable handle"
    );
    if let Some(digest) = &result.ephemeral_artifact_digest {
        anyhow::ensure!(
            matches!(
                action,
                ControlActionKindV1::ClaimVerification | ControlActionKindV1::RequestVerification
            ) && !result.durable_pending,
            "only a terminal review result may schedule artifact cleanup"
        );
        crate::control_review_artifact::validate_digest(digest)?;
    }
    Ok(())
}

fn authorize_and_commit_intent(
    ledger: &Ledger,
    projected: &ProjectedControl,
    control_id: &str,
    token: &PresentedActionToken,
) -> anyhow::Result<AuthorizedLocalTransition> {
    if let Some(existing) = projected.intents.get(&token.action_id) {
        let intent = &existing.1;
        anyhow::ensure!(
            intent.control_id == control_id
                && intent.observed_state_version == projected.status.state_version
                && intent.action_id == action_id(projected, intent.action_kind, &intent.target)?,
            "existing durable intent does not match this action"
        );
        let created_at = OffsetDateTime::parse(&intent.created_at, &Rfc3339)
            .map_err(|_| anyhow::anyhow!("durable intent creation time is malformed"))?;
        anyhow::ensure!(
            created_at <= token.expiry,
            "durable intent was not authorized before token expiry"
        );
        anyhow::ensure!(
            token.expiry > OffsetDateTime::now_utc(),
            "expired pending action token refused"
        );
        verify_presented_token(
            ledger,
            projected,
            control_id,
            token,
            intent.action_kind,
            &intent.target,
        )?;
        anyhow::ensure!(
            intent.action_token == token.commitment,
            "recovery token does not match the durable intent authorization commitment"
        );
        return Ok(AuthorizedLocalTransition {
            action_id: intent.action_id.clone(),
            action_kind: intent.action_kind,
            target: intent.target.clone(),
            intent_event_id: existing.0.clone(),
            intent: intent.clone(),
        });
    }

    anyhow::ensure!(
        token.expiry > OffsetDateTime::now_utc(),
        "expired action token refused"
    );
    let next = next_for_projected(ledger, projected)?;
    anyhow::ensure!(
        next.token.is_some(),
        "CONTROL_UNAVAILABLE: action has no applicable local token"
    );
    anyhow::ensure!(
        next.action_kind.is_applicable(),
        "observation-only control action cannot be applied"
    );
    anyhow::ensure!(
        token.action_id == next.action_id,
        "foreign action token refused"
    );
    verify_presented_token(
        ledger,
        projected,
        control_id,
        token,
        next.action_kind,
        &next.target,
    )?;
    let intent_event_id = deterministic_event_id("intent", &next.action_id);
    let mut intent = ControlIntentV1 {
        intent_version: CONTROL_INTENT_VERSION,
        control_id: control_id.to_string(),
        step_id: format!("step_{}", projected.status.state_version),
        action_id: next.action_id.clone(),
        observed_state_version: projected.status.state_version,
        action_token: token.commitment.clone(),
        action_kind: next.action_kind,
        target: next.target.clone(),
        // Persist the issuance instant, not the bearer. Together with the
        // immutable intent fields it lets a replacement controller recompute
        // the exact issuer-authenticated token without any state mutation.
        created_at: (token.expiry
            - time::Duration::seconds(crate::control_projection::ACTION_TOKEN_TTL_SECONDS))
        .format(&Rfc3339)?,
        seal: String::new(),
    };
    intent.seal = seal_local(ledger, &intent_without_seal(&intent)?)?;
    let event = make_control_intent_event(
        intent_event_id.clone(),
        &ledger.head_branch()?,
        ledger.last_event_hash()?.as_deref(),
        serde_json::json!({"control_intent": intent}),
        Refs {
            events: vec![projected.record.manifest_event_id.clone()],
            ..Refs::default()
        },
    )?;
    ledger.append_event(&event)?;
    Ok(AuthorizedLocalTransition {
        action_id: next.action_id,
        action_kind: next.action_kind,
        target: next.target,
        intent_event_id,
        intent,
    })
}

impl Ledger {
    /// Compile referenced brief inputs and their control manifest in one SQLite
    /// transaction under a locally sealed strong-controller capability.
    pub fn compile_control(
        &self,
        input: ControlCompileInputV1,
        caller_session: &str,
        authority_token: &str,
    ) -> anyhow::Result<CompiledControlV1> {
        anyhow::ensure!(
            input.compile_version == CONTROL_COMPILE_INPUT_VERSION,
            "unsupported control compile input version"
        );
        validate_control_raw(&serde_json::to_vec(&input)?, "control compile input")?;
        let _lock = WorkspaceLock::acquire(&self.paths)?;
        for event in self.iter_events_by_type(CONTROL_MANIFEST_EVENT_TYPE)? {
            let record = parse_manifest_event(&event)?;
            anyhow::ensure!(
                record.manifest.control_id != input.manifest.control_id,
                "control_id already exists"
            );
        }
        let mut proof = authorize_command(
            self,
            caller_session,
            ACTION_COMPILE,
            input.manifest.basis.portable_repo_id.as_deref(),
            input.manifest.basis.github_repository.as_deref(),
            authority_token,
        )?;
        let branch = self.head_branch()?;
        let manifest_event_id = new_event_id();
        let mut compiled_briefs = Vec::with_capacity(input.brief_inputs.len());
        let mut accepted = Vec::with_capacity(input.brief_inputs.len());
        for brief_input in input.brief_inputs {
            let event_id = new_event_id();
            let (brief, canonical_bytes) =
                compile_execution_brief(brief_input, event_id, &proof.principal_id)?;
            accepted.push((brief.clone(), canonical_bytes));
            compiled_briefs.push(brief);
        }
        let (manifest, canonical_bytes) =
            compile_control_manifest(input.manifest, &compiled_briefs, 1)?;
        let digest = edda_core::hash::sha256_hex(&canonical_bytes);
        proof = seal_manifest_proof(
            self,
            proof,
            &manifest_event_id,
            &digest,
            manifest.manifest_version,
            None,
        )?;

        let mut events = Vec::with_capacity(accepted.len() + 1);
        let mut parent = self.last_event_hash()?;
        for (brief, brief_bytes) in &accepted {
            let authority = ExecutionBriefAuthorityV1 {
                principal_id: proof.principal_id.clone(),
                session_id: proof.session_id.clone(),
                authority_event_id: manifest_event_id.clone(),
            };
            let record = ExecutionBriefRecordV1 {
                record_version: EXECUTION_BRIEF_RECORD_VERSION,
                trust: BriefTrustV1::LocallyAccepted,
                authority,
                brief_event_id: brief.brief_event_id.clone(),
                content_digest: brief.content_digest.clone(),
                canonical_bytes_hex: hex::encode(brief_bytes),
                brief: brief.clone(),
            };
            let event = make_execution_brief_event(
                brief.brief_event_id.clone(),
                &branch,
                parent.as_deref(),
                serde_json::json!({
                    "trust": "locally_accepted",
                    "execution_brief": record,
                }),
                Refs::default(),
            )?;
            parent = Some(event.hash.clone());
            events.push(event);
        }
        let authority_principal = proof.principal_id.clone();
        let authority_session = proof.session_id.clone();
        let record = ControlManifestRecordV1 {
            record_version: CONTROL_MANIFEST_RECORD_VERSION,
            manifest_event_id: manifest_event_id.clone(),
            manifest_digest: digest.clone(),
            canonical_bytes_hex: hex::encode(&canonical_bytes),
            manifest: manifest.clone(),
            authority: proof,
            adjudication: None,
        };
        let refs = Refs {
            events: manifest
                .tasks
                .iter()
                .map(|task| task.brief.brief_event_id.clone())
                .collect(),
            ..Refs::default()
        };
        events.push(make_control_manifest_event(
            manifest_event_id.clone(),
            &branch,
            parent.as_deref(),
            serde_json::json!({"control_manifest": record}),
            refs,
        )?);
        self.sqlite.append_control_compile_batch(&events)?;

        let briefs = accepted
            .into_iter()
            .map(|(brief, canonical_bytes)| AcceptedExecutionBriefV1 {
                event_id: brief.brief_event_id.clone(),
                content_digest: brief.content_digest.clone(),
                canonical_bytes,
                authority: ExecutionBriefAuthorityV1 {
                    principal_id: authority_principal.clone(),
                    session_id: authority_session.clone(),
                    authority_event_id: manifest_event_id.clone(),
                },
                brief,
            })
            .collect();
        Ok(CompiledControlV1 {
            manifest_event_id,
            manifest_digest: digest,
            manifest,
            briefs,
        })
    }

    /// Read-only projection. Opening the ledger with `open_existing` keeps this
    /// method from creating, migrating, or checkpointing state.
    pub fn control_status(&self, control_id: &str) -> anyhow::Result<ControlStatusV1> {
        Ok(project_control(self, control_id)?.status)
    }

    pub fn control_receipts(&self, control_id: &str) -> anyhow::Result<Vec<ControlReceiptV1>> {
        Ok(project_control(self, control_id)?
            .receipts
            .into_values()
            .map(|(_, receipt)| receipt)
            .collect())
    }

    /// Resolve the one sealed dispatch correlation for an active Task Rail
    /// attempt. Ambiguous bindings fail closed rather than selecting by order.
    pub fn control_dispatch_binding(
        &self,
        task_id: u64,
        attempt: u32,
    ) -> anyhow::Result<Option<(String, String)>> {
        let mut bindings = Vec::new();
        for event in self.iter_events_by_type(CONTROL_MANIFEST_EVENT_TYPE)? {
            let record = parse_manifest_event(&event)?;
            let Some(task) = record
                .manifest
                .tasks
                .iter()
                .find(|task| task.task_id == Some(task_id))
            else {
                continue;
            };
            let target = ControlTargetV1::Task {
                task_key: task.task_key.clone(),
                attempt,
            };
            let projected = project_control(self, &record.manifest.control_id)?;
            if !projected.authority_valid {
                continue;
            }
            bindings.extend(projected.receipts.values().filter_map(|(_, receipt)| {
                if receipt.next_state != ControlStateV1::NeedsDecision
                    && receipt.action_kind == ControlActionKindV1::DispatchTask
                    && receipt.target == target
                {
                    Some((receipt.control_id.clone(), receipt.step_id.clone()))
                } else {
                    None
                }
            }));
            bindings.extend(projected.intents.values().filter_map(|(_, intent)| {
                if intent.action_kind == ControlActionKindV1::DispatchTask
                    && intent.target == target
                    && projected
                        .receipts
                        .get(&intent.action_id)
                        .is_none_or(|(_, receipt)| {
                            receipt.next_state != ControlStateV1::NeedsDecision
                        })
                {
                    Some((intent.control_id.clone(), intent.step_id.clone()))
                } else {
                    None
                }
            }));
        }
        bindings.sort();
        bindings.dedup();
        anyhow::ensure!(
            bindings.len() <= 1,
            "controlled task attempt has ambiguous dispatch correlation"
        );
        Ok(bindings.pop())
    }

    /// Read-only next-action computation. Effectful actions receive an opaque
    /// state/action-bound token; wait and error observations never do.
    pub fn control_next(&self, control_id: &str) -> anyhow::Result<ControlNextV1> {
        let projected = project_control(self, control_id)?;
        anyhow::ensure!(
            projected.authority_valid,
            "CONTROL_UNAVAILABLE: control authority is absent, expired, revoked, or foreign"
        );
        next_for_projected(self, &projected)
    }

    /// Apply a local transition without exposing an effect seam. External
    /// actions are available only through [`Self::control_apply_with`].
    pub fn control_apply(&self, control_id: &str, token: &str) -> anyhow::Result<ControlApplyV1> {
        self.control_apply_with(control_id, token, |request| {
            anyhow::ensure!(
                request.intent.action_kind.is_local_s6a(),
                "CONTROL_UNAVAILABLE: external action requires its product adapter"
            );
            ControlEffectResultV1::applied(
                request.intent.action_kind,
                "local_ledger_transition_applied",
            )
        })
    }

    /// Commit intent, release the ledger lock, run exactly one product-owned
    /// adapter, then append its sealed result. Adapter errors leave the intent
    /// pending so the exact token may retry and adopt a deterministic external
    /// identity; an adapter that cannot distinguish the outcome returns a
    /// `needs_decision` result instead of guessing.
    pub fn control_apply_with<F>(
        &self,
        control_id: &str,
        raw_token: &str,
        effect: F,
    ) -> anyhow::Result<ControlApplyV1>
    where
        F: FnOnce(&ControlEffectRequestV1) -> anyhow::Result<ControlEffectResultV1>,
    {
        // Expiry is checked before cleanup or lock-file creation so an expired
        // bearer causes no product mutation of any kind.
        let token = parse_live_action_token(raw_token)?;
        crate::control_review_artifact::cleanup_recorded(self, control_id)?;
        let lock = WorkspaceLock::acquire(&self.paths)?;
        let projected = project_control(self, control_id)?;
        anyhow::ensure!(
            projected.authority_valid,
            "CONTROL_UNAVAILABLE: invalid control authority"
        );
        if token.observed_state_version != projected.status.state_version {
            return Ok(stale_apply(
                &projected,
                token.action_id,
                "stale_token_refused",
            ));
        }
        if projected.receipts.contains_key(&token.action_id) {
            return Ok(stale_apply(
                &projected,
                token.action_id,
                "replayed_token_refused",
            ));
        }
        let recovering = projected.intents.contains_key(&token.action_id);
        let authorized = authorize_and_commit_intent(self, &projected, control_id, &token)?;
        let request = ControlEffectRequestV1 {
            manifest: projected.record.manifest.clone(),
            manifest_digest: projected.record.manifest_digest.clone(),
            authority: projected.record.authority.clone(),
            intent_event_id: authorized.intent_event_id.clone(),
            intent: authorized.intent.clone(),
            recovering,
        };
        drop(lock);
        // The intent is durable before this lock is taken. Holding one
        // process-shared lock per immutable action across both the adapter and
        // receipt append prevents two presenters of the same token from
        // releasing the same external effect concurrently. A crash releases
        // the OS lock while preserving the intent for deterministic adoption.
        let effect_lock_dir = self.paths.edda_dir.join("control-local/effect-locks");
        std::fs::create_dir_all(&effect_lock_dir)?;
        let _effect_lock =
            edda_store::lock_file(&effect_lock_dir.join(format!("{}.lock", authorized.action_id)))?;
        let after_wait = project_control(self, control_id)?;
        if after_wait.receipts.contains_key(&authorized.action_id) {
            let mut adopted = stale_apply(
                &after_wait,
                authorized.action_id,
                "effect_result_already_recorded",
            );
            adopted.stale = false;
            return Ok(adopted);
        }
        anyhow::ensure!(
            after_wait
                .intents
                .get(&authorized.action_id)
                .is_some_and(|(event_id, intent)| {
                    event_id == &authorized.intent_event_id && intent == &authorized.intent
                }),
            "control intent changed before product effect execution"
        );
        let result = effect(&request)?;
        validate_effect_result(authorized.action_kind, &result)?;
        if result.durable_pending {
            return Ok(ControlApplyV1 {
                applied: false,
                stale: false,
                control_id: control_id.to_string(),
                action_id: authorized.action_id,
                intent_event_id: Some(authorized.intent_event_id),
                receipt_event_id: None,
                state_version: projected.status.state_version,
                state: projected.status.state,
                result: result.product_result,
            });
        }

        let _lock = WorkspaceLock::acquire(&self.paths)?;
        let current = project_control(self, control_id)?;
        if current.receipts.contains_key(&authorized.action_id) {
            let mut adopted = stale_apply(
                &current,
                authorized.action_id,
                "effect_result_already_recorded",
            );
            adopted.stale = false;
            return Ok(adopted);
        }
        anyhow::ensure!(
            current.status.state_version == authorized.intent.observed_state_version
                && current
                    .intents
                    .get(&authorized.action_id)
                    .is_some_and(|(event_id, intent)| {
                        event_id == &authorized.intent_event_id && intent == &authorized.intent
                    }),
            "control state changed before effect result append"
        );
        let receipt_event_id = deterministic_event_id("receipt", &authorized.action_id);
        let external_identity = crate::control_review_artifact::receipt_external_identity(&result)?;
        let cleanup_digest =
            crate::control_review_artifact::effect_cleanup_digest(authorized.action_kind, &result);
        let mut event_ids = result.event_ids;
        if !event_ids.contains(&authorized.intent_event_id) {
            event_ids.insert(0, authorized.intent_event_id.clone());
        }
        let mut receipt = ControlReceiptV1 {
            receipt_version: CONTROL_RECEIPT_VERSION,
            control_id: control_id.to_string(),
            step_id: authorized.intent.step_id.clone(),
            action_id: authorized.action_id.clone(),
            intent_event_id: authorized.intent_event_id.clone(),
            observed_state_version: current.status.state_version,
            action_token: token.commitment,
            action_kind: authorized.action_kind,
            target: authorized.target,
            product_result: result.product_result,
            dispatch_handle: result.dispatch_handle,
            event_ids,
            external_identity,
            cost_microusd: result.cost_microusd,
            elapsed_ms: result.elapsed_ms,
            next_state: result.next_state,
            next_wake: result.next_wake,
            recorded_at: now_rfc3339()?,
            seal: String::new(),
        };
        receipt.seal = seal_local(self, &receipt_without_seal(&receipt)?)?;
        let event = make_control_receipt_event(
            receipt_event_id.clone(),
            &self.head_branch()?,
            self.last_event_hash()?.as_deref(),
            serde_json::json!({"control_receipt": receipt}),
            Refs {
                events: vec![
                    authorized.intent_event_id.clone(),
                    current.record.manifest_event_id.clone(),
                ],
                ..Refs::default()
            },
        )?;
        self.append_event(&event)?;
        if let Some(digest) = cleanup_digest {
            crate::control_review_artifact::cleanup(self, &digest)?;
        }
        Ok(ControlApplyV1 {
            applied: true,
            stale: false,
            control_id: control_id.to_string(),
            action_id: authorized.action_id,
            intent_event_id: Some(authorized.intent_event_id),
            receipt_event_id: Some(receipt_event_id),
            state_version: current.status.state_version + 1,
            state: result.next_state,
            result: "effect_result_recorded".into(),
        })
    }

    pub fn authorize_control_merge(
        &self,
        control_id: &str,
        request: ControlMergeCapabilityProvision<'_>,
    ) -> anyhow::Result<VerifiedControlMergeCapabilityV1> {
        let projected = project_control(self, control_id)?;
        let policy = &projected.record.manifest.merge_policy;
        anyhow::ensure!(
            policy.required,
            "control manifest does not require a delegated merge"
        );
        let pr = policy
            .pr_number
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its PR binding"))?;
        let head = policy
            .expected_head_sha
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its head binding"))?;
        let base = policy
            .expected_base_sha
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its base binding"))?;
        provision_local_merge_capability(
            self,
            control_id,
            &projected.record.manifest_digest,
            projected.record.manifest.basis.portable_repo_id.as_deref(),
            pr,
            head,
            base,
            request,
        )
    }

    pub fn control_merge_capability(
        &self,
        control_id: &str,
    ) -> anyhow::Result<VerifiedControlMergeCapabilityV1> {
        let projected = project_control(self, control_id)?;
        let policy = &projected.record.manifest.merge_policy;
        let pr = policy
            .pr_number
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its PR binding"))?;
        let head = policy
            .expected_head_sha
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its head binding"))?;
        let base = policy
            .expected_base_sha
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delegated merge manifest omits its base binding"))?;
        verify_local_merge_capability(
            self,
            control_id,
            &projected.record.manifest_digest,
            projected.record.manifest.basis.portable_repo_id.as_deref(),
            pr,
            head,
            base,
        )
    }

    pub fn adjudicate_control(
        &self,
        control_id: &str,
        state_version: u64,
        input: ControlAdjudicationInputV1,
        caller_session: &str,
        authority_token: &str,
    ) -> anyhow::Result<ControlStatusV1> {
        validate_control_adjudication(&input)?;
        let _lock = WorkspaceLock::acquire(&self.paths)?;
        let projected = project_control(self, control_id)?;
        anyhow::ensure!(
            projected.status.state == ControlStateV1::NeedsDecision
                || has_expired_pending_intent(&projected)?,
            "control adjudication requires needs_decision state or an expired pending intent"
        );
        anyhow::ensure!(
            projected.status.state_version == state_version,
            "stale control adjudication state version"
        );
        anyhow::ensure!(
            projected.record.manifest_digest == input.expected_manifest_digest,
            "stale control adjudication manifest digest"
        );
        let mut proof = authorize_command(
            self,
            caller_session,
            ACTION_ADJUDICATE,
            projected.record.manifest.basis.portable_repo_id.as_deref(),
            projected.record.manifest.basis.github_repository.as_deref(),
            authority_token,
        )?;
        let mut manifest = projected.record.manifest.clone();
        manifest.manifest_version = manifest
            .manifest_version
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("control manifest version exhausted"))?;
        if input.clear_return_for_decision {
            manifest.return_for_decision.clear();
        }
        validate_control_manifest(&manifest)?;
        let canonical = canonical_manifest_bytes(&manifest)?;
        let digest = edda_core::hash::sha256_hex(&canonical);
        let event_id = new_event_id();
        let adjudication = ControlAdjudicationRecordV1 {
            adjudication_version: input.adjudication_version,
            reason_code: input.reason_code,
            evidence: input.evidence,
            prior_state_version: state_version,
            prior_manifest_digest: input.expected_manifest_digest,
        };
        proof = seal_manifest_proof(
            self,
            proof,
            &event_id,
            &digest,
            manifest.manifest_version,
            Some(&adjudication),
        )?;
        let record = ControlManifestRecordV1 {
            record_version: CONTROL_MANIFEST_RECORD_VERSION,
            manifest_event_id: event_id.clone(),
            manifest_digest: digest,
            canonical_bytes_hex: hex::encode(canonical),
            manifest,
            authority: proof,
            adjudication: Some(adjudication),
        };
        let event = make_control_manifest_event(
            event_id,
            &self.head_branch()?,
            self.last_event_hash()?.as_deref(),
            serde_json::json!({"control_manifest": record}),
            Refs {
                events: vec![projected.record.manifest_event_id],
                ..Refs::default()
            },
        )?;
        self.append_event(&event)?;
        self.control_status(control_id)
    }

    /// Accept one S5 brief through the same sealed compiler path. It creates a
    /// local control record but never starts/completes a Task Rail task.
    pub fn prepare_execution_brief(
        &self,
        input: ExecutionBriefInputV1,
        caller_session: &str,
        authority_token: &str,
    ) -> anyhow::Result<AcceptedExecutionBriefV1> {
        let control_id = format!("control_{}", ulid::Ulid::new().to_string().to_lowercase());
        let task = ControlTaskInputV1 {
            task_key: "step_prepare".into(),
            task_id: input.task_ref,
            depends_on: vec![],
            exact_input_sha: input.basis.base_full_sha.clone(),
            brief_id: input.brief_id.clone(),
            runtime_profile: input.runtime_profile,
            model_target: "selected_by_controller".into(),
            owned_paths: input.scope.allowed_paths.clone(),
            build_lane: None,
            issue_binding: None,
            local_only: true,
            execution_host_affinity: "local".into(),
            required_delivery: DeliveryRequirementV1::LocalOnly,
        };
        let compile = ControlCompileInputV1 {
            compile_version: CONTROL_COMPILE_INPUT_VERSION,
            manifest: ControlManifestInputV1 {
                control_version: CONTROL_MANIFEST_VERSION,
                control_id,
                program_id: "program_taskprepare".into(),
                command_profile: ControlCommandProfileV1::Strong,
                goal: "accept one immutable execution brief without starting work".into(),
                exclusions: vec![
                    "Task Rail state transitions".into(),
                    "external effects".into(),
                ],
                basis: ControlBasisV1 {
                    portable_repo_id: input.basis.portable_repo_id.clone(),
                    github_repository: None,
                    base_full_sha: input.basis.base_full_sha.clone(),
                    references: input.basis.issue_spec_refs.clone(),
                },
                tasks: vec![task],
                capacity: ControlCapacityV1 {
                    max_workers: 1,
                    verifier_capacity: 1,
                },
                admission_policy: ControlAdmissionPolicyV1 {
                    allowed_issue_stage_labels: vec![],
                    forbidden_hold_labels: vec![],
                    claim_identity: "local/task-prepare".into(),
                    winner_readback_required: true,
                    existing_claim_check_required: false,
                    delivery_pr_check_required: false,
                },
                routes: vec![],
                retry_cost_policy: ControlRetryCostPolicyV1 {
                    per_action_preflight_cost_microusd: 0,
                    per_action_incremental_cap_microusd: 0,
                    aggregate_stop_microusd: 0,
                    missing_cost_needs_decision: true,
                    retry_cap: 0,
                },
                review_policy: ControlReviewPolicyV1 {
                    verifier_identity: "not_dispatched".into(),
                    verifier_profile: "not_dispatched".into(),
                    frozen_surface_source: "accepted_brief_scope".into(),
                    one_live_claim_per_pr_head: true,
                },
                merge_policy: ControlMergePolicyV1 {
                    required: false,
                    pr_number: None,
                    expected_head_sha: None,
                    expected_base_sha: None,
                    authority_capability_ref: None,
                    eligibility_product_verb: "edda review merge".into(),
                },
                return_for_decision: vec![],
                completion_condition: ControlCompletionConditionV1::LocalPreparationOnly,
            },
            brief_inputs: vec![input],
        };
        let mut result = self.compile_control(compile, caller_session, authority_token)?;
        result
            .briefs
            .pop()
            .ok_or_else(|| anyhow::anyhow!("control compile omitted its accepted brief"))
    }

    pub(crate) fn load_sealed_execution_brief(
        &self,
        event: &Event,
        expected_digest: &str,
    ) -> anyhow::Result<AcceptedExecutionBriefV1> {
        let accepted = crate::guided_execution::execution_brief_data(event, expected_digest)?;
        let manifest_event = self
            .get_event(&accepted.authority.authority_event_id)?
            .ok_or_else(|| anyhow::anyhow!("execution brief authority manifest is missing"))?;
        let record = parse_manifest_event(&manifest_event)?;
        verify_manifest_record(self, &record)?;
        anyhow::ensure!(
            accepted.authority.principal_id == record.authority.principal_id
                && accepted.authority.session_id == record.authority.session_id,
            "execution brief principal/session authority does not match its sealed control manifest"
        );
        anyhow::ensure!(
            record.manifest.tasks.iter().any(|task| {
                task.brief.brief_event_id == accepted.event_id
                    && task.brief.content_digest == accepted.content_digest
                    && task.brief.brief_id == accepted.brief.brief_id
            }),
            "execution brief is not referenced by its sealed control manifest"
        );
        Ok(accepted)
    }
}
