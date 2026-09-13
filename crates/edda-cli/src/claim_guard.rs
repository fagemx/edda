//! GitHub claim guard shared by dispatch and the fleet convention (GH-782).
//!
//! Only `taking: <machine>/<role>` comments carry ownership. Queue labels
//! are written, never interpreted as claims. Open or delivered PRs also
//! block dispatch, even if queue labels and comments are stale.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::cmd_dispatch::DispatchArgs;

#[cfg(test)]
pub(crate) static GH_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    state: Option<IssueState>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
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

/// Report the claim guard's decision for a dispatch turn. `Ok(None)` means
/// dispatch may proceed; `Ok(Some(2))` means the issue is claimed by another
/// machine and the turn must not start. Lives here so `cmd_dispatch` keeps its
/// file-length ratchet while the guard stays next to the rest of claim logic.
pub(crate) fn dispatch_claim_outcome(
    args: &crate::cmd_dispatch::DispatchArgs,
    cwd: &Path,
) -> Result<Option<i32>> {
    let Some((issue, machine, reason)) = claim_guard_refusal(args, cwd)? else {
        return Ok(None);
    };
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "outcome": "claim_refused",
                "error": reason,
                "issue": issue,
                "machine": machine,
            })
        );
    } else {
        eprintln!("Error: {reason}");
    }
    Ok(Some(2))
}

/// The cross-machine claim guard for one dispatch (GH-656). `Ok(None)`
/// means dispatch may proceed; `Ok(Some((issue, machine, reason)))` means
/// the issue is claimed by another machine and the turn must not start.
/// Runs only when `--issue` is given; an explicit `--machine` without
/// `--issue` is refused (it could never fire the guard, so accepting it
/// would silently drop it — the GH-574 honesty rule). The machine label
/// must be explicit: `--machine` or `EDDA_MACHINE`, never the hostname.
pub(crate) fn claim_guard_refusal(
    args: &DispatchArgs,
    repo_cwd: &std::path::Path,
) -> Result<Option<(u64, String, String)>> {
    let Some(issue) = args.issue else {
        if args.machine.is_some() {
            bail!(
                "--machine is only meaningful with --issue: pass --issue <N> so the \
                 cross-machine claim guard can check it; an explicit value is never \
                 silently dropped"
            );
        }
        return Ok(None);
    };
    let machine = match args.machine.as_deref() {
        Some(machine) => machine.to_owned(),
        None => std::env::var("EDDA_MACHINE").unwrap_or_default(),
    };
    if machine.is_empty() {
        bail!(
            "--issue {issue} requires an explicit machine identity: pass --machine <machine>/<role> \
             or set EDDA_MACHINE (the hostname is never guessed)"
        );
    }
    // A malformed identity is a usage error (GH-782: exit 2), not a crash:
    // route it through the same refusal path as every other admission
    // failure so --json still prints exactly one JSON object (round 2, F4).
    if let Err(error) = crate::claim_guard::validate_machine(&machine) {
        return Ok(Some((issue, machine, error.to_string())));
    }
    let state = match crate::claim_guard::fetch_claim_state(issue, &machine, repo_cwd) {
        Ok(state) => state,
        Err(error) => return Ok(Some((issue, machine, error.to_string()))),
    };
    match state {
        crate::claim_guard::ClaimState::Unclaimed
        | crate::claim_guard::ClaimState::ClaimedBySelf => {
            let already_claimed = state == crate::claim_guard::ClaimState::ClaimedBySelf;
            match crate::claim_guard::write_claim(issue, &machine, already_claimed, repo_cwd) {
                Ok(()) => Ok(None),
                Err(error) => Ok(Some((issue, machine, error.to_string()))),
            }
        }
        crate::claim_guard::ClaimState::InFlight { pr, state } => {
            Ok(Some((issue, machine, state.refusal(issue, pr))))
        }
        crate::claim_guard::ClaimState::ClaimedByOther {
            machine: other,
            when,
            source,
        } => {
            let when = when.map(|when| format!(" at {when}")).unwrap_or_default();
            Ok(Some((
                issue,
                machine,
                format!(
                    "issue {issue} is already claimed by '{other}' ({source}{when}); \
                     dispatch refused — leave the claim to that owner \
                     (fleet.cross-machine-claim)"
                ),
            )))
        }
    }
}

