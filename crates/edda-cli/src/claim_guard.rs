//! Cross-machine claim guard (GH-656).
//!
//! Two machines each have their own `.edda/`, so an `edda claim` on one is
//! invisible to the other; GitHub is the only shared truth. The
//! `fleet.cross-machine-claim` convention (leave a `taking: <machine>`
//! comment and a `lane:<machine>` label on the issue before dispatching)
//! was pure discipline — nothing checked it. This module turns that check
//! mechanical: given an issue number and the caller's machine label, read
//! the issue's labels and comments through `gh` and decide whether the
//! issue is free, already claimed by this machine, or claimed by another
//! machine (refusal).
//!
//! Machine labels are compared verbatim — never guessed from the hostname.
//! The write side of the same convention lives in
//! `scripts/fleet-claim-issue.sh`; this module is the read-only guard used
//! by `edda dispatch --issue <N>`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// The claim state of one issue for one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimState {
    /// No `lane:*` label and no `taking:` comment naming any machine.
    Unclaimed,
    /// This machine already claimed it (idempotent re-dispatch is fine).
    ClaimedBySelf,
    /// Another machine claimed it — dispatch must refuse.
    ClaimedByOther {
        machine: String,
        /// Comment timestamp, when the claim was made by comment.
        when: Option<String>,
        /// Which surface carried the claim (`label lane:<m>` or
        /// `comment "taking: <m>"`), for the refusal message.
        source: String,
    },
}

