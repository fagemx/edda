//! Cross-vendor review: the host owns evidence and policy, the engine judges.
mod args;
mod brief;
mod evidence;
mod git;
mod github;
mod identity;
mod prepare;
mod render;
mod subject;
#[cfg(test)]
mod tests;
mod verdict;

use crate::agent_kind::{
    build_launcher, validate_dispatch_options, AgentKind, DispatchOptions, LauncherOptions,
};
use crate::cmd_dispatch::{build_phase, CapabilityOptions};
use anyhow::{bail, Result};
pub use args::ReviewArgs;
use edda_conductor::agent::launcher::{AgentLauncher, PhaseResult};
use edda_core::{
    ReviewBrief, ReviewCost, ReviewFinding, ReviewReviewer, ReviewSubject, ReviewVerdictPayload,
};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn run(args: ReviewArgs, cwd: &Path) -> Result<()> {
    let result = run_inner(&args, cwd);
    match result {
        Ok((payload, event_id)) => {
            if args.json {
                let mut value = serde_json::to_value(&payload)?;
                value["event_id"] = serde_json::json!(event_id);
                println!("{value}");
            } else {
                print!("{}", render::render(&payload, &event_id));
            }
            let code = verdict::exit_code(&payload);
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Err(error) => {
            eprintln!("edda review: {error:#}");
            std::process::exit(2);
        }
    }
}

fn run_inner(args: &ReviewArgs, cwd: &Path) -> Result<(ReviewVerdictPayload, String)> {
    // Empty diff and same-author refusal happen before launcher probing/spawn.
    let prepared = prepare::prepare(args, cwd)?;
    validate(args)?;
    if let Some(warning) =
        crate::cmd_conduct::budget_warning_for_agent(args.agent, args.budget_usd.is_some())
    {
        eprintln!("{warning}");
    }
    let session_dir = (args.agent == AgentKind::Pi).then(|| review_session_dir(&prepared));
    if args.resume && args.agent == AgentKind::Pi {
        let continued = session_dir.as_ref().is_some_and(|dir| {
            pi_session_continues(dir, &prepared.session, &review_scratch(&prepared))
        });
        anyhow::ensure!(continued, "--resume requires the persisted Pi conversation; refusing a new session with the old UUID");
    }
    let launcher = build_launcher(
        args.agent,
        LauncherOptions {
            verbose: false,
            transcript_dir: None,
            persistent_codex_threads: args.resume,
            session_dir,
            resume: args.resume && args.agent == AgentKind::Claude,
        },
    )?;
    tokio::runtime::Runtime::new()?.block_on(run_with(prepared, args, launcher.as_ref()))
}

fn pi_session_continues(dir: &std::path::Path, session: &str, cwd: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let Some(header) = lines.next() else {
            return false;
        };
        let Ok(header) = serde_json::from_str::<serde_json::Value>(header) else {
            return false;
        };
        header["type"] == "session"
            && header["id"] == session
            && header["cwd"]
                .as_str()
                .is_some_and(|stored| std::path::Path::new(stored) == cwd)
            // Pi resumes context from `message` entries carrying a message
            // payload, or from a compaction summary. Metadata such as model
            // changes changes runtime settings but produces no context.
            && lines
                .map(serde_json::from_str::<serde_json::Value>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .is_ok_and(|entries| entries.iter().any(pi_entry_has_context))
    })
}

fn pi_entry_has_context(entry: &serde_json::Value) -> bool {
    match entry["type"].as_str() {
        Some("message") => entry["message"].as_object().is_some_and(|message| {
            message
                .get("role")
                .is_some_and(serde_json::Value::is_string)
                && message.contains_key("content")
        }),
        Some("compaction") | Some("branch_summary") => entry["summary"]
            .as_str()
            .is_some_and(|summary| !summary.is_empty()),
        _ => false,
    }
}

fn review_session_dir(prepared: &prepare::Prepared) -> std::path::PathBuf {
    // pi keys its session lookup by working directory.  Keep both that
    // directory and its explicit session store stable for a reviewer UUID.
    // The UUID is validated before this path is formed.
    std::env::temp_dir()
        .join("edda-review-sessions")
        .join(edda_store::project_id(&prepared.repo))
        .join(&prepared.session)
}

