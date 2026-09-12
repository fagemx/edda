use super::*;
use crate::ControlReviewClaimOutcomeV1;
use crate::{ControlEffectRequestV1, ControlEffectResultV1};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

#[test]
fn product_effect_is_intent_first_and_recovery_adopts_the_same_action() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_effectorder");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let claim = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(claim.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with("control_effectorder", &claim.token.unwrap(), |request| {
            crate::ControlEffectResultV1::applied(
                request.intent.action_kind,
                "review_claim_recorded",
            )
        })
        .unwrap();
    let next = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(next.action_kind, ControlActionKindV1::RequestVerification);
    let token = next.token.unwrap();

    let error = ledger
        .control_apply_with("control_effectorder", &token, |request| {
            assert!(!request.recovering);
            assert_eq!(
                ledger
                    .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                ledger
                    .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
                    .unwrap()
                    .len(),
                1
            );
            anyhow::bail!("simulated crash boundary")
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("simulated crash boundary"), "{error}");
    assert!(ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .iter()
        .all(|event| !serde_json::to_string(event).unwrap().contains(&token)));
    assert_eq!(
        ledger
            .control_status("control_effectorder")
            .unwrap()
            .state_version,
        1
    );
    let before_recovery_next = ledger.count_events().unwrap();
    let recovered = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(ledger.count_events().unwrap(), before_recovery_next);
    assert_eq!(recovered.token.as_deref(), Some(token.as_str()));
    assert_eq!(recovered.reason_code, "PENDING_INTENT_RECOVERY_READY");

    let pending = ledger
        .control_apply_with(
            "control_effectorder",
            recovered.token.as_deref().unwrap(),
            |request| {
                assert!(request.recovering);
                Ok(crate::ControlEffectResultV1::durable_pending(
                    "verifier_running",
                    "dispatch-review-action".into(),
                    "dispatch_manifest:dispatch-review-action",
                ))
            },
        )
        .unwrap();
    assert!(!pending.applied);
    assert_eq!(pending.receipt_event_id, None);
    let recovered_after_pending = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(
        recovered_after_pending.token.as_deref(),
        Some(token.as_str())
    );

    let applied = ledger
        .control_apply_with(
            "control_effectorder",
            recovered_after_pending.token.as_deref().unwrap(),
            |request| {
                assert!(request.recovering);
                let mut result = crate::ControlEffectResultV1::applied(
                    request.intent.action_kind,
                    "structured_request_adopted",
                )?;
                result.dispatch_handle = Some("dispatch-review-action".into());
                Ok(result)
            },
        )
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.state, ControlStateV1::Verifying);
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ledger
            .control_receipts("control_effectorder")
            .unwrap()
            .last()
            .unwrap()
            .dispatch_handle
            .as_deref(),
        Some("dispatch-review-action")
    );
    let waiting = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(waiting.action_kind, ControlActionKindV1::WaitForWorkers);
    assert_eq!(
        waiting.availability,
        ControlAvailabilityV1::ControlUnavailable
    );
    assert!(waiting.token.is_none());
}

#[test]
fn concurrent_presenters_execute_one_product_effect_for_the_durable_intent() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_oneeffect");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let token = ledger
        .control_next("control_oneeffect")
        .unwrap()
        .token
        .unwrap();
    let root = fixture.root.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(2));
    let effects = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let root = root.clone();
        let token = token.clone();
        let barrier = Arc::clone(&barrier);
        let effects = Arc::clone(&effects);
        threads.push(std::thread::spawn(move || {
            let ledger = Ledger::open(root).unwrap();
            barrier.wait();
            ledger
                .control_apply_with("control_oneeffect", &token, |request| {
                    effects.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(100));
                    ControlEffectResultV1::applied(
                        request.intent.action_kind,
                        "single_external_effect",
                    )
                })
                .unwrap()
        }));
    }
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(results.iter().filter(|result| result.applied).count(), 1);
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn delegated_merge_is_unavailable_before_an_effect_intent_without_provider_base_cas() {
    let fixture = Fixture::new();
    let portable_repo_id = format!("repo_{}", "d".repeat(64));
    let authority_token = fixture.provision_for(Some(&portable_repo_id));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_nobasecas");
    input.manifest.completion_condition = ControlCompletionConditionV1::DelegatedMergeSucceeded;
    input.manifest.merge_policy.required = true;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    input.manifest.basis.portable_repo_id = Some(portable_repo_id);
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    for expected in [
        ControlActionKindV1::ClaimVerification,
        ControlActionKindV1::RequestVerification,
    ] {
        let next = ledger.control_next("control_nobasecas").unwrap();
        assert_eq!(next.action_kind, expected);
        ledger
            .control_apply_with(
                "control_nobasecas",
                next.token.as_deref().unwrap(),
                |request| ControlEffectResultV1::applied(request.intent.action_kind, "fixture"),
            )
            .unwrap();
    }
    let before = ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .len();
    let merge = ledger.control_next("control_nobasecas").unwrap();
    assert_eq!(merge.action_kind, ControlActionKindV1::MergeDelegated);
    assert_eq!(
        merge.availability,
        ControlAvailabilityV1::ControlUnavailable
    );
    assert_eq!(
        merge.reason_code,
        "DELEGATED_MERGE_ATOMIC_BASE_PRECONDITION_UNAVAILABLE"
    );
    assert!(merge.token.is_none());
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        before
    );
}

