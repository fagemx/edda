use super::control_review_bundle::ReviewBundle;
use super::control_verification::{
    review_correlation_marker, review_dispatch_completion, review_execution_brief,
    visible_review_union_from_comments, CompletedReviewReceipt, ControlledReviewComment,
    ReviewDispatchCompletion,
};
use super::*;
use crate::cmd_review::claim::StructuredReviewRequest;

#[allow(clippy::too_many_arguments)]
fn completed_review_receipt(
    output: &detached_dispatch::DetachedOutput,
    brief: &edda_core::guided_execution::ExecutionBriefV1,
    review: &StructuredReviewRequest,
    session_id: &str,
    agent: AgentKind,
    requested_model: &str,
    step_id: &str,
    bundle: &ReviewBundle,
) -> anyhow::Result<Option<CompletedReviewReceipt>> {
    match review_dispatch_completion(
        output,
        brief,
        review,
        session_id,
        agent,
        requested_model,
        step_id,
        bundle,
    )? {
        ReviewDispatchCompletion::Pending => Ok(None),
        ReviewDispatchCompletion::Receipt(completed) => Ok(Some(*completed)),
        ReviewDispatchCompletion::Refused { reason_code, .. } => {
            anyhow::bail!("verification dispatch refused: {reason_code}")
        }
    }
}

#[test]
fn issue_binding_is_closed_and_agent_kind_is_not_inferred() {
    assert_eq!(issue_number("issue:#1141").unwrap(), 1141);
    assert!(issue_number("GH-1141").is_err());
    assert!(parse_agent("flash").is_err());
}

#[test]
fn delivery_binding_refuses_wrong_repository_input_and_branch() {
    let repo = format!("repo_{}", "a".repeat(64));
    let input = "b".repeat(40);
    let branch = "edda/task-7/attempt-2";
    let delivery = DeliveryV1 {
        portable_repo_id: Some(repo.clone()),
        input_sha: input.clone(),
        branch: branch.into(),
        result_head_sha: "c".repeat(40),
        pr_number: None,
        pr_base_ref: None,
        observed_base_tip_sha: None,
    };
    assert!(validate_declared_delivery_binding(Some(&repo), &input, branch, &delivery).is_ok());
    assert!(validate_declared_delivery_binding(
        Some(&format!("repo_{}", "d".repeat(64))),
        &input,
        branch,
        &delivery
    )
    .is_err());
    assert!(
        validate_declared_delivery_binding(Some(&repo), &"e".repeat(40), branch, &delivery)
            .is_err()
    );
    assert!(
        validate_declared_delivery_binding(Some(&repo), &input, "other-branch", &delivery).is_err()
    );
}

