//! `edda review drift` — the readiness signal `mergeStateStatus` does not
//! provide (GH-914), in the product (GH-1105).
//!
//! One line per open PR: does the current head carry the newest verdict, and
//! does that verdict resolve the PR? Readiness is keyed on the §7 verdict
//! comment pinned to the head SHA (rules.md R23/R24), never on
//! `mergeStateStatus`: the ruleset protects `main` only, so a stacked PR
//! reports CLEAN with zero verdicts. This module replaces
//! `scripts/fleet/verdict-drift.sh` — now a one-line adapter over it — and
//! its stdout and exit codes are byte-compatible with that shell, so
//! `daily-digest.sh` (which awk-parses the lines) keeps working unchanged.
//!
//! The rules are the shell's, moved not rewritten (GH-1105 doneWhen):
//!
//! - **Trust** — [`super::deliver::trusted_association`], the one place
//!   GH-1103 put it; the shell's jq `select` on `authorAssociation` is gone.
//! - **Heading grammar** — [`super::deliver::heading_parts`], the same
//!   parser `extract` uses; R23 first-line headings only.
//! - **SHADOW** — the heading suffix (either recorded position) **or** a
//!   `^- shadow: true$` body line, exactly the shell's two markers; a
//!   SHADOW verdict never counts as the head's authoritative verdict.
//! - **Orphan response** (GH-993) — the newest `## Review Response: Round
//!   N` must answer a round number some §7 comment actually posted; only
//!   the newest response is judged, so the #974 repair shape (jump to
//!   Round 2, never backfill Round 1) reads clean.
//! - **Fail closed** — a `gh` read that fails is exit 2 with no clean
//!   bill, never an empty output that reads as "all clean".
//!
//! Read-only: no posting, no labels, no merges, no ledger writes.

use super::deliver::{self, comments, heading_parts, trusted_association, Comment};
use super::github::gh;
use anyhow::{Context, Result};
use std::path::Path;

/// Flags for `edda review drift`.
#[derive(clap::Args)]
pub struct DriftArgs {
    /// Emit the drift report as JSON
    #[arg(long)]
    pub json: bool,
    /// Maximum open PRs to examine; defaults to EDDA_OPEN_PR_LIMIT, then 200
    #[arg(long, value_name = "N")]
    pub limit: Option<u64>,
}

/// One open PR, as the enumeration returns it.
#[derive(Debug, Clone)]
pub(crate) struct PrRow {
    pub number: u64,
    pub head: String,
    pub base: String,
    /// `MERGEABLE` / `CONFLICTING` / `UNKNOWN`; empty when the field was
    /// absent — the older-fixture shape, which annotates nothing.
    pub mergeable: String,
}

/// The readiness state of one PR's verdicts — the `<state>` field of the
/// output line, and the not-ready decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrState {
    /// No §7 verdict comment exists at all.
    NoVerdict,
    /// The newest verdict is pinned to an older SHA (`stale from <sha12>`).
    StaleFrom(String),
    /// Verdicts exist and the newest is pinned to head, but every one
    /// pinned to head is SHADOW.
    ShadowOnly,
    /// The newest head-pinned authoritative verdict resolves LGTM.
    Lgtm,
    /// Anything else that is pinned to head holds the PR, whatever it says.
    ChangesRequested,
}

impl PrState {
    /// The exact `<state>` token the shell printed — byte-compatible.
    fn as_str(&self) -> String {
        match self {
            PrState::NoVerdict => "no verdict on head".into(),
            PrState::StaleFrom(sha) => format!("stale from {}", &sha[..12]),
            PrState::ShadowOnly => "SHADOW only".into(),
            PrState::Lgtm => "LGTM".into(),
            PrState::ChangesRequested => "Changes Requested".into(),
        }
    }

    fn holds(&self) -> bool {
        matches!(self, PrState::NoVerdict | PrState::StaleFrom(_))
    }
}

/// One trusted §7 comment reduced to what the drift rule needs.
#[derive(Debug, Clone)]
struct VerdictRow {
    sha: String,
    lgtm: bool,
}

/// `## Review Response: Round <N>` on the first line — the GH-993 trigger
/// shape. Round number only; matching is by round NUMBER, never SHA.
fn response_round(first_line: &str) -> Option<&str> {
    let rest = first_line.strip_prefix("## Review Response: Round ")?;
    let after = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    if after.len() == rest.len() || !after.is_empty() {
        return None; // no round number, or trailing junk: not the shape
    }
    Some(&rest[..rest.len() - after.len()])
}

