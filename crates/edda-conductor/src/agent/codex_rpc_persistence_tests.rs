    // ── Cross-process thread-map persistence (GH-535) ──

    /// The map file a launcher with `root` writes for `cwd`.
    fn map_path_for(root: &Path, cwd: &Path) -> PathBuf {
        root.join("projects")
            .join(edda_store::project_id(cwd))
            .join("state")
            .join("codex-threads.json")
    }

    #[test]
    fn thread_store_round_trips_the_session_map() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore::from_root(root.path().to_path_buf());
        let mut threads = HashMap::new();
        threads.insert("sess-1".to_owned(), "t-1".to_owned());

        store.persist(cwd.path(), &threads, &HashSet::new(), true)?;

        let loaded = store.load(cwd.path())?;
        assert_eq!(loaded.get("sess-1").map(String::as_str), Some("t-1"));
        Ok(())
    }

    #[test]
    fn thread_store_persist_merges_with_entries_from_another_process() -> Result<()> {
        // Simulate the other process having written its own binding to the
        // shared map between our load and our write: the merge must keep it.
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore::from_root(root.path().to_path_buf());
        let foreign = r#"{"sess-other":"t-other","sess-mine":"t-stale"}"#;
        std::fs::create_dir_all(map_path_for(root.path(), cwd.path()).parent().unwrap())?;
        std::fs::write(map_path_for(root.path(), cwd.path()), foreign)?;

        let mut threads = HashMap::new();
        threads.insert("sess-mine".to_owned(), "t-fresh".to_owned());
        store.persist(cwd.path(), &threads, &HashSet::new(), true)?;

        let loaded = store.load(cwd.path())?;
        assert_eq!(
            loaded.get("sess-other").map(String::as_str),
            Some("t-other")
        );
        assert_eq!(loaded.get("sess-mine").map(String::as_str), Some("t-fresh"));
        Ok(())
    }

    #[test]
    fn thread_store_persist_honors_removal_tombstones() -> Result<()> {
        // A tombstone removes the disk binding even without a replacement.
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore::from_root(root.path().to_path_buf());
        let map = map_path_for(root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, r#"{"sess-1":"stale","sess-other":"t-other"}"#)?;

        let mut threads = HashMap::new();
        threads.insert("sess-2".to_owned(), "t-2".to_owned());
        let mut removals = HashSet::new();
        removals.insert("sess-1".to_owned());
        store.persist(cwd.path(), &threads, &removals, true)?;

        let loaded = store.load(cwd.path())?;
        assert_eq!(
            loaded.get("sess-other").map(String::as_str),
            Some("t-other")
        );
        assert_eq!(loaded.get("sess-2").map(String::as_str), Some("t-2"));
        assert!(
            !loaded.contains_key("sess-1"),
            "the tombstoned binding must be deleted from disk, got {loaded:?}"
        );
        Ok(())
    }

    #[test]
    fn thread_store_distinguishes_missing_from_corrupt() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore::from_root(root.path().to_path_buf());
        assert!(store.load(cwd.path())?.is_empty(), "missing file is empty");

        let map = map_path_for(root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, b"{ not json")?;
        let error = store
            .load(cwd.path())
            .expect_err("corrupt map must surface");
        assert!(
            error.to_string().contains("parse codex thread map"),
            "{error:#}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn fresh_launcher_resumes_the_thread_a_previous_process_recorded() -> Result<()> {
        // Two launcher processes share one persisted session binding.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");

        let (_fake_dir, first_bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("first fake written");
        let first =
            CodexLauncher::with_bin(first_bin).with_thread_store(store_root.path().to_path_buf());
        let first_result = first
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("first dispatch runs");
        assert!(
            matches!(&first_result, PhaseResult::AgentDone { result_text, .. } if result_text.as_deref() == Some("turn complete")),
            "first dispatch should complete, got {first_result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(map_path_for(store_root.path(), cwd.path()))
                .expect("map persisted"),
            r#"{"sess-1":"t-1"}"#
        );

        let (_fake_dir, second_bin) =
            fake_app_server_bin(FakeScenario::ResumeOnly).expect("second fake written");
        let second =
            CodexLauncher::with_bin(second_bin).with_thread_store(store_root.path().to_path_buf());
        let second_result = second
            .run_phase(
                &phase,
                "turn two",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("second dispatch runs");
        match second_result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("resumed answer"));
            }
            other => panic!("second dispatch should resume the recorded thread, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn required_first_round_persists_before_returning_agent_done() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let (_fake_dir, bin) = fake_app_server_bin(FakeScenario::RunTurnCompletes)?;
        let launcher = CodexLauncher::with_bin(bin)
            .with_thread_store(store_root.path().to_path_buf())
            .with_required_persistence();
        let result = launcher
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "first review",
                "",
                "reviewer-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await?;
        assert!(matches!(result, PhaseResult::AgentDone { .. }));
        assert_eq!(
            std::fs::read_to_string(map_path_for(store_root.path(), cwd.path()))?,
            r#"{"reviewer-1":"t-1"}"#
        );
        Ok(())
    }

    #[tokio::test]
    async fn required_first_round_reports_final_persist_failure() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let (_fake_dir, bin) = fake_app_server_bin(FakeScenario::RunTurnCompletes)?;
        let launcher = CodexLauncher::with_bin(bin)
            .with_thread_store(store_root.path().to_path_buf())
            .with_required_persistence()
            .with_store_failure(StoreFailure::Persist);
        let result = launcher
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "first review",
                "",
                "reviewer-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error }
                if error.contains(REQUIRED_PERSISTENCE_ERROR_PREFIX)
                    && error.contains("injected codex thread map persist failure")),
            "required persist failure must replace AgentDone, got {result:?}"
        );
        assert!(
            !map_path_for(store_root.path(), cwd.path()).exists(),
            "failed persistence must not claim a durable mapping"
        );
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_required_store_refuses_before_app_server_launch() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().expect("map parent"))?;
        std::fs::write(&map, b"{ corrupt")?;
        let launcher = CodexLauncher::with_bin(PathBuf::from("must-not-be-spawned"))
            .with_thread_store(store_root.path().to_path_buf())
            .with_required_persistence();
        let result = launcher
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "first review",
                "",
                "reviewer-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error }
                if error.contains(REQUIRED_PERSISTENCE_ERROR_PREFIX)
                    && error.contains("parse codex thread map")),
            "corrupt required store must win over spawn, got {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unreadable_required_store_refuses_before_app_server_launch() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let launcher = CodexLauncher::with_bin(PathBuf::from("must-not-be-spawned"))
            .with_thread_store(store_root.path().to_path_buf())
            .with_required_persistence()
            .with_store_failure(StoreFailure::Load);
        let result = launcher
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "first review",
                "",
                "reviewer-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error }
                if error.contains(REQUIRED_PERSISTENCE_ERROR_PREFIX)
                    && error.contains("injected codex thread map read failure")),
            "unreadable required store must win over spawn, got {result:?}"
        );
        Ok(())
    }

    async fn run_strict_fake(
        scenario: FakeScenario,
        root: &Path,
        cwd: &Path,
    ) -> Result<PhaseResult> {
        let (_fake_dir, bin) = fake_app_server_bin(scenario)?;
        CodexLauncher::with_bin(bin)
            .with_thread_store(root.to_path_buf())
            .with_required_persistence()
            .with_required_thread()
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "resume review",
                "",
                "reviewer-1",
                cwd,
                CancellationToken::new(),
            )
            .await
    }

    #[tokio::test]
    async fn strict_resume_requires_real_mapping_and_never_falls_back() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        let missing = run_strict_fake(
            FakeScenario::RunTurnCompletes,
            store_root.path(),
            cwd.path(),
        )
        .await?;
        assert!(
            matches!(&missing, PhaseResult::AgentCrash { error } if error.contains("requires an existing persisted Codex thread mapping"))
        );
        assert!(!map.exists(), "strict refusal manufactured a binding");

        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, br#"{"reviewer-1":"t-1"}"#)?;
        let resumed =
            run_strict_fake(FakeScenario::ResumeOnly, store_root.path(), cwd.path()).await?;
        assert!(
            matches!(&resumed, PhaseResult::AgentDone { result_text, .. } if result_text.as_deref() == Some("resumed answer"))
        );

        std::fs::write(&map, br#"{"reviewer-1":"stale-thread"}"#)?;
        let rejected = run_strict_fake(
            FakeScenario::ResumeErrorThenStart,
            store_root.path(),
            cwd.path(),
        )
        .await?;
        assert!(
            matches!(&rejected, PhaseResult::AgentCrash { error } if error.contains("unknown thread")),
            "strict rejection reached fresh fallback: {rejected:?}"
        );
        let loaded: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&map)?)?;
        assert!(
            !loaded.contains_key("reviewer-1"),
            "stale binding survived: {loaded:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn strict_rejection_reports_tombstone_persist_failure_too() -> Result<()> {
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().expect("map parent"))?;
        std::fs::write(&map, br#"{"reviewer-1":"stale-thread"}"#)?;
        let (_fake_dir, bin) = fake_app_server_bin(FakeScenario::ResumeErrorThenStart)?;
        let launcher = CodexLauncher::with_bin(bin)
            .with_thread_store(store_root.path().to_path_buf())
            .with_required_persistence()
            .with_required_thread()
            .with_store_failure(StoreFailure::Persist);
        let result = launcher
            .run_phase(
                &phase_from_yaml("  - id: a\n    prompt: x\n"),
                "resume review",
                "",
                "reviewer-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error }
                if error.contains(REQUIRED_PERSISTENCE_ERROR_PREFIX)
                    && error.contains("injected codex thread map persist failure")
                    && error.contains("unknown thread")),
            "must report rejection and failed tombstone, got {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&map)?,
            r#"{"reviewer-1":"stale-thread"}"#,
            "failed tombstone write cannot claim the stale binding was removed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_store_entry_degrades_to_thread_start() -> Result<()> {
        // A corrupt map must never fail the dispatch: the fresh launcher
        // warns and falls back to thread/start, which completes normally.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, b"{ corrupted")?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("dispatch must not fail on a corrupt store entry");
        assert!(
            matches!(&result, PhaseResult::AgentDone { result_text, .. } if result_text.as_deref() == Some("turn complete")),
            "degraded dispatch should complete via thread/start, got {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_persisted_binding_degrades_to_thread_start_and_is_not_rewritten() -> Result<()> {
        // Ordinary dispatch replaces a stale binding with its fresh fallback.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, br#"{"sess-1":"stale-thread"}"#)?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::ResumeErrorThenStart).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("a stale binding must degrade, not fail the dispatch");
        match result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("fresh answer"));
            }
            other => panic!(
                "stale binding should degrade to thread/start in the same dispatch, got {other:?}"
            ),
        }
        assert_eq!(
            std::fs::read_to_string(&map).expect("map rewritten"),
            r#"{"sess-1":"t-1"}"#,
            "the stale binding must not be written back"
        );
        Ok(())
    }

    /// Seed the in-memory session→thread map of a launcher built by
    /// [`launcher_with_server`], without touching the real store.
    async fn seed_threads(launcher: &CodexLauncher, session: &str, thread: &str) {
        let mut state = launcher.state.lock().await;
        state.threads.insert(session.to_owned(), thread.to_owned());
    }

    #[tokio::test]
    async fn conduct_without_persistence_still_crashes_when_resume_is_rejected() -> Result<()> {
        // Non-persistent conduct surfaces rejection instead of fresh fallback.
        let (_dir, server) = spawn_fake_server(FakeScenario::ResumeErrorThenStart).await;
        let launcher = launcher_with_server(server);
        seed_threads(&launcher, "sid", "stale-thread").await;
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "redispatch turn",
                "",
                "sid",
                Path::new("."),
                CancellationToken::new(),
            )
            .await
            .expect("run_phase returns a result, not an IO error");
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(
                    error.contains("unknown thread"),
                    "conduct resume failure must surface the server's own error, got {error}"
                );
            }
            other => {
                panic!("non-persistent conduct must not fall back to thread/start, got {other:?}")
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_fallback_still_erases_the_stale_binding_from_disk() -> Result<()> {
        // Failed ordinary fallback still persists the rejection tombstone.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, br#"{"sess-1":"stale-thread"}"#)?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::ResumeErrorThenStartError).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("run_phase returns a result, not an IO error");
        // The fallback thread/start fails too, so the phase ends in a
        // crash — but the rejected binding must still be gone from disk.
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error } if error.contains("unknown thread")),
            "expected the failed fallback to surface as AgentCrash, got {result:?}"
        );
        let loaded: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&map).expect("map still readable"))
                .expect("map stays valid JSON");
        assert!(
            !loaded.contains_key("sess-1"),
            "the rejected binding must not survive on disk after a failed fallback, got {loaded:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_store_entry_starts_a_fresh_thread() -> Result<()> {
        // No map file at all: the first dispatch with a session id is a
        // plain thread/start, not a failure and not a warning.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sess-fresh",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("first dispatch runs");
        assert!(matches!(result, PhaseResult::AgentDone { .. }));
        Ok(())
    }