/// The slice of `gh issue view --json labels,comments` the guard reads.
#[derive(Debug, Deserialize)]
struct GhIssue {
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    comments: Vec<GhComment>,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhComment {
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: Option<String>,
}

/// Validate the caller-supplied machine label: it becomes part of a GitHub
/// label (`lane:<machine>`) and of a comment (`taking: <machine>`), so it
/// must be a single token without surrounding whitespace.
pub fn validate_machine(machine: &str) -> Result<()> {
    if machine.is_empty() || machine != machine.trim() || machine.split_whitespace().count() != 1 {
        bail!(
            "machine label must be one token without whitespace, got {machine:?} \
             (it becomes the lane:<machine> label and taking: <machine> marker)"
        );
    }
    Ok(())
}

/// Read the claim state of `issue` for machine `ours` through `gh`.
///
/// The gh binary is `EDDA_GH_BIN` when set (the same override pattern as
/// `EDDA_CODEX_BIN` — also how the tests inject a stub), else `gh` on
/// PATH. One read-only call: `gh issue view <N> --json labels,comments`.
pub fn fetch_claim_state(issue: u64, ours: &str) -> Result<ClaimState> {
    validate_machine(ours)?;
    let gh = std::env::var_os("EDDA_GH_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"));
    let output = std::process::Command::new(&gh)
        .args([
            "issue",
            "view",
            &issue.to_string(),
            "--json",
            "labels,comments",
        ])
        .output()
        .with_context(|| {
            format!(
                "claim guard: could not run {} for issue {issue}",
                gh.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "claim guard: gh issue view {issue} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let parsed: GhIssue = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("claim guard: could not parse gh output for issue {issue}"))?;
    Ok(claim_state(ours, &parsed))
}

/// Decide the claim state from already-fetched labels and comments.
///
/// Any signal naming another machine refuses — a label on one side and a
/// comment on the other is treated as claimed, never as free (fail closed:
/// a stale `lane:X` label with a fresh `taking: docs` comment must not
/// wave a dispatch through). Labels are examined first because they are
/// the more durable surface.
fn claim_state(ours: &str, issue: &GhIssue) -> ClaimState {
    let mut other_label: Option<(String, String)> = None;
    for label in &issue.labels {
        if let Some(machine) = label.name.strip_prefix("lane:") {
            if machine != ours {
                other_label = Some((machine.to_owned(), format!("label {}", label.name)));
                break;
            }
        }
    }
    for comment in &issue.comments {
        if let Some(machine) = parse_taking(&comment.body) {
            if machine != ours {
                let source = if let Some((ref l_machine, ref l_source)) = other_label {
                    if l_machine == &machine {
                        l_source.clone()
                    } else {
                        "comment \"taking:\"".to_owned()
                    }
                } else {
                    "comment \"taking:\"".to_owned()
                };
                return ClaimState::ClaimedByOther {
                    machine,
                    when: comment.created_at.clone(),
                    source,
                };
            }
        }
    }
    if let Some((machine, source)) = other_label {
        return ClaimState::ClaimedByOther {
            machine,
            when: None,
            source,
        };
    }
    let self_label = issue
        .labels
        .iter()
        .any(|label| label.name == format!("lane:{ours}"));
    let self_comment = issue
        .comments
        .iter()
        .any(|comment| parse_taking(&comment.body).as_deref() == Some(ours));
    if self_label || self_comment {
        ClaimState::ClaimedBySelf
    } else {
        ClaimState::Unclaimed
    }
}

/// The machine a `taking: <machine>` comment names, if any. Recognized
/// only at the start of a line (`taking:` followed by the machine token);
/// a mention mid-sentence is not a claim.
fn parse_taking(body: &str) -> Option<String> {
    for line in body.lines() {
        let rest = line.trim_start();
        if let Some(rest) = rest.strip_prefix("taking:") {
            let machine = rest.split_whitespace().next().unwrap_or("");
            if !machine.is_empty() {
                return Some(machine.to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(labels: &[&str], comments: &[(&str, &str)]) -> GhIssue {
        GhIssue {
            labels: labels
                .iter()
                .map(|name| GhLabel {
                    name: (*name).to_owned(),
                })
                .collect(),
            comments: comments
                .iter()
                .map(|(created_at, body)| GhComment {
                    body: (*body).to_owned(),
                    created_at: Some((*created_at).to_owned()),
                })
                .collect(),
        }
    }

    #[test]
    fn unclaimed_when_no_signal_names_any_machine() {
        let state = claim_state("docs", &issue(&["fleet:ready"], &[]));
        assert_eq!(state, ClaimState::Unclaimed);
        let state = claim_state(
            "docs",
            &issue(
                &[],
                &[("2026-09-02T07:06:00Z", "claimed by session a at 07:06")],
            ),
        );
        assert_eq!(state, ClaimState::Unclaimed);
    }

    #[test]
    fn a_lane_label_for_another_machine_refuses() {
        let state = claim_state("docs", &issue(&["lane:4090"], &[]));
        assert_eq!(
            state,
            ClaimState::ClaimedByOther {
                machine: "4090".into(),
                when: None,
                source: "label lane:4090".into(),
            }
        );
    }

    #[test]
    fn a_taking_comment_for_another_machine_refuses_with_its_timestamp() {
        let state = claim_state(
            "docs",
            &issue(
                &[],
                &[(
                    "2026-09-02T06:30:00Z",
                    "taking: 4090 at 2026-09-02T06:30:00Z",
                )],
            ),
        );
        assert_eq!(
            state,
            ClaimState::ClaimedByOther {
                machine: "4090".into(),
                when: Some("2026-09-02T06:30:00Z".into()),
                source: "comment \"taking:\"".into(),
            }
        );
    }

    #[test]
    fn self_signals_are_idempotent_not_refusals() {
        let state = claim_state(
            "docs",
            &issue(&["lane:docs"], &[("2026-09-02T07:06:00Z", "taking: docs")]),
        );
        assert_eq!(state, ClaimState::ClaimedBySelf);
        let state = claim_state("docs", &issue(&["lane:docs"], &[]));
        assert_eq!(state, ClaimState::ClaimedBySelf);
        let state = claim_state("docs", &issue(&[], &[("t", "taking: docs")]));
        assert_eq!(state, ClaimState::ClaimedBySelf);
    }

    #[test]
    fn any_other_machine_signal_refuses_even_when_we_also_appear() {
        // Fail closed: our label plus their comment is still their claim.
        let state = claim_state(
            "docs",
            &issue(&["lane:docs"], &[("2026-09-02T08:00:00Z", "taking: 4090")]),
        );
        assert!(matches!(state, ClaimState::ClaimedByOther { .. }));
    }

    #[test]
    fn taking_is_only_a_claim_at_line_start() {
        assert_eq!(parse_taking("taking: 4090").as_deref(), Some("4090"));
        assert_eq!(parse_taking("  taking: docs").as_deref(), Some("docs"));
        // A mention mid-sentence is prose, not a claim.
        assert_eq!(parse_taking("remember: taking: docs first"), None);
        assert_eq!(parse_taking("taking:"), None);
        assert_eq!(parse_taking("taking:   "), None);
    }

    #[test]
    fn machine_label_must_be_one_explicit_token() {
        assert!(validate_machine("docs").is_ok());
        assert!(validate_machine("4090").is_ok());
        assert!(validate_machine("").is_err());
        assert!(validate_machine("docs lane").is_err());
        assert!(validate_machine(" docs ").is_err());
    }

    static GH_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct GhBinEnvGuard {
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for GhBinEnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("EDDA_GH_BIN", value),
                None => std::env::remove_var("EDDA_GH_BIN"),
            }
        }
    }

    fn gh_bin_env_guard(value: &str) -> GhBinEnvGuard {
        let lock = GH_BIN_ENV_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let previous = std::env::var_os("EDDA_GH_BIN");
        std::env::set_var("EDDA_GH_BIN", value);
        GhBinEnvGuard {
            previous,
            _lock: lock,
        }
    }

    #[test]
    fn fetch_fails_loudly_when_gh_is_missing() {
        // EDDA_GH_BIN pointing nowhere is an error, never a silent pass.
        // Protected by GhBinEnvGuard to avoid racing parallel tests in the binary.
        let _guard = gh_bin_env_guard("definitely-no-such-gh-9f3a");
        let result = fetch_claim_state(656, "docs");
        assert!(result.is_err(), "a broken gh must fail the guard loudly");
    }

    #[test]
    fn gh_comment_deserializes_camel_case_created_at() {
        let json = r#"{
            "labels": [{"name": "lane:docs"}],
            "comments": [
                {
                    "body": "taking: 4090 at 2026-09-02T13:00:00Z",
                    "createdAt": "2026-09-02T13:00:00Z"
                }
            ]
        }"#;
        let issue: GhIssue = serde_json::from_str(json).expect("valid JSON");
        assert_eq!(issue.comments.len(), 1);
        assert_eq!(
            issue.comments[0].created_at.as_deref(),
            Some("2026-09-02T13:00:00Z")
        );
        let state = claim_state("docs", &issue);
        match state {
            ClaimState::ClaimedByOther {
                machine,
                when,
                source,
            } => {
                assert_eq!(machine, "4090");
                assert_eq!(when.as_deref(), Some("2026-09-02T13:00:00Z"));
                assert_eq!(source, "comment \"taking:\"");
            }
            other => panic!("expected ClaimedByOther, got {other:?}"),
        }
    }
}
