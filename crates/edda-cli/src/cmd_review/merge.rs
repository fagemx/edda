//! `edda review merge` — the operator's merge entrypoint, in the product
//! (GH-1105).
//!
//! `scripts/merge-reviewed-pr.sh` — now a one-line adapter over this verb —
//! decided whether a PR may merge entirely in shell: the fleet-wide drift
//! gate (GH-993), the trusted-review checks, the union rule (GH-769,
//! GH-742), the malformed-comment refusal (#917), and required checks.
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
use super::github::{gh, gh_write, gh_write_stdin};
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
    /// The REST issue comments, newest-last, each with its `updated_at`.
    fn comments(&self, pr: u64) -> Result<Vec<TimedComment>>;
    /// The fleet-wide drift walk (`edda review drift`'s own query).
    fn drift(&self) -> Result<(Vec<String>, bool)>;
    /// Do the ruleset's required checks pass? (`gh pr checks --required`)
    fn checks_green(&self, pr: u64) -> Result<bool>;
    /// The `CI Gate` check-run URL for the receipt body, if resolvable.
    fn ci_gate_url(&self, sha: &str) -> Option<String>;
    /// Execute the squash merge with subject and body pinned (GH-1100).
    fn squash(&self, pr: u64, head: &str, subject: &str, body: &str) -> Result<()>;
}

