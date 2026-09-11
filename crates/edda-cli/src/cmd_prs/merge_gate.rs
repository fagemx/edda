use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::Path;

const ADVISORY_NOTICE: &str = "Deprecated advisory scalar diagnostics only; not trusted R6 evidence. Use `edda review merge --pr <PR>` for live merge readiness.";

/// Arguments for deprecated `edda prs check-merge` compatibility.
///
/// A positional PR without `--input` forwards to `edda review merge`. The
/// `--input` and direct-fact forms remain advisory scalar diagnostics only.
#[derive(Debug, Clone, Args, Default)]
pub struct CheckMergeArgs {
    /// PR number. Without --input, forward to canonical `edda review merge`
    #[arg(value_name = "PR")]
    pub pr: Option<u64>,

    /// Advisory direct-fact head commit SHA (deprecated)
    #[arg(long)]
    pub head_sha: Option<String>,

    /// Advisory direct-fact verdict commit SHA (deprecated)
    #[arg(long)]
    pub verdict_sha: Option<String>,

    /// Advisory direct-fact verdict: exactly "lgtm" or "approved" (deprecated)
    #[arg(long)]
    pub verdict: Option<String>,

    /// Optional advisory author of the direct-fact verdict (deprecated)
    #[arg(long)]
    pub verdict_author: Option<String>,

    /// Advisory count of P0 blocking findings (deprecated)
    #[arg(long, default_value_t = 0)]
    pub p0: usize,

    /// Advisory count of P1 blocking findings (deprecated)
    #[arg(long, default_value_t = 0)]
    pub p1: usize,

    /// Advisory assertion that required CI checks are green (deprecated)
    #[arg(long)]
    pub ci_green: bool,

    /// Unsupported for live PR forwarding; canonical trust uses GitHub
    /// author_association (OWNER, MEMBER, or COLLABORATOR)
    #[arg(long, value_delimiter = ',')]
    pub allowed_reviewers: Option<Vec<String>>,

    /// Advisory MergeGateInput JSON file, or '-' for stdin (deprecated)
    #[arg(long, value_name = "FILE")]
    pub input: Option<String>,

    /// Forward a live PR to canonical `edda review merge --merge`; forbidden
    /// for advisory --input/direct-fact modes
    #[arg(long)]
    pub merge: bool,

    /// Output canonical live JSON, or the advisory compatibility result as JSON
    #[arg(long)]
    pub json: bool,

    /// Override an active process claim in advisory --input/direct-fact modes
    /// only; unsupported for a forwarded live PR (GH-581)
    #[arg(long)]
    pub force: bool,
}

/// Deprecated host-agnostic advisory scalar input. This schema is retained for
/// compatibility; it is not trusted-comment union or R6 evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeGateInput {
    pub head_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_author: Option<String>,
    pub verdict: Option<String>,
    pub verdict_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_author: Option<String>,
    #[serde(default)]
    pub p0_count: usize,
    #[serde(default)]
    pub p1_count: usize,
    #[serde(default)]
    pub required_ci_green: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_checks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub force: bool,
}

/// Deprecated advisory scalar result. The shape remains compatible, but
/// `can_merge` is not trusted R6 merge-readiness evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MergeGateResult {
    pub can_merge: bool,
    pub head_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_author: Option<String>,
    pub verdict: Option<String>,
    pub verdict_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_author: Option<String>,
    pub p0_count: usize,
    pub p1_count: usize,
    pub required_ci_green: bool,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub force: bool,
}