fn review_scratch(prepared: &prepare::Prepared) -> std::path::PathBuf {
    // WorktreeGuard only ever removes a worktree it added itself.  If this
    // stable path already exists, it could be a concurrent review and is
    // refused rather than reclaimed.
    std::env::temp_dir()
        .join("edda-review")
        .join(edda_store::project_id(&prepared.repo))
        .join(&prepared.session)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // Resume preflight is adjacent to its persisted-history regression.
mod pi_resume_tests {
    use super::pi_session_continues;

    #[test]
    fn persisted_pi_history_must_match_session_cwd_and_have_resumable_context() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("scratch");
        std::fs::create_dir_all(&cwd).unwrap();
        let session = "00000000-0000-4000-8000-000000000001";

        let header =
            |id: &str| serde_json::json!({"type":"session", "id": id, "cwd": cwd}).to_string();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!("{}\n", header(session)),
        )
        .unwrap();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("unrelated.jsonl"),
            format!(
                "{}\n{{\"type\":\"message\"}}\n",
                header("00000000-0000-4000-8000-000000000002")
            ),
        )
        .unwrap();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!(
                "{}\n{{\"type\":\"model_change\",\"provider\":\"x\"}}\n",
                header(session)
            ),
        )
        .unwrap();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!("{}\n{{\"type\":\"message\"}}\n", header(session)),
        )
        .unwrap();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!(
                "{}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"review\"}}]}}}}\n",
                header(session)
            ),
        )
        .unwrap();
        assert!(pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!(
                "{}\n{{\"type\":\"compaction\",\"summary\":\"prior review\"}}\n",
                header(session)
            ),
        )
        .unwrap();
        assert!(pi_session_continues(temp.path(), session, &cwd));
        std::fs::write(
            temp.path().join("empty.jsonl"),
            format!(
                "{}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":[]}}}}\nnot-json\n",
                header(session)
            ),
        )
        .unwrap();
        assert!(!pi_session_continues(temp.path(), session, &cwd));
    }
}

fn tools(agent: AgentKind) -> Option<Vec<String>> {
    match agent {
        AgentKind::Pi => Some(["read", "grep", "find", "ls"].map(str::to_owned).into()),
        AgentKind::Claude => Some(["Read", "Grep", "Glob"].map(str::to_owned).into()),
        AgentKind::Codex => None,
        AgentKind::AcpGrok | AgentKind::AcpKilo | AgentKind::AcpPi | AgentKind::AcpClaude => {
            unreachable!("ACP agents are refused by validate() before tool selection")
        }
    }
}

fn validate(args: &ReviewArgs) -> Result<()> {
    if args.agent.is_acp() {
        bail!(
            "agent \"{}\" does not support review dispatch: no enforced tool allowlist \
             exists over ACP and edda review never dispatches an unrestricted reviewer",
            args.agent.as_str()
        );
    }
    let allow = tools(args.agent);
    validate_dispatch_options(
        args.agent,
        &DispatchOptions {
            model: args.model.as_deref(),
            thinking: args.thinking.as_deref(),
            tools: allow.as_deref(),
            resume: args.resume && args.agent == AgentKind::Claude,
            ..Default::default()
        },
    )
}

