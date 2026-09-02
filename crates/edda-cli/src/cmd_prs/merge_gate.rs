use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// Command line arguments for `edda prs check-merge`
#[derive(Debug, Clone, Args, Default)]
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

    /// Optional author of the review verdict
    #[arg(long)]
    pub verdict_author: Option<String>,

    /// Count of P0 blocking findings
    #[arg(long, default_value_t = 0)]
    pub p0: usize,

    /// Count of P1 blocking findings
    #[arg(long, default_value_t = 0)]
    pub p1: usize,

    /// Mark required CI checks as green
    #[arg(long)]
    pub ci_green: bool,

    /// Optional list of allowed reviewer usernames
    #[arg(long, value_delimiter = ',')]
    pub allowed_reviewers: Option<Vec<String>>,

    /// Path to JSON file containing MergeGateInput (or '-' for stdin)
    #[arg(long, value_name = "FILE")]
    pub input: Option<String>,

    /// Execute `gh pr merge <PR> --squash` if preconditions pass
    #[arg(long)]
    pub merge: bool,

    /// Output evaluation report as JSON
    #[arg(long)]
    pub json: bool,

    /// Override process claim held by another session (GH-581)
    #[arg(long)]
    pub force: bool,
}

/// Host-agnostic input for evaluating merge preconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MergeGateInput {
    pub head_sha: String,
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

/// Evaluation outcome for merge preconditions.
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

    // 6. Process object claim protection (GH-581)
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

// GitHub API / `gh` structures
#[derive(Debug, Clone, Deserialize)]
pub struct GhViewPr {
    #[serde(rename = "headRefOid")]
    pub head_ref_oid: String,
    pub author: Option<GhAuthor>,
    #[serde(default)]
    pub comments: Vec<GhComment>,
    #[serde(default)]
    pub reviews: Vec<GhReviewItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhAuthor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhComment {
    pub body: String,
    pub author: Option<GhAuthor>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhReviewItem {
    pub state: String,
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

/// Determines if a comment is a structured review verdict, preventing casual comments and
/// implementer responses from triggering a verdict.
pub fn is_structured_review_comment(body: &str) -> bool {
    // 1. Explicitly reject implementer review responses
    if body.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("## Review Response") || trimmed.starts_with("# Review Response")
    }) {
        return false;
    }

    // 2. Must contain an anchored review header line
    let has_review_header = body.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("## Code Review:")
            || trimmed.starts_with("### Verdict")
            || trimmed.starts_with("<<<VERDICT")
            || trimmed.starts_with("## Operator ruling")
            || trimmed.starts_with("Verdict as of the ruling:")
            || trimmed.starts_with("**Verdict as of the ruling:")
    });

    if !has_review_header {
        return false;
    }

    // 3. Must contain an explicit verdict indicator line
    body.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("### Verdict")
            || trimmed.starts_with("Verdict as of the ruling:")
            || trimmed.starts_with("**Verdict as of the ruling:")
            || trimmed.starts_with("- verdict:")
            || trimmed.starts_with("**Verdict:")
            || trimmed.starts_with("Verdict:")
            || trimmed.starts_with("LGTM (")
            || trimmed.starts_with("**LGTM")
            || trimmed.starts_with("Changes Requested,")
            || trimmed.starts_with("**Changes Requested")
    })
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

    // Look for SHA in header line first: "## Code Review: ... @ <SHA>"
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## Code Review:") || trimmed.starts_with("## Review:") {
            if let Some(at_idx) = trimmed.rfind('@') {
                let after = trimmed[at_idx + 1..].trim();
                let clean =
                    after.trim_matches(['*', '`', '(', ')', '.', ',', ':', '"', '\'', '[', ']']);
                let sha_candidate: String = clean
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .collect();
                if sha_candidate.len() == 40 {
                    verdict_sha = Some(sha_candidate);
                    break;
                }
            }
        }
    }

    // Focus parsing on the final verdict section if present (using rfind to avoid matching earlier quotes)
    let target_section = if let Some(idx) = text.rfind("\n### Verdict") {
        &text[idx..]
    } else if let Some(idx) = text.rfind("\n<<<VERDICT") {
        &text[idx..]
    } else if let Some(idx) = text.rfind("Verdict as of the ruling:") {
        &text[idx..]
    } else if let Some(idx) = text.rfind("### Verdict") {
        &text[idx..]
    } else {
        text
    };

    for line in target_section.lines() {
        let trimmed = line.trim().trim_matches('*').trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("<<<") {
            continue;
        }
        let clean = if let Some(idx) = trimmed.find("Verdict as of the ruling:") {
            trimmed[idx + "Verdict as of the ruling:".len()..]
                .trim()
                .trim_matches('*')
                .trim()
        } else if let Some(idx) = trimmed.find("Verdict:") {
            trimmed[idx + "Verdict:".len()..]
                .trim()
                .trim_matches('*')
                .trim()
        } else {
            trimmed
        };

        if clean.starts_with("LGTM") || clean.starts_with("Approved") {
            verdict = Some("LGTM".to_string());
            break;
        } else if clean.starts_with("Changes Requested") || clean.starts_with("changes_requested") {
            verdict = Some("Changes Requested".to_string());
            break;
        }
    }

    if verdict.is_none() {
        let has_changes_requested = target_section.contains("Changes Requested")
            || target_section.contains("changes_requested");
        let has_lgtm = target_section.contains("LGTM") || target_section.contains("lgtm");

        if has_changes_requested {
            verdict = Some("Changes Requested".to_string());
        } else if has_lgtm {
            verdict = Some("LGTM".to_string());
        }
    }

    for line in target_section.lines() {
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

    if verdict_sha.is_none() {
        for word in target_section.split_whitespace() {
            let clean = word.trim_matches(['*', '`', '(', ')', '.', ',', ':', '"', '\'', '[', ']']);
            if clean.len() == 40 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
                verdict_sha = Some(clean.to_string());
                break;
            }
        }
    }

    (verdict, verdict_sha, p0, p1)
}

