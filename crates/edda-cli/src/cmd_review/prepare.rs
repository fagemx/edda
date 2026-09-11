use super::{args::ReviewArgs, brief, evidence, git, github, identity, qualification, subject};
use anyhow::{bail, Result};
use edda_core::{ReviewRefs, ReviewSpec, ReviewVerdictPayload};
use edda_ledger::Ledger;
use std::path::{Path, PathBuf};

pub(crate) struct Prepared {
    pub repo: PathBuf,
    pub ledger: Ledger,
    pub subject: subject::Subject,
    pub refs: ReviewRefs,
    pub spec: ReviewSpec,
    pub spec_text: String,
    pub review_md: String,
    pub has_review_md: bool,
    pub fm: brief::FrontMatter,
    pub authors: identity::Authors,
    pub session: String,
    pub notes: Vec<String>,
    pub prior: Option<ReviewVerdictPayload>,
}

pub(crate) fn prepare(args: &ReviewArgs, cwd: &Path) -> Result<Prepared> {
    let repo = git::repo_root_from(cwd)?;
    let pr = args.pr.map(|n| github::resolve_pr(cwd, n)).transpose()?;
    let subject = subject::resolve_subject(
        cwd,
        args.base
            .as_deref()
            .or_else(|| pr.as_ref().map(|p| p.base.as_str())),
        pr.as_ref().map(|p| p.head.as_str()).unwrap_or(&args.head),
    )?;
    let ledger = Ledger::open(&repo)?;
    let (mut refs, prior) = subject::history(&ledger, &repo, &subject, args.pr)?;
    let (spec, spec_text, issue) = github::load_spec(
        cwd,
        cwd,
        args.spec.as_deref(),
        pr.as_ref().and_then(|p| p.issue),
        args.trust_spec,
    )?;
    refs.issue = issue;
    let range = format!("{}..{}", subject.base_sha, subject.head_sha);
    let commits = git::git(cwd, &["rev-list", &range])?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let subjects = git::git(cwd, &["log", "--format=%s", &range])?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let trailers = git::git(cwd, &["log", "--format=%(trailers)", &range])?;
    let authors = identity::authors(&ledger, &commits, &subjects, &trailers)?;
    let session = if args.resume {
        let previous = prior.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--resume requires an existing review for this range or PR")
        })?;
        if previous.reviewer.agent != args.agent.as_str() {
            bail!("resume requires the same reviewer agent");
        }
        if args
            .session_id
            .as_ref()
            .is_some_and(|s| !identity::same_session(s, &previous.reviewer.session_id))
        {
            bail!("resume session differs from recorded reviewer session");
        }
        previous.reviewer.session_id.clone()
    } else {
        args.session_id
            .clone()
            .unwrap_or_else(crate::cmd_dispatch::generate_session_id)
    };
    // Backends expect a UUID, not an arbitrary human label.
    if session.len() != 36
        || session.char_indices().any(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c != '-'
            } else {
                !c.is_ascii_hexdigit()
            }
        })
    {
        bail!("reviewer --session-id must be a UUID");
    }
    identity::independence(&authors, &session, None)?;
    let review_blob = format!("{}:REVIEW.md", subject.base_sha);
    let has_review_md = git::git_ok(cwd, &["cat-file", "-e", &review_blob])?;
    let review_md = if has_review_md {
        git::git(cwd, &["show", &review_blob])?
    } else {
        String::new()
    };
    let (mut fm, _, note) = brief::parse_review_md(&review_md);
    if fm.classes.is_empty() {
        fm.classes = brief::default_classes();
    }
    if fm.ran_allowlist.is_empty() {
        fm.ran_allowlist.push("edda".into());
    }
    let mut notes = note.into_iter().collect::<Vec<_>>();
    notes.push("Structured conductor cost receipts currently lack author session/model/SHA; author identity uses linked session digests and git trailers.".into());
    Ok(Prepared {
        repo,
        ledger,
        subject,
        refs,
        spec,
        spec_text,
        review_md,
        has_review_md,
        fm,
        authors,
        session,
        notes,
        prior,
    })
}

