use crate::{ControlAuthorityProvision, Ledger};
use edda_core::guided_execution::*;
use edda_store::SessionHeartbeat;
#[cfg(unix)]
use std::sync::{Arc, Barrier};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[cfg(unix)]
fn write_test_secret(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[cfg(windows)]
fn write_test_secret(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

struct Fixture {
    _store: edda_store::test_support::IsolatedStoreRoot,
    root: tempfile::TempDir,
    session: String,
}

impl Fixture {
    fn new() -> Self {
        let store = edda_store::test_support::isolated_store_root().unwrap();
        let root = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        std::fs::write(
            root.path().join(".edda/actors.yaml"),
            "version: 2\nactors:\n  planner:\n    roles: [controller]\n    kind: agent\n    runtime: strong\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join(".edda/control-local")).unwrap();
        write_test_secret(
            &root.path().join(".edda/control-local/root.key"),
            "11".repeat(32).as_bytes(),
        );
        std::fs::write(
            root.path().join(".edda/policy.yaml"),
            "version: 2\nroles: [controller]\nrules: []\npermissions:\n  default: deny\n  grants:\n    - actions: [control_compile, control_adjudicate]\n      roles: [controller]\n",
        )
        .unwrap();
        let session = "session-control-one".to_string();
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let project_id = edda_store::project_id(root.path());
        edda_store::ensure_dirs(&project_id).unwrap();
        edda_store::write_heartbeat(
            &project_id,
            &SessionHeartbeat {
                session_id: session.clone(),
                started_at: now.clone(),
                last_heartbeat: now,
                label: "planner".into(),
                focus_files: vec![],
                active_tasks: vec![],
                files_modified_count: 0,
                total_edits: 0,
                recent_commits: vec![],
                branch: Some("main".into()),
                current_phase: Some("planning".into()),
                parent_session_id: None,
                plan: Some("control-test".into()),
                phase: Some("compile".into()),
                attempt: Some(1),
                stage: Some("running".into()),
                pid: Some(std::process::id()),
            },
        )
        .unwrap();
        Self {
            _store: store,
            root,
            session,
        }
    }

    #[cfg(unix)]
    fn provision(&self) -> String {
        self.provision_for(None)
    }

    #[cfg(unix)]
    fn provision_for(&self, portable_repo_id: Option<&str>) -> String {
        self.try_provision_for(portable_repo_id).unwrap();
        std::fs::read_to_string(self.root.path().join("authority-token")).unwrap()
    }

    #[cfg(unix)]
    fn try_provision(&self) -> anyhow::Result<()> {
        self.try_provision_for(None)
    }

    #[cfg(unix)]
    fn try_provision_for(&self, portable_repo_id: Option<&str>) -> anyhow::Result<()> {
        let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
            .format(&Rfc3339)
            .unwrap();
        Ledger::open(self.root.path())?.provision_control_authority(ControlAuthorityProvision {
            principal_id: "planner",
            session_id: &self.session,
            command_profile: ControlCommandProfileV1::Strong,
            portable_repo_id,
            expires_at: &expiry,
            issuer_token: &"11".repeat(32),
            authority_token_out: &self.root.path().join("authority-token"),
        })?;
        Ok(())
    }
}

fn manifest(control_id: &str) -> ControlManifestInputV1 {
    ControlManifestInputV1 {
        control_version: 1,
        control_id: control_id.into(),
        program_id: "program_testone".into(),
        command_profile: ControlCommandProfileV1::Strong,
        goal: "prove one local transition".into(),
        exclusions: vec!["all external effects".into()],
        basis: ControlBasisV1 {
            portable_repo_id: None,
            github_repository: None,
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
            claim_identity: "machine/controller".into(),
            winner_readback_required: true,
            existing_claim_check_required: true,
            delivery_pr_check_required: true,
        },
        routes: vec![],
        retry_cost_policy: ControlRetryCostPolicyV1 {
            per_action_preflight_cost_microusd: 1,
            per_action_incremental_cap_microusd: 10,
            aggregate_stop_microusd: 100,
            missing_cost_needs_decision: true,
            retry_cap: 1,
        },
        review_policy: ControlReviewPolicyV1 {
            verifier_identity: "independent-verifier".into(),
            verifier_profile: "pi:provider/model".into(),
            frozen_surface_source: "manifest".into(),
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
    }
}

fn compile_input(control_id: &str) -> ControlCompileInputV1 {
    ControlCompileInputV1 {
        compile_version: 1,
        manifest: manifest(control_id),
        brief_inputs: vec![],
    }
}

#[test]
fn authority_absence_and_flash_profile_fail_closed_without_events() {
    let fixture = Fixture::new();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let before = ledger.count_events().unwrap();
    let error = ledger
        .compile_control(
            compile_input("control_absent"),
            &fixture.session,
            &"00".repeat(32),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("CONTROL_UNAVAILABLE"), "{error}");
    assert_eq!(ledger.count_events().unwrap(), before);

    let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&Rfc3339)
        .unwrap();
    let error = ledger
        .provision_control_authority(ControlAuthorityProvision {
            principal_id: "planner",
            session_id: &fixture.session,
            command_profile: ControlCommandProfileV1::Flash,
            portable_repo_id: None,
            expires_at: &expiry,
            issuer_token: &"11".repeat(32),
            authority_token_out: &fixture.root.path().join("flash-token"),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("Flash command profile"), "{error}");
}

#[cfg(unix)]
#[test]
fn group_or_world_readable_root_key_fails_closed_before_append() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let key_path = fixture.root.path().join(".edda/control-local/root.key");
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let before = ledger.count_events().unwrap();
    let error = fixture.try_provision().unwrap_err().to_string();
    assert!(error.contains("permissions are not owner-only"), "{error}");
    assert_eq!(ledger.count_events().unwrap(), before);
    assert!(!fixture.root.path().join("authority-token").exists());
}

#[cfg(unix)]
#[test]
fn authority_bearer_output_is_private_create_new_and_never_clobbered() {
    let fixture = Fixture::new();
    let first = fixture.provision();
    assert_eq!(first.len(), 64);
    let capability_path = fixture
        .root
        .path()
        .join(".edda/control-local/authority.json");
    let before = std::fs::read(&capability_path).unwrap();
    let error = fixture.try_provision().unwrap_err().to_string();
    assert!(error.contains("refusing to create"), "{error}");
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("authority-token")).unwrap(),
        first
    );
    assert_eq!(std::fs::read(&capability_path).unwrap(), before);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(fixture.root.path().join("authority-token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[cfg(unix)]
#[test]
fn revoked_rbac_and_tampered_capability_fail_before_append() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let missing_bearer = ledger
        .compile_control(
            compile_input("control_missingbearer"),
            &fixture.session,
            &"22".repeat(32),
        )
        .unwrap_err()
        .to_string();
    assert!(
        missing_bearer.contains("does not possess"),
        "{missing_bearer}"
    );
    std::fs::write(
        fixture.root.path().join(".edda/policy.yaml"),
        "version: 2\nroles: [controller]\nrules: []\npermissions:\n  default: deny\n  grants: []\n",
    )
    .unwrap();
    let denied = ledger
        .compile_control(
            compile_input("control_denied"),
            &fixture.session,
            &authority_token,
        )
        .unwrap_err()
        .to_string();
    assert!(denied.contains("RBAC denied"), "{denied}");

    std::fs::write(
        fixture.root.path().join(".edda/policy.yaml"),
        "version: 2\nroles: [controller]\nrules: []\npermissions:\n  default: deny\n  grants:\n    - actions: [control_compile, control_adjudicate]\n      roles: [controller]\n",
    )
    .unwrap();
    let authority_path = fixture
        .root
        .path()
        .join(".edda/control-local/authority.json");
    let mut authority: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&authority_path).unwrap()).unwrap();
    authority["principal_id"] = serde_json::json!("forged");
    std::fs::write(
        &authority_path,
        serde_json::to_vec_pretty(&authority).unwrap(),
    )
    .unwrap();
    let forged = ledger
        .compile_control(
            compile_input("control_forgedcap"),
            &fixture.session,
            &authority_token,
        )
        .unwrap_err()
        .to_string();
    assert!(forged.contains("seal is invalid"), "{forged}");
    assert_eq!(ledger.count_events().unwrap(), 0);
}

#[cfg(unix)]
#[test]
fn local_complete_has_durable_intent_result_and_replay_is_stale() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    ledger
        .compile_control(
            compile_input("control_complete"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let before_next = ledger.count_events().unwrap();
    let first = ledger.control_next("control_complete").unwrap();
    let second = ledger.control_next("control_complete").unwrap();
    assert_eq!(
        first.action_id, second.action_id,
        "action id is deterministic"
    );
    assert_eq!(
        ledger.count_events().unwrap(),
        before_next,
        "next is read-only"
    );
    assert_eq!(first.action_kind, ControlActionKindV1::Complete);
    let applied = ledger
        .control_apply("control_complete", first.token.as_deref().unwrap())
        .unwrap();
    assert!(applied.applied);
    assert!(applied.intent_event_id.is_some());
    assert!(applied.receipt_event_id.is_some());
    let status = ledger.control_status("control_complete").unwrap();
    assert_eq!(status.state, ControlStateV1::Completed);
    assert_eq!(status.state_version, 2);
    assert!(status.pending_intent_event_id.is_none());
    let replay = ledger
        .control_apply("control_complete", first.token.as_deref().unwrap())
        .unwrap();
    assert!(!replay.applied && replay.stale);
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

#[cfg(unix)]
#[test]
fn expired_pending_intent_requires_strong_rotation_and_never_executes() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    ledger
        .compile_control(
            compile_input("control_recovery"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let fresh = ledger.control_next("control_recovery").unwrap();
    let fresh_token = fresh.token.clone().unwrap();
    let expired_unix = OffsetDateTime::now_utc().unix_timestamp() - 1;
    let expires_at = OffsetDateTime::from_unix_timestamp(expired_unix)
        .unwrap()
        .format(&Rfc3339)
        .unwrap();
    let projected =
        crate::control_projection::project_control(&ledger, "control_recovery").unwrap();
    let claims = crate::control_projection::TokenClaims {
        control_id: "control_recovery",
        manifest_digest: &projected.record.manifest_digest,
        capability_id: &projected.record.authority.capability_id,
        observed_state_version: fresh.state_version,
        action_id: &fresh.action_id,
        action_kind: fresh.action_kind,
        target: &fresh.target,
        expires_at: &expires_at,
    };
    let mac = crate::control_authority::seal_local(&ledger, &claims).unwrap();
    let original_token = format!(
        "ctl1.{}.{}.{}.{}",
        fresh.state_version, expired_unix, fresh.action_id, mac
    );
    let commitment = format!(
        "sha256:{}",
        edda_core::hash::sha256_hex(original_token.as_bytes())
    );
    let mut intent = ControlIntentV1 {
        intent_version: CONTROL_INTENT_VERSION,
        control_id: "control_recovery".into(),
        step_id: format!("step_{}", fresh.state_version),
        action_id: fresh.action_id.clone(),
        observed_state_version: fresh.state_version,
        action_token: commitment,
        action_kind: fresh.action_kind,
        target: fresh.target.clone(),
        created_at: OffsetDateTime::from_unix_timestamp(expired_unix - 300)
            .unwrap()
            .format(&Rfc3339)
            .unwrap(),
        seal: String::new(),
    };
    intent.seal = crate::control_authority::seal_local(
        &ledger,
        &crate::control_projection::intent_without_seal(&intent).unwrap(),
    )
    .unwrap();
    let intent_event_id =
        crate::control_projection::deterministic_event_id("intent", &fresh.action_id);
    let event = crate::control_projection::make_control_intent_event(
        intent_event_id,
        &ledger.head_branch().unwrap(),
        ledger.last_event_hash().unwrap().as_deref(),
        serde_json::json!({"control_intent": intent}),
        edda_core::Refs::default(),
    )
    .unwrap();
    ledger.append_event(&event).unwrap();

    let pending = ledger.control_next("control_recovery").unwrap();
    assert_eq!(
        pending.reason_code,
        "EXPIRED_PENDING_INTENT_REQUIRES_ADJUDICATION"
    );
    assert_eq!(pending.availability, ControlAvailabilityV1::NeedsDecision);
    assert!(pending.token.is_none());
    let mismatch = ledger
        .control_apply("control_recovery", &fresh_token)
        .unwrap_err()
        .to_string();
    assert!(mismatch.contains("authorization commitment"), "{mismatch}");
    let expired = ledger
        .control_apply("control_recovery", &original_token)
        .unwrap_err()
        .to_string();
    assert!(
        expired.contains("expired action token refused"),
        "{expired}"
    );
    assert!(ledger
        .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
        .unwrap()
        .is_empty());

    let projected =
        crate::control_projection::project_control(&ledger, "control_recovery").unwrap();
    let rotated = ledger
        .adjudicate_control(
            "control_recovery",
            projected.status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: CONTROL_ADJUDICATION_VERSION,
                reason_code: "ROTATE_EXPIRED_ACTION".into(),
                evidence: vec!["expired intent was not executed".into()],
                expected_manifest_digest: projected.record.manifest_digest,
                clear_return_for_decision: false,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    assert_eq!(rotated.manifest_version, 2);
    assert!(rotated.pending_intent_event_id.is_none());
    let next = ledger.control_next("control_recovery").unwrap();
    assert_ne!(next.action_id, fresh.action_id);
}

#[cfg(unix)]
#[test]
fn external_action_is_explicitly_unavailable_and_writes_no_intent() {
    let fixture = Fixture::new();
    let portable_repo_id = format!("repo_{}", "d".repeat(64));
    let authority_token = fixture.provision_for(Some(&portable_repo_id));
    let mut input = compile_input("control_external");
    input.manifest.basis.portable_repo_id = Some(portable_repo_id);
    input.manifest.basis.github_repository = Some("owner/repo".into());
    input.manifest.tasks.push(ControlTaskInputV1 {
        task_key: "step_worker".into(),
        task_id: None,
        depends_on: vec![],
        exact_input_sha: "a".repeat(40),
        brief_id: "brief_worker".into(),
        runtime_profile: RuntimeProfileV1::Strong,
        model_target: "strong".into(),
        owned_paths: vec!["src/**".into()],
        build_lane: None,
        issue_binding: None,
        local_only: true,
        execution_host_affinity: "local".into(),
        required_delivery: DeliveryRequirementV1::LocalOnly,
    });
    input.brief_inputs.push(execution_brief_input());
    input.brief_inputs[0].basis.portable_repo_id = input.manifest.basis.portable_repo_id.clone();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut repo_mismatch = input.clone();
    repo_mismatch.manifest.control_id = "control_repomismatch".into();
    repo_mismatch.brief_inputs[0].basis.portable_repo_id = Some(format!("repo_{}", "e".repeat(64)));
    let error = ledger
        .compile_control(repo_mismatch, &fixture.session, &authority_token)
        .unwrap_err()
        .to_string();
    assert!(error.contains("repository"), "{error}");

    let mut local_preparation = input.clone();
    local_preparation.manifest.control_id = "control_localtask".into();
    ledger
        .compile_control(local_preparation, &fixture.session, &authority_token)
        .unwrap();
    assert_eq!(
        ledger
            .control_next("control_localtask")
            .unwrap()
            .action_kind,
        ControlActionKindV1::Complete
    );

    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let accepted = compiled.briefs.first().unwrap();
    let manifest_event = ledger
        .get_event(&compiled.manifest_event_id)
        .unwrap()
        .unwrap();
    let manifest_record = crate::control_projection::parse_manifest_event(&manifest_event).unwrap();
    assert_eq!(
        accepted.authority.principal_id,
        manifest_record.authority.principal_id
    );
    assert_eq!(
        accepted.authority.session_id,
        manifest_record.authority.session_id
    );
    let brief_event = ledger.get_event(&accepted.event_id).unwrap().unwrap();
    for (field, value) in [
        ("principal_id", "other-principal"),
        ("session_id", "other-session"),
    ] {
        let mut tampered = brief_event.clone();
        tampered.payload["execution_brief"]["authority"][field] = serde_json::json!(value);
        finalize_for_test(&mut tampered);
        let error = ledger
            .load_sealed_execution_brief(&tampered, &accepted.content_digest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("principal/session authority"), "{error}");
    }
    let next = ledger.control_next("control_external").unwrap();
    assert_eq!(next.availability, ControlAvailabilityV1::NeedsDecision);
    assert_eq!(next.action_kind, ControlActionKindV1::ControlError);
    assert_eq!(next.reason_code, "TASK_RAIL_BINDING_REQUIRED");
    assert!(next.token.is_none());
    assert!(ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .is_empty());
    std::fs::remove_file(
        fixture
            .root
            .path()
            .join(".edda/control-local/authority.json"),
    )
    .unwrap();
    assert_eq!(
        ledger
            .load_execution_brief(&accepted.event_id, &accepted.content_digest)
            .unwrap()
            .brief
            .brief_id,
        "brief_worker"
    );
}

#[test]
fn controlled_verifier_is_a_canonical_github_login() {
    let mut value = manifest("control_loginshape");
    value.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    value.merge_policy.pr_number = Some(1141);
    value.merge_policy.expected_head_sha = Some("b".repeat(40));
    value.merge_policy.expected_base_sha = Some("c".repeat(40));
    value.basis.portable_repo_id = Some(format!("repo_{}", "d".repeat(64)));
    value.basis.github_repository = Some("owner/repo".into());

    value.review_policy.verifier_identity = "MixedCase".into();
    let error = compile_control_manifest(value.clone(), &[], 1)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("canonical lowercase GitHub login"),
        "{error}"
    );
}

#[cfg(unix)]
#[path = "control_effect_tests.rs"]
mod effect_tests;

#[cfg(unix)]
#[test]
fn taskless_external_completion_conditions_never_receive_local_complete_tokens() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();

    let mut invalid_profile = compile_input("control_invalidprofile");
    invalid_profile.manifest.completion_condition =
        ControlCompletionConditionV1::VerificationSucceeded;
    invalid_profile.manifest.basis.portable_repo_id = Some(format!("repo_{}", "d".repeat(64)));
    invalid_profile.manifest.basis.github_repository = Some("owner/repo".into());
    invalid_profile.manifest.merge_policy.pr_number = Some(1141);
    invalid_profile.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    invalid_profile.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    invalid_profile.manifest.review_policy.verifier_profile = "strong".into();
    let error = ledger
        .compile_control(invalid_profile, &fixture.session, &authority_token)
        .unwrap_err()
        .to_string();
    assert!(error.contains("verifier_profile"), "{error}");

    let mut verification = compile_input("control_verifyonly");
    verification.manifest.completion_condition =
        ControlCompletionConditionV1::VerificationSucceeded;
    verification.manifest.basis.portable_repo_id = Some(format!("repo_{}", "d".repeat(64)));
    verification.manifest.basis.github_repository = Some("owner/repo".into());
    let error = ledger
        .compile_control(verification, &fixture.session, &authority_token)
        .unwrap_err()
        .to_string();
    assert!(error.contains("exact PR, base, and head"), "{error}");

    let mut merge = compile_input("control_mergeonly");
    merge.manifest.completion_condition = ControlCompletionConditionV1::DelegatedMergeSucceeded;
    merge.manifest.merge_policy.required = true;
    merge.manifest.merge_policy.pr_number = Some(1141);
    merge.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    merge.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let error = ledger
        .compile_control(merge, &fixture.session, &authority_token)
        .unwrap_err()
        .to_string();
    assert!(error.contains("repository"), "{error}");
    assert!(ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn forged_foreign_expired_and_target_mismatched_tokens_never_mutate() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    ledger
        .compile_control(
            compile_input("control_tokena"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    ledger
        .compile_control(
            compile_input("control_tokenb"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let next = ledger.control_next("control_tokena").unwrap();
    let token = next.token.unwrap();
    let mut forged = token.clone();
    let last = forged.pop().unwrap();
    forged.push(if last == '0' { '1' } else { '0' });
    assert!(ledger.control_apply("control_tokena", &forged).is_err());
    assert!(ledger.control_apply("control_tokenb", &token).is_err());

    let mut parts: Vec<_> = token.split('.').map(str::to_string).collect();
    parts[2] = "1".into();
    let expired = parts.join(".");
    let error = ledger
        .control_apply("control_tokena", &expired)
        .unwrap_err()
        .to_string();
    assert!(error.contains("expired"), "{error}");
    assert_eq!(
        ledger
            .control_status("control_tokena")
            .unwrap()
            .state_version,
        1
    );
    assert!(ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn needs_decision_requires_versioned_strong_adjudication() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let mut input = compile_input("control_decide");
    input.manifest.return_for_decision = vec!["CHOOSE_LOCAL_CONTINUATION".into()];
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let next = ledger.control_next("control_decide").unwrap();
    ledger
        .control_apply("control_decide", next.token.as_deref().unwrap())
        .unwrap();
    let decision = ControlAdjudicationInputV1 {
        adjudication_version: 1,
        reason_code: "CONTINUE_LOCAL".into(),
        evidence: vec!["operator chose the modeled local route".into()],
        expected_manifest_digest: compiled.manifest_digest,
        clear_return_for_decision: true,
    };
    let stale = ledger
        .adjudicate_control(
            "control_decide",
            1,
            decision.clone(),
            &fixture.session,
            &authority_token,
        )
        .unwrap_err()
        .to_string();
    assert!(stale.contains("stale"), "{stale}");
    let status = ledger
        .adjudicate_control(
            "control_decide",
            2,
            decision,
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    assert_eq!(status.manifest_version, 2);
    assert_eq!(status.state_version, 3);
    assert_eq!(status.state, ControlStateV1::Prepared);

    let event = ledger
        .iter_events_by_type(CONTROL_MANIFEST_EVENT_TYPE)
        .unwrap()
        .pop()
        .unwrap();
    let record = crate::control_projection::parse_manifest_event(&event).unwrap();
    crate::control_projection::verify_manifest_record(&ledger, &record).unwrap();
    let mut reason = record.clone();
    reason.adjudication.as_mut().unwrap().reason_code = "OTHER_REASON".into();
    let mut evidence = record.clone();
    evidence
        .adjudication
        .as_mut()
        .unwrap()
        .evidence
        .push("unsealed evidence".into());
    let mut prior_state = record.clone();
    prior_state
        .adjudication
        .as_mut()
        .unwrap()
        .prior_state_version += 1;
    let mut prior_manifest = record.clone();
    prior_manifest
        .adjudication
        .as_mut()
        .unwrap()
        .prior_manifest_digest = "f".repeat(64);
    for tampered in [reason, evidence, prior_state, prior_manifest] {
        let error = crate::control_projection::verify_manifest_record(&ledger, &tampered)
            .unwrap_err()
            .to_string();
        assert!(error.contains("authority seal is invalid"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn imported_manifest_and_forged_boolean_do_not_confer_authority() {
    let source = Fixture::new();
    let authority_token = source.provision();
    let source_ledger = Ledger::open(source.root.path()).unwrap();
    source_ledger
        .compile_control(
            compile_input("control_imported"),
            &source.session,
            &authority_token,
        )
        .unwrap();
    let manifest_event = source_ledger
        .iter_events_by_type(CONTROL_MANIFEST_EVENT_TYPE)
        .unwrap()
        .pop()
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_ledger = Ledger::open_or_init(target.path()).unwrap();
    let mut imported = manifest_event;
    imported.parent_hash = None;
    imported.payload["trusted"] = serde_json::json!(true);
    finalize_for_test(&mut imported);
    target_ledger.append_event(&imported).unwrap();
    let status = target_ledger.control_status("control_imported").unwrap();
    assert!(!status.authority_valid);
    assert!(target_ledger.control_next("control_imported").is_err());
}

#[cfg(unix)]
#[test]
fn status_is_read_only_even_when_authority_is_unavailable() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    ledger
        .compile_control(
            compile_input("control_readonly"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    drop(ledger);
    std::fs::remove_file(
        fixture
            .root
            .path()
            .join(".edda/control-local/authority.json"),
    )
    .unwrap();
    let db = fixture.root.path().join(".edda/ledger.db");
    let before = std::fs::metadata(&db).unwrap().modified().unwrap();
    let count = Ledger::open_existing(fixture.root.path())
        .unwrap()
        .count_events()
        .unwrap();
    let status = Ledger::open_existing(fixture.root.path())
        .unwrap()
        .control_status("control_readonly")
        .unwrap();
    assert_eq!(status.state, ControlStateV1::Prepared);
    assert!(!status.authority_valid);
    assert_eq!(
        Ledger::open_existing(fixture.root.path())
            .unwrap()
            .count_events()
            .unwrap(),
        count
    );
    assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), before);
}

#[cfg(unix)]
#[test]
fn concurrent_apply_has_one_effective_transition() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision();
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    ledger
        .compile_control(
            compile_input("control_race"),
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let token = ledger.control_next("control_race").unwrap().token.unwrap();
    drop(ledger);

    let root = fixture.root.path().to_path_buf();
    let store_path = fixture._store.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(3));
    let mut joins = vec![];
    for _ in 0..2 {
        let root = root.clone();
        let token = token.clone();
        let barrier = Arc::clone(&barrier);
        let store_path = store_path.clone();
        joins.push(std::thread::spawn(move || {
            let _scope = edda_store::test_support::set_thread_override(&store_path);
            barrier.wait();
            Ledger::open(root).and_then(|ledger| ledger.control_apply("control_race", &token))
        }));
    }
    barrier.wait();
    let results: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_ref().is_ok_and(|value| value.applied))
            .count(),
        1
    );
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    assert_eq!(
        ledger.control_status("control_race").unwrap().state_version,
        2
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
            .unwrap()
            .len(),
        1
    );
}

#[cfg(unix)]
fn execution_brief_input() -> ExecutionBriefInputV1 {
    ExecutionBriefInputV1 {
        brief_version: 1,
        brief_id: "brief_worker".into(),
        task_ref: None,
        runtime_profile: RuntimeProfileV1::Strong,
        intent: ExecutionIntentV1::Implement,
        objective: "perform one bounded local change".into(),
        basis: ExecutionBasisV1 {
            portable_repo_id: None,
            base_full_sha: "a".repeat(40),
            issue_spec_refs: vec![],
        },
        scope: ExecutionScopeV1 {
            allowed_paths: vec!["src/**".into()],
            out_of_scope: vec![],
        },
        read_order: vec![],
        known_facts: vec![],
        allowed_decisions: vec![],
        return_for_decision: vec![],
        procedure: TrustedProcedureV1::ControllerAuthored {
            authored_by: "planner".into(),
            principles: vec!["stay in scope".into()],
            probe_cards: vec![],
            implementation_steps: vec![],
            validation: vec![],
        },
        outcome_codes: vec![OutcomeCodeV1 {
            code: "DONE".into(),
            result_class: ResultClassV1::Success,
        }],
        receipt_schema: ReceiptSchemaV1 {
            receipt_version: 1,
            required_fields: vec![
                ReceiptFieldV1::BriefIdentity,
                ReceiptFieldV1::OutcomeCode,
                ReceiptFieldV1::ChangedPaths,
                ReceiptFieldV1::ValidationRan,
                ReceiptFieldV1::ValidationRead,
                ReceiptFieldV1::RecommendedNextAction,
            ],
        },
    }
}

#[cfg(unix)]
fn finalize_for_test(event: &mut edda_core::Event) {
    event.hash.clear();
    event.digests.clear();
    edda_core::event::finalize_event(event).unwrap();
}
