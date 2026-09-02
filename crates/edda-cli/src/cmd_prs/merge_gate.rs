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
    #[arg(value_name = "PR")]
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

/// Pure evaluation function without host or network dependencies.
pub fn evaluate_merge_preconditions(input: &MergeGateInput) -> MergeGateResult {
    let mut reasons = Vec::new();

    // 1. Verdict presence & state
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

    // 2. Blocking findings (P0=0 and P1=0)
    if input.p0_count > 0 || input.p1_count > 0 {
        reasons.push(format!(
            "blocking findings present: {} P0, {} P1 (both must be 0)",
            input.p0_count, input.p1_count
        ));
    }

    // 3. SHA window check (verdict SHA must match current PR head)
    if let Some(v_sha) = &input.verdict_sha {
        let v_clean = v_sha.trim();
        let h_clean = input.head_sha.trim();
        let matches = if v_clean.len() >= 7 && h_clean.len() >= 7 {
            v_clean == h_clean || v_clean.starts_with(h_clean) || h_clean.starts_with(v_clean)
        } else {
            v_clean == h_clean
        };

        if !matches {
            reasons.push(format!(
                "SHA window mismatch: verdict pinned to '{v_clean}' but current head is '{h_clean}' (new commits pushed after review)"
            ));
        }
    } else if input.verdict.is_some() {
        reasons.push("review verdict is not pinned to any commit SHA".to_string());
    }

    // 4. Required CI checks
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
#[derive(Debug, Deserialize)]
struct GhViewPr {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(default)]
    comments: Vec<GhComment>,
    #[serde(default)]
    reviews: Vec<GhReviewItem>,
}

#[derive(Debug, Deserialize)]
struct GhComment {
    body: String,
    #[serde(rename = "createdAt")]
    _created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhReviewItem {
    state: String,
    body: Option<String>,
    #[serde(rename = "submittedAt")]
    _submitted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCheckItem {
    name: String,
    state: Option<String>,
    bucket: Option<String>,
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
        if let Some(idx) = line.find("P0=") {
            let rest = &line[idx + 3..];
            if let Some(digit) = rest.chars().next().and_then(|c| c.to_digit(10)) {
                p0 = digit as usize;
            }
        } else if let Some(idx) = line.find("P0:") {
            let rest = line[idx + 3..].trim();
            if let Some(digit) = rest.chars().next().and_then(|c| c.to_digit(10)) {
                p0 = digit as usize;
            }
        }

        if let Some(idx) = line.find("P1=") {
            let rest = &line[idx + 3..];
            if let Some(digit) = rest.chars().next().and_then(|c| c.to_digit(10)) {
                p1 = digit as usize;
            }
        } else if let Some(idx) = line.find("P1:") {
            let rest = line[idx + 3..].trim();
            if let Some(digit) = rest.chars().next().and_then(|c| c.to_digit(10)) {
                p1 = digit as usize;
            }
        }
    }

    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| {
            c == '*'
                || c == '`'
                || c == '('
                || c == ')'
                || c == '.'
                || c == ','
                || c == ':'
                || c == '"'
                || c == '\''
                || c == '['
                || c == ']'
        });
        if clean.len() == 40 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
            verdict_sha = Some(clean.to_string());
            break;
        }
    }

    (verdict, verdict_sha, p0, p1)
}

fn fetch_pr_from_gh(pr: u64, repo_root: &Path) -> anyhow::Result<MergeGateInput> {
    let view_output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "headRefOid,comments,reviews",
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

    let head_sha = gh_view.head_ref_oid;

    let mut verdict = None;
    let mut verdict_sha = None;
    let mut p0 = 0;
    let mut p1 = 0;

    for r in gh_view.reviews.iter().rev() {
        if r.state == "APPROVED" {
            verdict = Some("LGTM".to_string());
            if let Some(body) = &r.body {
                let (_, sha, parsed_p0, parsed_p1) = parse_verdict_text(body);
                if sha.is_some() {
                    verdict_sha = sha;
                }
                p0 = parsed_p0;
                p1 = parsed_p1;
            }
            break;
        } else if r.state == "CHANGES_REQUESTED" {
            verdict = Some("Changes Requested".to_string());
            if let Some(body) = &r.body {
                let (_, sha, parsed_p0, parsed_p1) = parse_verdict_text(body);
                if sha.is_some() {
                    verdict_sha = sha;
                }
                p0 = parsed_p0;
                p1 = parsed_p1;
            }
            break;
        }
    }

    for comment in gh_view.comments.iter().rev() {
        let (v, sha, parsed_p0, parsed_p1) = parse_verdict_text(&comment.body);
        if let Some(found_v) = v {
            verdict = Some(found_v);
            if sha.is_some() {
                verdict_sha = sha;
            }
            p0 = parsed_p0;
            p1 = parsed_p1;
            break;
        }
    }

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
                let pass =
                    c.bucket.as_deref() == Some("pass") || c.state.as_deref() == Some("SUCCESS");
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
            if let Some(pr_number) = args.pr {
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
                anyhow::bail!("--merge requires a PR number positional argument");
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

    #[test]
    fn test_refuse_when_no_verdict() {
        let input = MergeGateInput {
            head_sha: "abc1234567890123456789012345678901234567".into(),
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
            head_sha: "new1234567890123456789012345678901234567".into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some("old1234567890123456789012345678901234567".into()),
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
    fn test_refuse_when_ci_not_green() {
        let sha = "1234567890123456789012345678901234567890";
        let input = MergeGateInput {
            head_sha: sha.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(sha.into()),
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
        let sha = "1234567890123456789012345678901234567890";
        // Case A: Changes Requested
        let input_cr = MergeGateInput {
            head_sha: sha.into(),
            verdict: Some("Changes Requested".into()),
            verdict_sha: Some(sha.into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let res_cr = evaluate_merge_preconditions(&input_cr);
        assert!(!res_cr.can_merge);
        assert!(res_cr.reasons.iter().any(|r| r.contains("not LGTM")));

        // Case B: P0 > 0
        let input_p0 = MergeGateInput {
            head_sha: sha.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(sha.into()),
            p0_count: 1,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
        };
        let res_p0 = evaluate_merge_preconditions(&input_p0);
        assert!(!res_p0.can_merge);
        assert!(res_p0
            .reasons
            .iter()
            .any(|r| r.contains("blocking findings present")));

        // Case C: P1 > 0
        let input_p1 = MergeGateInput {
            head_sha: sha.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(sha.into()),
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
        let sha = "fa208a3ab4911d3fa9f2dcd60fb332a8575f5b69";
        let input = MergeGateInput {
            head_sha: sha.into(),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(sha.into()),
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
    fn test_parse_verdict_text_extraction() {
        let comment = r#"
### Verdict
LGTM (P0=0, P1=0) at `fa208a3ab4911d3fa9f2dcd60fb332a8575f5b69`
CI is all green.
"#;
        let (verdict, sha, p0, p1) = parse_verdict_text(comment);
        assert_eq!(verdict.as_deref(), Some("LGTM"));
        assert_eq!(
            sha.as_deref(),
            Some("fa208a3ab4911d3fa9f2dcd60fb332a8575f5b69")
        );
        assert_eq!(p0, 0);
        assert_eq!(p1, 0);
    }
}