#[allow(clippy::too_many_lines)] // Orchestrates the single review lifecycle and its fail-closed receipts.
async fn run_with(
    mut prepared: prepare::Prepared,
    args: &ReviewArgs,
    launcher: &dyn AgentLauncher,
) -> Result<(ReviewVerdictPayload, String)> {
    validate(args)?;
    let scratch = review_scratch(&prepared);
    let mut worktree = git::WorktreeGuard::create(
        &prepared.repo,
        &scratch,
        &prepared.subject.head_sha,
        args.keep_worktree,
    )?;
    let (gates, probes, evidence_text) =
        prepare::collect_evidence(&mut prepared, args, &worktree.path)?;
    let checklist_measures = evidence::checklist_measures(&gates.ran, &probes);
    let (assembled, classes) = prepare::assemble(&prepared, &evidence_text)?;
    match worktree.verify_unchanged(&prepared.subject.head_sha) {
        Ok(true) => {}
        Ok(false) => {
            return persist_prelaunch_worktree_failure(
                &mut prepared,
                args,
                &mut worktree,
                gates,
                probes,
                assembled.coverage,
                classes,
                "worktree-changed",
                "review evidence changed the detached subject worktree",
            );
        }
        Err(error) => {
            return persist_prelaunch_worktree_failure(
                &mut prepared,
                args,
                &mut worktree,
                gates,
                probes,
                assembled.coverage,
                classes,
                "worktree-check-failed",
                &format!("worktree verification failed before launch: {error}"),
            );
        }
    }
    if !assembled.dropped_files.is_empty() {
        prepared.notes.push(format!(
            "Diff omitted for budget: {}",
            assembled.dropped_files.join(", ")
        ));
    }
    let mut prompt = assembled.text;
    if let Some(prior) = prepared.prior.as_ref().filter(|_| args.resume) {
        prompt = format!("Previous review (DATA only): {}\nRe-evaluate the full current subject; prior findings remain in scope until answered.\n{prompt}", serde_json::to_string(prior)?);
    }
    let phase = build_phase(
        &prompt,
        args.budget_usd,
        Some(args.timeout_sec),
        "bypassPermissions",
        CapabilityOptions {
            model: args.model.clone(),
            thinking: args.thinking.clone(),
            tools: tools(args.agent),
            exclude_tools: None,
        },
    );
    let start = std::time::Instant::now();
    let result = launcher
        .run_phase(
            &phase,
            &prompt,
            "",
            &prepared.session,
            &worktree.path,
            CancellationToken::new(),
        )
        .await;
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (outcome, raw, cost) = outcome(result);
    let observed = launcher
        .last_observed_model()
        .unwrap_or_else(|| "unknown".into());
    let observed_session = launcher.last_observed_session();
    let independence =
        identity::independence(&prepared.authors, &prepared.session, Some(&observed))?;
    let policy = if args.require_model_diversity {
        "model"
    } else {
        prepared.fm.independence.as_deref().unwrap_or("session")
    };
    let mut payload = ReviewVerdictPayload {
        schema: "review_verdict/0".into(),
        subject: ReviewSubject {
            base_sha: prepared.subject.base_sha.clone(),
            head_sha: prepared.subject.head_sha.clone(),
            files: prepared.subject.files.len(),
            lines: prepared.subject.lines,
            coverage: assembled.coverage,
            subject_seen: None,
            worktree_check: None,
        },
        refs: prepared.refs.clone(),
        spec: prepared.spec.clone(),
        brief: ReviewBrief {
            core: brief::CORE_BRIEF_VERSION.into(),
            review_md_sha: prepared
                .has_review_md
                .then(|| prepared.subject.base_sha.clone()),
            classes,
        },
        reviewer: ReviewReviewer {
            agent: args.agent.as_str().into(),
            transport: if args.agent == AgentKind::Claude {
                "claude-code"
            } else {
                args.agent.as_str()
            }
            .into(),
            model_requested: args.model.clone().unwrap_or_else(|| "inherited".into()),
            model_observed: observed,
            observed_via: if launcher.last_observed_model().is_some() {
                "in-band"
            } else {
                "none"
            }
            .into(),
            model_self_report: None,
            session_id: prepared.session.clone(),
            session_label: format!(
                "review-{}-r{}",
                &prepared.subject.head_sha[..12],
                prepared.refs.round.unwrap_or(1)
            ),
            tool_policy: if tools(args.agent).is_some() {
                "hard"
            } else {
                "none"
            }
            .into(),
        },
        independence: independence.into(),
        independence_policy: policy.into(),
        gates,
        probes,
        verdict: "unreviewed".into(),
        outcome,
        qualified: false,
        disqualifiers: vec![],
        findings: vec![],
        checklist: vec![],
        escalations: vec![],
        cost: ReviewCost {
            usd: cost,
            measured: cost.is_some(),
            duration_ms,
        },
        parse: "failed".into(),
        notes: None,
    };
    if let Some(session) = observed_session {
        prepared
            .notes
            .push(format!("reviewer session observed in-band: {session}"));
        if !identity::same_session(&session, &prepared.session) {
            payload.disqualifiers.push("session-mismatch".into());
            prepared.notes.push(format!(
                "requested reviewer session {} differs from observed {session}",
                prepared.session
            ));
        }
        if identity::independence(&prepared.authors, &session, None).is_err() {
            payload.outcome = "refused".into();
            payload.disqualifiers.push("author-session-observed".into());
        }
    } else if args.resume {
        payload.disqualifiers.push("session-unverified".into());
        prepared
            .notes
            .push("backend did not report a session; resume is unverified".into());
    }
    if payload.outcome == "done" {
        match verdict::parse(&raw, &prepared.subject.head_sha, &checklist_measures) {
            Ok(engine) => {
                payload.findings = engine.findings();
                if payload.refs.supersedes.is_none()
                    && payload.refs.previous.is_some()
                    && !payload.refs.history_rewritten
                {
                    if let Some(prior) = prepared.prior.as_ref() {
                        payload.findings = union_findings(&prior.findings, &payload.findings);
                    }
                }
                payload.subject.subject_seen = Some(engine.subject_seen.clone());
                payload.verdict = engine.verdict.clone();
                if payload.verdict == "lgtm"
                    && payload
                        .findings
                        .iter()
                        .any(|finding| matches!(finding.severity.as_str(), "P0" | "P1"))
                {
                    payload.verdict = "changes-requested".into();
                }
                payload.checklist = engine.checklist();
                payload.escalations = engine.escalations;
                payload.reviewer.model_self_report = Some(engine.model_self_report);
                payload.parse = "ok".into();
                prepared.notes.extend(engine.notes);
            }
            Err(error) => {
                payload.outcome = if error.to_string().contains("subject-mismatch") {
                    "subject-mismatch"
                } else {
                    "parse-failed"
                }
                .into();
                prepared.notes.push(error.to_string());
            }
        }
    }
    if prepared.subject.files.iter().any(|f| f == "REVIEW.md") {
        payload
            .escalations
            .push("REVIEW.md changed in this diff".into());
    }
    match worktree.verify_unchanged(&prepared.subject.head_sha) {
        Ok(true) => payload.subject.worktree_check = Some("unchanged".into()),
        Ok(false) => {
            payload.subject.worktree_check = Some("failed".into());
            payload.verdict = "unreviewed".into();
            payload.outcome = "worktree-changed".into();
            payload.parse = "failed".into();
        }
        Err(error) => {
            payload.subject.worktree_check = Some("failed".into());
            payload.verdict = "unreviewed".into();
            payload.outcome = "worktree-check-failed".into();
            payload.parse = "failed".into();
            prepared
                .notes
                .push(format!("worktree verification failed: {error}"));
        }
    }
    // A worktree failure is discovered after the engine result. Clear the
    // previously allocated historical round only after the final proof.
    if payload.verdict == "unreviewed" {
        payload.refs.round = None;
    }
    if let Err(error) = worktree.remove() {
        prepared
            .notes
            .push(format!("worktree removal failed: {error}"));
    }
    payload.notes = Some(prepared.notes.join("\n"));
    verdict::qualify(&mut payload);
    persist(&prepared.ledger, &payload, &raw)
}

