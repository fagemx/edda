use super::*;
use edda_ledger::lock::WorkspaceLock;
use std::sync::atomic::Ordering;

#[test]
pub(super) fn git_preparation_failure_leaves_no_phantom_dispatch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("not-a-git-repo");
    std::fs::create_dir(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 1,
            title: "must not dispatch",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            scope_paths: &["src/nope.rs".into()],
        },
    )?)?;

    assert!(persist_reconciliation(&repo, &ReconcileConfig::test_defaults()).is_err());
    assert!(ledger.task_lease(1)?.is_none());
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.started"));
    Ok(())
}

#[test]
pub(super) fn batch_preflights_every_worktree_before_the_first_started_event() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    create_task(&ledger, 1, &["src/one.rs".into()])?;
    create_task(&ledger, 2, &["src/two.rs".into()])?;
    let blocked = attempt_worktree_path(&repo, 2, 1)?;
    std::fs::create_dir_all(&blocked)?;
    std::fs::write(blocked.join("unseen.txt"), "preserve")?;

    let result = persist_reconciliation(
        &repo,
        &ReconcileConfig {
            max_workers: 2,
            ..ReconcileConfig::test_defaults()
        },
    );

    assert!(result.is_err());
    assert!(ledger.task_lease(1)?.is_none());
    assert!(ledger.task_lease(2)?.is_none());
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.started"));
    assert_eq!(
        std::fs::read_to_string(blocked.join("unseen.txt"))?,
        "preserve"
    );
    Ok(())
}

#[test]
pub(super) fn same_attempt_resume_preserves_dirty_and_ahead_worktree_state() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    let worktree = ensure_attempt_worktree(
        &repo,
        &task(3, TaskStatus::Running, &["src/resume.rs"]),
        1,
        false,
    )?;
    std::fs::write(worktree.join("dirty.txt"), "keep")?;
    Command::new("git")
        .args(["add", "dirty.txt"])
        .current_dir(&worktree)
        .status()?;
    Command::new("git")
        .args([
            "-c",
            "user.name=Edda Test",
            "-c",
            "user.email=edda@example.test",
            "commit",
            "-qm",
            "recovery state",
        ])
        .current_dir(&worktree)
        .status()?;
    std::fs::write(worktree.join("untracked.txt"), "also keep")?;

    assert_eq!(
        ensure_attempt_worktree(
            &repo,
            &task(3, TaskStatus::Running, &["src/resume.rs"]),
            1,
            true,
        )?,
        worktree
    );
    assert_eq!(std::fs::read_to_string(worktree.join("dirty.txt"))?, "keep");
    assert_eq!(
        std::fs::read_to_string(worktree.join("untracked.txt"))?,
        "also keep"
    );
    assert!(ensure_attempt_worktree(
        &repo,
        &task(3, TaskStatus::Running, &["src/resume.rs"]),
        1,
        false,
    )
    .is_err());
    Ok(())
}

#[test]
pub(super) fn completion_from_linked_worktree_uses_the_original_ledger() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    create_task(&ledger, 4, &["src/done.rs".into()])?;
    let view = ledger.task_views()?.remove(0);
    let worktree = ensure_attempt_worktree(&repo, &view, 1, false)?;
    append_started(&ledger, 4, 1, 300)?;

    let resolved = edda_ledger::EddaPaths::find_root_bounded(&worktree, dir.path())
        .expect("original ledger root");
    crate::cmd_task::execute(
        crate::cmd_task::TaskCmd::Done {
            id: 4,
            receipt: "completed from attempt worktree".into(),
            evidence_paths: vec!["evidence.txt".into()],
        },
        &resolved,
    )?;

    assert_eq!(resolved.canonicalize()?, repo.canonicalize()?);
    assert!(!worktree.join(".edda").exists());
    assert_eq!(ledger.task_views()?[0].status, TaskStatus::Done);
    Ok(())
}