/// Check if string is a valid 40-character hex commit SHA.
pub fn is_valid_40_hex_sha(sha: &str) -> bool {
    let s = sha.trim();
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Pure deprecated advisory evaluation without host or network dependencies.
///
/// This deliberately evaluates only caller-supplied scalar facts. It cannot
/// establish the trusted-comment union and is never R6 evidence.
pub fn evaluate_merge_preconditions(input: &MergeGateInput) -> MergeGateResult {
    let mut reasons = Vec::new();

    let h_clean = input.head_sha.trim();
    if !is_valid_40_hex_sha(h_clean) {
        reasons.push(format!(
            "head_sha '{h_clean}' is not a valid 40-character hex commit SHA"
        ));
    }

    match &input.verdict {
        None => reasons.push("no review verdict found on PR".to_string()),
        Some(v) => {
            let v_clean = v.trim().to_lowercase();
            if v_clean.is_empty() || v_clean == "none" || v_clean == "unreviewed" {
                reasons.push("no review verdict found on PR".to_string());
            } else if !matches!(v_clean.as_str(), "lgtm" | "approved") {
                reasons.push(format!("review verdict is not LGTM (found '{v}')"));
            }
        }
    }

    if input.p0_count > 0 || input.p1_count > 0 {
        reasons.push(format!(
            "blocking findings present: {} P0, {} P1 (both must be 0)",
            input.p0_count, input.p1_count
        ));
    }

    if let Some(v_sha) = &input.verdict_sha {
        let v_clean = v_sha.trim();
        if !is_valid_40_hex_sha(v_clean) {
            reasons.push(format!(
                "verdict_sha '{v_clean}' is not a valid 40-character hex commit SHA"
            ));
        } else if !v_clean.eq_ignore_ascii_case(h_clean) {
            reasons.push(format!(
                "SHA window mismatch: verdict pinned to '{v_clean}' but current head is '{h_clean}' (new commits pushed after review)"
            ));
        }
    } else if input.verdict.is_some() {
        reasons.push("review verdict is not pinned to any commit SHA".to_string());
    }

    if input.required_ci_green && !input.failed_checks.is_empty() {
        reasons.push(format!(
            "required_ci_green=true contradicts nonempty failed_checks: {}",
            input.failed_checks.join(", ")
        ));
    } else if !input.required_ci_green {
        if input.failed_checks.is_empty() {
            reasons.push("required CI check(s) are not green".to_string());
        } else {
            reasons.push(format!(
                "required CI check(s) failed or pending: {}",
                input.failed_checks.join(", ")
            ));
        }
    }

    if let Some(holder) = &input.claimed_by {
        if !input.force {
            reasons.push(format!(
                "PR is claimed by active session '{holder}' — use --force to override"
            ));
        }
    }

    MergeGateResult {
        can_merge: reasons.is_empty(),
        head_sha: input.head_sha.clone(),
        pr_author: input.pr_author.clone(),
        verdict: input.verdict.clone(),
        verdict_sha: input.verdict_sha.clone(),
        verdict_author: input.verdict_author.clone(),
        p0_count: input.p0_count,
        p1_count: input.p1_count,
        required_ci_green: input.required_ci_green,
        reasons,
        claimed_by: input.claimed_by.clone(),
        force: input.force,
    }
}

/// Format the retained advisory compatibility report.
///
/// Existing report lines remain for compatibility, with an explicit advisory
/// line preventing them from being presented as trusted R6 evidence.
pub fn format_merge_report(result: &MergeGateResult) -> (String, String) {
    use std::fmt::Write;

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();

    if result.can_merge {
        let _ = writeln!(stdout_buf, "PASS: Merge preconditions satisfied.");
        stdout_buf.push_str(&format!("  Advisory: {ADVISORY_NOTICE}\n"));
        let _ = writeln!(stdout_buf, "  PR Head:     {}", result.head_sha);
        if let Some(author) = &result.pr_author {
            let _ = writeln!(stdout_buf, "  PR Author:   {author}");
        }
        let author_str = result
            .verdict_author
            .as_deref()
            .map(|a| format!(" by {a}"))
            .unwrap_or_default();
        let _ = writeln!(
            stdout_buf,
            "  Verdict:     {}{} (pinned at {})",
            result.verdict.as_deref().unwrap_or("none"),
            author_str,
            result.verdict_sha.as_deref().unwrap_or("none")
        );
        let _ = writeln!(
            stdout_buf,
            "  Findings:    P0={}, P1={}",
            result.p0_count, result.p1_count
        );
        let _ = writeln!(stdout_buf, "  Required CI: green");
        if let Some(holder) = &result.claimed_by {
            let _ = writeln!(
                stdout_buf,
                "  Claim Notice: Overriding process claim held by '{holder}' (--force specified)"
            );
        }
    } else {
        let _ = writeln!(
            stderr_buf,
            "REFUSED: Merge preconditions not satisfied for head {}:",
            result.head_sha
        );
        stderr_buf.push_str(&format!("  Advisory: {ADVISORY_NOTICE}\n"));
        if let Some(author) = &result.pr_author {
            let _ = writeln!(stderr_buf, "  PR Author:   {author}");
        }
        for reason in &result.reasons {
            let _ = writeln!(stderr_buf, "  - {reason}");
        }
    }

    (stdout_buf, stderr_buf)
}

/// Query the coordination board for an active process claim on `pr:<pr_number>`.
///
/// This compatibility guard is used only by advisory `--input`/direct-fact
/// evaluation. Canonical live merge readiness has no claim precondition.
pub(crate) fn check_active_pr_claim(
    repo_root: &Path,
    pr_number: u64,
    current_session_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let project_id = edda_store::project_id(repo_root);
    let target_subject = format!("pr:{pr_number}");
    let board = edda_bridge_claude::peers::compute_board_state(&project_id);

    for claim in &board.claims {
        if let Some(subject) = claim.subject.as_deref() {
            if crate::cmd_claim::has_wildcard(subject) {
                globset::GlobBuilder::new(subject)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("invalid claim subject glob '{subject}': {e}"))?;
            }
        }
    }

    for claim in board.claims {
        let Some(subject) = claim.subject.as_deref() else {
            continue;
        };
        let matches = crate::cmd_claim::subjects_intersect(&target_subject, subject)
            .map_err(|e| anyhow::anyhow!("failed to parse claim subject: {e}"))?;
        if !matches {
            continue;
        }
        let is_live = matches!(
            edda_bridge_claude::peers::classify_session_liveness(&project_id, &claim.session_id),
            edda_bridge_claude::peers::SessionLiveness::Live { .. }
        );
        if !is_live || current_session_id == Some(&claim.session_id) {
            continue;
        }
        return Ok(Some(format!(
            "{} (label: '{}', subject: '{}')",
            claim.session_id, claim.label, subject
        )));
    }
    Ok(None)
}