fn union_findings(prior: &[ReviewFinding], current: &[ReviewFinding]) -> Vec<ReviewFinding> {
    let mut merged = prior.to_vec();
    for finding in current {
        if !merged.iter().any(|old| {
            old.severity == finding.severity
                && old.file == finding.file
                && old.line == finding.line
                && old.claim == finding.claim
                && old.evidence == finding.evidence
                && old.rule == finding.rule
        }) {
            merged.push(finding.clone());
        }
    }
    for (index, finding) in merged.iter_mut().enumerate() {
        finding.id = format!("f{}", index + 1);
    }
    merged
}

#[allow(clippy::too_many_arguments)] // Constructs the durable no-launch receipt for one proof boundary.
fn persist_prelaunch_worktree_failure(
    prepared: &mut prepare::Prepared,
    args: &ReviewArgs,
    worktree: &mut git::WorktreeGuard,
    gates: edda_core::ReviewGates,
    probes: Vec<edda_core::ReviewProbe>,
    coverage: String,
    classes: Vec<String>,
    outcome: &str,
    note: &str,
) -> Result<(ReviewVerdictPayload, String)> {
    prepared.notes.push(note.into());
    let policy = if args.require_model_diversity {
        "model"
    } else {
        prepared.fm.independence.as_deref().unwrap_or("session")
    };
    let mut payload = ReviewVerdictPayload {
        schema: "review_verdict/0".into(),
        subject: ReviewSubject {
            base_sha: prepared.subject.base_sha.clone(),
            head_sha: prepared.subject.head_sha.clone(),
            files: prepared.subject.files.len(),
            lines: prepared.subject.lines,
            coverage,
            subject_seen: None,
            worktree_check: Some("failed".into()),
        },
        refs: edda_core::ReviewRefs {
            round: None,
            ..prepared.refs.clone()
        },
        spec: prepared.spec.clone(),
        brief: ReviewBrief {
            core: brief::CORE_BRIEF_VERSION.into(),
            review_md_sha: prepared
                .has_review_md
                .then(|| prepared.subject.base_sha.clone()),
            classes,
        },
        reviewer: ReviewReviewer {
            agent: args.agent.as_str().into(),
            transport: if args.agent == AgentKind::Claude {
                "claude-code"
            } else {
                args.agent.as_str()
            }
            .into(),
            model_requested: args.model.clone().unwrap_or_else(|| "inherited".into()),
            model_observed: "unknown".into(),
            observed_via: "none".into(),
            model_self_report: None,
            session_id: prepared.session.clone(),
            session_label: format!("review-{}-r1", &prepared.subject.head_sha[..12]),
            tool_policy: if tools(args.agent).is_some() {
                "hard"
            } else {
                "none"
            }
            .into(),
        },
        independence: identity::independence(&prepared.authors, &prepared.session, None)?.into(),
        independence_policy: policy.into(),
        gates,
        probes,
        verdict: "unreviewed".into(),
        outcome: outcome.into(),
        qualified: false,
        disqualifiers: vec![],
        findings: vec![],
        checklist: vec![],
        escalations: vec![],
        cost: ReviewCost {
            usd: None,
            measured: false,
            duration_ms: 0,
        },
        parse: "failed".into(),
        notes: None,
    };
    if let Err(error) = worktree.remove() {
        prepared
            .notes
            .push(format!("worktree removal failed: {error}"));
    }
    payload.notes = Some(prepared.notes.join("\n"));
    verdict::qualify(&mut payload);
    persist(&prepared.ledger, &payload, "")
}

