//! `edda review merge` — the operator's merge entrypoint, in the product
//! (GH-1105).
//!
//! `scripts/merge-reviewed-pr.sh` — now a one-line adapter over this verb —
//! decided whether a PR may merge entirely in shell: the fleet-wide drift
//! gate (GH-993, advisory since #1124), the trusted-review checks, the union
//! rule (GH-769, GH-742), the malformed-comment refusal (#917), and required
//! checks.
//! That is control flow, parsing, and a trust boundary — three for three
//! against `mechanism.shell-role=one-line-adapter-only`. Every rule moves
//! here, unit-tested in Rust rather than by fixture shell, and the merge
//! preconditions live in ONE enumerated order ([`merge_inner`]) the merge
//! path actually consults, so a rule change cannot leave the shell and the
//! product disagreeing.
//!
//! ## What changed against the shell it replaces
//!
//! - **Read-only without `--merge`.** The shell's last step before its gate
//!   was an `edda review deliver` call — a write surface, said plainly in
//!   its own comments. This verb computes the union read-only; the
//!   `review:*` label and the `Independent Review` status are `edda review
//!   deliver`'s to write, which the reviewing session runs when it
//!   delivers its own round.
//! - **Cross-PR drift is advisory (#1124).** The walk still runs and every
//!   line it produced still prints, and it refuses nothing: coupling one
//!   merge to the whole open set made every PR hostage to unrelated ones, and
//!   drift across the open set is not among R6's conditions. The subject PR's
//!   own hold is enforced from the subject's own reads — mergeability in
//!   stage 2 (R24 forbids reporting a `CONFLICTING` PR ready, and `--check`
//!   is that report), an orphan Review Response after stage 3, the verdict's
//!   pinning in stage 4 — so a walk that cannot read cannot hide any of them
//!   (GH-1132 Round 2).
//! - **#1100 is folded in.** The squash subject is always the PR title —
//!   never the branch commit's own subject on a single-commit PR — and it
//!   is validated against the commit convention before any merge executes,
//!   then carries the ` (#N)` back-reference GitHub suppresses whenever
//!   `--subject` is supplied ([`merge_subject`]). The merge body comes from
//!   `--body-file` — which refuses an empty operand and refuses outside
//!   `--merge`, the only path that reads it — or a minimal gate receipt is
//!   composed naming the reviewed SHA, the LGTM round and the CI run.
//!
//! ## The window step
//!
//! Deliberately not the tree-level `edda review gate --base` check: that
//! needs both commits locally, and this entrypoint must work without a
//! checkout. At the moment that counts, the forge makes the equivalent
//! refusal — `--match-head-commit <head>` rejects anything landed on the
//! PR after the reviewed SHA, and branch protection rejects a base the PR
//! is behind. R6's window record stays a checkout-side act.

use super::deliver::{
    self, comments_argv, heading_parts, heading_shaped, trusted_association, Comment,
};
use super::drift;
use super::gate;
use super::github::{gh, gh_write_stdin, required_checks, RequiredChecks};
use anyhow::{Context, Result};
use std::path::Path;

/// Flags for `edda review merge`.
#[derive(clap::Args)]
pub struct MergeArgs {
    /// The pull request whose merge preconditions are checked
    #[arg(long)]
    pub pr: u64,
    /// Execute the squash merge after every precondition passes; without
    /// this the verb is validation only (requires operator authority)
    #[arg(long)]
    pub merge: bool,
    /// Validation only — the default; accepted for the shell's usage shape
    #[arg(long)]
    pub check: bool,
    /// Merge-commit body from this file; default composes a minimal gate
    /// receipt (GH-1100)
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<String>,
    /// Emit the merge report as JSON
    #[arg(long)]
    pub json: bool,
}

