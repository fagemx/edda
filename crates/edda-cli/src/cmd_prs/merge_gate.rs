use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// Command line arguments for `edda prs check-merge`
#[derive(Debug, Clone, Args)]
pub struct CheckMergeArgs {
    /// PR number to evaluate (queries GitHub via `gh` CLI)
    #[arg(value_name = "PR", conflicts_with = "input")]
    pub pr: Option<u64>,

    /// Explicit head commit SHA
    #[arg(long)]
    pub head_sha: Option<String>,

    /// Explicit verdict commit SHA
    #[arg(long)]
    pub verdict_sha: Option<String>,

    /// Explicit verdict (e.g. "lgtm", "changes-requested")
    #[arg(long)]
    pub verdict: Option<String>,

    /// Count of P0 blocking findings
    #[arg(long, default_value_t = 0)]
    pub p0: usize,

    /// Count of P1 blocking findings
    #[arg(long, default_value_t = 0)]
    pub p1: usize,

    /// Mark required CI checks as green
    #[arg(long)]
    pub ci_green: bool,

    /// Path to JSON file containing MergeGateInput (or '-' for stdin)
    #[arg(long, value_name = "FILE")]
    pub input: Option<String>,

    /// Execute `gh pr merge <PR> --squash` if preconditions pass
    #[arg(long)]
    pub merge: bool,

    /// Output evaluation report as JSON
    #[arg(long)]
    pub json: bool,
}

/// Host-agnostic input for evaluating merge preconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeGateInput {
    pub head_sha: String,
    pub verdict: Option<String>,
    pub verdict_sha: Option<String>,
    #[serde(default)]
    pub p0_count: usize,
    #[serde(default)]
    pub p1_count: usize,
    #[serde(default)]
    pub required_ci_green: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_checks: Vec<String>,
}

/// Evaluation outcome for merge preconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeGateResult {
    pub can_merge: bool,
    pub head_sha: String,
    pub verdict: Option<String>,
    pub verdict_sha: Option<String>,
    pub p0_count: usize,
    pub p1_count: usize,
    pub required_ci_green: bool,
    pub reasons: Vec<String>,
}