/// Run gh bound to `repo_cwd`, the selected dispatch repository directory
/// (GH-782 round 2, F2): gh resolves the repository from the working
/// directory it is spawned in, so reads and writes without it would target
/// whatever repo the edda process happened to start in instead of the
/// `--cwd` the agent will run in.
fn run_gh(args: &[&str], repo_cwd: &Path) -> Result<Vec<u8>> {
    let gh = std::env::var_os("EDDA_GH_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"));
    let repository = crate::cmd_review::github::GitHubRepository::from_checkout(repo_cwd)?;
    let output = std::process::Command::new(&gh)
        .args(args)
        .args(["--repo", &repository.full_name()])
        .current_dir(repo_cwd)
        .env_remove("GH_REPO")
        .env_remove("EDDA_REPO")
        .env_remove("GH_HOST")
        .output()
        .with_context(|| format!("claim guard: could not run {}", gh.display()))?;
    if !output.status.success() {
        bail!(
            "claim guard: GitHub request failed with exit {:?}",
            output.status.code()
        );
    }
    Ok(output.stdout)
}

fn run_controlled_gh(
    args: &[&str],
    repo_cwd: &Path,
    repository: &crate::cmd_review::github::GitHubRepository,
    transport: &crate::cmd_review::github::ControlledGitHubTransport,
) -> Result<Vec<u8>> {
    let output = crate::cmd_review::github::controlled_gh_capture_for(
        transport, repo_cwd, repository, args,
    )?;
    anyhow::ensure!(
        output.status.success(),
        "claim guard: controlled GitHub request failed with exit {:?}",
        output.status.code()
    );
    Ok(output.stdout)
}

/// Read comments and the PR history. gh's default 30-row page must not hide
/// older deliveries, so request its full practical result limit explicitly.
pub fn fetch_claim_state(issue: u64, ours: &str, repo_cwd: &Path) -> Result<ClaimState> {
    validate_machine(ours)?;
    let (parsed, prs) = fetch_issue_and_prs(issue, repo_cwd)?;
    Ok(pr_state(issue, &prs).unwrap_or_else(|| claim_state(ours, &parsed)))
}

/// Revalidate a control-owned issue immediately before preparing local work.
/// This is intentionally read-only: admission owns the claim mutation, while
/// preparation must refuse an issue that moved, gained a hold, or lost its
/// claimed-stage label after admission.
pub fn ensure_control_issue_ready(
    issue: u64,
    identity: &str,
    forbidden_hold_labels: &[String],
    repo_cwd: &Path,
    repository: &crate::cmd_review::github::GitHubRepository,
    transport: &crate::cmd_review::github::ControlledGitHubTransport,
) -> Result<()> {
    validate_machine(identity)?;
    let (current, prs) = fetch_control_issue_and_prs(issue, repo_cwd, repository, transport)?;
    anyhow::ensure!(
        current.state == Some(IssueState::Open),
        "control issue is not open"
    );
    anyhow::ensure!(
        pr_state(issue, &prs).is_none(),
        "control issue already has a delivery PR"
    );
    if let Some(reason) = post_claim_label_refusal(&current, forbidden_hold_labels) {
        anyhow::bail!(reason);
    }
    let winner = control_claims(&current).into_iter().next();
    anyhow::ensure!(
        winner.is_some_and(|winner| winner.identity == identity),
        "control issue claim is no longer owned by the manifested identity"
    );
    Ok(())
}

fn fetch_issue_and_prs(issue: u64, repo_cwd: &Path) -> Result<(GhIssue, Vec<GhPr>)> {
    let output = run_gh(
        &[
            "issue",
            "view",
            &issue.to_string(),
            "--json",
            "comments,labels,state",
        ],
        repo_cwd,
    )?;
    let parsed: GhIssue = serde_json::from_slice(&output)
        .with_context(|| format!("claim guard: could not parse gh issue {issue}"))?;
    let output = run_gh(
        &[
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "1000000",
            "--json",
            "number,state,title,headRefName",
        ],
        repo_cwd,
    )?;
    let prs: Vec<GhPr> =
        serde_json::from_slice(&output).context("claim guard: could not parse gh PR history")?;
    Ok((parsed, prs))
}