#[test]
pub(super) fn simultaneous_reconciles_create_one_attempt_and_release_the_lock() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 1,
            title: "one attempt",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            scope_paths: &["src/one.rs".into()],
        },
    )?)?;
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let repo = repo.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                persist_reconciliation(&repo, &ReconcileConfig::test_defaults())
                    .map(|outcome| outcome.plans.len())
            })
        })
        .collect();
    let dispatched: usize = handles
        .into_iter()
        .map(|handle| handle.join().expect("reconcile thread"))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .sum();

    assert_eq!(dispatched, 1);
    assert_eq!(
        ledger
            .task_events()?
            .iter()
            .filter(|event| event.event_type == "task.started")
            .count(),
        1
    );
    assert_eq!(ledger.task_lease(1)?.expect("lease").attempt, 1);
    let lock = WorkspaceLock::acquire(&ledger.paths)?;
    drop(lock);
    Ok(())
}

#[test]
pub(super) fn attempt_worktree_reuses_matching_state_and_refuses_dirty_state() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    let worktree =
        ensure_attempt_worktree(&repo, &task(9, TaskStatus::Ready, &["src/x.rs"]), 2, false)?;
    assert_eq!(
        worktree,
        ensure_attempt_worktree(&repo, &task(9, TaskStatus::Ready, &["src/x.rs"]), 2, false)?
    );
    assert_eq!(attempt_branch(9, 2), "codex/task-9-attempt-2");
    std::fs::write(worktree.join("untracked.txt"), "keep")?;
    assert!(
        ensure_attempt_worktree(&repo, &task(9, TaskStatus::Ready, &["src/x.rs"]), 2, false)
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("untracked.txt"))?,
        "keep"
    );
    Ok(())
}

#[test]
pub(super) fn failed_retry_records_requeue_before_the_replacement_start() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 1,
            title: "retry",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            scope_paths: &["src/retry.rs".into()],
        },
    )?)?;
    append_started(&ledger, 1, 1, 300)?;
    append_failed(&ledger, 1, "crash")?;

    persist_reconciliation(&repo, &ReconcileConfig::test_defaults())?;

    let kinds: Vec<_> = ledger
        .task_events()?
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(kinds[kinds.len() - 2..], ["task.requeued", "task.started"]);
    assert_eq!(ledger.task_lease(1)?.expect("replacement").attempt, 2);
    Ok(())
}

#[test]
pub(super) fn stale_runner_cannot_append_session_or_failure_for_replacement() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    edda_ledger::Ledger::ensure_initialized(repo)?;
    let ledger = edda_ledger::Ledger::open(repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 7,
            title: "replacement safety",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            scope_paths: &["src/safety.rs".into()],
        },
    )?)?;
    ledger.upsert_task_lease(&TaskLease {
        task_id: 7,
        attempt: 2,
        owner: "new-runner".into(),
        expires_at: "2026-08-16T02:00:00Z".into(),
        heartbeat_at: "2026-08-16T01:00:00Z".into(),
    })?;

    assert!(!record_session_if_current(repo, 7, 1, "old-thread", 300)?);
    finish_runner(
        repo,
        7,
        1,
        Some("old runner"),
        false,
        &ReconcileConfig::test_defaults(),
    )?;

    assert_eq!(ledger.task_lease(7)?.expect("replacement lease").attempt, 2);
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.session"));
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.failed"));
    Ok(())
}

#[test]
pub(super) fn initially_stale_runner_rings_doorbell_without_mutating_replacement(
) -> anyhow::Result<()> {
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    edda_ledger::Ledger::ensure_initialized(repo)?;
    let ledger = edda_ledger::Ledger::open(repo)?;
    create_task(&ledger, 12, &["src/stale.rs".into()])?;
    ledger.upsert_task_lease(&lease(12, 2, "2026-08-16T02:00:00Z"))?;
    DOORBELL_COUNT.store(0, Ordering::SeqCst);

    run_task(repo, 12, 1, &ReconcileConfig::test_defaults(), true)?;

    assert_eq!(ledger.task_lease(12)?.expect("replacement").attempt, 2);
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| { event.event_type != "task.session" && event.event_type != "task.failed" }));
    Ok(())
}

