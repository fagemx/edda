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
//!
//! Since GH-1103, the read half is also the trust boundary: a §7 comment
//! counts only from an author whose `author_association` GitHub vouches
//! for ([`TRUSTED_ASSOCIATIONS`]), and this module is where that set is
//! stated — the drift and merge readers GH-1105 adds read it from here
//! rather than carrying a second definition.

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
    /// The author's `author_association` as GitHub reports it (`OWNER`,
    /// `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, `NONE`, …); `None` when the
    /// field is absent or not a string — which reads as untrusted, never as
    /// trusted (GH-1103's fail-closed direction).
    pub author_association: Option<String>,
}

/// The author associations a §7 comment is accepted from, stated once for
/// every reader of verdict comments (GH-1103; the drift and merge readers
/// GH-1105 adds share this). Matches the set `merge-reviewed-pr.sh` and
/// `verdict-drift.sh` filter on: this repository is PUBLIC, so a §7 heading
/// any GitHub account can post is not a verdict.
pub(crate) const TRUSTED_ASSOCIATIONS: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

/// Is this author association one [`TRUSTED_ASSOCIATIONS`] names?
///
/// A missing association is untrusted: GitHub not vouching for anyone is
/// not GitHub vouching for everyone.
pub(crate) fn trusted_association(association: Option<&str>) -> bool {
    association.is_some_and(|value| TRUSTED_ASSOCIATIONS.contains(&value))
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
    /// `(id, association)` of §7-shaped comments from authors outside
    /// [`TRUSTED_ASSOCIATIONS`] (or whose association GitHub did not report,
    /// the `None` arm). Not verdicts, not malformed, not SHADOW — but not
    /// silently dropped either: the delivery report names them, because a
    /// comment that looks exactly like a verdict and was refused for who
    /// wrote it is a fact the operator needs, not noise (GH-1103).
    pub untrusted: Vec<(String, Option<String>)>,
}

/// Does this line open a §7 verdict comment?
///
/// The R23 heading shape, with the ` (SHADOW)` suffix accepted in **both**
/// recorded positions — after `Round <N>` (REVIEW.md §7) and at the end of the
/// line (the #917 trim contract). Accepting it here is what makes a SHADOW
/// round *well-formed*; [`pinned_to`] is what keeps it out of the union.
fn is_heading(line: &str) -> bool {
    heading_parts(line).is_some()
}

