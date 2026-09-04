use super::*;

#[test]
pub(super) fn planner_starts_ready_tasks_by_id_subject_to_wip() {
    let actions = plan_actions(
        &[
            task(9, TaskStatus::Ready, &["src/nine.rs"]),
            task(2, TaskStatus::Ready, &["src/two.rs"]),
        ],
        &[],
        &[],
        "2026-08-16T01:00:00Z",
        1,
        3,
    );

    assert_eq!(
        actions,
        vec![ReconcileAction::Start {
            task_id: 2,
            attempt: 1
        }]
    );
}

#[test]
pub(super) fn planner_leaves_live_running_and_resumes_expired_bound_session() {
    let mut live = task(1, TaskStatus::Running, &["src/live.rs"]);
    live.attempts = 1;
    let mut expired = task(2, TaskStatus::Running, &["src/expired.rs"]);
    expired.attempts = 2;
    expired.session_id = Some("thread-2".into());
    expired.session_agent_kind = Some("codex".into());
    expired.session_attempt = Some(2);
    let actions = plan_actions(
        &[live, expired],
        &[
            lease(1, 1, "2026-08-16T02:00:00Z"),
            lease(2, 2, "2026-08-16T00:00:00Z"),
        ],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );

    assert_eq!(
        actions,
        vec![ReconcileAction::Resume {
            task_id: 2,
            attempt: 2,
            session_id: "thread-2".into(),
        }]
    );
}

#[test]
pub(super) fn planner_requeues_expired_unresumable_work_and_stops_at_retry_cap() {
    let mut retry = task(1, TaskStatus::Running, &["src/retry.rs"]);
    retry.attempts = 1;
    let mut capped = task(2, TaskStatus::Running, &["src/capped.rs"]);
    capped.attempts = 3;
    let actions = plan_actions(
        &[retry, capped],
        &[
            lease(1, 1, "2026-08-16T00:00:00Z"),
            lease(2, 3, "2026-08-16T00:00:00Z"),
        ],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );

    assert_eq!(
        actions,
        vec![
            ReconcileAction::Requeue {
                task_id: 1,
                next_attempt: 2,
                reason: "expired-without-session".into(),
            },
            ReconcileAction::Fail {
                task_id: 2,
                reason: "retry-cap-exhausted".into(),
            },
        ]
    );
}

#[test]
pub(super) fn planner_resumes_only_a_codex_session_bound_to_the_current_attempt() {
    let mut task = task(1, TaskStatus::Running, &["src/retry.rs"]);
    task.attempts = 2;
    task.session_id = Some("old-thread".into());
    task.session_agent_kind = Some("codex".into());
    task.session_attempt = Some(1);
    let actions = plan_actions(
        &[task],
        &[lease(1, 2, "2026-08-16T00:00:00Z")],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );

    assert_eq!(
        actions,
        vec![ReconcileAction::Requeue {
            task_id: 1,
            next_attempt: 3,
            reason: "expired-without-session".into(),
        }]
    );
}

#[test]
pub(super) fn planner_treats_exact_expiry_as_expired_and_blocks_missing_dependencies() {
    let mut live = task(1, TaskStatus::Running, &["src/live.rs"]);
    live.attempts = 1;
    let blocked = task(2, TaskStatus::Blocked, &["src/blocked.rs"]);
    assert!(plan_actions(
        &[live.clone(), blocked],
        &[lease(1, 1, "2026-08-16T01:00:00Z")],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    )
    .iter()
    .any(|action| matches!(action, ReconcileAction::Requeue { task_id: 1, .. })));
    assert!(plan_actions(
        &[live],
        &[lease(1, 1, "2026-08-16T01:00:01Z")],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    )
    .is_empty());
}

#[test]
pub(super) fn planner_prevents_selected_scopes_from_overlapping_each_other() {
    let actions = plan_actions(
        &[
            task(1, TaskStatus::Ready, &["src/auth"]),
            task(2, TaskStatus::Ready, &["src/auth/login.rs"]),
            task(3, TaskStatus::Ready, &["src/billing.rs"]),
        ],
        &[],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );
    assert_eq!(
        actions,
        vec![
            ReconcileAction::Start {
                task_id: 1,
                attempt: 1
            },
            ReconcileAction::Start {
                task_id: 3,
                attempt: 1
            },
        ]
    );
}

#[test]
pub(super) fn planner_treats_an_empty_selected_scope_as_repo_wide() {
    let actions = plan_actions(
        &[
            task(1, TaskStatus::Ready, &[]),
            task(2, TaskStatus::Ready, &["src/other.rs"]),
        ],
        &[],
        &[],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );
    assert_eq!(
        actions,
        vec![ReconcileAction::Start {
            task_id: 1,
            attempt: 1
        }]
    );
}

#[test]
pub(super) fn planner_ignores_live_peers_without_claimed_paths() {
    let actions = plan_actions(
        &[task(1, TaskStatus::Ready, &[])],
        &[],
        &[Vec::new()],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );
    assert_eq!(
        actions,
        vec![ReconcileAction::Start {
            task_id: 1,
            attempt: 1
        }]
    );
}