#[test]
pub(super) fn owned_finalization_records_reason_deletes_only_its_lease_and_rings_once(
) -> anyhow::Result<()> {
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    edda_ledger::Ledger::ensure_initialized(repo)?;
    let ledger = edda_ledger::Ledger::open(repo)?;
    create_task(&ledger, 8, &["src/finalize.rs".into()])?;
    ledger.upsert_task_lease(&lease(8, 1, "2026-08-16T02:00:00Z"))?;
    DOORBELL_COUNT.store(0, Ordering::SeqCst);

    finish_runner(
        repo,
        8,
        1,
        Some("runner-failed: test setup"),
        true,
        &ReconcileConfig::test_defaults(),
    )?;

    assert!(ledger.task_lease(8)?.is_none());
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.task_events()?.last().expect("failure event").payload["reason"],
        "runner-failed: test setup"
    );
    Ok(())
}

#[test]
pub(super) fn runner_spawn_failure_is_compensated_without_a_live_lease() -> anyhow::Result<()> {
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    edda_ledger::Ledger::ensure_initialized(repo)?;
    let ledger = edda_ledger::Ledger::open(repo)?;
    create_task(&ledger, 9, &["src/spawn.rs".into()])?;
    ledger.upsert_task_lease(&lease(9, 1, "2026-08-16T02:00:00Z"))?;
    let config = ReconcileConfig::test_defaults();
    let missing = repo.join("missing-runner.exe");

    let error = launch_runner_with(&missing, repo, 9, 1, &config).unwrap_err();
    let reason = format!("runner-spawn-failed: {error:#}");
    DOORBELL_COUNT.store(0, Ordering::SeqCst);
    finish_runner(repo, 9, 1, Some(&reason), true, &config)?;

    assert!(ledger.task_lease(9)?.is_none());
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    assert!(ledger
        .task_events()?
        .iter()
        .any(|event| event.payload["reason"]
            .as_str()
            .unwrap_or_default()
            .starts_with("runner-spawn-failed:")));
    Ok(())
}

