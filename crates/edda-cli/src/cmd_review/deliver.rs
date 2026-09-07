//! `edda review deliver` — the GitHub delivery of a review round (GH-1030).
//!
//! Judging a SHA is `edda review gate` (GH-769). Deciding whether a round is
//! owed is `edda review due` (GH-763). What has had no product verb until now
//! is **delivery**: the §7 verdict comment, the `review:*` labels and the
//! `Independent Review` commit status all live only in
//! `scripts/pr-review-watch.sh`, which is why that layer is the one that broke
//! on 2026-09-06/07 — the product could judge but could not publish.
//!
//! This module starts where the shell starts: turning §7 verdict **comments**
//! into the verdict facts the union rule already consumes. That direction
//! matters. Verdicts do not cross machines in the ledger yet (`D8-debt(#671)`:
//! `review_verdict` is not among the event types the committed mirror imports),
//! and a round published only through the comment path writes no event at all,
//! so the comments — not the ledger — are what a deliverer can actually see.
//! The extracted lines are the tab-separated shape `gate::from_lines` already
//! parses, so the union rule stays in one place (GH-769) and this module never
//! re-decides what a verdict means.

use super::gate;
use super::github::gh;
use anyhow::{Context, Result};
use std::path::Path;

/// Flags for `edda review deliver`.
#[derive(clap::Args)]
pub struct DeliverArgs {
    /// The pull request whose §7 verdict comments are read
    #[arg(long)]
    pub pr: u64,
    /// The reviewed commit, as a full 40-character SHA; defaults to the PR head
    #[arg(long, value_name = "SHA")]
    pub sha: Option<String>,
    /// Emit the delivery report as JSON
    #[arg(long)]
    pub json: bool,
}

/// One PR comment, as the deliverer sees it.
#[derive(Debug, Clone)]
pub(crate) struct Comment {
    /// GitHub's comment id, echoed back in a malformed report so the notice
    /// can name the comment it is about.
    pub id: String,
    pub body: String,
}

/// What §7 comments on one SHA amount to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Extracted {
    /// `verdict<TAB>p0<TAB>p1` records, in comment order — the exact shape
    /// `gate::from_lines` reads.
    pub lines: Vec<String>,
    /// Ids of comments that carry a §7 heading somewhere other than line 1.
    pub malformed: Vec<String>,
}