#[derive(Debug, Clone)]
struct ReviewTimelineEvent {
    timestamp: String,
    author: Option<String>,
    verdict: String,
    verdict_sha: Option<String>,
    p0_count: usize,
    p1_count: usize,
}

/// Pure timeline extractor: processes reviews and structured comments into the final verdict.
pub fn extract_timeline_verdict(
    gh_view: &GhViewPr,
    allowed_reviewers: Option<&[String]>,
) -> (Option<String>, Option<String>, Option<String>, usize, usize) {
    let mut timeline: Vec<ReviewTimelineEvent> = Vec::new();

    // 1. Process GitHub formal reviews (APPROVED / CHANGES_REQUESTED)
    for r in &gh_view.reviews {
        if let Some(allowed) = allowed_reviewers {
            match &r.author {
                Some(author)
                    if allowed
                        .iter()
                        .any(|u| u.eq_ignore_ascii_case(&author.login)) => {}
                _ => continue, // Fail closed: unauthenticated/unattributable events are rejected
            }
        }

        let verdict_opt = match r.state.as_str() {
            "APPROVED" => Some("LGTM".to_string()),
            "CHANGES_REQUESTED" => Some("Changes Requested".to_string()),
            _ => None,
        };

        if let Some(formal_verdict) = verdict_opt {
            let (_, sha, parsed_p0, parsed_p1) = if let Some(body) = &r.body {
                parse_verdict_text(body)
            } else {
                (None, None, 0, 0)
            };

            let ts = r.submitted_at.clone().unwrap_or_default();
            let author = r.author.as_ref().map(|a| a.login.clone());

            timeline.push(ReviewTimelineEvent {
                timestamp: ts,
                author,
                verdict: formal_verdict,
                verdict_sha: sha,
                p0_count: parsed_p0,
                p1_count: parsed_p1,
            });
        }
    }

    // 2. Process structured review comments
    for comment in &gh_view.comments {
        if let Some(allowed) = allowed_reviewers {
            match &comment.author {
                Some(author)
                    if allowed
                        .iter()
                        .any(|u| u.eq_ignore_ascii_case(&author.login)) => {}
                _ => continue, // Fail closed: unauthenticated/unattributable events are rejected
            }
        }

        if !is_structured_review_comment(&comment.body) {
            continue;
        }

        let (v, sha, parsed_p0, parsed_p1) = parse_verdict_text(&comment.body);
        if let Some(found_v) = v {
            let ts = comment.created_at.clone().unwrap_or_default();
            let author = comment.author.as_ref().map(|a| a.login.clone());

            timeline.push(ReviewTimelineEvent {
                timestamp: ts,
                author,
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
            latest.author,
            latest.p0_count,
            latest.p1_count,
        )
    } else {
        (None, None, None, 0, 0)
    }
}

fn fetch_pr_from_gh(
    pr: u64,
    allowed_reviewers: Option<&[String]>,
    repo_root: &Path,
) -> anyhow::Result<MergeGateInput> {
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
    let (verdict, verdict_sha, verdict_author, p0, p1) =
        extract_timeline_verdict(&gh_view, allowed_reviewers);

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
        pr_author: gh_view.author.as_ref().map(|a| a.login.clone()),
        verdict,
        verdict_sha,
        verdict_author,
        p0_count: p0,
        p1_count: p1,
        required_ci_green,
        failed_checks,
        claimed_by: None,
        force: false,
    })
}