#[cfg(windows)]
#[test]
pub(super) fn first_spawn_failure_does_not_prevent_later_plan_launch() -> anyhow::Result<()> {
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    edda_ledger::Ledger::ensure_initialized(repo)?;
    let ledger = edda_ledger::Ledger::open(repo)?;
    create_task(&ledger, 10, &["src/one.rs".into()])?;
    create_task(&ledger, 11, &["src/two.rs".into()])?;
    ledger.upsert_task_lease(&lease(10, 1, "2026-08-16T02:00:00Z"))?;
    ledger.upsert_task_lease(&lease(11, 1, "2026-08-16T02:00:00Z"))?;
    let launched_file = repo.join("later-launch.txt");
    let launcher = repo.join("later-launch.cmd");
    std::fs::write(
        &launcher,
        "@echo off\r\necho launched > \"%~dp0later-launch.txt\"\r\n",
    )?;
    let views = ledger.task_views()?;
    let plans = vec![
        RunnerPlan {
            task: task_view(&views, 10)?.clone(),
            attempt: 1,
            worktree: repo.join("attempt-10"),
        },
        RunnerPlan {
            task: task_view(&views, 11)?.clone(),
            attempt: 1,
            worktree: repo.join("attempt-11"),
        },
    ];
    DOORBELL_COUNT.store(0, Ordering::SeqCst);
    let (launched, errors) = launch_plans_with(
        repo,
        plans,
        &ReconcileConfig::test_defaults(),
        &[repo.join("missing.exe"), launcher],
    );

    assert_eq!(launched.len(), 1);
    assert_eq!(errors.len(), 1);
    // Spawning a .cmd child via cmd.exe routinely exceeds 1 s under full-suite
    // load; wait for the side effect with a realistic deadline instead of a
    // fixed 40 x 25 ms budget (same shape as the GH-524 lock-wait flake).
    let spawn_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !launched_file.exists() {
        anyhow::ensure!(
            std::time::Instant::now() < spawn_deadline,
            "later-launch.cmd side effect did not appear before the spawn deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(launched_file.exists());
    assert!(ledger.task_lease(10)?.is_none());
    assert_eq!(ledger.task_lease(11)?.expect("later lease").attempt, 1);
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    Ok(())
}

#[cfg(windows)]
#[test]
pub(super) fn fake_runner_records_session_in_main_ledger_before_turn_and_fails_without_receipt(
) -> anyhow::Result<()> {
    let _fake = test_lock(&FAKE_CODEX_LOCK);
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 1,
            title: "fake runner",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: Some("brief.md"),
            idempotency_key: None,
            scope_paths: &["src/runner.rs".into()],
        },
    )?)?;
    append_started(&ledger, 1, 1, 300)?;
    ledger.upsert_task_lease(&lease(1, 1, "2026-08-16T02:00:00Z"))?;
    let worktree = ensure_attempt_worktree(&repo, &ledger.task_views()?.remove(0), 1, false)?;
    let fake = fake_codex(dir.path(), 0, false)?;
    let mut config = ReconcileConfig::test_defaults();
    config.codex_bin = fake;

    let challenge = dir.path().join("turn.challenge");
    let allow = dir.path().join("turn.allow");
    let deny = dir.path().join("turn.deny");
    std::env::set_var("EDDA_FAKE_CHALLENGE", &challenge);
    std::env::set_var("EDDA_FAKE_ALLOW", &allow);
    std::env::set_var("EDDA_FAKE_DENY", &deny);
    let observer =
        allow_fake_turn_after_durable_session(repo.clone(), 1, 1, challenge, allow, deny);

    DOORBELL_COUNT.store(0, Ordering::SeqCst);
    let run_result = run_task(&repo, 1, 1, &config, true);
    let observer_result = observer.join();
    std::env::remove_var("EDDA_FAKE_CHALLENGE");
    std::env::remove_var("EDDA_FAKE_ALLOW");
    std::env::remove_var("EDDA_FAKE_DENY");
    run_result?;
    observer_result.expect("observer thread")?;

    assert_eq!(
        edda_ledger::EddaPaths::find_root_bounded(&worktree, dir.path())
            .expect("original ledger root")
            .canonicalize()?,
        repo.canonicalize()?
    );
    assert!(!worktree.join(".edda").exists());
    let events = ledger.task_events()?;
    let session = events
        .iter()
        .position(|event| event.event_type == "task.session")
        .unwrap();
    let failed = events
        .iter()
        .position(|event| event.event_type == "task.failed")
        .unwrap();
    assert!(session < failed);
    assert_eq!(events[session].payload["agent_kind"], "codex");
    assert_eq!(events[session].payload["attempt"], 1);
    assert_eq!(events[failed].payload["reason"], "ended-without-receipt");
    assert!(ledger.task_lease(1)?.is_none());
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    let requests = std::fs::read_to_string(dir.path().join("fake-codex.log"))?;
    assert!(requests.contains("\"method\":\"thread/start\""));
    assert!(requests.contains("\"method\":\"turn/start\""));
    Ok(())
}