/// One `(pr, head)` merge evaluation's GitHub needs, factored out so the
/// precondition order is testable without the network (the same shape
/// `delivery::Gh` took for GH-1030's "injectable gh, no network").
pub(crate) trait Reads {
    /// The PR's head SHA, state, and title (`gh pr view <pr> --json`).
    ///
    /// `pr` is not decoration: without it `gh` resolves the pull request of
    /// whatever branch the cwd is on, so the gate would judge one PR and
    /// squash another. See [`pr_argv`].
    fn pr(&self, pr: u64) -> Result<Pr>;
    /// Current base-tip SHA. Only delegated control merges require this
    /// additional exact-base binding; legacy review merge behavior is unchanged.
    fn base_sha(&self, _pr: u64) -> Result<String> {
        anyhow::bail!("PR base SHA is unavailable")
    }
    /// The REST issue comments, newest-last, each with its `updated_at`.
    fn comments(&self, pr: u64) -> Result<Vec<TimedComment>>;
    /// The fleet-wide drift walk (`edda review drift`'s own query). Stage 1
    /// prints every line of it and refuses on none of it (#1124): the subject
    /// PR's own hold is read from the subject, further down the stages.
    fn drift(&self) -> Result<drift::Report>;
    /// Authoritative plain required-check result plus its refusal diagnostic.
    fn required_checks(&self, pr: u64) -> RequiredChecks;
    /// The `CI Gate` check-run URL for the receipt body, if resolvable.
    fn ci_gate_url(&self, sha: &str) -> Option<String>;
    /// Execute the squash merge with subject and body pinned (GH-1100).
    fn squash(&self, pr: u64, head: &str, subject: &str, body: &str) -> Result<()>;
}

pub(crate) struct Pr {
    pub head: String,
    pub state: String,
    pub title: String,
    /// GitHub's mergeability answer (`MERGEABLE`, `CONFLICTING`, or the
    /// not-yet-computed `UNKNOWN`). Read on the subject so R24's
    /// "must not be CONFLICTING when reported ready" holds independently of
    /// the fleet-wide drift walk (#1124 Round 2).
    pub mergeable: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ControlledMergeBinding {
    pub pr: u64,
    pub expected_head_sha: String,
    pub expected_base_sha: String,
}

/// A REST issue comment plus the `updated_at` timestamp the
/// latest-trusted-review selection orders by — the one field
/// [`deliver::Comment`] does not carry, because nothing in delivery sorts.
#[derive(Debug, Clone)]
pub(crate) struct TimedComment {
    pub comment: Comment,
    pub updated_at: String,
}

/// The commit-convention types `.claude/CLAUDE.md` fixes — the same list
/// REVIEW.md enforces on a PR's commits, now enforced on the one commit
/// that actually lands.
const TYPES: [&str; 6] = ["feat", "fix", "docs", "refactor", "test", "chore"];

/// Why a PR title cannot be the squash subject (GH-1100). `None` is valid.
pub(crate) fn subject_problem(title: &str) -> Option<String> {
    let Some((head, description)) = title.split_once(": ") else {
        return Some("not <type>(<scope>): <description> — no ': ' separator".into());
    };
    let Some(close) = head.strip_suffix(')') else {
        return Some("no (scope) before ': '".into());
    };
    let Some(open) = close.find('(') else {
        return Some("no (scope) before ': '".into());
    };
    let (kind, scope) = (&head[..open], &close[open + 1..]);
    if !TYPES.contains(&kind) {
        return Some(format!("type '{kind}' is not one of {}", TYPES.join("|")));
    }
    if scope.is_empty()
        || !scope
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Some("scope is empty or carries characters a path cannot".into());
    }
    if description.trim().is_empty() {
        return Some("the description after ': ' is empty".into());
    }
    None
}

/// The squash subject GitHub itself would have written (GH-1100 Round 2).
///
/// GitHub appends the ` (#N)` PR back-reference only to a squash subject IT
/// chooses; a subject handed to `gh pr merge --subject` is used verbatim,
/// with no suffix. Pinning the title through `--subject` and stopping there
/// therefore strips the PR pointer from every squash commit that lands on
/// `main` — measured on #1118, not theorised: of the last 60 subjects on
/// `main`, 58 carry ` (#N)`, and the only two that do not are exactly the two
/// merged by hand with an explicit `--subject`. R7 forbids rewriting `main`,
/// so each such commit would stay pointer-less forever. Compose what GitHub
/// would have written instead.
///
/// The append is skipped only when the title already ends in THIS PR's own
/// number — the one case where appending would duplicate it. A title ending
/// in some OTHER PR's number still gets ` (#pr)` appended: leaving a foreign
/// number standing as the trailing back-reference would point `git log` at an
/// unrelated PR, which is worse than a subject reading `… (#999) (#pr)`.
///
/// [`subject_problem`] judges the bare title, never this: a trailing ` (#N)`
/// can neither rescue a bad title nor break a good one.
pub(crate) fn merge_subject(title: &str, pr: u64) -> String {
    let back_reference = format!(" (#{pr})");
    if title.ends_with(&back_reference) {
        title.to_owned()
    } else {
        format!("{title}{back_reference}")
    }
}

