//! `edda review deliver` — the GitHub delivery of a review round (GH-1030).
//!
//! Judging a SHA is `edda review gate` (GH-769). Deciding whether a round is
//! owed is `edda review due` (GH-763). What has had no product verb until now
//! is **delivery**: the §7 verdict comment, the `review:*` labels and the
//! `Independent Review` commit status lived only in the review shell, which is
//! why that layer is the one that broke on 2026-09-06/07 — the product could
//! judge but could not publish. That shell was retired in GH-1061; this module
//! is what replaces its publishing half.
//!
//! This module starts where that shell started: turning §7 verdict **comments**
//! into the verdict facts the union rule already consumes. That direction
//! matters. Verdicts do not cross machines in the ledger yet (`D8-debt(#671)`:
//! `review_verdict` is not among the event types the committed mirror imports),
//! and a round published only through the comment path writes no event at all,
//! so the comments — not the ledger — are what a deliverer can actually see.
//! The extracted lines are the tab-separated shape `gate::from_lines` already
//! parses, so the union rule stays in one place (GH-769) and this module never
//! re-decides what a verdict means.
//!
//! [`extract`] above is the read half (GH-1030 part 1, PR #1077). [`run`]
//! below is the write half: it feeds `extract`'s output to
//! [`super::delivery::deliver`], which performs the `review:*` label, the
//! `Independent Review` commit status, and the malformed-comment notice —
//! see that module's own doc comment for what it deliberately does not do
//! (post the primary verdict comment) and why.

use super::delivery;
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
    /// Ids of comments that are well-formed, sha-pinned §7 verdicts, self-
    /// declared ` (SHADOW)` (REVIEW.md §8, rules.md R22). Distinct from a
    /// SHA carrying no verdict at all: [`super::delivery`] performs zero
    /// GitHub writes when this is the only signal standing, rather than
    /// writing the `error` state [`super::gate::Union::None`] means for an
    /// unreviewed SHA.
    pub shadow: Vec<String>,
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

/// Exactly 40 lowercase hex characters — the shape REVIEW.md R5 requires of a
/// reviewed SHA.
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