#[test]
pub(super) fn planner_blocks_declared_claimed_and_empty_scopes_conservatively() {
    let actions = plan_actions(
        &[
            task(1, TaskStatus::Ready, &["src/auth/*.rs"]),
            task(2, TaskStatus::Ready, &["src/auth/login.rs"]),
            task(3, TaskStatus::Ready, &[]),
            task(4, TaskStatus::Ready, &["src/other.rs"]),
        ],
        &[],
        &[vec!["src/auth".into()]],
        "2026-08-16T01:00:00Z",
        3,
        3,
    );

    assert_eq!(
        actions,
        vec![ReconcileAction::Start {
            task_id: 4,
            attempt: 1
        }]
    );
}

#[test]
pub(super) fn static_prefixes_normalize_separators_and_glob_suffixes() {
    assert!(paths_overlap("src\\auth\\*.rs", "src/auth/login.rs"));
    assert!(paths_overlap("src/auth?", "src/auth/login.rs"));
    assert!(paths_overlap("src/auth[ab]", "src/auth/login.rs"));
    assert!(paths_overlap("src/auth{a,b}", "src/auth/login.rs"));
    assert!(paths_overlap(
        "src/./auth/../auth/*.rs",
        "src/auth/login.rs"
    ));
    assert!(paths_overlap("src/foo*", "src/foobar.rs"));
    assert!(paths_overlap("src/auth?", "src/authX"));
    assert!(paths_overlap("../outside", "src/billing.rs"));
    assert!(!paths_overlap("src/auth.rs", "src/billing.rs"));
    assert!(paths_overlap("*", "src/billing.rs"));
}

#[test]
pub(super) fn runner_prompt_embeds_bounded_brief_and_dependency_evidence() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("brief.md"),
        format!("brief-{}", "x".repeat(5000)),
    )?;
    let mut dependency = task(1, TaskStatus::Done, &[]);
    dependency.receipt = Some("dependency complete".into());
    dependency.evidence_paths = vec!["proof/a.txt".into(), "proof/b.txt".into()];
    let mut current = task(2, TaskStatus::Ready, &["src/reconcile.rs"]);
    current.after = vec![1];
    current.brief_ref = Some("brief.md".into());
    let worktree = dir.path().join("worktree");

    let prompt = runner_prompt(
        dir.path(),
        &[dependency, current.clone()],
        &current,
        3,
        &worktree,
    );

    assert!(prompt.contains("brief-"));
    assert!(prompt.contains("[brief truncated]"));
    assert!(prompt.contains("#1/dependency complete evidence=[\"proof/a.txt\", \"proof/b.txt\"]"));
    assert!(prompt.contains("codex/task-2-attempt-3"));
    assert!(prompt.contains(&worktree.display().to_string()));
    assert!(prompt.contains("edda task done 2"));
    Ok(())
}

#[test]
pub(super) fn reconciliation_persists_one_attempt_before_any_runner_launch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    let event = edda_core::event::new_task_created_event(&edda_core::event::TaskCreatedParams {
        branch: "main",
        parent_hash: None,
        task_id: 1,
        title: "reconcile me",
        assignee: None,
        agent_kind: Some("codex"),
        after: &[],
        plan_id: None,
        work_unit_ref: None,
        brief_ref: None,
        idempotency_key: None,
        scope_paths: &["src/reconcile.rs".into()],
    })?;
    ledger.append_event(&event)?;

    let first = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;
    let second = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;

    assert_eq!(first.plans.len(), 1);
    assert!(second.plans.is_empty());
    let events = ledger.task_events()?;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "task.started")
            .count(),
        1
    );
    assert_eq!(ledger.task_lease(1)?.expect("current lease").attempt, 1);
    Ok(())
}

#[test]
pub(super) fn event_and_lease_boundary_failures_never_leave_ownerless_started_truth(
) -> anyhow::Result<()> {
    for fail_started in [false, true] {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        create_task(&ledger, 1, &["src/boundary.rs".into()])?;
        if fail_started {
            FAIL_NEXT_STARTED.with(|flag| flag.set(true));
        } else {
            FAIL_NEXT_LEASE.with(|flag| flag.set(true));
        }

        let outcome = persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;
        assert!(outcome.plans.is_empty());
        assert_eq!(outcome.errors.len(), 1);
        assert!(ledger.task_lease(1)?.is_none());
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| event.event_type != "task.started"));
    }
    Ok(())
}

#[test]
pub(super) fn middle_persistence_fault_returns_first_and_later_launchable_plans(
) -> anyhow::Result<()> {
    for fail_started in [false, true] {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo)?;
        init_git(&repo)?;
        edda_ledger::Ledger::ensure_initialized(&repo)?;
        let ledger = edda_ledger::Ledger::open(&repo)?;
        for id in 1..=3 {
            create_task(&ledger, id, &[format!("src/{id}.rs")])?;
        }
        FAIL_TASK_ID.with(|target| target.set(Some(2)));
        if fail_started {
            FAIL_NEXT_STARTED.with(|flag| flag.set(true));
        } else {
            FAIL_NEXT_LEASE.with(|flag| flag.set(true));
        }
        let outcome = persist_reconciliation(
            &repo,
            &ReconcileConfig {
                max_workers: 3,
                ..ReconcileConfig::test_defaults()
            },
        )?;
        FAIL_TASK_ID.with(|target| target.set(None));

        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(
            outcome
                .plans
                .iter()
                .map(|plan| plan.task.task_id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(ledger.task_lease(2)?.is_none());
        assert!(ledger
            .task_events()?
            .iter()
            .all(|event| !(event.event_type == "task.started" && event.payload["task_id"] == 2)));
    }
    Ok(())
}