/// Is this comment a SHADOW round by the gate's own marker — the heading
/// suffix (`REVIEW.md` §8: "the suffix is the only marker, this field never
/// substitutes for it")?
///
/// A calibration round is not a verdict, so it is never the review that
/// decides a merge: it cannot approve one, and — pinned to an older SHA — it
/// cannot refuse one either (#1124 Round 8). The marker is read off the FIRST
/// heading-shaped line, the same line [`latest_review_round`] parses. A body
/// whose heading does not PARSE is not a SHADOW round and stays a candidate:
/// it has to block the gate rather than disappear from it (#917).
fn is_shadow_round(body: &str) -> bool {
    let lines: Vec<&str> = body
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    lines
        .iter()
        .copied()
        .find(|line| heading_shaped(line))
        .and_then(heading_parts)
        .is_some_and(|(_, _, shadow)| shadow)
}

/// Is this the §7 `- escalations: none` line the merge gate requires?
fn escalations_none(line: &str) -> bool {
    line.trim_end() == "- escalations: none"
}

/// The exact approving verdict line: `LGTM (P0=0, P1=0)` then end or space.
fn approves(line: &str) -> bool {
    let rest = line
        .strip_prefix("LGTM (P0=0, P1=0)")
        .map(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
    rest == Some(true) && !line.to_ascii_lowercase().contains("provisional")
}

/// The minimal merge-body receipt composed when `--body-file` is absent
/// (GH-1100): a merge with no receipt at all is the thing this repository
/// keeps re-learning to avoid.
fn receipt_body(pr: u64, head: &str, round: &str, ci: Option<&str>) -> String {
    format!(
        "Squash merge of PR #{pr}, reviewed at {head}.\n\n\
         - verdict: LGTM (P0=0, P1=0), Round {round} (§7 comment, trusted author)\n\
         - union: pass over every §7 verdict pinned to this SHA (GH-769)\n\
         - drift: advisory, not an R6 condition — the subject PR's own gate decides (#1124)\n\
         - required checks: green\n\
         - CI Gate: {}\n",
        ci.unwrap_or("run id not resolved"),
    )
}

/// R24's mergeability condition on the subject (GH-1124 Rounds 2-3): a
/// `CONFLICTING` PR must not be reported ready — `--check` is that report —
/// and an unreadable answer is never a green one. The match is exhaustive
/// over the three values GitHub returns; everything else, including a missing
/// or non-string field (both parse as an empty string), is exit 2. `UNKNOWN`,
/// returned before GitHub has computed mergeability, is not a hold, matching
/// the walk's own rule. Read on the subject, so a fleet walk that cannot read
/// cannot hide it. `Err(exit_code)` after printing the refusal.
fn mergeability_refusal(pr: u64, mergeable: &str, unknown_refuses: bool) -> Result<(), i32> {
    match mergeable {
        "MERGEABLE" => Ok(()),
        "UNKNOWN" if !unknown_refuses => Ok(()),
        "UNKNOWN" => {
            eprintln!(
                "PR #{pr} mergeability is UNKNOWN — delegated control never treats an unknown \
                 forge state as mergeable; refusing"
            );
            Err(1)
        }
        "CONFLICTING" => {
            eprintln!(
                "PR #{pr} is CONFLICTING — R24 requires mergeable != CONFLICTING before a PR \
                 is reported ready; refusing (#1124)"
            );
            Err(1)
        }
        other => {
            eprintln!(
                "cannot read PR #{pr}'s mergeability (gh reported {other:?}) — an unread \
                 answer is never a green one; refusing (#1124, R24)"
            );
            Err(2)
        }
    }
}

/// The subject PR's own holds, decided over the subject's own comments with
/// the walk's own parsing — the same rule the walk applies, without a second
/// read and without depending on the walk completing (#1124 Rounds 2, 4, 7):
///
/// - a `## Review Response: Round N` answering a round that was never posted
///   (GH-993);
/// - the newest AUTHORITATIVE §7 round in comment order pinned to an older
///   SHA ([`drift::stale_authoritative`]) — authoritative only: a SHADOW round
///   is not a verdict (`docs/fleet/rules.md` R18) and never enters the union,
///   so it must not be able to hold the subject.
///
/// The second half is not decoration. The walk's reducer orders a PR's comments
/// by creation and the merge gate's own latest-review selection
/// ([`latest_review_round`]) orders them by GitHub's edit time, so an older
/// head-pinned LGTM edited after a later authoritative verdict is newest by
/// edit — stage 4 approves it — while the other ordering still reads the later
/// verdict as newest, and stale. Two readings of the subject's own comments
/// that disagree are not a green (Round 4's first P0) — but only verdicts get
/// a reading at all (Round 7's P1).
///
/// `Err(1)` after printing the refusal.
fn subject_hold_refusal(pr: u64, head: &str, comments: &[Comment]) -> Result<(), i32> {
    if let Some(round) = drift::reduce(head, comments).1 {
        eprintln!(
            "PR #{pr} carries an orphan Review Response for Round {round} — a response to \
             a round that was never posted (GH-993); refusing (#1124)"
        );
        return Err(1);
    }
    if let Some(sha) = drift::stale_authoritative(head, comments) {
        eprintln!(
            "PR #{pr}'s newest authoritative §7 round in comment order reads stale from {} — \
             the walk orders these comments by creation and the merge gate orders them by edit, \
             and two readings of the subject that disagree are not a green; refusing (#1124)",
            &sha[..12]
        );
        return Err(1);
    }
    Ok(())
}

/// The latest trusted §7 review's own four checks, in the shell's order:
/// a trusted comment carrying a §7-*shaped* heading exists at all
/// ([`heading_shaped`] — the shell's prefix rule, so a round whose heading
/// does not parse blocks the gate instead of disappearing from it); its
/// FIRST §7 heading line is pinned to `head` (`grep -m1` — line 1 for a
/// well-formed comment, any line for a transcript dump, #867); escalations
/// resolve; the verdict approves with
/// P0=0/P1=0 and is not Provisional. Latest is by `(updated_at, numeric
/// id)` — GitHub's edit ordering, so an edited round returns to newest the
/// way the shell's `sort_by([.updated_at, .id])` sorted it.
///
/// A SHADOW round ([`is_shadow_round`]) is never a candidate: it is not a
/// verdict, so it decides nothing — neither approving nor, pinned to an older
/// SHA, refusing (#1124 Round 8).
///
/// `Ok(round)` on every check passing; `Err(exit_code)` after printing the
/// refusal, mirroring `merge_inner`'s own stages.
fn latest_review_round(timed: &[TimedComment], pr: u64, head: &str) -> Result<String, i32> {
    // `(updated_at, id, comment)` rather than a `key()` over the comment: the
    // ordering keys are validated once, below, and carried, so nothing here
    // can fall back to a default the sort would then trust.
    let mut latest: Option<(&str, u64, &TimedComment)> = None;
    for candidate in timed.iter().filter(|t| {
        trusted_association(t.comment.author_association.as_deref())
            && t.comment
                .body
                .lines()
                .any(|line| heading_shaped(line.trim_end_matches('\r')))
            && !is_shadow_round(&t.comment.body)
    }) {
        // Fail closed on the REST keys this ordering is built from, as the
        // shell did: `error("trusted review lacks REST updated_at or numeric
        // id")` → `die 'cannot select latest updated trusted review'`, exit 1
        // (`scripts/merge-reviewed-pr.sh`, pre-adapter blob). Defaulting them
        // instead — `unwrap_or_default()` on the read, `unwrap_or(0)` on the
        // parse — sorts an unreadable comment silently to the bottom, so an
        // edited blocker the gate cannot order loses to an older LGTM. A
        // source that cannot be read yields a refusal, never "clean"
        // (GH-1105 doneWhen 4); exit 1 is the shell's own code for it.
        let Ok(id) = candidate.comment.id.parse::<u64>() else {
            eprintln!(
                "cannot select latest updated trusted review: a trusted §7 review carries no \
                 numeric REST id (got {:?})",
                candidate.comment.id
            );
            return Err(1);
        };
        if candidate.updated_at.is_empty() {
            eprintln!(
                "cannot select latest updated trusted review: trusted §7 review {id} carries no \
                 REST updated_at"
            );
            return Err(1);
        }
        let key = (candidate.updated_at.as_str(), id);
        if latest.is_none_or(|(updated_at, current_id, _)| key > (updated_at, current_id)) {
            latest = Some((key.0, key.1, candidate));
        }
    }
    let Some((_, _, latest)) = latest else {
        eprintln!("no trusted §7 review on PR #{pr}");
        return Err(1);
    };
    let body: Vec<&str> = latest
        .comment
        .body
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    // The shell's `grep -m1 '^## Code Review: Round '`: the FIRST
    // heading-shaped line, then parsed — not the first line that happens to
    // parse. Scanning for a parseable heading instead would step over a
    // broken one and judge a well-formed heading further down the same body.
    let Some(heading) = body.iter().copied().find(|line| heading_shaped(line)) else {
        eprintln!("latest trusted review carries no §7 heading");
        return Err(1);
    };
    let Some((round, pinned_to, _)) = heading_parts(heading) else {
        eprintln!(
            "latest trusted review's §7 heading does not parse: {heading:?} is not \
             `## Code Review: Round <N> — PR #<n> @ <40 lowercase hex>`, so it is not pinned to \
             current head {head}"
        );
        return Err(1);
    };
    if pinned_to != head {
        eprintln!("latest trusted review is not pinned to current head {head}");
        return Err(1);
    }
    if !body.iter().any(|line| escalations_none(line)) {
        eprintln!("review has missing or unresolved escalations");
        return Err(1);
    }
    match deliver::verdict_line(&body).as_deref().map(approves) {
        Some(true) => Ok(round),
        _ => {
            eprintln!("latest review does not approve with P0=0/P1=0");
            Err(1)
        }
    }
}

/// The whole merge decision, one enumerated order (R6). Fail-fast on the
/// same stages the shell refused at, for the same reasons and in the same
/// order: the drift walk (stage 1, reported since #1124 and no longer a
/// refusal stage) before the PR's own facts before the review's own facts
/// before the forge's required checks.
pub(crate) fn merge_inner(args: &MergeArgs, reads: &dyn Reads) -> Result<i32> {
    merge_inner_with_binding(args, reads, None)
}

#[allow(clippy::too_many_lines)]
fn merge_inner_with_binding(
    args: &MergeArgs,
    reads: &dyn Reads,
    binding: Option<&ControlledMergeBinding>,
) -> Result<i32> {
    if args.merge && args.check {
        anyhow::bail!("--merge and --check are mutually exclusive; --check is the default");
    }
    // `--body-file`'s own two refusals, ported with the rest of #1100 and
    // decided here, before any read, exactly where the shell decided them.
    //
    // An empty operand short-circuited the shell's `-f` test and read as "no
    // body file", so the one malformed operand that did not refuse was the
    // emptiest; and only the merge path ever opens the file, so accepting it
    // under `--check` told the caller their body had been taken when nothing
    // would ever read it. The existence check joins them because a typo'd
    // path should cost an error message, not the whole fleet-wide drift walk
    // that stage 1 is about to run.
    if let Some(path) = &args.body_file {
        if path.is_empty() {
            anyhow::bail!("--body-file requires a non-empty path");
        }
        if !args.merge {
            anyhow::bail!("--body-file applies to --merge only; --check reads no body");
        }
        if !Path::new(path).is_file() {
            anyhow::bail!("body file not found: {path}");
        }
    }
    // 1. Fleet-wide drift (GH-993) — printed for every PR, refused for none
    //    (#1124): cross-PR drift is not an R6 condition, and while it refused,
    //    every merge was hostage to the whole open set. The subject PR's own
    //    hold is read from the subject below (its mergeability in stage 2, its
    //    own orphan response after stage 3, its verdict pinning in stage 4),
    //    so a walk that cannot read — or another PR's unreadable comments —
    //    cannot hide it.
    match reads.drift() {
        Ok(report) => {
            let block = drift::advisory(&report.lines, report.any_holds);
            if !block.is_empty() {
                eprintln!("{block}");
            }
        }
        Err(error) => eprintln!(
            "verdict-drift could not read PR state ({error:#}) — advisory unavailable; the \
             subject PR's own reads above and below still decide (#1124)"
        ),
    }
    // 2. The PR's own facts.
    let pr = match reads.pr(args.pr) {
        Ok(pr) => pr,
        Err(error) => {
            eprintln!("cannot read PR head: {error:#}");
            return Ok(2);
        }
    };
    if !is_head(&pr.head) {
        eprintln!("invalid PR head {}", pr.head);
        return Ok(2);
    }
    if let Some(binding) = binding {
        if args.pr != binding.pr || pr.head != binding.expected_head_sha {
            eprintln!(
                "delegated merge subject moved: expected PR #{} @ {}, observed PR #{} @ {}",
                binding.pr, binding.expected_head_sha, args.pr, pr.head
            );
            return Ok(1);
        }
        let base = match reads.base_sha(args.pr) {
            Ok(base) if is_head(&base) => base,
            Ok(base) => {
                eprintln!("invalid PR base SHA {base}");
                return Ok(2);
            }
            Err(error) => {
                eprintln!("cannot read PR base: {error:#}");
                return Ok(2);
            }
        };
        if base != binding.expected_base_sha {
            eprintln!(
                "delegated merge base moved: expected {}, observed {}",
                binding.expected_base_sha, base
            );
            return Ok(1);
        }
        if pr.state == "MERGED" {
            println!(
                "review merge adopted: PR #{} already merged at expected head {}",
                args.pr, pr.head
            );
            return Ok(0);
        }
    }
    if pr.state != "OPEN" {
        eprintln!("PR is {}", pr.state);
        return Ok(1);
    }
    // R24's mergeability condition, read on the subject rather than inferred
    // from the fleet walk (#1124 Rounds 2-3).
    if let Err(code) = mergeability_refusal(args.pr, &pr.mergeable, binding.is_some()) {
        return Ok(code);
    }
    // 3. The verdict comments the union and the review checks read.
    let timed = match reads.comments(args.pr) {
        Ok(timed) => timed,
        Err(error) => {
            eprintln!("cannot read reviews: {error:#}");
            return Ok(2);
        }
    };
    let comments: Vec<Comment> = timed.iter().map(|t| t.comment.clone()).collect();

    // 3b. The subject's own holds, decided over the subject's own comments by
    //     the walk's own reducer — the orphan response and the states that
    //     hold (#1124 Rounds 2 and 4).
    if let Err(code) = subject_hold_refusal(args.pr, &pr.head, &comments) {
        return Ok(code);
    }

    // 4. The latest trusted §7 review's own checks.
    let round = match latest_review_round(&timed, args.pr, &pr.head) {
        Ok(round) => round,
        Err(code) => return Ok(code),
    };

    // 5. The union gate (GH-1057): an ADDITIONAL refusal over every §7
    //    verdict pinned to this head — a later LGTM never overrides an
    //    earlier standing Changes Requested (GH-742).
    let extracted = deliver::extract(&pr.head, &comments);
    let union = gate::union(&gate::from_lines(&extracted.lines.join("\n")));
    let state = match union {
        gate::Union::Pass => "success",
        gate::Union::Fail => "failure",
        gate::Union::None => "error",
    };
    if state != "success" {
        eprintln!(
            "union over every §7 verdict comment pinned to {} is '{state}', not a pass; a \
             later LGTM does not override an earlier Changes Requested (GH-742)",
            pr.head
        );
        return Ok(1);
    }
    if !extracted.malformed.is_empty() {
        eprintln!(
            "{} verdict comment(s) on {} are malformed and outside the union; refusing",
            extracted.malformed.len(),
            pr.head
        );
        return Ok(1);
    }

    // 6. Only the original plain check's success is Green; JSON only diagnoses refusal.
    if let Some((code, message)) = reads.required_checks(args.pr).refusal(args.pr) {
        eprintln!("{message}");
        return Ok(code);
    }

    // 7. The squash subject, derived from the PR title and validated
    //    against the commit convention (GH-1100) — `wip`, an empty scope,
    //    or a missing type refuse with the offending string named.
    if let Some(problem) = subject_problem(&pr.title) {
        eprintln!(
            "PR title {:?} cannot be the squash subject: {problem} — the subject is always \
             derived from the PR title, never from a branch commit (GH-1100)",
            pr.title
        );
        return Ok(1);
    }
    // …and only then composed, so the convention check above judged the bare
    // title and this carries the back-reference `--subject` would otherwise
    // suppress (GH-1100 Round 2).
    let subject = merge_subject(&pr.title, args.pr);

    // 8. The merge body: the operator's file, or the composed receipt.
    let body_text = match &args.body_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("read merge body {path}: {error}");
                return Ok(2);
            }
        },
        None => receipt_body(
            args.pr,
            &pr.head,
            &round,
            reads.ci_gate_url(&pr.head).as_deref(),
        ),
    };

    if args.merge {
        // Re-read the delegated base at the last product-owned boundary before
        // invoking GitHub's merge operation. GitHub exposes an atomic expected
        // head precondition but no expected-base operand, so this is the
        // strongest available exact-base refusal without replacing the sole
        // review merge implementation.
        if let Some(binding) = binding {
            let base = match reads.base_sha(args.pr) {
                Ok(base) if is_head(&base) => base,
                Ok(base) => {
                    eprintln!("invalid PR base SHA {base}");
                    return Ok(2);
                }
                Err(error) => {
                    eprintln!("cannot re-read PR base before merge: {error:#}");
                    return Ok(2);
                }
            };
            if base != binding.expected_base_sha {
                eprintln!(
                    "delegated merge base moved before effect: expected {}, observed {}",
                    binding.expected_base_sha, base
                );
                return Ok(1);
            }
        }
        if let Err(error) = reads.squash(args.pr, &pr.head, &subject, &body_text) {
            eprintln!("merge failed: {error:#}");
            return Ok(2);
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "pr": args.pr,
                "head": pr.head,
                "subject": subject,
                "accepted": true,
                "merged": args.merge,
            })
        );
    } else {
        println!("review accepted: PR #{} @ {}", args.pr, pr.head);
    }
    Ok(0)
}