/// Does the verdict word of this §7 body read LGTM? The shell's resolver:
/// a `^LGTM` line means lgtm, anything else (`^Changes Requested`,
/// Provisional, nothing) does not. [`super::deliver::verdict_line`] already
/// reads the word in the blocking-first order, so "Changes Requested … the
/// LGTM cannot stand" stays a hold; a Provisional round contains LGTM but
/// starts with Provisional, and the word selection keeps it a hold too.
fn resolves_lgtm(body: &str) -> bool {
    let lines: Vec<&str> = body
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    match deliver::verdict_line(&lines) {
        Some(line) => !line.contains("Changes Requested") && !line.starts_with("Provisional"),
        None => false,
    }
}

/// Reduce one PR's comments to its drift state and orphan annotation.
///
/// Pure over `(head, comments)` so every shell fixture ports here without a
/// stubbed `gh`: newest verdict by comment order, last authoritative verdict
/// pinned to head decides resolution, round-number membership decides
/// orphan responses.
pub(crate) fn reduce(head: &str, comments: &[Comment]) -> (PrState, Option<String>) {
    let mut newest: Option<VerdictRow> = None;
    let mut authoritative_head_lgtm: Option<bool> = None;
    let mut rounds: Vec<String> = Vec::new();
    let mut response: Option<String> = None;
    for comment in comments {
        if !trusted_association(comment.author_association.as_deref()) {
            continue;
        }
        let lines: Vec<&str> = comment
            .body
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .collect();
        let Some(first) = lines.first().copied() else {
            continue;
        };
        if let Some((round, sha, shadow)) = heading_parts(first) {
            let shadow = shadow || lines.iter().any(|line| line.trim_end() == "- shadow: true");
            rounds.push(round);
            newest = Some(VerdictRow {
                sha,
                lgtm: resolves_lgtm(&comment.body),
            });
            if !shadow {
                // Authoritative: only while pinned to this head does it
                // decide resolution (the shell's authoritative_present).
                if newest.as_ref().is_some_and(|row| row.sha == head) {
                    authoritative_head_lgtm = Some(newest.as_ref().is_some_and(|row| row.lgtm));
                }
            }
        } else if let Some(round) = response_round(first) {
            response = Some(round.to_owned());
        }
    }
    let state = match &newest {
        None => PrState::NoVerdict,
        Some(row) if row.sha != head => PrState::StaleFrom(row.sha.clone()),
        Some(_) => match authoritative_head_lgtm {
            None => PrState::ShadowOnly,
            Some(true) => PrState::Lgtm,
            Some(false) => PrState::ChangesRequested,
        },
    };
    let orphan = response.filter(|round| !rounds.contains(round));
    (state, orphan)
}

/// The exact line the shell printed for one PR — byte-compatible, because
/// `daily-digest.sh` awk-parses `$4` and every field after it verbatim.
pub(crate) fn line_for(row: &PrRow, state: &PrState, orphan: &Option<String>) -> String {
    let head12: String = row.head.chars().take(12).collect();
    let mut line = format!("#{} {} {} {}", row.number, head12, row.base, state.as_str());
    if row.mergeable == "CONFLICTING" || row.mergeable == "UNKNOWN" {
        line = format!("{line} mergeable={}", row.mergeable);
    }
    if row.base != "main" {
        line = format!("{line} base={} (status contexts not enforced)", row.base);
    }
    if let Some(round) = orphan {
        line = format!("{line} orphan-response=Round-{round}");
    }
    line
}

/// Does the line's state hold the PR (drift found)?
fn holds(row: &PrRow, state: &PrState, orphan: &Option<String>) -> bool {
    state.holds() || row.mergeable == "CONFLICTING" || orphan.is_some()
}

/// The `--limit` value this run uses: the flag, then EDDA_OPEN_PR_LIMIT
/// (the variable daily-digest.sh exports so both halves of GH-958's shared
/// enumeration see the same number), then 200.
fn effective_limit(flag: Option<u64>) -> u64 {
    flag.or_else(|| {
        std::env::var("EDDA_OPEN_PR_LIMIT")
            .ok()
            .and_then(|value| value.parse().ok())
    })
    .unwrap_or(200)
}