fn review_request_fixture() -> (StructuredReviewRequest, ControlEffectRequestV1) {
    use edda_core::guided_execution::*;
    let review = StructuredReviewRequest {
        claim_version: CONTROL_REVIEW_CLAIM_VERSION,
        repository: "owner/repo".into(),
        control_id: "control_reviewtest".into(),
        action_id: format!("action_{}", "a".repeat(64)),
        manifest_digest: "d".repeat(64),
        charter_digest: "c".repeat(64),
        review_bundle_ref: format!("blob:sha256:{}", "4".repeat(64)),
        generation: 1,
        state_version: 1,
        pr_number: 1141,
        base_sha: "e".repeat(40),
        head_sha: "b".repeat(40),
        claimant: "machine/controller".into(),
        claimant_session: "session-controller".into(),
        controller_identity: "controller".into(),
        verifier_identity: "verifier-one".into(),
        verifier_profile: "pi:provider/model".into(),
        frozen_surface_source: "manifest".into(),
        seal: "9".repeat(64),
    };
    let manifest = ControlManifestV1 {
        control_version: 1,
        manifest_version: 1,
        control_id: review.control_id.clone(),
        program_id: "program_reviewtest".into(),
        command_profile: ControlCommandProfileV1::Strong,
        goal: "review exact delivery".into(),
        exclusions: vec![],
        basis: ControlBasisV1 {
            portable_repo_id: Some(format!("repo_{}", "c".repeat(64))),
            github_repository: Some("owner/repo".into()),
            base_full_sha: "a".repeat(40),
            references: vec!["issue:#1141".into()],
        },
        tasks: vec![],
        capacity: ControlCapacityV1 {
            max_workers: 1,
            verifier_capacity: 1,
        },
        admission_policy: ControlAdmissionPolicyV1 {
            allowed_issue_stage_labels: vec![],
            forbidden_hold_labels: vec!["hold".into()],
            claim_identity: review.claimant.clone(),
            winner_readback_required: true,
            existing_claim_check_required: true,
            delivery_pr_check_required: true,
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
            verifier_identity: review.verifier_identity.clone(),
            verifier_profile: review.verifier_profile.clone(),
            frozen_surface_source: review.frozen_surface_source.clone(),
            one_live_claim_per_pr_head: true,
        },
        merge_policy: ControlMergePolicyV1 {
            required: false,
            pr_number: Some(review.pr_number),
            expected_head_sha: Some(review.head_sha.clone()),
            expected_base_sha: Some("e".repeat(40)),
            authority_capability_ref: None,
            eligibility_product_verb: "edda review merge".into(),
        },
        return_for_decision: vec![],
        completion_condition: ControlCompletionConditionV1::VerificationSucceeded,
    };
    let intent = ControlIntentV1 {
        intent_version: 1,
        control_id: review.control_id.clone(),
        step_id: "step_2".into(),
        action_id: format!("action_{}", "f".repeat(64)),
        observed_state_version: 2,
        action_token: format!("sha256:{}", "1".repeat(64)),
        action_kind: ControlActionKindV1::RequestVerification,
        target: ControlTargetV1::PullRequest {
            number: review.pr_number,
            head_sha: review.head_sha.clone(),
        },
        created_at: "2026-09-12T00:00:00Z".into(),
        seal: "2".repeat(64),
    };
    (
        review,
        ControlEffectRequestV1 {
            manifest,
            manifest_digest: "d".repeat(64),
            authority: ControlAuthorityProofV1 {
                capability_id: format!("cap_{}", "1".repeat(64)),
                principal_id: "controller".into(),
                session_id: "session-controller".into(),
                command_profile: ControlCommandProfileV1::Strong,
                local_project_id: "2".repeat(32),
                portable_repo_id: Some(format!("repo_{}", "c".repeat(64))),
                github_repository: Some("owner/repo".into()),
                permitted_action: "control_compile".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                seal: "3".repeat(64),
            },
            intent_event_id: format!("evt_{}", "a".repeat(26)),
            intent,
            recovering: false,
        },
    )
}

fn bundle() -> ReviewBundle {
    ReviewBundle {
        blob_ref: format!("blob:sha256:{}", "4".repeat(64)),
        path: std::path::PathBuf::from("review-bundle.json"),
        digest: "4".repeat(64),
        changed_paths: vec!["src/lib.rs".into()],
        audit_path: std::path::PathBuf::from("review.diff"),
        audit_digest: "5".repeat(64),
    }
}

#[test]
fn review_charter_identity_changes_with_pr_head_or_manifest() {
    let (review, request) = review_request_fixture();
    let original = review_execution_brief(&review, &request, &bundle()).unwrap();
    let mut moved = review.clone();
    moved.head_sha = "9".repeat(40);
    assert_ne!(
        original.content_digest,
        review_execution_brief(&moved, &request, &bundle())
            .unwrap()
            .content_digest
    );
    let mut other_manifest = review;
    other_manifest.manifest_digest = "8".repeat(64);
    assert_ne!(
        original.content_digest,
        review_execution_brief(&other_manifest, &request, &bundle())
            .unwrap()
            .content_digest
    );
}