pub(crate) fn collect_evidence(
    prepared: &mut Prepared,
    args: &ReviewArgs,
    worktree: &Path,
) -> Result<(edda_core::ReviewGates, Vec<edda_core::ReviewProbe>, String)> {
    let verify = if matches!(prepared.spec.trust.as_str(), "operator" | "maintainer") {
        evidence::extract_verify(&prepared.spec_text)
    } else {
        vec![]
    };
    let set = evidence::gate_set(&prepared.fm, &args.gates, &verify);
    let (_, mut read, mut uncovered) = evidence::read_gates(
        &prepared.ledger,
        &prepared.subject.head_sha,
        &set,
        &prepared.subject.files,
    )?;
    if let Some(pr) = args.pr {
        let checks = evidence::gh_required_checks(&prepared.repo, pr, &prepared.subject.head_sha)?;
        let (_, required_rows) = evidence::read_ci(&checks);
        read.extend(required_rows);
        let (_, mapped_rows, mapped_gates) =
            evidence::read_ci_job_map(&checks, &set, &prepared.fm.ci_gates);
        read.extend(mapped_rows);
        evidence::remove_mapped_uncovered(&mut uncovered, &mapped_gates);
    }
    let (ran, notes) = if args.run_gates {
        evidence::ran_gates(
            worktree,
            &set.cmds,
            args.max_ran_sec,
            std::env::var_os("CARGO_TARGET_DIR").is_some_and(|v| !v.is_empty()),
            &prepared.ledger.paths,
            &prepared.repo,
        )
    } else {
        (vec![], vec![])
    };
    prepared.notes.extend(notes);
    let status = evidence::gate_status(&set.cmds, &read, &ran);
    let diff = git::git(
        &prepared.repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            &format!(
                "{}..{}",
                prepared.subject.base_sha, prepared.subject.head_sha
            ),
            "--",
        ],
    )?;
    let verbs =
        evidence::extract_probe_verbs(&diff, Some(&prepared.spec_text), &prepared.fm.ran_allowlist);
    let probes = evidence::run_probes(worktree, &verbs);
    let wiring = evidence::run_wiring_scan(
        &prepared.repo,
        &prepared.subject.base_sha,
        &prepared.subject.head_sha,
    )?;
    let measures = evidence::checklist_measures(&ran, &probes);
    let text = evidence::evidence_text(&read, &uncovered, &ran, &probes, wiring.as_deref());
    Ok((
        edda_core::ReviewGates {
            status,
            declared_by: set.declared_by,
            read,
            ran,
        },
        probes,
        format!(
            "{text}\n### Checklist `ran` measure IDs (exact strings only)\n{}",
            measures.join("\n")
        ),
    ))
}

pub(crate) fn assemble(
    prepared: &Prepared,
    args: &ReviewArgs,
    evidence: &str,
) -> Result<(brief::Brief, Vec<String>, qualification::Qualification)> {
    let classes = brief::route_classes(&prepared.subject.files, &prepared.fm.classes);
    // REVIEW.md §6.1 reads brief silence as "checklist-type engine", so the
    // brief must name the engine's R22 authority for this PR's surface.
    let qualified = qualification::assess(
        &prepared.subject.files,
        &super::model_requested(args),
        super::transport(args.agent),
        args.require_model_diversity,
    )?;
    let paths = prepared
        .subject
        .files
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let decisions = prepared.ledger.query_by_paths(&paths, None, Some(100))?;
    let ratified = prepared.ledger.ratified_decision_events()?;
    let pack = serde_json::to_string(
        &serde_json::json!({"decisions":decisions,"ratified_event_ids":ratified}),
    )?;
    let range = format!(
        "{}..{}",
        prepared.subject.base_sha, prepared.subject.head_sha
    );
    let chunks = prepared
        .subject
        .files
        .iter()
        .map(|path| {
            Ok((
                path.clone(),
                git::git(
                    &prepared.repo,
                    &[
                        "--literal-pathspecs",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        &range,
                        "--",
                        path,
                    ],
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let budget = std::env::var("EDDA_REVIEW_DIFF_BUDGET_CHARS")
        .ok()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(200_000);
    let qualification_text = qualified.brief_section();
    let inputs = brief::BriefInputs {
        review_md: &prepared.review_md,
        classes: &classes,
        qualification: &qualification_text,
        spec: &prepared.spec_text,
        spec_trust: &prepared.spec.trust,
        ledger_pack: &pack,
        evidence,
        head_sha: &prepared.subject.head_sha,
    };
    Ok((
        brief::assemble(&inputs, chunks, &prepared.fm.classes, budget)?,
        classes,
        qualified,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_review::git::testrepo;
    use edda_core::event::{new_cmd_event_with_git_context_and_dirty_paths, CmdEventParams};

    #[test]
    fn evidence_selection_receives_the_reviewed_subject_files() {
        let (_temp, root) = testrepo::init();
        testrepo::run(&root, &["checkout", "-qb", "feature"]);
        std::fs::create_dir_all(root.join("docs/archive")).expect("archive dir");
        let subject_path = "docs/archive/probe.txt";
        let head = testrepo::commit_file(&root, subject_path, "subject\n", "feature change");
        let ledger = Ledger::open_or_init(&root).expect("ledger");
        let argv = vec!["gate".to_owned()];
        let untracked = vec![subject_path.to_owned()];
        let event = new_cmd_event_with_git_context_and_dirty_paths(
            &CmdEventParams {
                branch: "main",
                parent_hash: ledger.last_event_hash().expect("parent").as_deref(),
                argv: &argv,
                cwd: root.to_str().expect("utf8 root"),
                exit_code: 0,
                duration_ms: 1,
                stdout_blob: "",
                stderr_blob: "",
            },
            Some(&head),
            Some(true),
            Some((&[], &untracked)),
        )
        .expect("receipt");
        ledger.append_event(&event).expect("append receipt");
        let args = ReviewArgs {
            gates: vec!["gate".into()],
            ..Default::default()
        };
        let mut prepared = prepare(&args, &root).expect("prepare");
        let (gates, _, _) = collect_evidence(&mut prepared, &args, &root).expect("evidence");
        assert_eq!(gates.status, "unverified");
        assert!(gates.read.is_empty());
    }
}