/// The exact `gh pr list` argv [`open_prs`] shells out with, split out for
/// the same reason `deliver::comments_argv` is (GH-1079): the argv is
/// asserted directly, so a silent revert of any part of it goes red.
fn open_prs_argv(limit: u64) -> Vec<String> {
    vec![
        "pr".to_owned(),
        "list".to_owned(),
        "--state".to_owned(),
        "open".to_owned(),
        "--limit".to_owned(),
        limit.to_string(),
        "--json".to_owned(),
        "number,headRefOid,baseRefName,mergeable".to_owned(),
    ]
}

fn parse_open_prs(value: &serde_json::Value) -> Vec<PrRow> {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| PrRow {
            number: entry["number"].as_u64().unwrap_or_default(),
            head: entry["headRefOid"].as_str().unwrap_or_default().to_owned(),
            base: entry["baseRefName"].as_str().unwrap_or_default().to_owned(),
            mergeable: entry["mergeable"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// The drift query over the whole open-PR set: one output line per PR and
/// whether any PR is not ready. Split from [`run`] so `edda review merge`
/// consults the same walk without a subprocess (GH-993's blocking scope is
/// the whole open set, not the one PR being merged).
pub(crate) fn evaluate(cwd: &Path, limit: u64) -> Result<(Vec<String>, bool)> {
    let argv = open_prs_argv(limit);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let value = gh(cwd, &args).context("list open PRs")?;
    let rows = parse_open_prs(&value);
    if rows.len() as u64 >= limit {
        eprintln!(
            "edda review drift: the open-PR enumeration hit its limit of {limit}; PRs past it \
             were not examined (raise --limit / EDDA_OPEN_PR_LIMIT)"
        );
    }
    let mut lines = Vec::new();
    let mut not_ready = false;
    for row in &rows {
        let list = comments(cwd, row.number)
            .with_context(|| format!("read comments of PR #{}", row.number))?;
        let (state, orphan) = reduce(&row.head, &list);
        if holds(row, &state, &orphan) {
            not_ready = true;
        }
        lines.push(line_for(row, &state, &orphan));
    }
    Ok((lines, not_ready))
}

/// CLI entry point. Exit: 0 every open PR is ready, 1 drift found, 2 cannot
/// judge — a check that could not read must never print nothing and exit 0.
pub fn run(args: DriftArgs, cwd: &Path) -> Result<()> {
    let limit = effective_limit(args.limit);
    match evaluate(cwd, limit) {
        Ok((lines, not_ready)) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "prs": lines,
                        "not_ready": not_ready,
                    })
                );
            } else {
                for line in &lines {
                    println!("{line}");
                }
            }
            if not_ready {
                std::process::exit(1);
            }
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "edda review drift: could not read PR state from gh ({error:#}) — refusing to \
                 print a clean bill"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "1111111111111111111111111111111111111111";
    const OTHER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn comment(body: &str, association: &str) -> Comment {
        Comment {
            id: "1".into(),
            body: body.into(),
            author_association: Some(association.into()),
        }
    }

    fn row(number: u64, head: &str, base: &str, mergeable: &str) -> PrRow {
        PrRow {
            number,
            head: head.into(),
            base: base.into(),
            mergeable: mergeable.into(),
        }
    }

    fn verdict_line_body(sha: &str, verdict: &str) -> String {
        format!("## Code Review: Round 1 — PR #1 @ {sha}\n\n### Verdict\n\n{verdict}\n")
    }

    // The 18 fixtures scripts/fleet/test-verdict-drift.sh carried, ported
    // one-for-one (GH-1105 doneWhen: every rule the shell encodes is
    // unit-tested in Rust rather than by fixture shell).

    #[test]
    fn case1_verdict_pinned_to_head_lgtm_is_ready() {
        let list = [comment(
            &verdict_line_body(SHA, "LGTM (P0=0, P1=0)"),
            "OWNER",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(1, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#1 111111111111 main LGTM");
        assert!(!holds(&row(1, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case2_verdict_pinned_to_an_older_sha_is_stale() {
        // The #899 regression: a verdict pinned to an older SHA than
        // headRefOid was reported complete.
        let list = [comment(
            &verdict_line_body(OTHER, "LGTM (P0=0, P1=0)"),
            "OWNER",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(2, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#2 111111111111 main stale from aaaaaaaaaaaa");
        assert!(holds(&row(2, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case3_no_verdict_comment_holds() {
        let (state, orphan) = reduce(SHA, &[]);
        let line = line_for(&row(3, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#3 111111111111 main no verdict on head");
        assert!(holds(&row(3, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case4_shadow_only_on_head_is_ready() {
        // Both recorded SHADOW markers at once, as the shell's fixture did:
        // heading suffix and `- shadow: true` body line.
        let body = format!(
            "## Code Review: Round 2 (SHADOW) — PR #1 @ {SHA}\n- shadow: true\n\n### \
             Verdict\n\nLGTM (P0=0, P1=0)"
        );
        let list = [comment(&body, "OWNER")];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(4, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#4 111111111111 main SHADOW only");
        assert!(!holds(&row(4, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case4b_shadow_by_body_line_alone_is_still_shadow() {
        // The second marker the shell accepted: no heading suffix, but the
        // body says `- shadow: true`.
        let body = format!(
            "## Code Review: Round 2 — PR #1 @ {SHA}\n\n- shadow: true\n\n### Verdict\n\nLGTM \
             (P0=0, P1=0)"
        );
        let (state, _) = reduce(SHA, &[comment(&body, "OWNER")]);
        assert_eq!(state, PrState::ShadowOnly);
    }

    #[test]
    fn case5_a_base_other_than_main_is_annotated() {
        let (state, orphan) = reduce(SHA, &[]);
        let line = line_for(&row(5, SHA, "feat/x", ""), &state, &orphan);
        assert_eq!(
            line,
            "#5 111111111111 feat/x no verdict on head base=feat/x (status contexts not enforced)"
        );
    }

    #[test]
    fn case6_a_heading_mid_body_is_not_a_verdict() {
        // The #867 shape: a transcript that happens to contain the heading.
        // First-line-only parsing is the R23 rule.
        let body =
            format!("narration first\n## Code Review: Round 3 — PR #1 @ {SHA}\nLGTM (P0=0, P1=0)");
        let (state, orphan) = reduce(SHA, &[comment(&body, "OWNER")]);
        let line = line_for(&row(6, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#6 111111111111 main no verdict on head");
    }

    #[test]
    fn case7_a_failed_read_exits_2_and_prints_no_bill() {
        // Wiring shape, ported at the evaluate seam: a gh failure surfaces
        // as Err — the caller prints the cannot-judge message and exits 2,
        // never an empty success.
        let err = evaluate(Path::new("definitely-not-a-repo"), 200)
            .expect_err("a gh call from a bogus cwd must fail");
        assert!(!format!("{err:#}").is_empty());
    }

    #[test]
    fn case8_mergeable_conflicting_holds_the_pr() {
        // R24 field 3 (GH-958): every verdict field says ready; the
        // conflict does not.
        let list = [comment(
            &verdict_line_body(SHA, "LGTM (P0=0, P1=0)"),
            "OWNER",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(8, SHA, "main", "CONFLICTING"), &state, &orphan);
        assert_eq!(line, "#8 111111111111 main LGTM mergeable=CONFLICTING");
        assert!(holds(&row(8, SHA, "main", "CONFLICTING"), &state, &orphan));
    }

    #[test]
    fn case9_mergeable_unknown_is_surfaced_without_holding() {
        let list = [comment(
            &verdict_line_body(SHA, "LGTM (P0=0, P1=0)"),
            "OWNER",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(9, SHA, "main", "UNKNOWN"), &state, &orphan);
        assert_eq!(line, "#9 111111111111 main LGTM mergeable=UNKNOWN");
        assert!(!holds(&row(9, SHA, "main", "UNKNOWN"), &state, &orphan));
    }

    #[test]
    fn case10_the_limit_flag_reaches_the_gh_argv() {
        assert_eq!(
            open_prs_argv(7),
            vec![
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "7",
                "--json",
                "number,headRefOid,baseRefName,mergeable",
            ]
        );
        assert_eq!(effective_limit(Some(7)), 7);
        assert_eq!(effective_limit(None), 200);
    }

    #[test]
    fn case12_an_orphan_review_response_holds_the_pr() {
        // The GH-993 defect, as a fixture: a `Review Response: Round 1`
        // answering a round that was never posted.
        let list = [comment(
            &format!("## Review Response: Round 1\n\nNew head: `{SHA}`\n\n### f1 — fixed"),
            "OWNER",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(12, SHA, "main", ""), &state, &orphan);
        assert_eq!(
            line,
            "#12 111111111111 main no verdict on head orphan-response=Round-1"
        );
        assert!(holds(&row(12, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case13_a_response_answering_a_posted_round_is_clean() {
        // Ordinary Changes-Requested flow: the round exists, the response
        // answers it, and a head verdict that resolves Changes Requested is
        // a visible signal, not drift.
        let list = [
            comment(
                &verdict_line_body(SHA, "Changes Requested, P0=0, P1=1"),
                "OWNER",
            ),
            comment(
                "## Review Response: Round 1\n\nNew head: unchanged",
                "OWNER",
            ),
        ];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(13, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#13 111111111111 main Changes Requested");
        assert!(!holds(&row(13, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case14_the_974_repair_shape_reads_clean() {
        // Response 1 is answered by nothing, permanently; only the NEWEST
        // response is judged, and Round 2's is paired. A PR that recovered
        // by jumping to Round 2 must not be held forever by this check.
        let mid = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let list = [
            comment(
                &format!("## Review Response: Round 1\n\nNew head: {mid}"),
                "OWNER",
            ),
            comment(
                &format!(
                    "## Code Review: Round 2 — PR #14 @ {mid}\n\n### Verdict\n\nChanges \
                     Requested, P0=0, P1=1"
                ),
                "OWNER",
            ),
            comment(
                &format!("## Review Response: Round 2\n\nNew head: {SHA}"),
                "OWNER",
            ),
            comment(
                &format!(
                    "## Code Review: Round 3 — PR #14 @ {SHA}\n\n### Verdict\n\nLGTM (P0=0, \
                     P1=0)"
                ),
                "OWNER",
            ),
        ];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(14, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#14 111111111111 main LGTM");
        assert!(!holds(&row(14, SHA, "main", ""), &state, &orphan));
    }

    #[test]
    fn case15_a_response_heading_below_line_one_is_not_a_trigger() {
        // Symmetric with case 6's rule for Code Review headings.
        let (state, orphan) = reduce(
            SHA,
            &[comment(
                "note first\n## Review Response: Round 1\n\nignored",
                "OWNER",
            )],
        );
        assert_eq!(state, PrState::NoVerdict);
        assert!(orphan.is_none());
    }

    #[test]
    fn case16_an_untrusted_verdict_does_not_silence_the_check() {
        // GH-1103's rule read through the drift lens: a §7 LGTM from an
        // author GitHub does not vouch for is not a verdict — byte-identical
        // to case 1's shape, only the trust differs.
        let list = [comment(
            &verdict_line_body(SHA, "LGTM (P0=0, P1=0)"),
            "NONE",
        )];
        let (state, orphan) = reduce(SHA, &list);
        let line = line_for(&row(16, SHA, "main", ""), &state, &orphan);
        assert_eq!(line, "#16 111111111111 main no verdict on head");
    }

    #[test]
    fn case17_an_untrusted_response_does_not_hold_the_pr() {
        // The other direction: an outsider's Review Response is invisible,
        // not judged-and-cleared — no orphan annotation either way.
        let list = [comment(
            &format!("## Review Response: Round 9\n\nNew head: `{SHA}`"),
            "NONE",
        )];
        let (state, orphan) = reduce(SHA, &list);
        assert_eq!(state, PrState::NoVerdict);
        assert!(orphan.is_none(), "untrusted comments cannot hold a PR");
    }

    #[test]
    fn case18_a_missing_association_fails_closed() {
        let list = [Comment {
            id: "1".into(),
            body: verdict_line_body(SHA, "LGTM (P0=0, P1=0)"),
            author_association: None,
        }];
        let (state, _) = reduce(SHA, &list);
        assert_eq!(state, PrState::NoVerdict);
    }

    #[test]
    fn round_numbers_match_as_elements_not_substrings() {
        // Round "1" must not satisfy a response to Round "11": membership
        // is over the parsed round token, never a substring scan.
        let list = [
            comment(&verdict_line_body(SHA, "LGTM (P0=0, P1=0)"), "OWNER"),
            comment("## Review Response: Round 11\n\nNew head: x", "OWNER"),
        ];
        let (state, orphan) = reduce(SHA, &list);
        assert_eq!(state, PrState::Lgtm);
        assert_eq!(orphan.as_deref(), Some("11"));
    }

    #[test]
    fn parse_open_prs_reads_the_gh_pr_list_shape() {
        let value = serde_json::json!([
            {"number": 8, "headRefOid": SHA, "baseRefName": "main", "mergeable": "CONFLICTING"},
            {"number": 9, "headRefOid": SHA, "baseRefName": "main"},
        ]);
        let rows = parse_open_prs(&value);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].mergeable, "CONFLICTING");
        assert_eq!(rows[1].mergeable, "", "absent mergeable annotates nothing");
    }
}