#[test]
fn pr_visible_review_must_carry_the_exact_dispatch_correlation_marker() {
    use crate::cmd_review::{ReviewComment as Comment, ReviewUnion as Union};
    let (_, request) = review_request_fixture();
    let marker = review_correlation_marker(
        &request.manifest.control_id,
        &request.intent.step_id,
        "control-review-action",
        "dispatch-action",
    );
    let verdict = format!(
        "## Code Review: Round 1 — PR #1141 @ {}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n",
        request
            .manifest
            .merge_policy
            .expected_head_sha
            .as_deref()
            .unwrap()
    );
    let old = ControlledReviewComment {
        comment: Comment {
            id: "1".into(),
            body: verdict.clone(),
            author_association: Some("MEMBER".into()),
        },
        author_login: Some("verifier-one".into()),
    };
    assert_eq!(
        visible_review_union_from_comments(
            std::slice::from_ref(&old),
            request
                .manifest
                .merge_policy
                .expected_head_sha
                .as_deref()
                .unwrap(),
            &marker,
            "verifier-one",
        ),
        Union::None
    );
    let correlated = ControlledReviewComment {
        comment: Comment {
            id: "2".into(),
            body: format!("{verdict}\n{marker}\n"),
            author_association: Some("MEMBER".into()),
        },
        author_login: Some("verifier-one".into()),
    };
    assert_eq!(
        visible_review_union_from_comments(
            &[old, correlated.clone()],
            request
                .manifest
                .merge_policy
                .expected_head_sha
                .as_deref()
                .unwrap(),
            &marker,
            "verifier-one",
        ),
        Union::Pass
    );
    let wrong_collaborator = ControlledReviewComment {
        author_login: Some("another-collaborator".into()),
        ..correlated
    };
    assert_eq!(
        visible_review_union_from_comments(
            &[wrong_collaborator],
            request
                .manifest
                .merge_policy
                .expected_head_sha
                .as_deref()
                .unwrap(),
            &marker,
            "verifier-one",
        ),
        Union::Fail,
        "trusted association alone cannot authenticate the compiled verifier"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One fixture exercises identity, stderr, and accounting attacks.
fn verifier_ingestion_accepts_only_one_correlated_work_receipt_value() {
    use edda_core::guided_execution::*;
    let (review, request) = review_request_fixture();
    let review_bundle = bundle();
    let brief = review_execution_brief(&review, &request, &review_bundle).unwrap();
    let session = edda_conductor::agent::launcher::phase_session_id(
        "control-review",
        &request.intent.action_id,
    )
    .to_string();
    let dir = tempfile::tempdir().unwrap();
    let output = detached_dispatch::DetachedOutput {
        handle: format!("dispatch-{}", &request.intent.action_id[7..33]),
        log: dir.path().join("review.log"),
        error_log: dir.path().join("review.log.err"),
        manifest: dir.path().join("review.json"),
        task: None,
    };
    std::fs::write(
        &output.manifest,
        serde_json::to_vec(&serde_json::json!({"state":"completed","exit_code":0})).unwrap(),
    )
    .unwrap();
    let receipt = WorkReceiptV1 {
        receipt_version: 1,
        receipt_id: "receipt_reviewone".into(),
        brief: BriefIdentityV1 {
            brief_id: brief.brief_id.clone(),
            brief_event_id: brief.brief_event_id.clone(),
            content_digest: brief.content_digest.clone(),
        },
        control_ref: Some(ControlReceiptRefV1 {
            control_id: review.control_id.clone(),
            step_id: request.intent.step_id.clone(),
        }),
        task_ref: None,
        dispatch_handle: Some(output.handle.clone()),
        agent: AgentRuntimeIdentityV1 {
            agent_kind: "pi".into(),
            runtime_profile: RuntimeProfileV1::Strong,
            session_id: Some(session.clone()),
        },
        basis_full_sha: review.base_sha.clone(),
        started_at: "2026-09-12T00:00:00Z".into(),
        ended_at: "2026-09-12T00:01:00Z".into(),
        outcome_code: "REVIEW_LGTM".into(),
        observations: vec!["complete frozen-surface audit".into()],
        commands_run: vec![],
        hypotheses_supported: vec![],
        hypotheses_rejected: vec![],
        changed_paths: vec![],
        delivery: None,
        validation_ran: vec![],
        validation_read: vec![
            ValidationReceiptV1 {
                check_id: "check_reviewbundle".into(),
                result: "read_complete".into(),
                evidence_handle: review_bundle.blob_ref.clone(),
            },
            ValidationReceiptV1 {
                check_id: "check_reviewinput".into(),
                result: "read_complete".into(),
                evidence_handle: format!("sha256:{}", review_bundle.audit_digest),
            },
        ],
        unknowns: vec![],
        recommended_next_action: "publish the real review through the review loop".into(),
        result_class: ResultClassV1::Success,
    };
    let envelope = serde_json::json!({
        "outcome":"done",
        "session_id":session,
        "session_observed":session,
        "model_requested":"provider/model",
        "model_observed":"provider/model",
        "cost_usd":0.25,
        "elapsed_ms":1234,
        "result_text":serde_json::to_string(&receipt).unwrap()
    });
    std::fs::write(&output.log, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let completed = completed_review_receipt(
        &output,
        &brief,
        &review,
        envelope["session_id"].as_str().unwrap(),
        AgentKind::Pi,
        "provider/model",
        &request.intent.step_id,
        &review_bundle,
    )
    .unwrap()
    .expect("matching observed dispatch identity");
    assert_eq!(completed.cost_microusd, Some(250_000));
    assert_eq!(completed.elapsed_ms, Some(1234));

    std::fs::write(&output.error_log, "untrusted stderr\n").unwrap();
    assert!(completed_review_receipt(
        &output,
        &brief,
        &review,
        envelope["session_id"].as_str().unwrap(),
        AgentKind::Pi,
        "provider/model",
        &request.intent.step_id,
        &review_bundle,
    )
    .unwrap()
    .is_some());

    for (field, wrong) in [
        ("model_observed", "fallback/model"),
        ("session_observed", "resumed-copy"),
    ] {
        let mut hostile = envelope.clone();
        hostile[field] = wrong.into();
        std::fs::write(&output.log, serde_json::to_vec(&hostile).unwrap()).unwrap();
        match review_dispatch_completion(
            &output,
            &brief,
            &review,
            envelope["session_id"].as_str().unwrap(),
            AgentKind::Pi,
            "provider/model",
            &request.intent.step_id,
            &review_bundle,
        )
        .unwrap()
        {
            ReviewDispatchCompletion::Refused {
                reason_code,
                cost_microusd,
                elapsed_ms,
            } => {
                assert_eq!(reason_code, "VERIFICATION_DISPATCH_IDENTITY_MISMATCH");
                assert_eq!(cost_microusd, Some(250_000));
                assert_eq!(elapsed_ms, Some(1234));
            }
            _ => panic!("identity mismatch must preserve measured accounting"),
        }
    }

    std::fs::write(
        &output.manifest,
        serde_json::to_vec(&serde_json::json!({"state":"completed","exit_code":3})).unwrap(),
    )
    .unwrap();
    let mut budget_exceeded = envelope.clone();
    budget_exceeded["outcome"] = "budget_exceeded".into();
    budget_exceeded["result_text"] = serde_json::Value::Null;
    std::fs::write(&output.log, serde_json::to_vec(&budget_exceeded).unwrap()).unwrap();
    match review_dispatch_completion(
        &output,
        &brief,
        &review,
        envelope["session_id"].as_str().unwrap(),
        AgentKind::Pi,
        "provider/model",
        &request.intent.step_id,
        &review_bundle,
    )
    .unwrap()
    {
        ReviewDispatchCompletion::Refused {
            reason_code,
            cost_microusd,
            elapsed_ms,
        } => {
            assert_eq!(reason_code, "REVIEW_INCREMENTAL_COST_CAP_EXCEEDED");
            assert_eq!(cost_microusd, Some(250_000));
            assert_eq!(elapsed_ms, Some(1234));
        }
        _ => panic!("budget terminal must preserve measured accounting"),
    }
}