/// Does this line open a §7 verdict comment?
///
/// The R23 heading shape, with the ` (SHADOW)` suffix accepted in **both**
/// recorded positions — after `Round <N>` (REVIEW.md §7) and at the end of the
/// line (the #917 trim contract). Accepting it here is what makes a SHADOW
/// round *well-formed*; [`pinned_to`] is what keeps it out of the union.
fn is_heading(line: &str) -> bool {
    let rest = match line.strip_prefix("## Code Review: Round ") {
        Some(rest) => rest,
        None => return false,
    };
    let after_round = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_round.len() == rest.len() {
        return false; // "Round " with no round number is not the R23 shape
    }
    let rest = after_round.strip_prefix(" (SHADOW)").unwrap_or(after_round);
    let rest = match rest.strip_prefix(" — PR #") {
        Some(rest) => rest,
        None => return false,
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = match rest.strip_prefix(" @ ") {
        Some(rest) => rest,
        None => return false,
    };
    let rest = rest.strip_suffix(" (SHADOW)").unwrap_or(rest);
    is_full_sha(rest)
}

/// Exactly 40 lowercase hex characters — the same shape `pr-review-watch.sh`'s
/// `is_full_sha` and REVIEW.md R5 require of a reviewed SHA.
fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Is this heading a verdict pinned to `sha`?
///
/// Deliberately **no** ` (SHADOW)` allowance: a SHADOW round is calibration
/// evidence, never a verdict, and never enters the union (REVIEW.md §8). A
/// SHADOW comment is therefore well-formed by [`is_heading`] — so it raises no
/// malformed notice — and simply contributes nothing.
fn pinned_to(line: &str, sha: &str) -> bool {
    let Some(rest) = line.strip_prefix("## Code Review: Round ") else {
        return false;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let Some(rest) = rest.strip_prefix(" — PR #") else {
        return false;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    rest.strip_prefix(" @ ").is_some_and(|tail| tail == sha)
}

/// The verdict word and its counts, from the `### Verdict` section.
///
/// Blocking side first, then the `Provisional — …` line the product adapter
/// writes for an unqualified LGTM (`edda review` exit 3, REVIEW.md §6.4, #998),
/// then plain LGTM. Order is the rule: a line reading
/// `Changes Requested, P0=0, P1=1` also contains no LGTM, but one reading
/// `Provisional — LGTM pending escalation` contains both words and must not be
/// read as a pass.
fn verdict_line(lines: &[&str]) -> Option<String> {
    let mut in_verdict = false;
    for line in lines.iter().skip(1) {
        let trimmed = line.trim_start_matches('#');
        if trimmed.len() != line.len() && trimmed.trim_start().starts_with("Verdict") {
            in_verdict = true;
            continue;
        }
        if !in_verdict {
            continue;
        }
        if line.contains("LGTM")
            || line.contains("Changes Requested")
            || line.starts_with("Provisional")
        {
            return Some((*line).to_owned());
        }
    }
    None
}

/// `P0=<n>` / `P1=<n>`; absent reads as an unstated count, which the union
/// rule treats as non-zero.
fn count(line: &str, key: &str) -> String {
    let Some(at) = line.find(key) else {
        return String::new();
    };
    line[at + key.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// Reduce §7 comments to the verdict facts standing on `sha`.
pub(crate) fn extract(sha: &str, comments: &[Comment]) -> Extracted {
    let mut out = Extracted::default();
    for comment in comments {
        let normalized: Vec<&str> = comment
            .body
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .collect();
        let Some(first) = normalized.first() else {
            continue;
        };
        if !is_heading(first) {
            // A §7 heading anywhere but line 1 is a transcript dump (#867),
            // not a verdict: no status, no label, no round. It earns one
            // notice so the round is not silently lost (#917).
            if normalized.iter().any(|line| is_heading(line)) {
                out.malformed.push(comment.id.clone());
            }
            continue;
        }
        if !pinned_to(first, sha) {
            continue;
        }
        let Some(vline) = verdict_line(&normalized) else {
            continue;
        };
        let word = if vline.contains("Changes Requested") {
            "Changes Requested"
        } else if vline.starts_with("Provisional") {
            "Provisional"
        } else {
            "LGTM"
        };
        out.lines.push(format!(
            "{word}\t{}\t{}",
            count(&vline, "P0="),
            count(&vline, "P1=")
        ));
    }
    out
}

/// The §7 comments GitHub holds for one PR.
fn comments(repo: &Path, pr: u64) -> Result<Vec<Comment>> {
    let value = gh(repo, &["pr", "view", &pr.to_string(), "--json", "comments"])
        .with_context(|| format!("read comments of PR #{pr}"))?;
    Ok(value["comments"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| Comment {
            id: entry["id"].as_str().unwrap_or_default().to_owned(),
            body: entry["body"].as_str().unwrap_or_default().to_owned(),
        })
        .collect())
}

/// `edda review deliver --pr <N>` — what the §7 comments on the reviewed SHA
/// amount to, and what a deliverer would therefore publish.
///
/// This is the read half. It moves the watcher's `verdict_body_lines` awk into
/// the product and answers with the union rule GH-769 owns; the GitHub writes
/// (comment, `review:*` labels, `Independent Review` status), their idempotency
/// and their exit-code contract are the next step of GH-1030 and are not
/// performed here. Nothing in this path writes to GitHub or to the ledger.
pub fn run(args: DeliverArgs, cwd: &Path) -> Result<()> {
    let sha = match &args.sha {
        Some(sha) => sha.clone(),
        None => super::github::resolve_pr(cwd, args.pr)?.head,
    };
    anyhow::ensure!(
        is_full_sha(&sha),
        "--sha must be a full 40-character lowercase hex commit"
    );
    let extracted = extract(&sha, &comments(cwd, args.pr)?);
    let union = gate::union(&gate::from_lines(&extracted.lines.join("\n")));
    let state = match union {
        gate::Union::Pass => "success",
        gate::Union::Fail => "failure",
        gate::Union::None => "error",
    };
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "pr": args.pr,
                "sha": sha,
                "status": state,
                "verdicts": extracted.lines,
                "malformed": extracted.malformed,
            })
        );
    } else {
        println!("{state} {sha} verdicts={}", extracted.lines.len());
        for id in &extracted.malformed {
            println!("malformed {id}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn comment(id: &str, body: &str) -> Comment {
        Comment {
            id: id.into(),
            body: body.into(),
        }
    }

    fn round(sha: &str, verdict: &str) -> String {
        format!("## Code Review: Round 1 — PR #1030 @ {sha}\n\n### Verdict\n\n{verdict}\n")
    }

    #[test]
    fn a_clean_lgtm_pinned_to_the_sha_is_one_verdict_line() {
        let got = extract(SHA, &[comment("1", &round(SHA, "LGTM (P0=0, P1=0)"))]);
        assert_eq!(got.lines, vec!["LGTM\t0\t0"]);
        assert!(got.malformed.is_empty());
    }

    #[test]
    fn changes_requested_wins_over_the_word_lgtm_on_the_same_line() {
        // "Changes Requested, P0=0, P1=2 — the LGTM cannot stand" contains
        // both words; reading it as a pass is how a blocking round is lost.
        let body = round(SHA, "Changes Requested, P0=0, P1=2 — the LGTM cannot stand");
        let got = extract(SHA, &[comment("1", &body)]);
        assert_eq!(got.lines, vec!["Changes Requested\t0\t2"]);
    }

    #[test]
    fn a_provisional_round_is_not_an_lgtm() {
        // The product adapter writes this for an unqualified LGTM (exit 3).
        let body = round(SHA, "Provisional — LGTM (P0=0, P1=0), escalation pending");
        let got = extract(SHA, &[comment("1", &body)]);
        assert_eq!(got.lines, vec!["Provisional\t0\t0"]);
    }

    #[test]
    fn a_verdict_for_another_sha_stands_on_that_sha_not_this_one() {
        let got = extract(SHA, &[comment("1", &round(OTHER, "LGTM (P0=0, P1=0)"))]);
        assert!(got.lines.is_empty());
        assert!(got.malformed.is_empty());
    }

    #[test]
    fn a_shadow_round_is_well_formed_and_contributes_nothing() {
        // REVIEW.md §8: a SHADOW round is never a verdict. It must not enter
        // the union — and it must not be reported malformed either, or every
        // calibration round would post a notice.
        for heading in [
            format!("## Code Review: Round 2 (SHADOW) — PR #1030 @ {SHA}"),
            format!("## Code Review: Round 2 — PR #1030 @ {SHA} (SHADOW)"),
        ] {
            let body = format!("{heading}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n");
            let got = extract(SHA, &[comment("1", &body)]);
            assert!(got.lines.is_empty(), "SHADOW entered the union: {heading}");
            assert!(
                got.malformed.is_empty(),
                "SHADOW reported malformed: {heading}"
            );
        }
    }

    #[test]
    fn a_heading_below_line_one_is_malformed_and_is_no_verdict() {
        // #867: a transcript dump that happens to contain the heading.
        let body = format!(
            "Here is the transcript of the round:\n\n{}",
            round(SHA, "LGTM (P0=0, P1=0)")
        );
        let got = extract(SHA, &[comment("42", &body)]);
        assert!(got.lines.is_empty(), "a transcript dump is not a verdict");
        assert_eq!(got.malformed, vec!["42"]);
    }

    #[test]
    fn an_ordinary_comment_is_neither_a_verdict_nor_malformed() {
        let got = extract(SHA, &[comment("7", "Rebased onto main, CI is green.")]);
        assert_eq!(got, Extracted::default());
    }

    #[test]
    fn a_heading_with_no_verdict_section_contributes_nothing() {
        let body = format!("## Code Review: Round 1 — PR #1030 @ {SHA}\n\n### Rules\n\nU1 PASS\n");
        assert!(extract(SHA, &[comment("1", &body)]).lines.is_empty());
    }

    #[test]
    fn an_unstated_count_stays_unstated_rather_than_becoming_zero() {
        // gate::from_lines reads a missing count as non-zero (fail closed).
        // Writing "0" here would invent a clean round out of a silent one.
        let got = extract(SHA, &[comment("1", &round(SHA, "LGTM"))]);
        assert_eq!(got.lines, vec!["LGTM\t\t"]);
    }

    #[test]
    fn crlf_bodies_parse_the_same_as_lf() {
        let body = round(SHA, "LGTM (P0=0, P1=0)").replace('\n', "\r\n");
        assert_eq!(
            extract(SHA, &[comment("1", &body)]).lines,
            vec!["LGTM\t0\t0"]
        );
    }

    #[test]
    fn comment_order_is_preserved_so_the_union_sees_every_standing_verdict() {
        let got = extract(
            SHA,
            &[
                comment("1", &round(SHA, "Changes Requested, P0=0, P1=1")),
                comment("2", &round(SHA, "LGTM (P0=0, P1=0)")),
            ],
        );
        assert_eq!(got.lines, vec!["Changes Requested\t0\t1", "LGTM\t0\t0"]);
    }

    #[test]
    fn an_uppercase_or_short_sha_in_the_heading_is_not_a_heading() {
        for bad in ["0123456789ABCDEF0123456789abcdef01234567", "0123456"] {
            let body = format!(
                "## Code Review: Round 1 — PR #1030 @ {bad}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
            );
            let got = extract(bad, &[comment("1", &body)]);
            assert!(got.lines.is_empty(), "accepted a malformed sha: {bad}");
        }
    }
}