/// Execute deprecated `edda prs check-merge` compatibility.
///
/// A live positional PR goes through the exact `edda review merge` product
/// entrypoint. Only host-agnostic `--input` and direct facts retain the legacy
/// advisory evaluator, claim guard, and 0/1 report contract.
pub fn run_check_merge(args: CheckMergeArgs, repo_root: &Path) -> anyhow::Result<()> {
    if let Some(pr) = args.pr.filter(|_| args.input.is_none()) {
        if args.allowed_reviewers.is_some() {
            eprintln!(
                "edda prs check-merge: --allowed-reviewers is unsupported in live-forward mode; \
                 canonical `edda review merge` trusts GitHub author_association \
                 OWNER/MEMBER/COLLABORATOR; refusing"
            );
            std::process::exit(2);
        }
        if args.force {
            eprintln!(
                "edda prs check-merge: --force is unsupported in live-forward mode; process \
                 claims apply only to deprecated --input/direct-fact advisory diagnostics and \
                 are not an R6 condition; refusing"
            );
            std::process::exit(2);
        }
        return crate::cmd_review::run_merge_compat(pr, args.merge, args.json, repo_root);
    }

    if args.merge {
        anyhow::bail!(
            "--merge is forbidden for deprecated --input/direct-fact advisory diagnostics; use \
             `edda review merge --pr <PR> --merge`"
        );
    }

    let mut input = if let Some(input_source) = &args.input {
        let raw = if input_source == "-" {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            fs::read_to_string(input_source)
                .with_context(|| format!("read input file: {input_source}"))?
        };
        serde_json::from_str::<MergeGateInput>(&raw).context("parse MergeGateInput JSON")?
    } else if let Some(head_sha) = args.head_sha {
        MergeGateInput {
            head_sha,
            pr: None,
            pr_author: None,
            verdict: args.verdict,
            verdict_sha: args.verdict_sha,
            verdict_author: args.verdict_author,
            p0_count: args.p0,
            p1_count: args.p1,
            required_ci_green: args.ci_green,
            failed_checks: Vec::new(),
            claimed_by: None,
            force: false,
        }
    } else {
        anyhow::bail!(
            "Specify a live PR number, deprecated '--input <FILE>', or deprecated direct facts \
             beginning with '--head-sha <SHA>'"
        );
    };

    if args.force {
        input.force = true;
    }

    if let Some(pr_number) = args.pr.or(input.pr) {
        if input.claimed_by.is_none() {
            let my_sid = std::env::var("EDDA_SESSION_ID").ok();
            input.claimed_by = check_active_pr_claim(repo_root, pr_number, my_sid.as_deref())?;
        }
    }

    let result = evaluate_merge_preconditions(&input);

    if args.json {
        eprintln!("{ADVISORY_NOTICE}");
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let (out, err) = format_merge_report(&result);
        if !out.is_empty() {
            print!("{out}");
        }
        if !err.is_empty() {
            eprint!("{err}");
        }
    }

    if !result.can_merge {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SHA_A: &str = "1234567890abcdef1234567890abcdef12345678";
    const VALID_SHA_B: &str = "abcdef1234567890abcdef1234567890abcdef12";

    fn base_input() -> MergeGateInput {
        MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            pr: None,
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            claimed_by: None,
            force: false,
        }
    }

    #[test]
    fn no_verdict_refuses() {
        let input = MergeGateInput {
            verdict: None,
            verdict_sha: None,
            ..base_input()
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(!result.can_merge);
        assert!(result
            .reasons
            .iter()
            .any(|reason| reason.contains("no review verdict")));
    }

    #[test]
    fn only_exact_lgtm_or_approved_is_accepted() {
        for verdict in ["LGTM", " lgtm ", "APPROVED", " approved "] {
            let result = evaluate_merge_preconditions(&MergeGateInput {
                verdict: Some(verdict.into()),
                ..base_input()
            });
            assert!(result.can_merge, "{verdict:?}: {:?}", result.reasons);
        }
        for verdict in ["not lgtm", "LGTM later", "unapproved", "changes-requested"] {
            let result = evaluate_merge_preconditions(&MergeGateInput {
                verdict: Some(verdict.into()),
                ..base_input()
            });
            assert!(!result.can_merge, "accepted {verdict:?}");
            assert!(result
                .reasons
                .iter()
                .any(|reason| reason.contains("not LGTM")));
        }
    }

    #[test]
    fn stale_or_invalid_sha_refuses() {
        for input in [
            MergeGateInput {
                head_sha: VALID_SHA_B.into(),
                verdict_sha: Some(VALID_SHA_A.into()),
                ..base_input()
            },
            MergeGateInput {
                head_sha: "short".into(),
                verdict_sha: Some("short".into()),
                ..base_input()
            },
        ] {
            assert!(!evaluate_merge_preconditions(&input).can_merge);
        }
    }

    #[test]
    fn failed_or_contradictory_ci_refuses() {
        for input in [
            MergeGateInput {
                required_ci_green: false,
                failed_checks: vec!["CI Gate (fail)".into()],
                ..base_input()
            },
            MergeGateInput {
                required_ci_green: true,
                failed_checks: vec!["CI Gate (fail)".into()],
                ..base_input()
            },
        ] {
            let result = evaluate_merge_preconditions(&input);
            assert!(!result.can_merge);
            assert!(result
                .reasons
                .iter()
                .any(|reason| reason.contains("CI Gate (fail)")));
        }
    }

    #[test]
    fn blocking_findings_refuse() {
        for input in [
            MergeGateInput {
                p0_count: 12,
                ..base_input()
            },
            MergeGateInput {
                p1_count: 2,
                ..base_input()
            },
        ] {
            let result = evaluate_merge_preconditions(&input);
            assert!(!result.can_merge);
            assert!(result
                .reasons
                .iter()
                .any(|reason| reason.contains("blocking findings present")));
        }
    }

    #[test]
    fn all_advisory_scalars_met_approves() {
        let input = MergeGateInput {
            verdict_sha: Some(VALID_SHA_A.to_uppercase()),
            verdict_author: Some("reviewer".into()),
            ..base_input()
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(result.can_merge);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn retained_reports_are_explicitly_advisory() {
        let pass = evaluate_merge_preconditions(&base_input());
        let (stdout, stderr) = format_merge_report(&pass);
        assert!(stderr.is_empty());
        assert!(stdout.contains("PASS: Merge preconditions satisfied."));
        assert!(stdout.contains(ADVISORY_NOTICE));

        let refused = evaluate_merge_preconditions(&MergeGateInput {
            verdict: Some("not lgtm".into()),
            ..base_input()
        });
        let (stdout, stderr) = format_merge_report(&refused);
        assert!(stdout.is_empty());
        assert!(stderr.contains("REFUSED: Merge preconditions not satisfied"));
        assert!(stderr.contains(ADVISORY_NOTICE));
    }

    #[test]
    fn input_schema_round_trips_without_compatibility_markers() {
        let value = serde_json::to_value(base_input()).unwrap();
        assert_eq!(value["head_sha"], VALID_SHA_A);
        assert_eq!(value["verdict"], "LGTM");
        assert_eq!(value["required_ci_green"], true);
        assert!(value.get("advisory").is_none());
        assert!(value.get("deprecated").is_none());
    }

    #[test]
    fn input_file_report_only_still_runs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input_file = temp_dir.path().join("input.json");
        fs::write(&input_file, serde_json::to_string(&base_input()).unwrap()).unwrap();
        let args = CheckMergeArgs {
            input: Some(input_file.to_string_lossy().into_owned()),
            json: true,
            ..Default::default()
        };
        assert!(run_check_merge(args, temp_dir.path()).is_ok());
    }

    #[test]
    fn merge_with_input_remains_forbidden() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input_file = temp_dir.path().join("input.json");
        fs::write(&input_file, "{}").unwrap();
        let args = CheckMergeArgs {
            input: Some(input_file.to_string_lossy().into_owned()),
            merge: true,
            ..Default::default()
        };
        let error = run_check_merge(args, temp_dir.path()).unwrap_err();
        assert!(error.to_string().contains("--merge is forbidden"));
        assert!(error.to_string().contains("advisory"));
    }

    #[test]
    fn active_claim_semantics_remain_available_to_advisory_modes() {
        let repo = tempfile::tempdir().expect("repo");
        std::fs::create_dir_all(repo.path().join(".edda")).expect("edda");
        std::fs::create_dir_all(repo.path().join(".git")).expect("git");
        let project_id = edda_store::project_id(repo.path());
        let _store = crate::test_support::isolated_store();

        assert_eq!(check_active_pr_claim(repo.path(), 570, None).unwrap(), None);
        edda_bridge_claude::peers::write_heartbeat_minimal(&project_id, "s1", "reviewer", "/tmp");
        edda_bridge_claude::peers::write_claim_with_subject(
            &project_id,
            "s1",
            "rev-570",
            &[],
            Some("pr:570"),
        );
        assert!(check_active_pr_claim(repo.path(), 570, Some("s2"))
            .unwrap()
            .is_some());
        assert_eq!(
            check_active_pr_claim(repo.path(), 570, Some("s1")).unwrap(),
            None
        );
        assert_eq!(
            check_active_pr_claim(repo.path(), 571, Some("s2")).unwrap(),
            None
        );

        edda_bridge_claude::peers::write_heartbeat_minimal(&project_id, "s_bad", "bad", "/tmp");
        edda_bridge_claude::peers::write_claim_with_subject(
            &project_id,
            "s_bad",
            "rev-bad",
            &[],
            Some("pr:57[0-"),
        );
        assert!(check_active_pr_claim(repo.path(), 570, Some("s2")).is_err());
    }
}
