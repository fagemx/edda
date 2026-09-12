use super::*;

fn test_brief(
    task_id: u64,
    base_full_sha: &str,
    scope: &str,
) -> anyhow::Result<edda_ledger::AcceptedExecutionBriefV1> {
    let input = serde_json::from_value(serde_json::json!({
        "brief_version": 1,
        "brief_id": format!("brief_reconcile{task_id}"),
        "task_ref": task_id,
        "runtime_profile": "flash",
        "intent": "fix",
        "objective": "describe one controlled attempt without launching it",
        "basis": {"base_full_sha": base_full_sha},
        "scope": {"allowed_paths": [scope]},
        "procedure": {
            "kind": "controller_authored",
            "authored_by": "controller",
            "principles": ["use immutable identity"],
            "implementation_steps": [{
                "step_id": "step_fix",
                "instruction": "apply the bounded fix"
            }]
        },
        "outcome_codes": [{"code": "DONE", "result_class": "success"}],
        "receipt_schema": {
            "receipt_version": 1,
            "required_fields": [
                "brief_identity", "task_identity", "outcome_code", "changed_paths",
                "validation_ran", "validation_read", "recommended_next_action"
            ]
        }
    }))?;
    let event_id = format!("evt_{}", "a".repeat(26));
    let authority = edda_core::guided_execution::ExecutionBriefAuthorityV1 {
        principal_id: "controller".into(),
        session_id: "test-controller-session".into(),
        authority_event_id: "evt_testauthority1".into(),
    };
    let (brief, canonical_bytes) = edda_core::guided_execution::compile_execution_brief(
        input,
        event_id.clone(),
        &authority.principal_id,
    )?;
    Ok(edda_ledger::AcceptedExecutionBriefV1 {
        event_id,
        content_digest: brief.content_digest.clone(),
        canonical_bytes,
        brief,
        authority,
    })
}

fn setup_controlled_descriptor(
    task_id: u64,
) -> anyhow::Result<(
    tempfile::TempDir,
    std::path::PathBuf,
    Ledger,
    edda_ledger::AcceptedExecutionBriefV1,
)> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    create_task(&ledger, task_id, &["src/controlled.rs".into()])?;
    append_started(&ledger, task_id, 1, 300)?;
    ledger.upsert_task_lease(&TaskLease {
        expires_at: "2999-01-01T00:00:00Z".into(),
        ..lease(task_id, 1, "2999-01-01T00:00:00Z")
    })?;
    let head = git(&repo, ["rev-parse", "HEAD"])?;
    let accepted = test_brief(task_id, head.trim(), "src/controlled.rs")?;
    Ok((dir, repo, ledger, accepted))
}

#[test]
pub(super) fn controlled_reconcile_returns_exact_base_descriptor_and_never_launches(
) -> anyhow::Result<()> {
    let (_dir, repo, ledger, accepted) = setup_controlled_descriptor(21)?;
    let suggested = attempt_worktree_path(&repo, 21, 1)?;
    assert!(!suggested.exists());
    let original_owner = ledger.task_lease(21)?.expect("lease").owner;
    let mut config = ReconcileConfig::test_defaults();
    config.codex_bin = repo.join("must-not-launch.exe");
    config.brief_event_id = Some(accepted.event_id.clone());
    config.brief_digest = Some(accepted.content_digest.clone());

    PROCESS_CALL_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
    let descriptor = controlled_attempt_descriptor_from_data(
        &repo,
        21,
        1,
        &original_owner,
        &accepted.event_id,
        &accepted.content_digest,
        &ledger,
        accepted.clone(),
    )?;
    assert_eq!(
        PROCESS_CALL_COUNT.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "descriptor construction must not invoke any process seam"
    );
    let json = serde_json::to_value(&descriptor)?;
    assert_eq!(json["execution"], "none");
    assert_eq!(json["task_id"], 21);
    assert_eq!(json["attempt"], 1);
    assert_eq!(json["brief_event_id"], accepted.event_id);
    assert_eq!(json["brief_digest"], accepted.content_digest);
    assert_eq!(json["base_full_sha"], accepted.brief.basis.base_full_sha);
    assert_eq!(
        json["allowed_paths"],
        serde_json::json!(["src/controlled.rs"])
    );
    assert!(!suggested.exists());
    let bound_owner = ledger.task_lease(21)?.expect("bound lease").owner;
    assert_eq!(json["current_lease_owner"], bound_owner);
    assert!(bound_owner.starts_with(edda_ledger::task_actions::CONTROLLED_TASK_LEASE_PREFIX));
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| { event.event_type != "task.session" && event.event_type != "task.failed" }));

    let launch_error = launch_runner_with(&config.codex_bin, &repo, 21, 1, &bound_owner, &config)
        .unwrap_err()
        .to_string();
    assert!(launch_error.contains("never child runner launches"));
    let error = run_task(&repo, 21, 1, &bound_owner, &config, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("descriptor-only"));
    assert!(!suggested.exists());
    assert_eq!(
        ledger.task_lease(21)?.expect("preserved lease").owner,
        bound_owner
    );
    Ok(())
}

#[test]
pub(super) fn controlled_descriptor_refuses_owner_mismatch_before_binding() -> anyhow::Result<()> {
    let (_dir, repo, ledger, accepted) = setup_controlled_descriptor(22)?;
    let original = ledger.task_lease(22)?.expect("lease").owner;

    assert!(controlled_attempt_descriptor_from_data(
        &repo,
        22,
        1,
        "replacement",
        &accepted.event_id,
        &accepted.content_digest,
        &ledger,
        accepted.clone(),
    )
    .is_err());
    assert_eq!(ledger.task_lease(22)?.expect("unchanged").owner, original);
    Ok(())
}

#[test]
pub(super) fn controlled_descriptor_refuses_completed_attempt_without_mutation(
) -> anyhow::Result<()> {
    let (_dir, repo, ledger, accepted) = setup_controlled_descriptor(23)?;
    let owner = ledger.task_lease(23)?.expect("lease").owner;
    let parent = ledger.last_event_hash()?;
    ledger.append_event(&edda_core::event::new_task_done_event(
        "main",
        parent.as_deref(),
        23,
        "legacy completion before descriptor",
        &[],
    )?)?;
    assert!(controlled_attempt_descriptor_from_data(
        &repo,
        23,
        1,
        &owner,
        &accepted.event_id,
        &accepted.content_digest,
        &ledger,
        accepted.clone(),
    )
    .is_err());
    assert_eq!(
        ledger.task_lease(23)?.expect("untouched lease").owner,
        owner
    );
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.session"));
    Ok(())
}