/// Format standard CLI output reports for merge precondition results.
pub fn format_merge_report(result: &MergeGateResult) -> (String, String) {
    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();

    if result.can_merge {
        use std::fmt::Write;
        let _ = writeln!(stdout_buf, "PASS: Merge preconditions satisfied.");
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
        use std::fmt::Write;
        let _ = writeln!(
            stderr_buf,
            "REFUSED: Merge preconditions not satisfied for head {}:",
            result.head_sha
        );
        if let Some(author) = &result.pr_author {
            let _ = writeln!(stderr_buf, "  PR Author:   {author}");
        }
        for reason in &result.reasons {
            let _ = writeln!(stderr_buf, "  - {reason}");
        }
    }

    (stdout_buf, stderr_buf)
}

/// Execute the merge precondition check CLI entrypoint.
pub fn run_check_merge(args: CheckMergeArgs, repo_root: &Path) -> anyhow::Result<()> {
    if args.merge && (args.pr.is_none() || args.input.is_some()) {
        anyhow::bail!("--merge requires a live PR number and cannot be used with '--input'");
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
    } else if let Some(pr_number) = args.pr {
        fetch_pr_from_gh(pr_number, args.allowed_reviewers.as_deref(), repo_root)?
    } else if let Some(head_sha) = args.head_sha {
        MergeGateInput {
            head_sha,
            pr_author: None,
            verdict: args.verdict,
            verdict_sha: args.verdict_sha,
            verdict_author: args.verdict_author,
            p0_count: args.p0,
            p1_count: args.p1,
            required_ci_green: args.ci_green,
            failed_checks: Vec::new(),
            claimed_by: None,
            force: args.force,
        }
    } else {
        anyhow::bail!(
            "Specify either a PR number (e.g. 'edda prs check-merge 580') or '--input <FILE>'"
        );
    };

    if args.force {
        input.force = true;
    }

    if let Some(pr_number) = args.pr {
        if input.claimed_by.is_none() {
            let project_id = edda_store::project_id(repo_root);
            let target_subject = format!("pr:{pr_number}");
            let board = edda_bridge_claude::peers::compute_board_state(&project_id);
            if let Some(c) = board.claims.iter().find(|c| {
                c.subject
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&target_subject))
                    .unwrap_or(false)
            }) {
                let is_live = matches!(
                    edda_bridge_claude::peers::classify_session_liveness(
                        &project_id,
                        &c.session_id,
                    ),
                    edda_bridge_claude::peers::SessionLiveness::Live { .. }
                );
                if is_live {
                    let my_sid = std::env::var("EDDA_SESSION_ID").ok();
                    if my_sid.as_deref() != Some(&c.session_id) {
                        input.claimed_by = Some(format!(
                            "{} (label: '{}', subject: '{target_subject}')",
                            c.session_id, c.label
                        ));
                    }
                }
            }
        }
    }

    let result = evaluate_merge_preconditions(&input);

    if args.json {
        let json_out = serde_json::to_string_pretty(&result)?;
        println!("{json_out}");
    } else {
        let (out, err) = format_merge_report(&result);
        if !out.is_empty() {
            print!("{out}");
        }
        if !err.is_empty() {
            eprint!("{err}");
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
            pr_author: None,
            verdict: None,
            verdict_sha: None,
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
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
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
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
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some("short".into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
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
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: false,
            failed_checks: vec!["CI Gate (fail)".into()],
            ..Default::default()
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
            pr_author: None,
            verdict: Some("Changes Requested".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
        };
        let res_cr = evaluate_merge_preconditions(&input_cr);
        assert!(!res_cr.can_merge);
        assert!(res_cr.reasons.iter().any(|r| r.contains("not LGTM")));

        // Case B: P0 > 0 (e.g. 12)
        let input_p0 = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 12,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
        };
        let res_p0 = evaluate_merge_preconditions(&input_p0);
        assert!(!res_p0.can_merge);
        assert!(res_p0.reasons.iter().any(|r| r.contains("12 P0")));

        // Case C: P1 > 0
        let input_p1 = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: None,
            p0_count: 0,
            p1_count: 2,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
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
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.to_uppercase()), // case-insensitive equality
            verdict_author: Some("opus-reviewer".into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
        };
        let result = evaluate_merge_preconditions(&input);
        assert!(result.can_merge);
        assert!(result.reasons.is_empty());
        assert_eq!(result.verdict_author.as_deref(), Some("opus-reviewer"));
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
            "## Code Review: Round 1\n\n### Verdict\nLGTM (P0=0, P1=0) at `1234567890abcdef1234567890abcdef12345678`"
        ));
        assert!(is_structured_review_comment(
            "### Verdict\nChanges Requested"
        ));
        assert!(is_structured_review_comment(
            "<<<VERDICT\n**Verdict:** LGTM\nVERDICT>>>"
        ));
        assert!(is_structured_review_comment(
            "## Operator ruling on #699\n\nVerdict as of the ruling: LGTM"
        ));

        // Implementer review responses must NEVER match
        assert!(!is_structured_review_comment(
            "## Review Response: Round 1 — PR #711\n\n- Requires comments to be structured review verdicts via is_structured_review_comment (e.g. ## Code Review:)"
        ));
        assert!(!is_structured_review_comment(
            "# Review Response: Round 2\n\nLGTM is mentioned here"
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

        let (verdict, sha, author, p0, p1) = extract_timeline_verdict(&gh_view, None);
        assert_eq!(verdict.as_deref(), Some("Changes Requested"));
        assert_eq!(sha.as_deref(), Some(VALID_SHA_A));
        assert_eq!(author.as_deref(), Some("reviewer"));
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
                    login: "reviewer-a".into(),
                }),
                created_at: Some("2026-09-02T08:00:00Z".into()),
            }],
            reviews: vec![GhReviewItem {
                state: "CHANGES_REQUESTED".into(),
                author: Some(GhAuthor {
                    login: "reviewer-b".into(),
                }),
                body: Some(format!("blocking findings @ {VALID_SHA_A}")),
                submitted_at: Some("2026-09-02T09:00:00Z".into()),
            }],
        };

        // Newer CHANGES_REQUESTED review (09:00) wins over older LGTM comment (08:00)
        let (verdict, _, author, _, _) = extract_timeline_verdict(&gh_view, None);
        assert_eq!(verdict.as_deref(), Some("Changes Requested"));
        assert_eq!(author.as_deref(), Some("reviewer-b"));
    }

    #[test]
    fn test_allowed_reviewers_filter() {
        let allowed = vec!["authorized-reviewer".to_string()];

        // Case 1: Unauthorized user comment is rejected
        let gh_view_unauth = GhViewPr {
            head_ref_oid: VALID_SHA_A.into(),
            author: Some(GhAuthor {
                login: "fagemx".into(),
            }),
            comments: vec![GhComment {
                body: format!(
                    "## Code Review: Round 1\n\n### Verdict\nLGTM (P0=0, P1=0) at `{VALID_SHA_A}`"
                ),
                author: Some(GhAuthor {
                    login: "unauthorized-user".into(),
                }),
                created_at: Some("2026-09-02T08:00:00Z".into()),
            }],
            reviews: vec![],
        };
        let (verdict, _, _, _, _) = extract_timeline_verdict(&gh_view_unauth, Some(&allowed));
        assert!(verdict.is_none());

        // Case 2: Unattributable (author: None) comment/review is rejected (fail closed)
        let gh_view_none = GhViewPr {
            head_ref_oid: VALID_SHA_A.into(),
            author: Some(GhAuthor {
                login: "fagemx".into(),
            }),
            comments: vec![GhComment {
                body: format!(
                    "## Code Review: Round 1\n\n### Verdict\nLGTM (P0=0, P1=0) at `{VALID_SHA_A}`"
                ),
                author: None,
                created_at: Some("2026-09-02T08:00:00Z".into()),
            }],
            reviews: vec![GhReviewItem {
                state: "APPROVED".into(),
                author: None,
                body: Some(format!("LGTM @ {VALID_SHA_A}")),
                submitted_at: Some("2026-09-02T08:30:00Z".into()),
            }],
        };
        let (verdict, _, _, _, _) = extract_timeline_verdict(&gh_view_none, Some(&allowed));
        assert!(verdict.is_none());

        // Case 3: Authorized reviewer is accepted
        let gh_view_auth = GhViewPr {
            head_ref_oid: VALID_SHA_A.into(),
            author: Some(GhAuthor {
                login: "fagemx".into(),
            }),
            comments: vec![GhComment {
                body: format!(
                    "## Code Review: Round 1\n\n### Verdict\nLGTM (P0=0, P1=0) at `{VALID_SHA_A}`"
                ),
                author: Some(GhAuthor {
                    login: "authorized-reviewer".into(),
                }),
                created_at: Some("2026-09-02T08:00:00Z".into()),
            }],
            reviews: vec![],
        };
        let (verdict, _, author, _, _) = extract_timeline_verdict(&gh_view_auth, Some(&allowed));
        assert_eq!(verdict.as_deref(), Some("LGTM"));
        assert_eq!(author.as_deref(), Some("authorized-reviewer"));
    }

    #[test]
    fn test_run_check_merge_input_file_report_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input_file = temp_dir.path().join("input.json");
        let valid_input = MergeGateInput {
            head_sha: VALID_SHA_A.into(),
            pr_author: None,
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: Some("reviewer".into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            failed_checks: vec![],
            ..Default::default()
        };
        fs::write(&input_file, serde_json::to_string(&valid_input).unwrap()).unwrap();

        let args = CheckMergeArgs {
            pr: None,
            head_sha: None,
            verdict_sha: None,
            verdict: None,
            verdict_author: None,
            p0: 0,
            p1: 0,
            ci_green: false,
            allowed_reviewers: None,
            input: Some(input_file.to_str().unwrap().into()),
            merge: false,
            json: true,
            ..Default::default()
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
            verdict_author: None,
            p0: 0,
            p1: 0,
            ci_green: false,
            allowed_reviewers: None,
            input: Some(input_file.to_str().unwrap().into()),
            merge: true,
            json: false,
            ..Default::default()
        };

        let res = run_check_merge(args, temp_dir.path());
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("cannot be used with '--input'"));
    }

    #[test]
    fn test_format_merge_report_pass() {
        let result = MergeGateResult {
            can_merge: true,
            head_sha: VALID_SHA_A.into(),
            pr_author: Some("alice".into()),
            verdict: Some("LGTM".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: Some("bob".into()),
            p0_count: 0,
            p1_count: 0,
            required_ci_green: true,
            reasons: vec![],
            ..Default::default()
        };

        let (stdout, stderr) = format_merge_report(&result);
        assert!(stderr.is_empty(), "PASS report must not write to stderr");
        assert!(stdout.contains("PASS: Merge preconditions satisfied."));
        assert!(stdout.contains("PR Head:     1234567890abcdef1234567890abcdef12345678"));
        assert!(stdout.contains("PR Author:   alice"));
        assert!(stdout.contains("Verdict:     LGTM by bob"));
        assert!(stdout.contains("Required CI: green"));
    }

    #[test]
    fn test_format_merge_report_refused() {
        let result = MergeGateResult {
            can_merge: false,
            head_sha: VALID_SHA_A.into(),
            pr_author: Some("alice".into()),
            verdict: Some("Changes Requested".into()),
            verdict_sha: Some(VALID_SHA_A.into()),
            verdict_author: Some("bob".into()),
            p0_count: 1,
            p1_count: 1,
            required_ci_green: false,
            reasons: vec![
                "review verdict is not LGTM (found 'Changes Requested')".into(),
                "blocking findings present: 1 P0, 1 P1 (both must be 0)".into(),
                "required CI check(s) are not green".into(),
            ],
            ..Default::default()
        };

        let (stdout, stderr) = format_merge_report(&result);
        assert!(stdout.is_empty(), "REFUSED report must not write to stdout");
        assert!(stderr.contains(
            "REFUSED: Merge preconditions not satisfied for head 1234567890abcdef1234567890abcdef12345678:"
        ));
        assert!(stderr.contains("PR Author:   alice"));
        assert!(stderr.contains("  - review verdict is not LGTM"));
        assert!(stderr.contains("  - blocking findings present: 1 P0, 1 P1"));
        assert!(stderr.contains("  - required CI check(s) are not green"));
    }
}
