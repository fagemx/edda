//! GitHub claim guard shared by dispatch and the fleet convention (GH-782).
//!
//! Only `taking: <machine>/<role>` comments carry ownership. Queue labels
//! are written, never interpreted as claims. Open or delivered PRs also
//! block dispatch, even if queue labels and comments are stale.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimState {
    Unclaimed,
    ClaimedBySelf,
    ClaimedByOther {
        machine: String,
        when: Option<String>,
        source: String,
    },
    InFlight {
        pr: u64,
        state: PrState,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

impl PrState {
    pub fn refusal(self, issue: u64, pr: u64) -> String {
        match self {
            Self::Merged => format!("issue {issue} delivered by #{pr} (merged) — drop fleet:ready"),
            _ => format!("issue {issue} has open PR #{pr}; dispatch refused"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    comments: Vec<GhComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhComment {
    body: String,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    state: PrState,
    title: String,
    head_ref_name: String,
}

/// Identity is an explicit machine/role pair, with no whitespace or extra slash.
/// Bare historical claim markers remain foreign; new callers cannot use them.
pub fn validate_machine(machine: &str) -> Result<()> {
    let parts: Vec<_> = machine.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| part.is_empty())
        || machine.chars().any(char::is_whitespace)
    {
        bail!("machine identity must be <machine>/<role> without whitespace, got {machine:?}");
    }
    Ok(())
}

fn run_gh(args: &[&str]) -> Result<Vec<u8>> {
    let gh = std::env::var_os("EDDA_GH_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"));
    let output = std::process::Command::new(&gh)
        .args(args)
        .output()
        .with_context(|| format!("claim guard: could not run {}", gh.display()))?;
    if !output.status.success() {
        bail!(
            "claim guard: gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// Read comments and the PR history. gh's default 30-row page must not hide
/// older deliveries, so request its full practical result limit explicitly.
pub fn fetch_claim_state(issue: u64, ours: &str) -> Result<ClaimState> {
    validate_machine(ours)?;
    let output = run_gh(&["issue", "view", &issue.to_string(), "--json", "comments"])?;
    let parsed: GhIssue = serde_json::from_slice(&output)
        .with_context(|| format!("claim guard: could not parse gh issue {issue}"))?;
    let output = run_gh(&[
        "pr",
        "list",
        "--state",
        "all",
        "--limit",
        "1000000",
        "--json",
        "number,state,title,headRefName",
    ])?;
    let prs: Vec<GhPr> =
        serde_json::from_slice(&output).context("claim guard: could not parse gh PR history")?;
    Ok(pr_state(issue, &prs).unwrap_or_else(|| claim_state(ours, &parsed)))
}

/// Complete the claim before starting an agent. Re-dispatch repairs a partial
/// label/assignee write without adding another taking comment. GitHub provides
/// no compare-and-swap across these calls; this is not a distributed lock.
pub fn write_claim(issue: u64, ours: &str, already_claimed: bool) -> Result<()> {
    validate_machine(ours)?;
    let issue = issue.to_string();
    if !already_claimed {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)?;
        run_gh(&[
            "issue",
            "comment",
            &issue,
            "--body",
            &format!("taking: {ours} at {now}"),
        ])?;
    }
    run_gh(&[
        "issue",
        "edit",
        &issue,
        "--add-label",
        "fleet:claimed",
        "--remove-label",
        "fleet:ready",
        "--add-assignee",
        "@me",
    ])?;
    Ok(())
}

fn pr_state(issue: u64, prs: &[GhPr]) -> Option<ClaimState> {
    // Prefer a delivery over an open duplicate, independent of API order.
    for state in [PrState::Merged, PrState::Open] {
        if let Some(pr) = prs.iter().find(|pr| {
            pr.state == state
                && (references_issue(&pr.title, &format!("gh-{issue}"))
                    || references_issue(&pr.head_ref_name, &format!("gh{issue}")))
        }) {
            return Some(ClaimState::InFlight {
                pr: pr.number,
                state,
            });
        }
    }
    None
}

fn references_issue(text: &str, marker: &str) -> bool {
    // GH-65 must not match GH-656. Prefixes and punctuation around the
    // full issue marker remain valid in both titles and branch names.
    text.to_ascii_lowercase()
        .match_indices(marker)
        .any(|(start, _)| {
            !text
                .as_bytes()
                .get(start + marker.len())
                .is_some_and(u8::is_ascii_digit)
        })
}

fn claim_state(ours: &str, issue: &GhIssue) -> ClaimState {
    let mut claimed_by_self = false;
    for comment in &issue.comments {
        for machine in taking_tokens(&comment.body) {
            if machine != ours {
                return ClaimState::ClaimedByOther {
                    machine: machine.to_owned(),
                    when: comment.created_at.clone(),
                    source: "comment \"taking:\"".to_owned(),
                };
            }
            claimed_by_self = true;
        }
    }
    if claimed_by_self {
        ClaimState::ClaimedBySelf
    } else {
        ClaimState::Unclaimed
    }
}

/// Every line-start marker is checked, including multiple markers in one
/// comment. A prose mention mid-sentence is not ownership.
fn taking_tokens(body: &str) -> impl Iterator<Item = &str> {
    body.lines().filter_map(|line| {
        line.trim_start()
            .strip_prefix("taking:")
            .and_then(|rest| rest.split_whitespace().next())
    })
}

#[cfg(test)]
#[path = "claim_guard_tests.rs"]
mod tests;