fn is_head(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// CLI entry point. Exit: 0 accepted (merged with `--merge`), 1 refused —
/// a precondition failed, 2 cannot judge — a read failed, and an
/// unreadable answer is never an approval.
pub fn run(args: MergeArgs, cwd: &Path) -> Result<()> {
    let reads = GhReads { cwd };
    match merge_inner(&args, &reads) {
        Ok(0) => Ok(()),
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("edda review merge: {error:#}");
            std::process::exit(2);
        }
    }
}

#[allow(dead_code)] // Retained as a direct safety seam; control refuses before intent without base CAS.
pub(crate) fn run_controlled_merge(cwd: &Path, binding: &ControlledMergeBinding) -> Result<i32> {
    let reads = GhReads { cwd };
    merge_inner_with_binding(
        &MergeArgs {
            pr: binding.pr,
            merge: true,
            check: false,
            body_file: None,
            json: false,
        },
        &reads,
        Some(binding),
    )
}

/// The exact `gh pr view` argv [`GhReads::pr`] shells out with, split out —
/// and actually consumed below — for the same reason [`comments_argv`] and
/// `drift::open_prs_argv` are (GH-1079): the argv is asserted directly.
///
/// The shell this replaced named the PR explicitly — the pre-adapter blob of
/// `scripts/merge-reviewed-pr.sh` read
/// `gh pr view "$pr" --repo "$repo" --json headRefOid,state`. Dropping the
/// selector does not fail loudly — `gh` falls back to the pull request of the
/// **current branch**, so from a checkout on `main` the verb exits 2, and from
/// a worktree carrying its own PR it evaluates every precondition, and derives
/// the squash subject (GH-1100), from a PR nobody asked about. The `Reads`
/// seam that keeps the other 17 tests offline is exactly what hid it: a `Fake`
/// answers whatever the fixture holds no matter what was requested. Hence this
/// function and the test that pins its shape.
///
/// The repository is not included in this logical argv: `github::command`
/// resolves and validates one explicit repository, then appends `--repo` to
/// every non-API `gh` child. API templates are concretized through that same
/// binding, while inherited selectors are removed from the child environment.
pub(crate) fn pr_argv(pr: u64) -> Vec<String> {
    vec![
        "pr".to_owned(),
        "view".to_owned(),
        pr.to_string(),
        "--json".to_owned(),
        "headRefOid,state,title,mergeable".to_owned(),
    ]
}

