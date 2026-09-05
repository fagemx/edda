use super::*;
use edda_conductor::plan::schema::Phase;
use git::testrepo;
use std::sync::Mutex;

struct Reviewer {
    answer: &'static str,
    session: Mutex<Option<String>>,
    cost: Option<f64>,
}

#[async_trait::async_trait]
impl AgentLauncher for Reviewer {
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        _: &str,
        session: &str,
        cwd: &Path,
        _: CancellationToken,
    ) -> Result<PhaseResult> {
        assert_eq!(phase.tools, tools(AgentKind::Pi));
        assert!(!phase
            .tools
            .as_ref()
            .unwrap()
            .iter()
            .any(|v| matches!(v.as_str(), "bash" | "powershell" | "write" | "edit")));
        assert!(prompt.ends_with(brief::OUTPUT_CONTRACT_V1));
        let head = std::fs::read_to_string(cwd.join(git::SUBJECT_MARKER))?;
        assert_eq!(git::commit(cwd, "HEAD")?, head);
        *self.session.lock().unwrap() = Some(session.into());
        match self.answer {
            "mutate-content" => std::fs::write(cwd.join("b.txt"), "tampered\n")?,
            "mutate-head" => {
                testrepo::commit_file(cwd, "engine.txt", "tampered\n", "engine mutation");
            }
            "mutate-remove" => std::fs::remove_file(cwd.join("b.txt"))?,
            _ => {}
        }
        if self.answer == "crash" {
            return Ok(PhaseResult::AgentCrash {
                error: "provider unavailable".into(),
            });
        }
        if self.answer == "malformed" {
            return Ok(PhaseResult::AgentDone {
                cost_usd: self.cost,
                result_text: Some("LGTM".into()),
            });
        }
        let verdict = if self.answer.starts_with("mutate-") {
            "lgtm"
        } else {
            self.answer
        };
        let findings = if verdict == "changes-requested" {
            serde_json::json!([{"severity":"P1","file":"b.txt","line":1,"claim":"fixture finding","evidence":"b.txt:1","rule":"core"}])
        } else {
            serde_json::json!([])
        };
        let value = serde_json::json!({"subject_seen":head, "verdict":verdict,"findings":findings,"checklist":[{"item":"changed file","result":"na","measure":"read b.txt; no execution claimed"}],"escalations":[],"model_self_report":"untrusted-name","notes":""});
        Ok(PhaseResult::AgentDone {
            cost_usd: self.cost,
            result_text: Some(format!("```edda-review-verdict/v1\n{value}\n```")),
        })
    }
    fn last_observed_model(&self) -> Option<String> {
        Some("openai-codex/gpt-5.6-sol".into())
    }
    fn last_observed_session(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
}

fn fixture(qualified: bool) -> (tempfile::TempDir, std::path::PathBuf, ReviewArgs) {
    let (temp, root) = testrepo::init();
    testrepo::run(&root, &["checkout", "-qb", "feature"]);
    let head = testrepo::commit_file(&root, "b.txt", "change\n", "feature change");
    let ledger = edda_ledger::Ledger::open_or_init(&root).unwrap();
    let mut args = ReviewArgs::default();
    if qualified {
        std::fs::write(root.join("acceptance.txt"), "Review b.txt correctness").unwrap();
        args.spec = Some("acceptance.txt".into());
        args.gates = vec!["cargo test -p fixture".into()];
        let argv = ["cargo", "test", "-p", "fixture"].map(str::to_owned);
        let event = edda_core::event::new_cmd_event_with_git_context(
            &edda_core::event::CmdEventParams {
                branch: "main",
                parent_hash: ledger.last_event_hash().unwrap().as_deref(),
                argv: &argv,
                cwd: root.to_str().unwrap(),
                exit_code: 0,
                duration_ms: 1,
                stdout_blob: "",
                stderr_blob: "",
            },
            Some(&head),
            Some(false),
        )
        .unwrap();
        ledger.append_event(&event).unwrap();
    }
    (temp, root, args)
}