/// Check if string is a valid 40-character hex commit SHA.
pub fn is_valid_40_hex_sha(sha: &str) -> bool {
    let s = sha.trim();
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Pure evaluation function without host or network dependencies.
pub fn evaluate_merge_preconditions(input: &MergeGateInput) -> MergeGateResult {
    let mut reasons = Vec::new();

    // 1. Validate head SHA shape
    let h_clean = input.head_sha.trim();
    if !is_valid_40_hex_sha(h_clean) {
        reasons.push(format!(
            "head_sha '{h_clean}' is not a valid 40-character hex commit SHA"
        ));
    }

    // 2. Verdict presence & state
    match &input.verdict {
        None => {
            reasons.push("no review verdict found on PR".to_string());
        }
        Some(v) => {
            let v_clean = v.trim().to_lowercase();
            if v_clean.is_empty() || v_clean == "none" || v_clean == "unreviewed" {
                reasons.push("no review verdict found on PR".to_string());
            } else if v_clean != "lgtm" && !v_clean.contains("lgtm") && v_clean != "approved" {
                reasons.push(format!("review verdict is not LGTM (found '{v}')"));
            }
        }
    }

    // 3. Blocking findings (P0=0 and P1=0)
    if input.p0_count > 0 || input.p1_count > 0 {
        reasons.push(format!(
            "blocking findings present: {} P0, {} P1 (both must be 0)",
            input.p0_count, input.p1_count
        ));
    }

    // 4. SHA window check (verdict SHA must match current PR head exactly)
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

    // 5. Required CI checks
    if !input.required_ci_green {
        if input.failed_checks.is_empty() {
            reasons.push("required CI check(s) are not green".to_string());
        } else {
            reasons.push(format!(
                "required CI check(s) failed or pending: {}",
                input.failed_checks.join(", ")
            ));
        }
    }

    MergeGateResult {
        can_merge: reasons.is_empty(),
        head_sha: input.head_sha.clone(),
        verdict: input.verdict.clone(),
        verdict_sha: input.verdict_sha.clone(),
        p0_count: input.p0_count,
        p1_count: input.p1_count,
        required_ci_green: input.required_ci_green,
        reasons,
    }
}

// GitHub API / `gh` structures
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct GhViewPr {
    #[serde(rename = "headRefOid")]
    pub head_ref_oid: String,
    #[serde(default)]
    pub author: Option<GhAuthor>,
    #[serde(default)]
    pub comments: Vec<GhComment>,
    #[serde(default)]
    pub reviews: Vec<GhReviewItem>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct GhAuthor {
    pub login: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct GhComment {
    pub body: String,
    #[serde(default)]
    pub author: Option<GhAuthor>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct GhReviewItem {
    pub state: String,
    #[serde(default)]
    pub author: Option<GhAuthor>,
    pub body: Option<String>,
    #[serde(rename = "submittedAt")]
    pub submitted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCheckItem {
    name: String,
    state: Option<String>,
    bucket: Option<String>,
}

/// Determines if a comment is a structured review verdict, preventing casual comments from triggering a verdict.
pub fn is_structured_review_comment(body: &str) -> bool {
    body.contains("## Code Review:")
        || body.contains("### Verdict")
        || body.contains("<<<VERDICT")
        || body.contains("Verdict as of the ruling:")
        || body.contains("**Verdict as of the ruling:")
}

fn parse_count(line: &str, prefix: &str) -> Option<usize> {
    let idx = line.find(prefix)?;
    let rest = line[idx + prefix.len()..].trim_start_matches([':', '=', ' ']);
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse::<usize>().ok()
}

/// Extract verdict info (verdict, verdict_sha, p0, p1) from comment or review body text
pub fn parse_verdict_text(text: &str) -> (Option<String>, Option<String>, usize, usize) {
    let mut verdict = None;
    let mut verdict_sha = None;
    let mut p0 = 0;
    let mut p1 = 0;

    let has_changes_requested =
        text.contains("Changes Requested") || text.contains("changes_requested");
    let has_lgtm = text.contains("LGTM") || text.contains("lgtm");

    if has_lgtm && !has_changes_requested {
        verdict = Some("LGTM".to_string());
    } else if has_changes_requested && !has_lgtm {
        verdict = Some("Changes Requested".to_string());
    } else if has_lgtm && has_changes_requested {
        let pos_cr = text.rfind("Changes Requested").unwrap_or(0);
        let pos_lgtm = text.rfind("LGTM").unwrap_or(0);
        if pos_lgtm > pos_cr {
            verdict = Some("LGTM".to_string());
        } else {
            verdict = Some("Changes Requested".to_string());
        }
    }

    for line in text.lines() {
        if let Some(val) = parse_count(line, "P0") {
            p0 = p0.max(val);
        } else if let Some(val) = parse_count(line, "p0") {
            p0 = p0.max(val);
        }

        if let Some(val) = parse_count(line, "P1") {
            p1 = p1.max(val);
        } else if let Some(val) = parse_count(line, "p1") {
            p1 = p1.max(val);
        }
    }

    for word in text.split_whitespace() {
        let clean = word.trim_matches(['*', '`', '(', ')', '.', ',', ':', '"', '\'', '[', ']']);
        if clean.len() == 40 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
            verdict_sha = Some(clean.to_string());
            break;
        }
    }

    (verdict, verdict_sha, p0, p1)
}

#[derive(Debug)]
struct ReviewTimelineEvent {
    timestamp: String,
    verdict: String,
    verdict_sha: Option<String>,
    p0_count: usize,
    p1_count: usize,
}

/// Pure timeline extractor: processes reviews and structured comments into the final verdict.
pub fn extract_timeline_verdict(
    gh_view: &GhViewPr,
) -> (Option<String>, Option<String>, usize, usize) {
    let mut timeline: Vec<ReviewTimelineEvent> = Vec::new();

    // 1. Process GitHub formal reviews (APPROVED / CHANGES_REQUESTED)
    // The formal review state is strictly authoritative for the verdict (cannot be overridden by prose)
    for r in &gh_view.reviews {
        let verdict_opt = match r.state.as_str() {
            "APPROVED" => Some("LGTM".to_string()),
            "CHANGES_REQUESTED" => Some("Changes Requested".to_string()),
            _ => None,
        };

        if let Some(formal_verdict) = verdict_opt {
            // Parse body only for SHA, p0, p1 (formal review state is NOT overridden by prose)
            let (_, sha, parsed_p0, parsed_p1) = if let Some(body) = &r.body {
                parse_verdict_text(body)
            } else {
                (None, None, 0, 0)
            };

            let ts = r.submitted_at.clone().unwrap_or_default();
            timeline.push(ReviewTimelineEvent {
                timestamp: ts,
                verdict: formal_verdict,
                verdict_sha: sha,
                p0_count: parsed_p0,
                p1_count: parsed_p1,
            });
        }
    }

    // 2. Process structured review comments
    // Comments must meet is_structured_review_comment to ensure casual comments are ignored
    for comment in &gh_view.comments {
        if !is_structured_review_comment(&comment.body) {
            continue;
        }

        let (v, sha, parsed_p0, parsed_p1) = parse_verdict_text(&comment.body);
        if let Some(found_v) = v {
            let ts = comment.created_at.clone().unwrap_or_default();
            timeline.push(ReviewTimelineEvent {
                timestamp: ts,
                verdict: found_v,
                verdict_sha: sha,
                p0_count: parsed_p0,
                p1_count: parsed_p1,
            });
        }
    }

    // Sort timeline chronologically (oldest to newest) so the newest review event wins
    timeline.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    if let Some(latest) = timeline.pop() {
        (
            Some(latest.verdict),
            latest.verdict_sha,
            latest.p0_count,
            latest.p1_count,
        )
    } else {
        (None, None, 0, 0)
    }
}

fn fetch_pr_from_gh(pr: u64, repo_root: &Path) -> anyhow::Result<MergeGateInput> {
    let view_output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "headRefOid,author,comments,reviews",
        ])
        .current_dir(repo_root)
        .output()
        .context("execute gh pr view")?;

    if !view_output.status.success() {
        let stderr = String::from_utf8_lossy(&view_output.stderr);
        anyhow::bail!("gh pr view {} failed: {}", pr, stderr.trim());
    }

    let gh_view: GhViewPr =
        serde_json::from_slice(&view_output.stdout).context("parse gh pr view json output")?;

    let head_sha = gh_view.head_ref_oid.clone();
    let (verdict, verdict_sha, p0, p1) = extract_timeline_verdict(&gh_view);

    // Fetch PR checks
    let checks_output = Command::new("gh")
        .args([
            "pr",
            "checks",
            &pr.to_string(),
            "--json",
            "name,state,bucket",
        ])
        .current_dir(repo_root)
        .output()
        .context("execute gh pr checks")?;

    let mut required_ci_green = true;
    let mut failed_checks = Vec::new();

    if checks_output.status.success() {
        let checks: Vec<GhCheckItem> =
            serde_json::from_slice(&checks_output.stdout).unwrap_or_default();

        let ci_gate_item = checks.iter().find(|c| c.name == "CI Gate");

        if let Some(ci_gate) = ci_gate_item {
            let pass = ci_gate.bucket.as_deref() == Some("pass")
                || ci_gate.bucket.as_deref() == Some("skipping")
                || ci_gate.state.as_deref() == Some("SUCCESS");
            if !pass {
                required_ci_green = false;
                failed_checks.push(format!(
                    "CI Gate ({})",
                    ci_gate.bucket.as_deref().unwrap_or("unknown")
                ));
            }
        } else if !checks.is_empty() {
            for c in &checks {
                let pass = c.bucket.as_deref() == Some("pass")
                    || c.bucket.as_deref() == Some("skipping")
                    || c.state.as_deref() == Some("SUCCESS");
                if !pass {
                    required_ci_green = false;
                    failed_checks.push(format!(
                        "{} ({})",
                        c.name,
                        c.bucket.as_deref().unwrap_or("unknown")
                    ));
                }
            }
        } else {
            required_ci_green = false;
            failed_checks.push("no checks reported".to_string());
        }
    } else {
        required_ci_green = false;
        failed_checks.push("failed to fetch CI checks via gh".to_string());
    }

    Ok(MergeGateInput {
        head_sha,
        verdict,
        verdict_sha,
        p0_count: p0,
        p1_count: p1,
        required_ci_green,
        failed_checks,
    })
}

/// Execute the merge precondition check CLI entrypoint.
pub fn run_check_merge(args: CheckMergeArgs, repo_root: &Path) -> anyhow::Result<()> {
    if args.merge && (args.pr.is_none() || args.input.is_some()) {
        anyhow::bail!("--merge requires a live PR number and cannot be used with '--input'");
    }

    let input = if let Some(input_source) = &args.input {
        let raw = if input_source == "-" {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            fs::read_to_string(input_source)
                .with_context(|| format!("read input file: {input_source}"))?
        };
        serde_json::from_str::<MergeGateInput>(&raw).context("parse MergeGateInput JSON")?
    } else if let Some(pr_number) = args.pr {
        fetch_pr_from_gh(pr_number, repo_root)?
    } else if let Some(head_sha) = args.head_sha {
        MergeGateInput {
            head_sha,
            verdict: args.verdict,
            verdict_sha: args.verdict_sha,
            p0_count: args.p0,
            p1_count: args.p1,
            required_ci_green: args.ci_green,
            failed_checks: Vec::new(),
        }
    } else {
        anyhow::bail!(
            "Specify either a PR number (e.g. 'edda prs check-merge 580') or '--input <FILE>'"
        );
    };

    let result = evaluate_merge_preconditions(&input);

    if args.json {
        let json_out = serde_json::to_string_pretty(&result)?;
        println!("{json_out}");
    } else if result.can_merge {
        println!("PASS: Merge preconditions satisfied.");
        println!("  PR Head:     {}", result.head_sha);
        println!(
            "  Verdict:     {} (pinned at {})",
            result.verdict.as_deref().unwrap_or("none"),
            result.verdict_sha.as_deref().unwrap_or("none")
        );
        println!(
            "  Findings:    P0={}, P1={}",
            result.p0_count, result.p1_count
        );
        println!("  Required CI: green");
    } else {
        eprintln!(
            "REFUSED: Merge preconditions not satisfied for head {}:",
            result.head_sha
        );
        for reason in &result.reasons {
            eprintln!("  - {reason}");
        }
    }

    if args.merge {
        if result.can_merge {
            let pr_number = match args.pr {
                Some(num) => num,
                None => anyhow::bail!("--merge requires a PR number positional argument"),
            };
            println!("Executing merge: gh pr merge {pr_number} --squash");
            let status = Command::new("gh")
                .args(["pr", "merge", &pr_number.to_string(), "--squash"])
                .current_dir(repo_root)
                .status()
                .context("execute gh pr merge")?;
            if !status.success() {
                anyhow::bail!("gh pr merge failed with status {}", status);
            }
        } else {
            eprintln!("Error: Cannot merge because preconditions failed.");
            std::process::exit(1);
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

    #[test]
    fn test_refuse_when_no_verdict() {
        let input = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: None,
            verdict_sha: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(!result.can_merge);
        assert!(result
            .reasons
            .iter()
            .any(|r| r.contains("no review verdict")));
    }

    #[test]
    fn test_refuse_when_verdict_stale_new_push() {
        let input = MergeGateInput {
            head_sha: VALID_SHA_B.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(!result.can_merge);
        assert!(result
            .reasons
            .iter()
            .any(|r| r.contains("SHA window mismatch")));
    }

    #[test]
    fn test_refuse_when_sha_is_invalid() {
        let input = MergeGateInput {
            head_sha: "short".into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some("short".into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(!result.can_merge);
        assert!(result
            .reasons
            .iter()
            .any(|r| r.contains("not a valid 40-character hex commit SHA")));
    }

    #[test]
    fn test_refuse_when_ci_not_green() {
        let input = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: false,
            failed_checks: vec!["CI Gate (fail)".into()],
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(!result.can_merge);
        assert!(result.reasons.iter().any(|r| r.contains("CI Gate (fail)")));
    }

    #[test]
    fn test_refuse_when_blocking_findings_or_changes_requested() {
        // Case A: Changes Requested
        let input_cr = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("Changes Requested".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let res_cr = evaluate_merge_preconditions(&input_cr);
        assert!(!res_cr.can_merge);
        assert!(res_cr.reasons.iter().any(|r| r.contains("not LGTM")));

        // Case B: P0 > 0 (e.g. 12)
        let input_p0 = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 12,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let res_p0 = evaluate_merge_preconditions(&input_p0);
        assert!(!res_p0.can_merge);
        assert!(res_p0.reasons.iter().any(|r| r.contains("12 P0")));

        // Case C: P1 > 0
        let input_p1 = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 0,
            p1_count: 2,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let res_p1 = evaluate_merge_preconditions(&input_p1);
        assert!(!res_p1.can_merge);
        assert!(res_p1
            .reasons
            .iter()
            .any(|r| r.contains("blocking findings present")));
    }

    #[test]
    fn test_approve_when_all_preconditions_met() {
        let input = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.to_uppercase()), // case-insensitive equality
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(result.can_merge);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn test_parse_verdict_text_multi_digits() {
        let comment = r#"
### Verdict
Changes Requested, P0=12, P1=3
Commit: 1234567890abcdef1234567890abcdef12345678
"#;
        let (verdict, sha, p0, p1) = parse_verdict_text(comment);
        assert_eq!(verdict.as_deref(), Some("Changes Requested"));
        assert_eq!(
            sha.as_deref(),
            Some("1234567890abcdef1234567890abcdef12345678")
        );
        assert_eq!(p0, 12);
        assert_eq!(p1, 3);
    }

    #[test]
    fn test_is_structured_review_comment() {
        assert!(is_structured_review_comment(
            "## Code Review: Round 1\nLGTM"
        ));
        assert!(is_structured_review_comment(
            "### Verdict\nChanges Requested"
        ));
        assert!(is_structured_review_comment("<<<VERDICT\nLGTM\nVERDICT>>>"));
        assert!(is_structured_review_comment(
            "Verdict as of the ruling: LGTM"
        ));
        assert!(is_structured_review_comment(
            "**Verdict as of the ruling: LGTM**"
        ));
        // Casual comments must NOT match
        assert!(!is_structured_review_comment("ready for LGTM review"));
        assert!(!is_structured_review_comment(
            "I fixed the bug, please LGTM"
        ));
        assert!(!is_structured_review_comment("Not an LGTM yet"));
    }

    #[test]
    fn test_formal_review_state_not_overridden_by_prose() {
        let gh_view = GhViewPr {
            head_ref_oid: VALID_SHA_A.into(),
            author: Some(GhAuthor {
                login: "fagemx".into(),
            }),
            comments: vec![],
            reviews: vec![GhReviewItem {
                state: "CHANGES_REQUESTED".into(),
                author: Some(GhAuthor {
                    login: "reviewer".into(),
                }),
                body: Some(format!(
                    "Not an LGTM yet - see inline notes. @ {VALID_SHA_A}"
                )),
                submitted_at: Some("2026-09-02T10:00:00Z".into()),
            }],
        };

        let (verdict, sha, p0, p1) = extract_timeline_verdict(&gh_view);
        assert_eq!(verdict.as_deref(), Some("Changes Requested"));
        assert_eq!(sha.as_deref(), Some(VALID_SHA_A));
        assert_eq!(p0, 0);
        assert_eq!(p1, 0);
    }

    #[test]
    fn test_timeline_chronological_order() {
        let gh_view = GhViewPr {
            head_ref_oid: VALID_SHA_A.into(),
            author: Some(GhAuthor {
                login: "fagemx".into(),
            }),
            comments: vec![GhComment {
                body: format!(
                    "## Code Review: Round 1\n\n### Verdict\nLGTM (P0=0, P1=0) at `{VALID_SHA_A}`"
                ),
                author: Some(GhAuthor {
                    login: "fagemx".into(),
                }),
                created_at: Some("2026-09-02T08:00:00Z".into()),
            }],
            reviews: vec![GhReviewItem {
                state: "CHANGES_REQUESTED".into(),
                author: Some(GhAuthor {
                    login: "fagemx".into(),
                }),
                body: Some(format!("blocking findings @ {VALID_SHA_A}")),
                submitted_at: Some("2026-09-02T09:00:00Z".into()),
            }],
        };

        // Newer CHANGES_REQUESTED review (09:00) wins over older LGTM comment (08:00)
        let (verdict, _, _, _) = extract_timeline_verdict(&gh_view);
        assert_eq!(verdict.as_deref(), Some("Changes Requested"));
    }

    #[test]
    fn test_run_check_merge_input_file_report_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input_file = temp_dir.path().join("input.json");
        let valid_input = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        fs::write(&input_file, serde_json::to_string(&valid_input).unwrap()).unwrap();

        let args = CheckMergeArgs {
            pr: None,
            head_sha: None,
            verdict_sha: None,
            verdict: None,
            p0: 0,
            p1: 0,
            ci_green: false,
            input: Some(input_file.to_str().unwrap().into()),
            merge: false,
            json: true,
        };

        let res = run_check_merge(args, temp_dir.path());
        assert!(res.is_ok());
    }

    #[test]
    fn test_run_check_merge_rejects_merge_with_input_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input_file = temp_dir.path().join("input.json");
        fs::write(&input_file, "{}").unwrap();

        let args = CheckMergeArgs {
            pr: None,
            head_sha: None,
            verdict_sha: None,
            verdict: None,
            p0: 0,
            p1: 0,
            ci_green: false,
            input: Some(input_file.to_str().unwrap().into()),
            merge: true,
            json: false,
        };

        let res = run_check_merge(args, temp_dir.path());
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("cannot be used with '--input'"));
    }
}