fn fetch_control_issue_and_prs(
    issue: u64,
    repo_cwd: &Path,
    repository: &crate::cmd_review::github::GitHubRepository,
    transport: &crate::cmd_review::github::ControlledGitHubTransport,
) -> Result<(GhIssue, Vec<GhPr>)> {
    let output = run_controlled_gh(
        &[
            "issue",
            "view",
            &issue.to_string(),
            "--json",
            "comments,labels,state",
        ],
        repo_cwd,
        repository,
        transport,
    )?;
    let parsed: GhIssue = serde_json::from_slice(&output)
        .with_context(|| format!("claim guard: could not parse gh issue {issue}"))?;
    let output = run_controlled_gh(
        &[
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "1000000",
            "--json",
            "number,state,title,headRefName",
        ],
        repo_cwd,
        repository,
        transport,
    )?;
    let prs: Vec<GhPr> =
        serde_json::from_slice(&output).context("claim guard: could not parse gh PR history")?;
    Ok((parsed, prs))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlIssueAdmission {
    Won {
        adopted: bool,
        external_identity: String,
    },
    Refused {
        reason_code: String,
    },
}

/// R1/R21 admission for one control action. Every control nonce is a distinct
/// claimant even when two controls use the same machine/role string. The first
/// immutable taking marker wins; the caller writes its marker and board labels,
/// then rereads comments and PRs before any task lease or launch is allowed.
#[allow(clippy::too_many_arguments)] // Atomic claim CAS needs every frozen policy/binding input.
pub fn admit_control_issue(
    issue: u64,
    identity: &str,
    action_id: &str,
    predecessor_action_ids: &[String],
    allowed_stage_labels: &[String],
    forbidden_hold_labels: &[String],
    repo_cwd: &Path,
    repository: &crate::cmd_review::github::GitHubRepository,
    transport: &crate::cmd_review::github::ControlledGitHubTransport,
) -> Result<ControlIssueAdmission> {
    validate_machine(identity)?;
    anyhow::ensure!(
        action_id.starts_with("action_")
            && action_id.len() == 71
            && action_id[7..].bytes().all(|byte| byte.is_ascii_hexdigit()),
        "control issue admission action ID is malformed"
    );
    let (before, prs) = fetch_control_issue_and_prs(issue, repo_cwd, repository, transport)?;
    if before.state != Some(IssueState::Open) {
        return Ok(ControlIssueAdmission::Refused {
            reason_code: "ISSUE_NOT_OPEN".into(),
        });
    }
    if let Some(ClaimState::InFlight { pr, state }) = pr_state(issue, &prs) {
        return Ok(ControlIssueAdmission::Refused {
            reason_code: state.refusal(issue, pr),
        });
    }
    let claims = control_claims(&before);
    if let Some(winner) = claims.first() {
        let action_is_ours = winner.action_id == Some(action_id)
            || winner
                .action_id
                .is_some_and(|action| predecessor_action_ids.iter().any(|prior| prior == action));
        if winner.identity != identity || !action_is_ours {
            return Ok(ControlIssueAdmission::Refused {
                reason_code: "ISSUE_CLAIM_LOST".into(),
            });
        }
        write_control_board_claim(issue, repo_cwd, repository, transport)?;
        let (after, prs) = fetch_control_issue_and_prs(issue, repo_cwd, repository, transport)?;
        if after.state != Some(IssueState::Open) || pr_state(issue, &prs).is_some() {
            return Ok(ControlIssueAdmission::Refused {
                reason_code: "ISSUE_ADMISSION_CHANGED_DURING_REPAIR".into(),
            });
        }
        if let Some(reason_code) = post_claim_label_refusal(&after, forbidden_hold_labels) {
            return Ok(ControlIssueAdmission::Refused { reason_code });
        }
        let winner = control_claims(&after).into_iter().next();
        return Ok(match winner {
            Some(winner)
                if winner.identity == identity
                    && winner.action_id.is_some_and(|action| {
                        action == action_id
                            || predecessor_action_ids.iter().any(|prior| prior == action)
                    }) =>
            {
                ControlIssueAdmission::Won {
                    adopted: true,
                    external_identity: format!("issue:{issue}:{identity}:{action_id}"),
                }
            }
            _ => ControlIssueAdmission::Refused {
                reason_code: "ISSUE_CLAIM_LOST".into(),
            },
        });
    }
    if let Some(reason) = label_refusal(&before, allowed_stage_labels, forbidden_hold_labels) {
        return Ok(ControlIssueAdmission::Refused {
            reason_code: reason,
        });
    }

    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    let issue_text = issue.to_string();
    run_controlled_gh(
        &[
            "issue",
            "comment",
            &issue_text,
            "--body",
            &format!("taking: {identity} at {now} control-action={action_id}"),
        ],
        repo_cwd,
        repository,
        transport,
    )?;
    write_control_board_claim(issue, repo_cwd, repository, transport)?;

    let (after, prs) = fetch_control_issue_and_prs(issue, repo_cwd, repository, transport)?;
    if after.state != Some(IssueState::Open) {
        return Ok(ControlIssueAdmission::Refused {
            reason_code: "ISSUE_NOT_OPEN".into(),
        });
    }
    if let Some(reason_code) = post_claim_label_refusal(&after, forbidden_hold_labels) {
        return Ok(ControlIssueAdmission::Refused { reason_code });
    }
    if let Some(ClaimState::InFlight { .. }) = pr_state(issue, &prs) {
        return Ok(ControlIssueAdmission::Refused {
            reason_code: "DELIVERY_PR_APPEARED_DURING_ADMISSION".into(),
        });
    }
    let winner = control_claims(&after).into_iter().next();
    Ok(match winner {
        Some(winner) if winner.identity == identity && winner.action_id == Some(action_id) => {
            ControlIssueAdmission::Won {
                adopted: false,
                external_identity: format!("issue:{issue}:{identity}:{action_id}"),
            }
        }
        _ => ControlIssueAdmission::Refused {
            reason_code: "ISSUE_CLAIM_LOST".into(),
        },
    })
}

fn post_claim_label_refusal(issue: &GhIssue, forbidden_hold_labels: &[String]) -> Option<String> {
    label_refusal(issue, &["fleet:claimed".into()], forbidden_hold_labels)
}

fn write_control_board_claim(
    issue: u64,
    repo_cwd: &Path,
    repository: &crate::cmd_review::github::GitHubRepository,
    transport: &crate::cmd_review::github::ControlledGitHubTransport,
) -> Result<()> {
    run_controlled_gh(
        &[
            "issue",
            "edit",
            &issue.to_string(),
            "--add-label",
            "fleet:claimed",
            "--remove-label",
            "fleet:ready",
            "--add-assignee",
            "@me",
        ],
        repo_cwd,
        repository,
        transport,
    )?;
    Ok(())
}

struct ControlClaim<'a> {
    identity: &'a str,
    action_id: Option<&'a str>,
}