/// The real `gh`-backed [`Reads`].
struct GhReads<'a> {
    cwd: &'a Path,
}

impl Reads for GhReads<'_> {
    fn pr(&self, pr: u64) -> Result<Pr> {
        let argv = pr_argv(pr);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let value = gh(self.cwd, &args).with_context(|| format!("read PR #{pr}"))?;
        Ok(Pr {
            head: value["headRefOid"].as_str().unwrap_or_default().to_owned(),
            state: value["state"].as_str().unwrap_or_default().to_owned(),
            title: value["title"].as_str().unwrap_or_default().to_owned(),
            mergeable: value["mergeable"].as_str().unwrap_or_default().to_owned(),
        })
    }

    fn base_sha(&self, pr: u64) -> Result<String> {
        let value = gh(
            self.cwd,
            &["pr", "view", &pr.to_string(), "--json", "baseRefOid"],
        )?;
        value["baseRefOid"]
            .as_str()
            .map(str::to_owned)
            .context("PR base SHA missing")
    }

    fn comments(&self, pr: u64) -> Result<Vec<TimedComment>> {
        let argv = comments_argv(pr);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let value = gh(self.cwd, &args).with_context(|| format!("read comments of PR #{pr}"))?;
        Ok(value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| TimedComment {
                comment: Comment {
                    id: entry["id"]
                        .as_u64()
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    body: entry["body"].as_str().unwrap_or_default().to_owned(),
                    author_association: entry["author_association"].as_str().map(str::to_owned),
                },
                updated_at: entry["updated_at"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    fn drift(&self) -> Result<drift::Report> {
        drift::evaluate(
            self.cwd,
            std::env::var("EDDA_OPEN_PR_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(200),
        )
    }

    fn required_checks(&self, pr: u64) -> RequiredChecks {
        required_checks(self.cwd, pr)
    }

    fn ci_gate_url(&self, sha: &str) -> Option<String> {
        let value = gh(
            self.cwd,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/commits/{sha}/check-runs"),
            ],
        )
        .ok()?;
        value["check_runs"]
            .as_array()?
            .iter()
            .find(|run| run["name"].as_str() == Some("CI Gate"))
            .and_then(|run| run["html_url"].as_str())
            .map(str::to_owned)
    }

    fn squash(&self, pr: u64, head: &str, subject: &str, body: &str) -> Result<()> {
        // `--match-head-commit` closes the last race (nothing may land on
        // the PR after the reviewed SHA); `--subject` pins the validated
        // PR title plus its ` (#N)` back-reference ([`merge_subject`]), so a
        // single-commit PR can never write its branch commit's subject into
        // main (GH-1100) and the squash commit keeps its PR pointer; the body
        // arrives on stdin through `--body-file -`.
        gh_write_stdin(
            self.cwd,
            &[
                "pr",
                "merge",
                &pr.to_string(),
                "--squash",
                "--match-head-commit",
                head,
                "--subject",
                subject,
                "--body-file",
                "-",
            ],
            body,
        )
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