/// Is this heading a §7 verdict for `sha`, self-declared SHADOW?
///
/// Same shape [`pinned_to`] pins, but — unlike it — accepts the ` (SHADOW)`
/// suffix in either recorded position (REVIEW.md §7/§8, rules.md R22): a
/// self-declared SHADOW round names its SHA exactly like any other verdict,
/// only decorated. [`pinned_to`] must keep refusing this shape (a SHADOW
/// round never enters the union); this is the delivery module's separate
/// signal for "well-formed, pinned here, but R22 says zero writes" —
/// distinguishing a SHADOW round from "nothing pinned to this SHA at all",
/// which [`pinned_to`] alone cannot do since both read as "not pinned".
fn shadow_pinned_to(line: &str, sha: &str) -> bool {
    let Some(rest) = line.strip_prefix("## Code Review: Round ") else {
        return false;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let round_shadow = rest.starts_with(" (SHADOW)");
    let rest = rest.strip_prefix(" (SHADOW)").unwrap_or(rest);
    let Some(rest) = rest.strip_prefix(" — PR #") else {
        return false;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let Some(tail) = rest.strip_prefix(" @ ") else {
        return false;
    };
    let tail_shadow = tail.ends_with(" (SHADOW)");
    let tail = tail.strip_suffix(" (SHADOW)").unwrap_or(tail);
    (round_shadow || tail_shadow) && tail == sha
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
            if shadow_pinned_to(first, sha) {
                out.shadow.push(comment.id.clone());
            }
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
///
/// REST (`gh api`), not GraphQL (`gh pr view --json comments`): the GraphQL
/// shape carries only base64 node ids, which the #917 malformed-notice
/// contract cannot use — it needs the numeric id a human can resolve in the
/// UI. `--paginate` alone (no `--slurp`) is enough: `gh` combines an
/// array-shaped REST endpoint's pages into one JSON array before this ever
/// sees it (verified live against this repo, forcing multiple pages with
/// `per_page=2`), so a comment list longer than one page is never silently
/// truncated on the merge-gate path.
pub(crate) fn comments(repo: &Path, pr: u64) -> Result<Vec<Comment>> {
    let value = gh(
        repo,
        &[
            "api",
            "--paginate",
            &format!("repos/{{owner}}/{{repo}}/issues/{pr}/comments"),
        ],
    )
    .with_context(|| format!("read comments of PR #{pr}"))?;
    Ok(parse_comments(&value))
}

/// Map REST issue-comment JSON (a numeric `id`, a string `body`) to
/// [`Comment`]. Split out from [`comments`] so the mapping is testable
/// without shelling out to `gh`.
fn parse_comments(value: &serde_json::Value) -> Vec<Comment> {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| Comment {
            id: entry["id"]
                .as_u64()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            body: entry["body"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// `edda review deliver --pr <N>` — deliver what the §7 comments on the
/// reviewed SHA amount to: the `review:*` label, the `Independent Review`
/// commit status, and a one-shot notice for any malformed comment.
///
/// Carries the retired watcher's `verdict_body_lines` awk logic into the
/// product and answers with the union rule GH-769 owns; [`delivery::deliver`]
/// performs the writes that rule implies, over the real `gh`-backed
/// [`delivery::GhCli`].
/// An unreadable comment list or an invalid `--sha` never reaches that far:
/// both leave through exit 2, the same "could not judge" contract
/// `edda review gate` uses, because nothing was delivered either way.
pub fn run(args: DeliverArgs, cwd: &Path) -> Result<()> {
    match deliver_inner(&args, cwd) {
        Ok(0) => Ok(()),
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("edda review deliver: {error:#}");
            std::process::exit(2);
        }
    }
}

/// The read, decide, and write sequence; returns the exit code `run` should
/// use on success (0 delivered, 1 partially delivered, 2 failed, 3 a due
/// status/label withheld under R23/#917 — never returned as an `Err`, since
/// something either succeeded or was deliberately withheld this round; see
/// [`delivery::Delivery::exit_code`]).
fn deliver_inner(args: &DeliverArgs, cwd: &Path) -> Result<i32> {
    let sha = match &args.sha {
        Some(sha) => sha.clone(),
        None => super::github::resolve_pr(cwd, args.pr)?.head,
    };
    anyhow::ensure!(
        is_full_sha(&sha),
        "--sha must be a full 40-character lowercase hex commit"
    );
    let existing_comments = comments(cwd, args.pr)?;
    let extracted = extract(&sha, &existing_comments);
    let union = gate::union(&gate::from_lines(&extracted.lines.join("\n")));
    let state = match union {
        gate::Union::Pass => "success",
        gate::Union::Fail => "failure",
        gate::Union::None => "error",
    };

    let gh_client = delivery::GhCli { repo: cwd };
    let result = delivery::deliver(
        &gh_client,
        args.pr,
        &sha,
        &existing_comments,
        &extracted,
        union,
    );
    let exit_code = result.exit_code();

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "pr": args.pr,
                "sha": sha,
                "status": state,
                "verdicts": extracted.lines,
                "malformed": extracted.malformed,
                "shadow": extracted.shadow,
                "notices": result.notices.iter().map(|(id, w)| serde_json::json!({
                    "comment_id": id, "outcome": w.tag(), "reason": w.reason(),
                })).collect::<Vec<_>>(),
                "status_write": result.status.as_ref().map(|w| serde_json::json!({
                    "outcome": w.tag(), "reason": w.reason(),
                })),
                "label": result.label.as_ref().map(|(name, w)| serde_json::json!({
                    "name": name, "outcome": w.tag(), "reason": w.reason(),
                })),
                "label_removed": result.label_removed.as_ref().map(|(name, w)| serde_json::json!({
                    "name": name, "outcome": w.tag(), "reason": w.reason(),
                })),
                "exit_code": exit_code,
            })
        );
    } else {
        println!("{state} {sha} verdicts={}", extracted.lines.len());
        for id in &extracted.malformed {
            println!("malformed {id}");
        }
        for (id, outcome) in &result.notices {
            println!(
                "notice {id} {}{}",
                outcome.tag(),
                outcome
                    .reason()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
        if let Some(outcome) = &result.status {
            println!(
                "status {state} {}{}",
                outcome.tag(),
                outcome
                    .reason()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
        if let Some((name, outcome)) = &result.label {
            println!(
                "label {name} {}{}",
                outcome.tag(),
                outcome
                    .reason()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
        if let Some((name, outcome)) = &result.label_removed {
            println!(
                "label_removed {name} {}{}",
                outcome.tag(),
                outcome
                    .reason()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(exit_code)
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
            // Distinct from "nothing pinned here at all": the delivery
            // module needs this to tell "SHADOW is the only signal" apart
            // from "no verdict exists" (R22 — zero writes vs. the `error`
            // status an unreviewed SHA gets).
            assert_eq!(got.shadow, vec!["1"], "SHADOW not surfaced: {heading}");
        }
    }

    #[test]
    fn a_shadow_round_for_another_sha_is_not_this_shas_shadow() {
        let body = format!(
            "## Code Review: Round 1 (SHADOW) — PR #1030 @ {OTHER}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
        );
        let got = extract(SHA, &[comment("1", &body)]);
        assert!(got.lines.is_empty());
        assert!(got.malformed.is_empty());
        assert!(got.shadow.is_empty());
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

    #[test]
    fn parse_comments_reads_the_rest_shape_a_numeric_id_not_a_graphql_node_id() {
        // gh api --paginate repos/{owner}/{repo}/issues/{n}/comments (REST)
        // returns a numeric id — not the base64 GraphQL node id
        // `gh pr view --json comments` would give, which the #917
        // malformed-notice contract cannot use. 5573431960 is the id from
        // this PR's own worked example (`malformed 5573431960`).
        let value = serde_json::json!([
            {"id": 5573431960u64, "body": "## Code Review: Round 1 — PR #1030 @ 0123"},
            {"id": 5579340504u64, "body": "another comment"},
        ]);
        let got = parse_comments(&value);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "5573431960");
        assert_eq!(got[0].body, "## Code Review: Round 1 — PR #1030 @ 0123");
        assert_eq!(got[1].id, "5579340504");
    }

    #[test]
    fn parse_comments_on_an_unexpected_shape_yields_no_comments_rather_than_panicking() {
        let got = parse_comments(&serde_json::json!({"not": "an array"}));
        assert!(got.is_empty());
    }
}