fn control_claims(issue: &GhIssue) -> Vec<ControlClaim<'_>> {
    issue
        .comments
        .iter()
        .flat_map(|comment| comment.body.lines())
        .filter_map(|line| {
            let mut parts = line
                .trim_start()
                .strip_prefix("taking:")?
                .split_whitespace();
            let identity = parts.next()?;
            let action_id = parts.find_map(|part| part.strip_prefix("control-action="));
            Some(ControlClaim {
                identity,
                action_id,
            })
        })
        .collect()
}

fn label_refusal(
    issue: &GhIssue,
    allowed_stage_labels: &[String],
    forbidden_hold_labels: &[String],
) -> Option<String> {
    let labels: Vec<String> = issue
        .labels
        .iter()
        .map(|label| label.name.trim().to_ascii_lowercase())
        .collect();
    if let Some(label) = forbidden_hold_labels.iter().find(|label| {
        labels
            .iter()
            .any(|actual| actual.eq_ignore_ascii_case(label))
    }) {
        return Some(format!("ISSUE_HELD_BY_LABEL:{label}"));
    }
    if !allowed_stage_labels.is_empty()
        && !allowed_stage_labels.iter().any(|label| {
            labels
                .iter()
                .any(|actual| actual.eq_ignore_ascii_case(label))
        })
    {
        return Some("ISSUE_STAGE_NOT_ADMITTED".into());
    }
    None
}

/// Complete the claim before starting an agent. Re-dispatch repairs a partial
/// label/assignee write without adding another taking comment. GitHub provides
/// no compare-and-swap across these calls; this is not a distributed lock.
pub fn write_claim(issue: u64, ours: &str, already_claimed: bool, repo_cwd: &Path) -> Result<()> {
    validate_machine(ours)?;
    let issue = issue.to_string();
    if !already_claimed {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)?;
        run_gh(
            &[
                "issue",
                "comment",
                &issue,
                "--body",
                &format!("taking: {ours} at {now}"),
            ],
            repo_cwd,
        )?;
    }
    run_gh(
        &[
            "issue",
            "edit",
            &issue,
            "--add-label",
            "fleet:claimed",
            "--remove-label",
            "fleet:ready",
            "--add-assignee",
            "@me",
        ],
        repo_cwd,
    )?;
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