fn review_claim_for(request: &ControlEffectRequestV1) -> ControlReviewClaimV1 {
    let ControlTargetV1::PullRequest { number, head_sha } = &request.intent.target else {
        panic!("review claim fixture target")
    };
    ControlReviewClaimV1 {
        claim_version: CONTROL_REVIEW_CLAIM_VERSION,
        repository: "owner/repo".into(),
        control_id: request.manifest.control_id.clone(),
        action_id: request.intent.action_id.clone(),
        manifest_digest: request.manifest_digest.clone(),
        charter_digest: "d".repeat(64),
        review_bundle_ref: format!("blob:sha256:{}", "e".repeat(64)),
        generation: request.manifest.manifest_version,
        state_version: request.intent.observed_state_version,
        pr_number: *number,
        base_sha: request
            .manifest
            .merge_policy
            .expected_base_sha
            .clone()
            .expect("base"),
        head_sha: head_sha.clone(),
        claimant: request.manifest.admission_policy.claim_identity.clone(),
        claimant_session: request.authority.session_id.clone(),
        controller_identity: "controller".into(),
        verifier_identity: request.manifest.review_policy.verifier_identity.clone(),
        verifier_profile: request.manifest.review_policy.verifier_profile.clone(),
        frozen_surface_source: request.manifest.review_policy.frozen_surface_source.clone(),
        seal: String::new(),
    }
}