pub(crate) struct Pr {
    pub head: String,
    pub state: String,
    pub title: String,
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
         - drift: clean across the open PR set (GH-993)\n\
         - required checks: green\n\
         - CI Gate: {}\n",
        ci.unwrap_or("run id not resolved"),
    )
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
/// order: drift (whole open set, GH-993) before the PR's own facts before
/// the review's own facts before the forge's required checks.
pub(crate) fn merge_inner(args: &MergeArgs, reads: &dyn Reads) -> Result<i32> {
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
    // 1. Fleet-wide drift (GH-993): the whole open set, not just this PR —
    //    the same scope pi-controller-runbook named by policy.
    match reads.drift() {
        Err(error) => {
            eprintln!("verdict-drift could not read PR state ({error:#}) — refusing until it can");
            return Ok(2);
        }
        Ok((lines, not_ready)) if not_ready => {
            for line in &lines {
                eprintln!("{line}");
            }
            eprintln!(
                "verdict-drift is not clean across the open PR set — refusing until every \
                 open PR carries a verdict on its head (output above)"
            );
            return Ok(1);
        }
        Ok(_) => {}
    }
    // 2. The PR's own facts.
    let pr = match reads.pr(args.pr) {
        Ok(pr) => pr,
        Err(error) => {
            eprintln!("cannot read PR head: {error:#}");
            return Ok(2);
        }
    };
    if pr.state != "OPEN" {
        eprintln!("PR is {}", pr.state);
        return Ok(1);
    }
    if !is_head(&pr.head) {
        eprintln!("invalid PR head {}", pr.head);
        return Ok(2);
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

    // 6. The forge's own required checks.
    match reads.checks_green(args.pr) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("required checks are not green");
            return Ok(1);
        }
        Err(error) => {
            eprintln!("cannot read required checks: {error:#}");
            return Ok(2);
        }
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
/// The repository is not a flag here: `github::command` puts `EDDA_REPO` on
/// every `gh` child as `GH_REPO`, which `gh` honors identically to `--repo`
/// (`github.rs:9-24`) — the same one door `comments_argv`'s `{owner}/{repo}`
/// templates already go through.
pub(crate) fn pr_argv(pr: u64) -> Vec<String> {
    vec![
        "pr".to_owned(),
        "view".to_owned(),
        pr.to_string(),
        "--json".to_owned(),
        "headRefOid,state,title".to_owned(),
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
        })
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

    fn drift(&self) -> Result<(Vec<String>, bool)> {
        drift::evaluate(
            self.cwd,
            std::env::var("EDDA_OPEN_PR_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(200),
        )
    }

    fn checks_green(&self, pr: u64) -> Result<bool> {
        // `gh pr checks --required` exits nonzero both when a required check
        // is red and when gh itself failed — the shell refused on either,
        // without distinguishing, and this stays at that parity: nonzero
        // reads as not green rather than as an unreadable answer.
        Ok(gh_write(self.cwd, &["pr", "checks", &pr.to_string(), "--required"]).is_ok())
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
mod tests {
    use super::*;
    use std::cell::RefCell;

    const HEAD: &str = "aaaaaaaabbbbbbbbccccccccdddddddd11112222";
    /// A SHA that is not the head — the side of the window where the
    /// latest-review ordering actually decides the outcome.
    const OLDER: &str = "1111111111111111111111111111111111111111";

    fn args(merge: bool) -> MergeArgs {
        MergeArgs {
            pr: 4242,
            merge,
            check: false,
            body_file: None,
            json: false,
        }
    }

    fn review(round: u64, sha: &str, verdict: &str, updated: &str) -> TimedComment {
        TimedComment {
            comment: Comment {
                id: format!("{round:03}"),
                body: format!(
                    "## Code Review: Round {round} — PR #4242 @ {sha}\n\n- escalations: \
                     none\n\n### Verdict\n\n{verdict}\n"
                ),
                author_association: Some("OWNER".into()),
            },
            updated_at: updated.into(),
        }
    }

    /// The fixture half of test-merge-reviewed-pr.sh: every read pre-seeded,
    /// every call recorded, so a case can prove what was and was not
    /// reached — a stub that shrugs at an unexpected call turns a wiring
    /// regression into a green test.
    struct Fake {
        head: &'static str,
        state: &'static str,
        title: String,
        comments: Vec<TimedComment>,
        drift: Result<(Vec<String>, bool), String>,
        fail_comments: bool,
        checks_green: bool,
        merged: RefCell<Vec<(u64, String, String, String)>>,
        /// Which PR each read was asked for — a `Fake` that shrugs at the
        /// argument is how the missing `--pr` selector stayed invisible to
        /// all 17 tests before it.
        asked: RefCell<Vec<(&'static str, u64)>>,
        reached_comments: RefCell<bool>,
        reached_checks: RefCell<bool>,
    }

    impl Fake {
        fn clean(comments: Vec<TimedComment>) -> Self {
            Self {
                head: HEAD,
                state: "OPEN",
                title: "fix(edda-cli): a merge the fleet already reviewed".into(),
                comments,
                drift: Ok((vec![], false)),
                fail_comments: false,
                checks_green: true,
                merged: RefCell::new(Vec::new()),
                asked: RefCell::new(Vec::new()),
                reached_comments: RefCell::new(false),
                reached_checks: RefCell::new(false),
            }
        }
    }

    impl Reads for Fake {
        fn pr(&self, pr: u64) -> Result<Pr> {
            self.asked.borrow_mut().push(("pr", pr));
            Ok(Pr {
                head: self.head.into(),
                state: self.state.into(),
                title: self.title.clone(),
            })
        }
        fn comments(&self, pr: u64) -> Result<Vec<TimedComment>> {
            self.asked.borrow_mut().push(("comments", pr));
            *self.reached_comments.borrow_mut() = true;
            if self.fail_comments {
                return Err(anyhow::anyhow!("gh: not authenticated"));
            }
            Ok(self.comments.clone())
        }
        fn drift(&self) -> Result<(Vec<String>, bool)> {
            self.drift.clone().map_err(anyhow::Error::msg)
        }
        fn checks_green(&self, pr: u64) -> Result<bool> {
            self.asked.borrow_mut().push(("checks", pr));
            *self.reached_checks.borrow_mut() = true;
            Ok(self.checks_green)
        }
        fn ci_gate_url(&self, _sha: &str) -> Option<String> {
            Some("https://github.com/fagemx/edda/runs/123".into())
        }
        fn squash(&self, pr: u64, head: &str, subject: &str, body: &str) -> Result<()> {
            self.merged
                .borrow_mut()
                .push((pr, head.into(), subject.into(), body.into()));
            Ok(())
        }
    }

    fn lgtm() -> Vec<TimedComment> {
        vec![review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z")]
    }

    // case 2/8: a lone qualifying LGTM is accepted; required checks are
    // still asked, byte-identical accept path.
    #[test]
    fn a_lone_qualifying_lgtm_is_accepted() {
        let fake = Fake::clean(lgtm());
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 0);
        assert!(*fake.reached_checks.borrow(), "checks must be asked");
        assert!(fake.merged.borrow().is_empty(), "no --merge, no squash");
    }

    // case 1: an earlier Changes Requested under a later LGTM is refused by
    // the union (GH-742) — and required checks are never asked after the
    // union already refused.
    #[test]
    fn a_standing_changes_requested_under_a_later_lgtm_is_refused() {
        let fake = Fake::clean(vec![
            review(
                1,
                HEAD,
                "Changes Requested, P0=0, P1=1",
                "2026-09-08T10:00:00Z",
            ),
            review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        ]);
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 1, "a standing Changes Requested was merged over");
        assert!(
            !*fake.reached_checks.borrow(),
            "required checks were queried after the union already refused"
        );
    }

    // case 3: a gate that cannot judge refuses — an unreadable comment list
    // is never an approval.
    #[test]
    fn an_unreadable_comment_list_refuses() {
        let mut fake = Fake::clean(lgtm());
        fake.fail_comments = true;
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 2);
    }

    // case 4: a verdict comment the union could not read refuses — the
    // union reads `success` precisely because the blocking round is
    // invisible to it (#917).
    #[test]
    fn a_malformed_verdict_comment_refuses() {
        let mut timed = review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z");
        timed.comment.body = format!(
            "narration first\n## Code Review: Round 1 — PR #4242 @ {HEAD}\n\n### \
             Verdict\n\nLGTM (P0=0, P1=0)"
        );
        let fake = Fake::clean(vec![timed]);
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 1, "a malformed verdict comment was ignored");
    }

    // case 6: an unrelated open PR with no verdict refuses everything
    // (GH-993) — before the comments read or required checks are reached.
    #[test]
    fn a_dirty_open_set_refuses_before_the_prs_own_checks() {
        let mut fake = Fake::clean(lgtm());
        fake.drift = Ok((
            vec!["#9001 dddddddddddd main no verdict on head".into()],
            true,
        ));
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 1);
        assert!(
            !*fake.reached_comments.borrow(),
            "the comment read was reached after drift already refused"
        );
        assert!(
            !*fake.reached_checks.borrow(),
            "required checks were queried after drift already refused"
        );
    }

    // case 7: the drift walk itself failing to read also refuses — a broken
    // read must not wave every merge through clean.
    #[test]
    fn a_failed_drift_read_refuses() {
        let mut fake = Fake::clean(lgtm());
        fake.drift = Err("pr list failed".into());
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 2);
        assert!(!*fake.reached_comments.borrow());
    }

    // case 8's non-vacuous half: a clean drift state over a populated open
    // set does not block.
    #[test]
    fn a_populated_clean_drift_state_does_not_block() {
        let mut fake = Fake::clean(lgtm());
        fake.drift = Ok((vec!["#9002 eeeeeeeeeeee main LGTM".into()], false));
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 0);
    }

    // ---- the per-review checks the shell carried ---------------------------

    #[test]
    fn the_latest_trusted_review_must_be_pinned_to_the_head() {
        let fake = Fake::clean(vec![review(
            1,
            "1111111111111111111111111111111111111111",
            "LGTM (P0=0, P1=0)",
            "2026-09-08T11:00:00Z",
        )]);
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    #[test]
    fn an_edit_moves_which_review_is_latest() {
        // Round 1 was edited after Round 2 was posted: GitHub's updated_at
        // ordering makes Round 1 the latest trusted review, and its blocker
        // stands even though a later round approved.
        let fake = Fake::clean(vec![
            review(
                1,
                HEAD,
                "Changes Requested, P0=0, P1=1",
                "2026-09-08T12:00:00Z",
            ),
            review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        ]);
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    /// The window `an_edit_moves_which_review_is_latest` does not reach: that
    /// fixture puts the edited blocker on the **head** SHA, where `gate::union`
    /// already refuses on its own, so it holds even with the ordering deleted.
    /// Here the blocker is pinned to an older SHA — invisible to the union —
    /// and only the `(updated_at, id)` ordering makes it the latest review the
    /// head-pinning check then rejects. Take the ordering away (select the
    /// first candidate, say) and the head-pinned LGTM wins and this merges.
    #[test]
    fn an_edited_blocker_on_an_older_sha_still_decides_which_review_is_latest() {
        let fake = Fake::clean(vec![
            // Deliberately first in the list: a selection that ignores
            // `updated_at` and takes what it sees first picks this one.
            review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
            review(
                1,
                OLDER,
                "Changes Requested, P0=0, P1=1",
                "2026-09-08T12:00:00Z",
            ),
        ]);
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(
            code, 1,
            "the most recently edited trusted review was not the one judged"
        );
        assert!(
            !*fake.reached_checks.borrow(),
            "required checks were queried after the review check already refused"
        );
    }

    /// GH-1105 review round 1: the shell refused outright when a trusted §7
    /// review lacked the REST keys the selection orders by — the port defaulted
    /// them, which sorts the unreadable comment to the bottom of the key and
    /// lets an older LGTM be chosen instead of stopping the gate.
    ///
    /// The fixture sits inside the window where that difference shows: the
    /// keyless comment is a blocker pinned to an **older** SHA, so it is
    /// outside the union and outside the head-pinning check. Restore the two
    /// `unwrap_or` defaults and the head-pinned LGTM is selected, every
    /// remaining precondition passes and this merges at exit 0.
    #[test]
    fn a_trusted_review_lacking_its_rest_ordering_keys_refuses() {
        for (label, id, updated_at) in [
            ("no updated_at", "700", ""),
            ("no numeric id", "", "2026-09-08T13:00:00Z"),
            ("node id, not a REST id", "IC_kwDOA", "2026-09-08T13:00:00Z"),
        ] {
            let mut broken = review(7, OLDER, "Changes Requested, P0=1, P1=0", updated_at);
            broken.comment.id = id.into();
            let fake = Fake::clean(vec![lgtm().remove(0), broken]);
            let code = merge_inner(&args(false), &fake).unwrap();
            assert_eq!(
                code, 1,
                "{label}: a trusted review the gate cannot order was sorted to the bottom \
                 instead of refusing"
            );
            assert!(
                !*fake.reached_checks.borrow(),
                "{label}: required checks were queried after the selection already refused"
            );
        }
    }

    /// GH-1105 review round 1: candidacy narrowed from the shell's heading
    /// prefix to the full §7 grammar, so a blocking round whose heading SHA is
    /// truncated, typo'd or uppercase stopped being a candidate at all — and
    /// `deliver::extract` did not report it malformed either, because its #917
    /// branch also asked the full grammar. Invisible to the latest-review
    /// check, to `malformed` and to the union at once is the exact hole #917's
    /// refusal exists to close. Widen the filter back to `heading_shaped` and
    /// the round blocks: either as the latest review whose heading will not
    /// parse, or through the malformed refusal.
    #[test]
    fn a_blocking_round_whose_heading_sha_is_malformed_still_stops_the_gate() {
        for bad in [
            "aaaaaaaabbbbbbbbccccccccdddddddd1111222", // truncated by one
            "AAAAAAAABBBBBBBBCCCCCCCCDDDDDDDD11112222", // uppercase
            "aaaaaaaabbbbbbbbccccccccdddddddd1111222g", // a typo'd digit
        ] {
            let fake = Fake::clean(vec![
                review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
                review(
                    2,
                    bad,
                    "Changes Requested, P0=1, P1=0",
                    "2026-09-08T12:00:00Z",
                ),
            ]);
            let code = merge_inner(&args(false), &fake).unwrap();
            assert_eq!(
                code, 1,
                "a blocking round headed @ {bad} was invisible to the whole gate"
            );
            assert!(
                fake.merged.borrow().is_empty(),
                "merged over a blocking round headed @ {bad}"
            );
        }
    }

    /// The same widening isolated at its own seam, because the end-to-end
    /// exit code above cannot separate the two halves of the fix: the
    /// malformed refusal at stage 5 would reach the same `1`. Here only the
    /// candidate filter decides — narrow it back to the full grammar and the
    /// broken round is skipped, the older LGTM is selected, and this returns
    /// `Ok("1")`.
    #[test]
    fn a_malformed_heading_is_the_latest_review_not_a_skipped_one() {
        let timed = vec![
            review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
            review(
                2,
                &HEAD.to_uppercase(),
                "Changes Requested, P0=1, P1=0",
                "2026-09-08T12:00:00Z",
            ),
        ];
        let code = latest_review_round(&timed, 4242, HEAD)
            .expect_err("the broken round was skipped and the older LGTM approved the merge");
        assert_eq!(code, 1);
    }

    #[test]
    fn a_provisional_verdict_never_merges() {
        let fake = Fake::clean(vec![review(
            1,
            HEAD,
            "Provisional — LGTM (P0=0, P1=0), escalation pending",
            "2026-09-08T11:00:00Z",
        )]);
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    #[test]
    fn unresolved_escalations_refuse() {
        let mut timed = lgtm().into_iter().next().unwrap();
        timed.comment.body = timed
            .comment
            .body
            .replace("- escalations: none", "- escalations: P2-1 pending");
        let fake = Fake::clean(vec![timed]);
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    #[test]
    fn a_closed_pr_refuses() {
        let mut fake = Fake::clean(lgtm());
        fake.state = "MERGED";
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    #[test]
    fn required_checks_not_green_refuses() {
        let mut fake = Fake::clean(lgtm());
        fake.checks_green = false;
        assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
    }

    // ---- GH-1100, folded in -------------------------------------------------

    #[test]
    fn the_squash_subject_is_always_the_pr_title_never_a_branch_commit() {
        // The single-commit-PR case #1100 filed: GitHub picks the branch
        // commit's own subject when the PR has exactly one commit. The verb
        // pins --subject itself, so it cannot.
        let mut fake = Fake::clean(lgtm());
        fake.title = "fix(edda-cli): the PR title, not the branch commit".into();
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
        let (pr, head, subject, _body) = fake.merged.borrow()[0].clone();
        assert_eq!(pr, 4242);
        assert_eq!(head, HEAD);
        assert_eq!(
            subject,
            "fix(edda-cli): the PR title, not the branch commit (#4242)"
        );
    }

    // ---- GH-1100 Round 2's other half: the (#N) back-reference -------------

    /// The regression this PR reintroduced against #1118 (GH-1105 Round 2's
    /// P0). GitHub appends ` (#N)` only to a squash subject it picks itself;
    /// handing it one through `--subject` suppresses the suffix entirely, so
    /// pinning the title and stopping there lands every squash commit on
    /// `main` without a PR pointer — permanently, since R7 forbids rewriting
    /// `main`.
    ///
    /// The second assertion names that shape explicitly rather than leaving
    /// it implied by the equality: drop the append in [`merge_subject`] and
    /// the bare title is exactly what `gh` receives.
    #[test]
    fn the_squash_subject_keeps_its_pr_back_reference() {
        let mut fake = Fake::clean(lgtm());
        fake.title = "fix(edda-cli): a merge the fleet already reviewed".into();
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
        let subject = fake.merged.borrow()[0].2.clone();
        assert_eq!(
            subject,
            "fix(edda-cli): a merge the fleet already reviewed (#4242)"
        );
        assert_ne!(
            subject, "fix(edda-cli): a merge the fleet already reviewed",
            "the squash subject carries no (#4242) back-reference — GitHub adds one only to a \
             subject it picks itself, so this commit would land on main with no PR pointer and \
             R7 forbids ever fixing it"
        );
    }

    /// The idempotence guard. A title copied back from a landed commit already
    /// ends in this PR's own number; appending again would write
    /// `… (#4242) (#4242)`.
    #[test]
    fn a_title_already_carrying_this_prs_number_is_not_doubled() {
        let mut fake = Fake::clean(lgtm());
        fake.title = "fix(edda-cli): a title that already carries its number (#4242)".into();
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
        let subject = fake.merged.borrow()[0].2.clone();
        assert_eq!(
            subject,
            "fix(edda-cli): a title that already carries its number (#4242)"
        );
        assert!(
            !subject.contains("(#4242) (#4242)"),
            "the back-reference was appended to a title that already carried it: {subject}"
        );
    }

    /// The interesting half of that skip rule: `… (#999)` is prose — a
    /// follow-up naming the PR it answers — not this commit's pointer.
    /// Treating it as one would send `git log` readers to an unrelated PR.
    #[test]
    fn a_foreign_pr_number_in_the_title_still_gets_this_prs_back_reference() {
        let mut fake = Fake::clean(lgtm());
        fake.title = "fix(edda-cli): follow-up to the earlier change (#999)".into();
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
        assert_eq!(
            fake.merged.borrow()[0].2,
            "fix(edda-cli): follow-up to the earlier change (#999) (#4242)"
        );
    }

    /// The subject assembly at its own seam, including the near-misses the
    /// end-to-end cases above do not reach: the skip is the exact ` (#N)`
    /// tail, so a number without the separating space, a number embedded
    /// mid-title, and a different PR's number all still get the real
    /// back-reference appended.
    #[test]
    fn merge_subject_matrix() {
        assert_eq!(merge_subject("fix(x): y", 4242), "fix(x): y (#4242)");
        assert_eq!(
            merge_subject("fix(x): y (#4242)", 4242),
            "fix(x): y (#4242)"
        );
        assert_eq!(
            merge_subject("fix(x): y(#4242)", 4242),
            "fix(x): y(#4242) (#4242)"
        );
        assert_eq!(
            merge_subject("fix(x): y (#4242) and more", 4242),
            "fix(x): y (#4242) and more (#4242)"
        );
        assert_eq!(
            merge_subject("fix(x): y (#42420)", 4242),
            "fix(x): y (#42420) (#4242)"
        );
        assert_eq!(
            merge_subject("fix(x): y (#999)", 4242),
            "fix(x): y (#999) (#4242)"
        );
    }

    /// U4 judges the bare title and the back-reference is composed after it,
    /// so a suffix can neither rescue a bad title nor break a good one — the
    /// two are separate values, and the composed one only ever reaches `gh`.
    #[test]
    fn u4_judges_the_bare_title_the_back_reference_is_composed_after() {
        let good = "fix(edda-cli): repair the gate";
        assert!(subject_problem(good).is_none());
        assert!(subject_problem(&merge_subject(good, 4242)).is_none());
        let bad = "wip(review): lane work in progress";
        assert!(subject_problem(bad).is_some());
        assert!(
            subject_problem(&merge_subject(bad, 4242)).is_some(),
            "a back-reference must not rescue a title U4 rejects"
        );
        // …and the refusal happens before anything is composed or merged.
        let mut fake = Fake::clean(lgtm());
        fake.title = bad.into();
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 1);
        assert!(fake.merged.borrow().is_empty());
    }

    // ---- GH-1100's two --body-file refusals ---------------------------------

    /// An empty operand short-circuited the shell's `-f` test and read as "no
    /// body file", so the one malformed operand that did not refuse was the
    /// emptiest.
    #[test]
    fn an_empty_body_file_operand_refuses() {
        let fake = Fake::clean(lgtm());
        let mut merge = args(true);
        merge.body_file = Some(String::new());
        let error = merge_inner(&merge, &fake)
            .expect_err("an empty --body-file operand was accepted as 'no body file'");
        assert!(
            error.to_string().contains("--body-file"),
            "the refusal must name --body-file: {error:#}"
        );
        assert!(fake.merged.borrow().is_empty());
    }

    /// Only the merge path opens the file. Accepting it under `--check` told
    /// the caller their body had been taken when nothing would ever read it.
    #[test]
    fn a_body_file_outside_merge_refuses_instead_of_being_ignored() {
        let fake = Fake::clean(lgtm());
        let mut check = args(false);
        check.body_file = Some("some-merge-body.md".into());
        let error = merge_inner(&check, &fake)
            .expect_err("--body-file was accepted and silently ignored outside --merge");
        assert!(
            error.to_string().contains("--merge"),
            "the refusal must say --body-file needs --merge: {error:#}"
        );
        assert!(
            !*fake.reached_comments.borrow(),
            "a rejected invocation still spent the PR reads"
        );
    }

    /// A typo'd path costs an error message, not the fleet-wide drift walk.
    #[test]
    fn a_body_file_that_does_not_exist_refuses_before_any_read() {
        let fake = Fake::clean(lgtm());
        let mut merge = args(true);
        merge.body_file = Some("no/such/merge-body.md".into());
        let error = merge_inner(&merge, &fake).expect_err("a missing body file was accepted");
        assert!(
            error.to_string().contains("body file not found"),
            "the refusal must name the missing file: {error:#}"
        );
        assert!(
            !*fake.reached_comments.borrow(),
            "the PR reads ran before the body file was known to exist"
        );
    }

    /// The supplied body rides through untouched — and does not cost the
    /// commit its back-reference, which is composed on the merge call rather
    /// than inside the compose-a-receipt branch.
    #[test]
    fn a_supplied_body_file_rides_through_and_keeps_the_back_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("merge-body.md");
        std::fs::write(&path, "choreographed merge body from the operator\n").expect("write body");
        let fake = Fake::clean(lgtm());
        let mut merge = args(true);
        merge.body_file = Some(path.to_string_lossy().into_owned());
        assert_eq!(merge_inner(&merge, &fake).unwrap(), 0);
        let (_, _, subject, body) = fake.merged.borrow()[0].clone();
        assert_eq!(body, "choreographed merge body from the operator\n");
        assert!(
            !body.contains("Squash merge of PR"),
            "a supplied body file still composed a receipt: {body}"
        );
        assert_eq!(
            subject, "fix(edda-cli): a merge the fleet already reviewed (#4242)",
            "a supplied body file dropped the (#4242) back-reference"
        );
    }

    #[test]
    fn a_non_conforming_title_refuses_with_the_offending_string() {
        for bad in [
            "wip(review): GH-1003 + GH-992 in progress — uncommitted lane work",
            "fix(): empty scope",
            "improve: things",
            "fix(edda-cli):",
            "no type or scope at all",
        ] {
            let mut fake = Fake::clean(lgtm());
            fake.title = bad.into();
            let code = merge_inner(&args(false), &fake).unwrap();
            assert_eq!(code, 1, "a non-conforming subject merged: {bad}");
            assert!(fake.merged.borrow().is_empty());
        }
    }

    #[test]
    fn the_composed_receipt_names_the_sha_round_and_ci_run() {
        let fake = Fake::clean(lgtm());
        merge_inner(&args(true), &fake).unwrap();
        let body = &fake.merged.borrow()[0].3;
        assert!(body.contains(HEAD), "receipt must name the reviewed SHA");
        assert!(body.contains("Round 1"), "receipt must name the LGTM round");
        assert!(
            body.contains("https://github.com/fagemx/edda/runs/123"),
            "receipt must name the CI run"
        );
    }

    // ---- the PR selector every read must carry --------------------------

    /// The argv itself, asserted the way `comments_argv` and `open_prs_argv`
    /// are (GH-1079). Round 1's P0: `gh pr view --json headRefOid,state,title`
    /// with no selector resolves the pull request of the current branch, so
    /// the head, the OPEN state and the squash subject came from whichever PR
    /// the cwd happened to be on.
    #[test]
    fn pr_argv_names_the_pull_request_it_reads() {
        assert_eq!(
            pr_argv(4242),
            vec!["pr", "view", "4242", "--json", "headRefOid,state,title"]
        );
        // Stated separately from the equality above so the regression this
        // guards is named where it fails: a selector-less argv is the defect,
        // not merely a different argv.
        assert!(
            pr_argv(4242).contains(&"4242".to_owned()),
            "gh would resolve the current branch's PR, not #4242"
        );
    }

    /// …and that argv is built from the `--pr` the operator gave, not from a
    /// constant: every read in the enumerated order is asked for the same PR.
    #[test]
    fn every_read_is_asked_for_the_requested_pr() {
        let fake = Fake::clean(lgtm());
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
        assert_eq!(
            *fake.asked.borrow(),
            vec![("pr", 4242), ("comments", 4242), ("checks", 4242)],
            "a precondition was evaluated against a PR other than --pr"
        );
        assert_eq!(fake.merged.borrow()[0].0, 4242, "squashed the wrong PR");
    }

    #[test]
    fn subject_validation_matrix() {
        assert!(subject_problem("fix(edda-cli): repair the gate").is_none());
        assert!(subject_problem("chore(release): prepare v0.6.1").is_none());
        assert!(subject_problem("docs(fleet.rules): clarify").is_none());
        for bad in [
            "wip(x): y",
            "fix(x): ",
            "fix: no scope parens",
            "fix(): empty scope",
            "nope(x): unknown type",
        ] {
            assert!(
                subject_problem(bad).is_some(),
                "accepted a non-conforming subject: {bad}"
            );
        }
    }
}