#[tokio::test]
async fn end_to_end_four_exit_codes_and_author_ledger() {
    for (answer, qualified, code) in [
        ("lgtm", true, 0),
        ("changes-requested", true, 1),
        ("malformed", true, 2),
        ("lgtm", false, 3),
        ("crash", true, 2),
    ] {
        let (_temp, root, args) = fixture(qualified);
        let reviewer = Reviewer {
            answer,
            session: Mutex::new(None),
            cost: None,
        };
        let prepared = prepare::prepare(&args, &root).unwrap();
        let (payload, event) = run_with(prepared, &args, &reviewer).await.unwrap();
        assert_eq!(
            verdict::exit_code(&payload),
            code,
            "{answer}: {:?}",
            payload.notes
        );
        assert_eq!(payload.cost.usd, None);
        assert!(!payload.cost.measured);
        assert!(render::render(&payload, &event).contains("unmeasured"));
        let ledger = edda_ledger::Ledger::open(&root).unwrap();
        let saved = ledger.get_event(&event).unwrap().unwrap();
        assert_eq!(saved.event_type, "review_verdict");
        assert_eq!(
            saved.payload["subject"]["head_sha"],
            git::commit(&root, "HEAD").unwrap()
        );
        assert_eq!(
            testrepo::run(&root, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn resume_reuses_ledger_session_and_increments_round() {
    let (_temp, root, mut args) = fixture(true);
    let reviewer = Reviewer {
        answer: "lgtm",
        session: Mutex::new(None),
        cost: Some(0.12),
    };
    let (first, _) = run_with(prepare::prepare(&args, &root).unwrap(), &args, &reviewer)
        .await
        .unwrap();
    args.resume = true;
    let (second, _) = run_with(prepare::prepare(&args, &root).unwrap(), &args, &reviewer)
        .await
        .unwrap();
    assert_eq!(first.reviewer.session_id, second.reviewer.session_id);
    assert_eq!(second.refs.round, Some(2));
    assert_eq!(second.cost.usd, Some(0.12));
    assert!(second.qualified);
}

#[tokio::test]
async fn same_head_prior_p1_survives_later_lgtm() {
    let (_temp, root, mut args) = fixture(true);
    let rejected = Reviewer {
        answer: "changes-requested",
        session: Mutex::new(None),
        cost: Some(0.12),
    };
    let first = run_with(prepare::prepare(&args, &root).unwrap(), &args, &rejected)
        .await
        .unwrap()
        .0;
    assert_eq!(first.verdict, "changes-requested");
    args.resume = true;
    let approving = Reviewer {
        answer: "lgtm",
        session: Mutex::new(None),
        cost: Some(0.12),
    };
    let second = run_with(prepare::prepare(&args, &root).unwrap(), &args, &approving)
        .await
        .unwrap()
        .0;
    assert!(second
        .findings
        .iter()
        .any(|finding| finding.severity == "P1"));
    assert_eq!(second.verdict, "changes-requested");
    assert_ne!(verdict::exit_code(&second), 0);
}

#[test]
fn empty_diff_and_author_session_refuse_before_launch_or_event() {
    let (_temp, root) = testrepo::init();
    edda_ledger::Ledger::open_or_init(&root).unwrap();
    assert!(prepare::prepare(&ReviewArgs::default(), &root).is_err());
    testrepo::run(&root, &["checkout", "-qb", "feature"]);
    testrepo::commit_file(&root, "b.txt", "b", "known author commit");
    let ledger = edda_ledger::Ledger::open(&root).unwrap();
    let author = "00000000-0000-4000-8000-000000000001";
    let mut event = edda_core::event::new_note_event(
        "main",
        ledger.last_event_hash().unwrap().as_deref(),
        "system",
        "digest",
        &[],
    )
    .unwrap();
    event.payload["source"] = serde_json::json!("bridge:session_digest");
    event.payload["session_id"] = serde_json::json!(author);
    event.payload["session_stats"] =
        serde_json::json!({"commits_made":["known author commit"],"model":"gpt-5.6-sol"});
    // The digest is deliberately authored as a valid ledger event.  Mutating
    // a finalized note without re-finalizing would test the hash-chain guard,
    // not the reviewer/author independence refusal below.
    edda_core::event::finalize_event(&mut event).unwrap();
    ledger.append_event(&event).unwrap();
    let args = ReviewArgs {
        session_id: Some(author.into()),
        ..Default::default()
    };
    let error = prepare::prepare(&args, &root).err().unwrap();
    assert!(error.to_string().contains("same session"));
    assert!(ledger
        .iter_events_by_type("review_verdict")
        .unwrap()
        .is_empty());
}

#[test]
fn review_refuses_acp_agents_without_an_enforced_tool_allowlist() {
    for agent in [
        AgentKind::AcpGrok,
        AgentKind::AcpKilo,
        AgentKind::AcpPi,
        AgentKind::AcpClaude,
    ] {
        let args = ReviewArgs {
            agent,
            ..Default::default()
        };
        let error = validate(&args)
            .err()
            .unwrap_or_else(|| panic!("{agent:?} review must be refused"));
        let text = error.to_string();
        assert!(text.contains(agent.as_str()), "{text}");
        assert!(text.contains("unrestricted reviewer"), "{text}");
    }
}

#[tokio::test]
async fn default_review_never_executes_declared_gate() {
    let (_temp, root, mut args) = fixture(false);
    let sentinel = root.join("unexpected");
    args.gates = vec![format!("echo executed > '{}'", sentinel.display())];
    let reviewer = Reviewer {
        answer: "lgtm",
        session: Mutex::new(None),
        cost: Some(0.01),
    };
    let (payload, _) = run_with(prepare::prepare(&args, &root).unwrap(), &args, &reviewer)
        .await
        .unwrap();
    assert!(!sentinel.exists());
    assert!(payload.gates.ran.is_empty());
}

#[tokio::test]
async fn proof_failures_are_unreviewed_unqualified_and_do_not_consume_rounds() {
    for (answer, expected_outcome) in [
        ("mutate-content", "worktree-changed"),
        ("mutate-head", "worktree-changed"),
        ("mutate-remove", "worktree-check-failed"),
    ] {
        let (_temp, root, args) = fixture(true);
        let reviewer = Reviewer {
            answer,
            session: Mutex::new(None),
            cost: Some(0.01),
        };
        let (payload, _) = run_with(prepare::prepare(&args, &root).unwrap(), &args, &reviewer)
            .await
            .unwrap();
        assert_eq!(payload.subject.worktree_check.as_deref(), Some("failed"));
        assert_eq!(payload.verdict, "unreviewed");
        assert_eq!(payload.outcome, expected_outcome);
        assert_eq!(payload.refs.round, None);
        assert!(!payload.qualified);
        assert!(payload
            .disqualifiers
            .iter()
            .any(|reason| reason == "worktree-check-not-unchanged"));
    }
}

#[tokio::test]
async fn mutating_ran_gate_persists_unreviewed_proof_failure_before_engine_launch() {
    let (_temp, root, mut args) = fixture(true);
    args.run_gates = true;
    args.gates = vec!["printf tampered > b.txt".into()];
    let reviewer = Reviewer {
        answer: "lgtm",
        session: Mutex::new(None),
        cost: Some(0.01),
    };
    let (payload, _) = run_with(prepare::prepare(&args, &root).unwrap(), &args, &reviewer)
        .await
        .unwrap();
    assert_eq!(payload.subject.worktree_check.as_deref(), Some("failed"));
    assert_eq!(payload.verdict, "unreviewed");
    assert_eq!(payload.refs.round, None);
    assert!(!payload.qualified);
    assert!(payload
        .notes
        .as_deref()
        .is_some_and(|notes| notes.contains("review evidence changed")));
}