#[test]
fn production_workspace_lock_selects_one_live_signed_claim() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewclaimrace");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let first = ledger.control_next("control_reviewclaimrace").unwrap();
    let root = fixture.root.path().to_path_buf();
    ledger
        .control_apply_with(
            "control_reviewclaimrace",
            first.token.as_deref().unwrap(),
            |request| {
                let one = ledger.seal_control_review_claim(review_claim_for(request))?;
                let mut other = review_claim_for(request);
                other.charter_digest = "f".repeat(64);
                other.review_bundle_ref = format!("blob:sha256:{}", "f".repeat(64));
                let two = ledger.seal_control_review_claim(other)?;
                let barrier = Arc::new(Barrier::new(2));
                let threads = [one, two].map(|claim| {
                    let barrier = Arc::clone(&barrier);
                    let root = root.clone();
                    std::thread::spawn(move || {
                        let contender = Ledger::open(&root).expect("contender ledger");
                        barrier.wait();
                        contender
                            .claim_control_review(&claim)
                            .expect("claim result")
                    })
                });
                let results = threads
                    .into_iter()
                    .map(|thread| thread.join().expect("contender"))
                    .collect::<Vec<_>>();
                assert_eq!(
                    results
                        .iter()
                        .filter(|result| matches!(result, ControlReviewClaimOutcomeV1::Won { .. }))
                        .count(),
                    1
                );
                assert_eq!(
                    results
                        .iter()
                        .filter(|result| matches!(result, ControlReviewClaimOutcomeV1::RefusedLive))
                        .count(),
                    1
                );
                ControlEffectResultV1::applied(request.intent.action_kind, "claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn signed_claim_payload_and_same_head_retry_require_inconclusive_adjudication() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewgeneration");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();

    let first = ledger.control_next("control_reviewgeneration").unwrap();
    let mut prior = None;
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            first.token.as_deref().unwrap(),
            |request| {
                let signed = ledger.seal_control_review_claim(review_claim_for(request))?;
                ledger.verify_control_review_claim(&signed)?;
                let mut altered = signed.clone();
                altered.charter_digest = "f".repeat(64);
                assert!(ledger.verify_control_review_claim(&altered).is_err());
                prior = Some(signed);
                ControlEffectResultV1::applied(request.intent.action_kind, "claim_recorded")
            },
        )
        .unwrap();
    let prior = prior.unwrap();
    let review = ledger.control_next("control_reviewgeneration").unwrap();
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            review.token.as_deref().unwrap(),
            |_| Ok(ControlEffectResultV1::needs_decision("REVIEW_INCONCLUSIVE")),
        )
        .unwrap();
    let status = ledger.control_status("control_reviewgeneration").unwrap();
    ledger
        .adjudicate_control(
            "control_reviewgeneration",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_REVIEW".into(),
                evidence: vec!["structured review was inconclusive".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_reviewgeneration").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            retry.token.as_deref().unwrap(),
            |request| {
                let next = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert!(ledger.control_review_claim_is_supersedable(&prior, &next)?);
                ControlEffectResultV1::applied(request.intent.action_kind, "retry_claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn adjudication_can_supersede_a_signed_claim_after_claim_evidence_failure() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewclaimretry");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();

    let first = ledger.control_next("control_reviewclaimretry").unwrap();
    let artifact_digest = "d".repeat(64);
    let artifact = fixture
        .root
        .path()
        .join(".edda/control-local/review-bundles")
        .join(format!("{artifact_digest}.json"));
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let mut prior = None;
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            first.token.as_deref().unwrap(),
            |request| {
                let claim = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert_eq!(
                    ledger.claim_control_review(&claim)?,
                    ControlReviewClaimOutcomeV1::Won { adopted: false }
                );
                prior = Some(claim);
                std::fs::write(&artifact, "ephemeral audit").unwrap();
                let mut result =
                    ControlEffectResultV1::needs_decision("REVIEW_CLAIM_EVIDENCE_INVALID");
                result.ephemeral_artifact_digest = Some(artifact_digest.clone());
                Ok(result)
            },
        )
        .unwrap();
    assert!(!artifact.exists(), "durable receipt triggers cleanup");
    std::fs::write(&artifact, "crash residue").unwrap();
    let status = ledger.control_status("control_reviewclaimretry").unwrap();
    assert!(artifact.exists(), "read-only status never performs cleanup");
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            first.token.as_deref().unwrap(),
            |_| panic!("stale token cannot execute its adapter"),
        )
        .unwrap();
    assert!(!artifact.exists(), "mutating recovery retries cleanup");
    ledger
        .adjudicate_control(
            "control_reviewclaimretry",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_CLAIM_EVIDENCE".into(),
                evidence: vec!["immutable remote evidence was unavailable".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_reviewclaimretry").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            retry.token.as_deref().unwrap(),
            |request| {
                let next = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert!(
                    ledger.control_review_claim_is_supersedable(prior.as_ref().unwrap(), &next,)?
                );
                assert_eq!(
                    ledger.claim_control_review(&next)?,
                    ControlReviewClaimOutcomeV1::Won { adopted: false }
                );
                ControlEffectResultV1::applied(request.intent.action_kind, "retry_claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn needs_decision_receipt_never_satisfies_a_product_prerequisite_after_adjudication() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_failedclaim");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let claim = ledger.control_next("control_failedclaim").unwrap();
    assert_eq!(claim.action_kind, ControlActionKindV1::ClaimVerification);
    let applied = ledger
        .control_apply_with("control_failedclaim", &claim.token.unwrap(), |_| {
            Ok(crate::ControlEffectResultV1::needs_decision(
                "CLAIM_REFUSED",
            ))
        })
        .unwrap();
    assert_eq!(applied.state, ControlStateV1::NeedsDecision);
    let status = ledger.control_status("control_failedclaim").unwrap();
    ledger
        .adjudicate_control(
            "control_failedclaim",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_CLAIM".into(),
                evidence: vec!["operator rechecked subject".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_failedclaim").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    assert_ne!(retry.action_id, claim.action_id);
    assert!(retry.token.is_some());
}