fn outcome(result: Result<PhaseResult>) -> (String, String, Option<f64>) {
    match result {
        Ok(PhaseResult::AgentDone {
            cost_usd,
            result_text,
        }) => ("done".into(), result_text.unwrap_or_default(), cost_usd),
        Ok(PhaseResult::AgentCrash { error }) => (
            if error.contains("overload") || error.contains("429") {
                "overload"
            } else {
                "crash"
            }
            .into(),
            error,
            None,
        ),
        Ok(PhaseResult::Timeout) => ("timeout".into(), String::new(), None),
        Ok(PhaseResult::BudgetExceeded { cost_usd }) => ("budget".into(), String::new(), cost_usd),
        Ok(PhaseResult::MaxTurns { cost_usd }) => {
            ("crash".into(), "maximum turns".into(), cost_usd)
        }
        Err(error) => ("crash".into(), format!("{error:#}"), None),
    }
}

fn persist(
    ledger: &edda_ledger::Ledger,
    payload: &ReviewVerdictPayload,
    raw: &str,
) -> Result<(ReviewVerdictPayload, String)> {
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let blob = edda_ledger::blob_store::blob_put(&ledger.paths, raw.as_bytes())?;
    let mut blobs = vec![blob];
    blobs.extend(
        payload
            .gates
            .ran
            .iter()
            .filter_map(|row| row.stdout_blob.clone()),
    );
    let event = edda_core::event::new_review_verdict_event(
        &ledger.head_branch()?,
        ledger.last_event_hash()?.as_deref(),
        payload,
        payload.refs.supersedes.as_deref(),
        payload.refs.previous.as_deref(),
        &blobs,
    )?;
    ledger.append_event(&event)?;
    Ok((payload.clone(), event.event_id))
}