/// The §7 heading, decomposed: `(round, sha, shadow)` (GH-1105).
///
/// One grammar, every reader: [`extract`] above and the drift and merge
/// readers in [`super::drift`] / [`super::merge`] parse the same heading
/// through this function rather than carrying a second regex for the same
/// rule. `shadow` is true when the heading self-declares ` (SHADOW)` in
/// either recorded position.
pub(crate) fn heading_parts(line: &str) -> Option<(String, String, bool)> {
    let rest = line.strip_prefix("## Code Review: Round ")?;
    let after_round = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_round.len() == rest.len() {
        return None; // "Round " with no round number is not the R23 shape
    }
    let round = &rest[..rest.len() - after_round.len()];
    let round_shadow = after_round.starts_with(" (SHADOW)");
    let rest = after_round.strip_prefix(" (SHADOW)").unwrap_or(after_round);
    let rest = rest.strip_prefix(" — PR #")?;
    let after_pr = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_pr.len() == rest.len() {
        return None; // "PR #" with no number is not the R23 shape
    }
    let rest = after_pr.strip_prefix(" @ ")?;
    let tail_shadow = rest.ends_with(" (SHADOW)");
    let sha = rest.strip_suffix(" (SHADOW)").unwrap_or(rest);
    if !is_full_sha(sha) {
        return None;
    }
    Some((
        round.to_owned(),
        sha.to_owned(),
        round_shadow || tail_shadow,
    ))
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
pub(crate) fn verdict_line(lines: &[&str]) -> Option<String> {
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
///
/// The trust gate runs **first** (GH-1103): a §7-shaped comment from an
/// author outside [`TRUSTED_ASSOCIATIONS`] is refused whole — no verdict
/// line, no malformed notice, no SHADOW signal — and is recorded in
/// [`Extracted::untrusted`] so the report says so. The malformed-notice
/// contract (#917) is therefore unchanged for trusted authors and
/// unreachable for untrusted ones, exactly as the issue requires.
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
        if !trusted_association(comment.author_association.as_deref()) {
            // Either position counts — line 1 or a transcript dump below it
            // — because both are "a §7 shape from someone untrusted", which
            // is the fact worth reporting; neither earns the #917 notice.
            if normalized.iter().any(|line| is_heading(line)) {
                out.untrusted
                    .push((comment.id.clone(), comment.author_association.clone()));
            }
            continue;
        }
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

/// The exact `gh` argv [`comments`] shells out with — REST (`gh api`), not
/// GraphQL (`gh pr view --json comments`): the GraphQL shape carries only
/// base64 node ids, which the #917 malformed-notice contract cannot use — it
/// needs the numeric id a human can resolve in the UI. `--paginate` alone (no
/// `--slurp`) is enough: `gh` combines an array-shaped REST endpoint's pages
/// into one JSON array before this ever sees it (verified live against this
/// repo, forcing multiple pages with `per_page=2`), so a comment list longer
/// than one page is never silently truncated on the merge-gate path.
///
/// Split out from [`comments`] — and actually consumed by it below, not just
/// left standing beside it — so the argv is asserted directly (GH-1079):
/// every prior test here feeds `extract`/`parse_comments` parsed JSON and
/// never sees what was actually requested, so Round 1's P1 (this endpoint
/// reverted back to `gh pr view --json comments`) left every module test
/// green. Because `comments` builds its call from this function's return
/// value rather than a second, independent literal, a revert has nowhere to
/// hide: either it changes what this returns (the test below goes red), or
/// it leaves `comments` not calling this at all (dead code, `-D warnings`).
///
/// GH-1105 shares this one definition with `edda review merge`'s comment
/// read — the same paginated REST endpoint, never a second literal.
pub(crate) fn comments_argv(pr: u64) -> Vec<String> {
    vec![
        "api".to_owned(),
        "--paginate".to_owned(),
        format!("repos/{{owner}}/{{repo}}/issues/{pr}/comments"),
    ]
}

/// The §7 comments GitHub holds for one PR.
pub(crate) fn comments(repo: &Path, pr: u64) -> Result<Vec<Comment>> {
    let argv = comments_argv(pr);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let value = gh(repo, &args).with_context(|| format!("read comments of PR #{pr}"))?;
    Ok(parse_comments(&value))
}

/// Map REST issue-comment JSON (a numeric `id`, a string `body`, a string
/// `author_association`) to [`Comment`]. Split out from [`comments`] so the
/// mapping is testable without shelling out to `gh`.
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
            author_association: entry["author_association"].as_str().map(str::to_owned),
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

/// Confirm `pr` is a pull request before trusting an operator-supplied
/// `--sha` to skip [`super::github::resolve_pr`]'s own PR-only fetch.
///
/// `--pr <issue-number> --sha <hex>` used to reach [`comments`] unvalidated:
/// `resolve_pr` is the only place that ever asked GitHub "is this really a
/// PR?", and `--sha` bypasses it (measured: `gh pr view 1030 --json comments`
/// exits 1 while `gh api .../issues/1030/comments` exits 0 with real
/// comments — GH-1079). `probe` is the injected PR-only check (the real path
/// passes [`super::github::pr_head`], which already exists for a lighter
/// reason and happens to fail exactly the way `resolve_pr` does on an issue
/// number); its `Ok` value is discarded here — this only wants its failure
/// mode. Tests fake `probe` to reproduce that failure without shelling out.
fn validate_pr_for_sha(pr: u64, probe: impl FnOnce(u64) -> Result<String>) -> Result<()> {
    probe(pr).map(|_| ()).with_context(|| {
        format!(
            "--pr {pr} did not resolve as a pull request (either {pr} is an issue rather than \
             a PR, or gh failed to confirm it — see the gh error this is chained to; --sha \
             only skips the PR head fetch, not this PR-vs-issue check, so an issue number \
             would otherwise read issue #{pr}'s comments)"
        )
    })
}

/// The read, decide, and write sequence; returns the exit code `run` should
/// use on success (0 delivered, 1 partially delivered, 2 failed, 3 a due
/// status/label withheld under R23/#917 — never returned as an `Err`, since
/// something either succeeded or was deliberately withheld this round; see
/// [`delivery::Delivery::exit_code`]).
///
/// Thin production wrapper over [`deliver_inner_with`]: the real PR-vs-issue
/// probe is [`super::github::pr_head`] and the real comment source is
/// [`comments`]. Split out so a fixture can drive the same sequence with both
/// seams faked — see that function's doc comment (GH-1079 Round 1 P1).
fn deliver_inner(args: &DeliverArgs, cwd: &Path) -> Result<i32> {
    deliver_inner_with(
        args,
        cwd,
        |number| super::github::pr_head(cwd, number),
        comments,
    )
}

/// [`deliver_inner`] with its `gh` seams injected.
///
/// This is what makes the `--pr <issue-number> --sha <hex>` wiring and
/// ordering testable without shelling out: a fixture can fake `probe` to
/// fail the way `gh` fails on an issue number, and fake `comments_fn` to
/// record whether it was ever reached, then assert both that the failure
/// surfaces naming `args.pr` and that `comments_fn` was never called — GH-1079
/// Round 1 P1. The prior test at this call site (`validate_pr_for_sha`
/// invoked directly) only proved that helper hands its own argument to its
/// own closure, which stays green even if the call below were deleted,
/// reordered after the comment read, or hardcoded to a number other than
/// `args.pr`; the seam here lets a fixture reach the call site itself.
fn deliver_inner_with(
    args: &DeliverArgs,
    cwd: &Path,
    probe: impl FnOnce(u64) -> Result<String>,
    comments_fn: impl FnOnce(&Path, u64) -> Result<Vec<Comment>>,
) -> Result<i32> {
    let sha = match &args.sha {
        Some(sha) => {
            // GH-1079 Round 1 P2: the free local shape check runs first, so
            // a malformed --sha is rejected without spending the gh
            // round-trip below.
            anyhow::ensure!(
                is_full_sha(sha),
                "--sha must be a full 40-character lowercase hex commit"
            );
            // GH-1079: without --sha, resolve_pr's own PR-only GraphQL fetch
            // already fails closed on an issue number. --sha skips that
            // fetch entirely, so this is the one gh call that stands in for
            // it here — gh's REST issue-comments endpoint below (unlike
            // resolve_pr) answers success on an issue number too, so nothing
            // downstream would otherwise notice #<pr> was never a PR.
            validate_pr_for_sha(args.pr, probe)?;
            sha.clone()
        }
        None => super::github::resolve_pr(cwd, args.pr)?.head,
    };
    // Redundant with the Some(sha) arm's own check above, but kept here too:
    // it is the only shape guard on the None arm's resolved head (resolve_pr
    // bails on a wrong-length or non-hex head, but not on stray uppercase —
    // is_full_sha does), and it stays cheap enough that checking it twice on
    // the --sha path costs nothing worth removing it for.
    anyhow::ensure!(
        is_full_sha(&sha),
        "--sha must be a full 40-character lowercase hex commit"
    );
    let existing_comments = comments_fn(cwd, args.pr)?;
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
                "untrusted": extracted.untrusted.iter().map(|(id, association)| {
                    serde_json::json!({
                        "comment_id": id,
                        "author_association": association,
                    })
                }).collect::<Vec<_>>(),
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
        for (id, association) in &extracted.untrusted {
            println!(
                "untrusted {id} {}",
                association.as_deref().unwrap_or("missing")
            );
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
        // OWNER: the fixture author is the repository owner, so every test
        // written before GH-1103's trust gate keeps meaning what it meant.
        Comment {
            id: id.into(),
            body: body.into(),
            author_association: Some("OWNER".into()),
        }
    }

    /// The same comment from an author GitHub vouches for in one of the
    /// other two recorded ways — GH-1103's trusted set is three wide, not
    /// one, and a test that only ever passes OWNER proves nothing about the
    /// other two.
    fn as_collaborator(mut comment: Comment) -> Comment {
        comment.author_association = Some("COLLABORATOR".into());
        comment
    }

    fn as_member(mut comment: Comment) -> Comment {
        comment.author_association = Some("MEMBER".into());
        comment
    }

    fn as_untrusted(mut comment: Comment) -> Comment {
        comment.author_association = Some("NONE".into());
        comment
    }

    fn as_unattributed(mut comment: Comment) -> Comment {
        comment.author_association = None;
        comment
    }

    fn round(sha: &str, verdict: &str) -> String {
        format!("## Code Review: Round 1 — PR #1030 @ {sha}\n\n### Verdict\n\n{verdict}\n")
    }

    #[test]
    fn a_clean_lgtm_pinned_to_the_sha_is_one_verdict_line() {
        let base = comment("1", &round(SHA, "LGTM (P0=0, P1=0)"));
        // All three trusted associations read identically (GH-1103): a
        // MEMBER's round and a COLLABORATOR's round are the same verdict
        // an OWNER's is.
        for variant in [base.clone(), as_member(base.clone()), as_collaborator(base)] {
            let got = extract(SHA, &[variant]);
            assert_eq!(got.lines, vec!["LGTM\t0\t0"]);
            assert!(got.malformed.is_empty());
            assert!(got.untrusted.is_empty());
        }
    }

    // ---- GH-1103: an untrusted author's §7 comment is not a verdict -------

    #[test]
    fn a_well_formed_lgtm_from_an_untrusted_author_is_no_verdict_gh1103() {
        // The forged-verdict direction: on a PUBLIC repo this comment is
        // one `gh api` call away from any account. It must not reach the
        // union, so it can never deliver a `success` status or a
        // `review:lgtm` label — with no verdict standing at all, the union
        // reads None (the `error` status of an unreviewed SHA), which is
        // the refusal this test pins.
        let got = extract(
            SHA,
            &[as_untrusted(comment("5", &round(SHA, "LGTM (P0=0, P1=0)")))],
        );
        assert!(got.lines.is_empty(), "an untrusted LGTM joined the union");
        assert!(got.malformed.is_empty());
        assert!(got.shadow.is_empty());
        assert_eq!(
            got.untrusted,
            vec![("5".to_owned(), Some("NONE".to_owned()))],
            "the refusal must be reported, not silent"
        );
    }

    #[test]
    fn a_changes_requested_from_an_untrusted_author_is_not_held_gh1103() {
        // The hold-the-fleet direction (R18's union): an outsider's blocker
        // must not keep a SHA's status at `failure` either. Untrusted reads
        // the same in both directions: nothing standing, union None.
        let got = extract(
            SHA,
            &[as_untrusted(comment(
                "5",
                &round(SHA, "Changes Requested, P0=0, P1=1"),
            ))],
        );
        assert!(got.lines.is_empty());
        assert_eq!(
            got.untrusted,
            vec![("5".to_owned(), Some("NONE".to_owned()))]
        );
    }

    #[test]
    fn a_comment_whose_association_is_missing_reads_untrusted_gh1103() {
        // Fail closed: GitHub not vouching for anyone is not GitHub
        // vouching for everyone. The report says `missing` (the None arm),
        // not a made-up association.
        let got = extract(
            SHA,
            &[as_unattributed(comment(
                "5",
                &round(SHA, "LGTM (P0=0, P1=0)"),
            ))],
        );
        assert!(got.lines.is_empty());
        assert_eq!(got.untrusted, vec![("5".to_owned(), None)]);
    }

    #[test]
    fn an_untrusted_malformed_comment_earns_no_notice_and_no_verdict_gh1103() {
        // #917's one-shot notice is for trusted authors only: an
        // untrusted transcript dump is simply not a verdict, and reporting
        // it as malformed would let any account mint a notice comment
        // through the delivery's own writer.
        let body = format!(
            "Here is the transcript of the round:\n\n{}",
            round(SHA, "LGTM (P0=0, P1=0)")
        );
        let got = extract(SHA, &[as_untrusted(comment("42", &body))]);
        assert!(got.malformed.is_empty(), "untrusted cannot earn a notice");
        assert!(got.lines.is_empty());
        assert_eq!(
            got.untrusted,
            vec![("42".to_owned(), Some("NONE".to_owned()))]
        );
    }

    #[test]
    fn an_untrusted_shadow_round_is_no_shadow_signal_gh1103() {
        // R22's "zero writes on SHADOW-only" is for rounds the fleet
        // posted; an outsider's (SHADOW) heading must not suppress the
        // `error` status an unreviewed SHA would otherwise get.
        let body = format!(
            "## Code Review: Round 2 (SHADOW) — PR #1030 @ {SHA}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
        );
        let got = extract(SHA, &[as_untrusted(comment("9", &body))]);
        assert!(got.shadow.is_empty());
        assert!(got.lines.is_empty());
        assert_eq!(
            got.untrusted,
            vec![("9".to_owned(), Some("NONE".to_owned()))]
        );
    }

    #[test]
    fn an_untrusted_ordinary_comment_is_not_even_reported_gh1103() {
        // Only §7-shaped comments are worth naming: an outsider saying
        // "looks good" is background noise, not a refused verdict.
        let got = extract(
            SHA,
            &[as_untrusted(comment(
                "7",
                "Rebased onto main, CI is green.",
            ))],
        );
        assert_eq!(got, Extracted::default());
    }

    #[test]
    fn a_trusted_verdict_stands_unchanged_alongside_an_untrusted_one_gh1103() {
        // Mixed comment list: the gate refuses exactly the untrusted half.
        let got = extract(
            SHA,
            &[
                as_untrusted(comment("5", &round(SHA, "Changes Requested, P0=0, P1=1"))),
                comment("6", &round(SHA, "LGTM (P0=0, P1=0)")),
            ],
        );
        assert_eq!(got.lines, vec!["LGTM\t0\t0"]);
        assert_eq!(
            got.untrusted,
            vec![("5".to_owned(), Some("NONE".to_owned()))]
        );
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
            {"id": 5573431960u64, "body": "## Code Review: Round 1 — PR #1030 @ 0123", "author_association": "OWNER"},
            {"id": 5579340504u64, "body": "another comment"},
        ]);
        let got = parse_comments(&value);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "5573431960");
        assert_eq!(got[0].body, "## Code Review: Round 1 — PR #1030 @ 0123");
        assert_eq!(
            got[0].author_association.as_deref(),
            Some("OWNER"),
            "GH-1103: the REST association must survive the parse"
        );
        assert_eq!(got[1].id, "5579340504");
        // The REST shape carries the association on every comment; a
        // fixture without it (or a non-string value) maps to None, which
        // extract reads as untrusted — the fail-closed arm.
        let stripped = serde_json::json!([
            {"id": 1u64, "body": "b"},
            {"id": 2u64, "body": "b", "author_association": null},
            {"id": 3u64, "body": "b", "author_association": 7},
        ]);
        for entry in parse_comments(&stripped) {
            assert_eq!(entry.author_association, None);
        }
    }

    #[test]
    fn parse_comments_on_an_unexpected_shape_yields_no_comments_rather_than_panicking() {
        let got = parse_comments(&serde_json::json!({"not": "an array"}));
        assert!(got.is_empty());
    }

    // ---- GH-1079: argv-level regression guard on the comment-source seam --

    #[test]
    fn comments_argv_is_the_paginated_rest_endpoint_not_the_graphql_view() {
        // Locks the exact call `comments` shells out with. A revert back to
        // `gh pr view <pr> --json comments` (Round 1's P1: base64 node ids
        // where #917 needs numeric ones, plus an unpaginated union input on
        // the merge-gate path) must change this function's return value —
        // `comments` has no other source for its argv — so this goes red on
        // that revert instead of staying green like every test above it,
        // which only ever sees already-parsed JSON.
        assert_eq!(
            comments_argv(1030),
            vec![
                "api",
                "--paginate",
                "repos/{owner}/{repo}/issues/1030/comments"
            ],
        );
    }

    #[test]
    fn comments_argv_is_not_the_old_pr_view_json_comments_shape() {
        // The specific shape a revert would reintroduce, spelled out so the
        // guard fails for the right, legible reason rather than merely any
        // mismatch.
        let argv = comments_argv(1030);
        assert_ne!(argv, vec!["pr", "view", "1030", "--json", "comments"]);
    }

    // ---- GH-1079: --sha validates PR-vs-issue before reading comments -----

    #[test]
    fn sha_path_rejects_a_number_gh_cannot_resolve_as_a_pull_request() {
        // Reproduces gh's real failure mode on an issue number
        // (`gh pr view 1030 --json headRefOid` => "Could not resolve to a
        // PullRequest with the number of 1030.") without shelling out.
        let err = validate_pr_for_sha(1030, |_| {
            Err(anyhow::anyhow!(
                "gh: GraphQL: Could not resolve to a PullRequest with the number of 1030. \
                 (repository.pullRequest)"
            ))
        })
        .expect_err("an issue number must not validate as a PR");
        let message = format!("{err:#}");
        assert!(
            message.contains("pull request") || message.contains("PR"),
            "message must name the PR-vs-issue distinction: {message}"
        );
        assert!(
            message.contains("1030"),
            "message must name the offending number: {message}"
        );
    }

    #[test]
    fn sha_path_accepts_a_number_gh_resolves_as_a_pull_request() {
        assert!(validate_pr_for_sha(1030, |_| Ok("deadbeef".repeat(5))).is_ok());
    }

    #[test]
    fn deliver_inner_probes_the_pr_number_before_reading_comments_and_names_it_on_failure() {
        // GH-1079 Round 1 P1: the test that used to stand here
        // (`sha_path_probe_is_called_with_the_pr_number_argument`) called
        // `validate_pr_for_sha` directly and asserted its closure saw the
        // number it was given — true no matter what `deliver_inner` does,
        // since it only proves that helper hands its own argument to its own
        // closure (`validate_pr_for_sha`'s entire body is `probe(pr)`). This
        // drives `deliver_inner_with` instead — the real `--pr <issue-number>
        // --sha <hex>` layer — through its injected seams, and checks all
        // three ways the wiring could break: the validation call deleted,
        // moved after the comment read, or wired to a number other than the
        // one `--pr` carries.
        let args = DeliverArgs {
            pr: 4242,
            sha: Some(SHA.to_owned()),
            json: false,
        };
        let seen_probe = std::cell::Cell::new(None);
        let comments_reached = std::cell::Cell::new(false);

        let result = deliver_inner_with(
            &args,
            Path::new("."),
            |number| {
                seen_probe.set(Some(number));
                Err(anyhow::anyhow!(
                    "gh: GraphQL: Could not resolve to a PullRequest with the number of \
                     {number}. (repository.pullRequest)"
                ))
            },
            |_cwd, _pr| {
                comments_reached.set(true);
                Ok(Vec::new())
            },
        );

        let err = result.expect_err("a PR that gh cannot resolve must fail deliver_inner");
        let message = format!("{err:#}");
        assert!(
            message.contains("4242"),
            "the surfaced error must name the number --pr carries: {message}"
        );
        assert_eq!(
            seen_probe.get(),
            Some(4242),
            "the probe must be called with the number --pr carries, not a hardcoded \
             or mismatched one"
        );
        assert!(
            !comments_reached.get(),
            "comments must never be fetched once PR-vs-issue validation has failed"
        );
    }
}