#[cfg(windows)]
#[test]
pub(super) fn periodic_renewal_stops_old_runner_before_failure_after_lease_replacement(
) -> anyhow::Result<()> {
    let _fake = test_lock(&FAKE_CODEX_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    ledger.append_event(&edda_core::event::new_task_created_event(
        &edda_core::event::TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id: 1,
            title: "long fake runner",
            assignee: None,
            agent_kind: Some("codex"),
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            scope_paths: &["src/runner.rs".into()],
        },
    )?)?;
    append_started(&ledger, 1, 1, 1)?;
    ledger.upsert_task_lease(&lease(1, 1, "2026-08-16T02:00:00Z"))?;
    let fake = fake_codex(dir.path(), 2, false)?;
    let mut config = ReconcileConfig::test_defaults();
    config.lease_ttl_s = 1;
    config.codex_bin = fake;
    let runner_repo = repo.clone();
    let runner_config = config.clone();
    let runner = std::thread::spawn(move || run_task(&runner_repo, 1, 1, &runner_config, false));

    let session_deadline = std::time::Instant::now() + FAKE_CODEX_STARTUP_BUDGET;
    while std::time::Instant::now() < session_deadline {
        if ledger
            .task_events()?
            .iter()
            .any(|event| event.event_type == "task.session")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(ledger
        .task_events()?
        .iter()
        .any(|event| event.event_type == "task.session"));
    let after_session = ledger.task_lease(1)?.expect("session lease");
    let mut saw_periodic_renewal = false;
    for _ in 0..100 {
        let current = ledger.task_lease(1)?.expect("current lease");
        if current.heartbeat_at != after_session.heartbeat_at
            || current.expires_at != after_session.expires_at
        {
            saw_periodic_renewal = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        saw_periodic_renewal,
        "runner crossed a periodic renewal interval"
    );
    ledger.upsert_task_lease(&TaskLease {
        task_id: 1,
        attempt: 2,
        owner: "replacement".into(),
        expires_at: "2026-08-16T03:00:00Z".into(),
        heartbeat_at: "2026-08-16T01:00:00Z".into(),
    })?;
    runner.join().expect("runner thread")?;

    assert_eq!(ledger.task_lease(1)?.expect("replacement").attempt, 2);
    assert!(ledger
        .task_events()?
        .iter()
        .all(|event| event.event_type != "task.failed"));
    Ok(())
}

#[cfg(windows)]
#[test]
pub(super) fn fake_runner_resumes_current_attempt_after_slow_startup_before_turn(
) -> anyhow::Result<()> {
    let _fake = test_lock(&FAKE_CODEX_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    create_task(&ledger, 2, &["src/resume.rs".into()])?;
    append_started(&ledger, 2, 1, 300)?;
    ledger.upsert_task_lease(&lease(2, 1, "2026-08-16T02:00:00Z"))?;
    assert!(record_session_if_current(&repo, 2, 1, "saved-thread", 300)?);
    let fake = fake_codex(dir.path(), 0, false)?;
    let mut config = ReconcileConfig::test_defaults();
    config.codex_bin = fake;

    let challenge = dir.path().join("resume.challenge");
    let allow = dir.path().join("resume.allow");
    let deny = dir.path().join("resume.deny");
    std::env::set_var("EDDA_FAKE_CHALLENGE", &challenge);
    std::env::set_var("EDDA_FAKE_ALLOW", &allow);
    std::env::set_var("EDDA_FAKE_DENY", &deny);
    let observer =
        allow_fake_turn_after_durable_session(repo.clone(), 2, 1, challenge, allow, deny);

    std::thread::sleep(std::time::Duration::from_millis(2_100));

    let run_result = run_task(&repo, 2, 1, &config, false);
    let observer_result = observer.join();
    std::env::remove_var("EDDA_FAKE_CHALLENGE");
    std::env::remove_var("EDDA_FAKE_ALLOW");
    std::env::remove_var("EDDA_FAKE_DENY");
    run_result?;
    observer_result.expect("observer thread")?;

    let requests = std::fs::read_to_string(dir.path().join("fake-codex.log"))?;
    assert!(requests.contains("\"method\":\"thread/resume\""));
    assert!(requests.contains("\"method\":\"turn/start\""));
    Ok(())
}

#[cfg(windows)]
#[test]
pub(super) fn fake_permission_request_is_rejected_then_finalized_once() -> anyhow::Result<()> {
    let _fake = test_lock(&FAKE_CODEX_LOCK);
    let _doorbell = test_lock(&DOORBELL_LOCK);
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo)?;
    init_git(&repo)?;
    edda_ledger::Ledger::ensure_initialized(&repo)?;
    let ledger = edda_ledger::Ledger::open(&repo)?;
    create_task(&ledger, 5, &["src/permission.rs".into()])?;
    append_started(&ledger, 5, 1, 300)?;
    ledger.upsert_task_lease(&lease(5, 1, "2026-08-16T02:00:00Z"))?;
    let mut config = ReconcileConfig::test_defaults();
    config.codex_bin = fake_codex(dir.path(), 0, true)?;
    DOORBELL_COUNT.store(0, Ordering::SeqCst);

    let error = run_task(&repo, 5, 1, &config, true).expect_err("permission must fail");

    assert!(error.to_string().contains("runner-failed"));
    assert!(ledger.task_lease(5)?.is_none());
    assert_eq!(DOORBELL_COUNT.load(Ordering::SeqCst), 1);
    let failed = ledger
        .task_events()?
        .into_iter()
        .find(|event| event.event_type == "task.failed")
        .expect("permission failure event");
    assert!(failed.payload["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("requestApproval"));
    Ok(())
}

#[cfg(windows)]
pub(super) fn fake_codex(dir: &Path, delay_s: u64, permission: bool) -> anyhow::Result<PathBuf> {
    let script = dir.join("fake-codex.ps1");
    let log = dir.join("fake-codex.log");
    std::fs::write(
            &script,
            r#"$ErrorActionPreference = 'Stop'
function Read-Line { $line = [Console]::In.ReadLine(); Add-Content -LiteralPath 'LOGFILE' -Value $line }
function Write-Line([string]$line) { [Console]::Out.WriteLine($line); [Console]::Out.Flush() }
Read-Line
Write-Line '{"id":1,"result":{}}'
Read-Line
Read-Line
Write-Line '{"id":2,"result":{"thread":{"id":"fake-thread"}}}'
Read-Line
if ($env:EDDA_FAKE_CHALLENGE) {
  New-Item -ItemType File -Force -Path $env:EDDA_FAKE_CHALLENGE | Out-Null
  $startupDeadline = [System.DateTime]::UtcNow.AddMilliseconds(STARTUP_BUDGET_MS)
  while ([System.DateTime]::UtcNow -lt $startupDeadline) {
    if (Test-Path $env:EDDA_FAKE_ALLOW) { break }
    if (Test-Path $env:EDDA_FAKE_DENY) { exit 7 }
    Start-Sleep -Milliseconds 10
  }
  if (-not (Test-Path $env:EDDA_FAKE_ALLOW)) { exit 7 }
}
Start-Sleep -Seconds DELAY
EVENTS
Write-Line '{"id":3,"result":{"turn":{"id":"fake-turn"}}}'
"#
            .replace("DELAY", &delay_s.to_string())
            .replace(
                "STARTUP_BUDGET_MS",
                &FAKE_CODEX_STARTUP_BUDGET.as_millis().to_string(),
            )
            .replace("LOGFILE", &log.to_string_lossy().replace("'", "''"))
            .replace(
                "EVENTS",
                if permission {
                    "Write-Line '{\"id\":\"approval-1\",\"method\":\"item/commandExecution/requestApproval\",\"params\":{}}'"
                } else {
                    "Write-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"fake-thread\",\"turnId\":\"fake-turn\",\"item\":{\"type\":\"agentMessage\",\"text\":\"prose only\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"fake-thread\",\"turn\":{\"id\":\"fake-turn\",\"status\":\"completed\"}}}'"
                },
            ),
        )?;
    let launcher = dir.join("fake-codex.cmd");
    std::fs::write(
            &launcher,
            "@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0fake-codex.ps1\"\r\n",
        )?;
    Ok(launcher)
}
