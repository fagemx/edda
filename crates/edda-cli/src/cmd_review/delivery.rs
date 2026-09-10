//! The write half of `edda review deliver` (GH-1030): given the §7 verdict
//! facts the read half ([`super::deliver::extract`]) already reduced
//! GitHub's comments to, perform exactly the writes the ratified semantics
//! call for — the malformed notice, the `review:*` label, and the
//! `Independent Review` commit status — and nothing else.
//!
//! ## What this module does not do
//!
//! It does not post the primary §7 verdict comment. In the architecture the
//! parsing half (GH-1030 part 1, PR #1077) landed, that comment is read as
//! already present on GitHub — posted by whatever ran the review, which since
//! GH-1061 is the reviewing session itself with `gh pr comment`. The issue's
//! own list of what the follow-up adapter PR converts into calls on this verb
//! — `post_review_status`, `settle_pending`, `verdict_body_lines` — never
//! named the retired shell's `gh pr comment --body-file` verdict post, so that
//! call stayed where it was; this module owns only the writes derived from
//! a verdict once one exists to read. See the PR body for the fuller
//! discussion of this divergence from the issue's original "What".
//!
//! ## SHADOW and non-authoritative engines (R22)
//!
//! REVIEW.md §8 and rules.md R22 are explicit that the ` (SHADOW)` heading
//! suffix is the *only* SHADOW marker a reviewer emits — including for an
//! engine that R22 does not make authoritative on the PR's touched surface;
//! R22 itself routes that case through the same suffix, not a second
//! channel ("SHADOW 輪次標題尾加 (SHADOW)"). This module honors exactly that
//! marker, via [`super::deliver::Extracted::shadow`], and performs zero
//! writes when it is the only signal standing on the SHA. It does not
//! additionally re-derive engine/surface authority from the PR's changed
//! files the way the retired review shell's `verdict_engine` / `classify_surface`
//! / `verdict_surface_ok` do as defense-in-depth against a reviewer that
//! fails to self-declare — that independent server-side cross-check is a
//! separate, larger feature (a new `gh` read of the PR's changed files, plus
//! the R22 path table) and is not implemented here; see the PR body.
//!
//! ## Status is unconditional on the current head
//!
//! Only the `review:*` label is gated on `current head == reviewed sha`
//! (doneWhen's own moved-head bullet names only the label). The issue's
//! "Independent Review commit status on the reviewed SHA by the union rule"
//! carries no head-equality clause either. the retired review shell's
//! `finish_verdict` happens to skip both together, but that is a side effect
//! of its early-return control flow, not a stated rule — so this module
//! writes the status regardless of whether the PR has since moved past the
//! reviewed SHA.

use super::deliver::{Comment, Extracted};
use super::gate::Union;
use super::github;
use anyhow::Result;
use std::path::Path;

pub(crate) const LABEL_LGTM: &str = "review:lgtm";
pub(crate) const LABEL_CHANGES: &str = "review:changes-requested";
const STATUS_CONTEXT: &str = "Independent Review";

/// What `edda review deliver` needs GitHub to do, factored out so union,
/// idempotency and failure-contract fixtures never touch the network
/// (GH-1030 doneWhen: "injectable gh, no network").
pub(crate) trait Gh {
    /// The PR's current head SHA.
    fn head(&self, pr: u64) -> Result<String>;
    /// Labels currently on the PR.
    fn labels(&self, pr: u64) -> Result<Vec<String>>;
    /// Add one label. Deliver only calls this after checking `labels`
    /// itself, so a fake's call log is the truth for "zero new writes";
    /// this method is not required to be idempotent on its own.
    fn add_label(&self, pr: u64, label: &str) -> Result<()>;
    /// Remove one label. Deliver only calls this as a best-effort sibling
    /// cleanup right after a successful `add_label` — mirroring
    /// the retired review shell's `gh pr edit --remove-label ... || true` — so a
    /// failure here must never turn a delivered label into a failed write.
    fn remove_label(&self, pr: u64, label: &str) -> Result<()>;
    /// The latest `Independent Review` status state on `sha`, if that
    /// context has ever been posted there (`None` otherwise).
    fn latest_status(&self, sha: &str) -> Result<Option<String>>;
    /// Post the `Independent Review` commit status.
    fn post_status(&self, sha: &str, state: &str, description: &str) -> Result<()>;
    /// Post a plain-text PR comment (used only for the malformed notice).
    fn post_comment(&self, pr: u64, body: &str) -> Result<()>;
}

/// The outcome of one attempted write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Write {
    /// No call was made: the desired state already held, or a rule (moved
    /// head) says this write must not happen.
    Skipped(&'static str),
    /// The call was made and succeeded.
    Done,
    /// The call was made and failed; the message is the `gh` failure.
    Failed(String),
    /// No call was made this round, but — unlike `Skipped` — the write WAS
    /// due: R23 (#917) withholds status/label for any run that posts a new
    /// malformed-comment notice, mirroring the retired review shell's
    /// `post_review_status` returning 3 ("status withheld this poll") to
    /// tell its caller to come back. A withheld write must read as
    /// outstanding, never as delivered — see [`Delivery::exit_code`].
    Withheld(&'static str),
}

impl Write {
    pub(crate) fn tag(&self) -> &'static str {
        match self {
            Write::Skipped(_) => "skipped",
            Write::Done => "done",
            Write::Failed(_) => "failed",
            Write::Withheld(_) => "withheld",
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Write::Skipped(r) => Some(r),
            Write::Failed(e) => Some(e.as_str()),
            Write::Withheld(r) => Some(r),
            Write::Done => None,
        }
    }
}

/// Every write `edda review deliver` attempted or deliberately withheld for
/// one `(pr, sha)` pair.
#[derive(Debug, Default)]
pub(crate) struct Delivery {
    /// One entry per malformed comment id, in `Extracted::malformed` order.
    pub notices: Vec<(String, Write)>,
    /// `None` only when nothing was due at all: the only standing signal on
    /// the SHA is SHADOW, or a malformed notice failed to post (already
    /// non-zero via the notice's own `Write::Failed`). A status that WAS due
    /// but got deferred by a new, successfully posted malformed notice
    /// (R23/#917) is `Some(Write::Withheld(_))`, never `None` — see
    /// `Write::Withheld` and `exit_code`.
    pub status: Option<Write>,
    /// `(label, outcome)`; `None` when the union is `Union::None` (no label
    /// is due either way).
    pub label: Option<(String, Write)>,
    /// `(sibling_label, outcome)` for the best-effort cleanup that follows a
    /// due `label` (GH-1081 Round 2 P1): removing the *other* `review:*`
    /// label so the two never stand together on one SHA (ratified
    /// `review.same-sha-multiple-verdicts`). `None` when no sibling stood,
    /// or when `label` itself was never due/attempted this run. Unlike the
    /// Round 1 shape this replaces, a failed removal is recorded here as
    /// `Write::Failed` — never swallowed via `let _ =` — so it reaches
    /// stdout/JSON and `exit_code()` the same as any other write.
    pub label_removed: Option<(String, Write)>,
}

impl Delivery {
    /// 0 delivered, 1 partially delivered, 2 failed, 3 withheld — the
    /// exit-code contract doneWhen requires: nothing here is silently
    /// dropped, every attempted write's outcome feeds this. A run whose
    /// intended action (a notice, or a status+label) fully succeeds, or
    /// whose intended action was correctly "nothing" (SHADOW-only, an
    /// idempotent rerun), is `delivered`. A run that withholds a due
    /// status/label under R23 (#917) — a new malformed notice standing
    /// alongside a real verdict — is never `delivered` even when the
    /// notice itself posted cleanly: `Write::Withheld` must outrank 0, the
    /// same way the shell's `post_review_status` returns 3 rather than 0
    /// so the caller comes back. `Withheld` outcomes never count toward
    /// `attempted`/`failed` — they were not attempted, deliberately.
    pub(crate) fn exit_code(&self) -> i32 {
        let outcomes: Vec<&Write> = self
            .notices
            .iter()
            .map(|(_, w)| w)
            .chain(self.status.iter())
            .chain(self.label.iter().map(|(_, w)| w))
            .chain(self.label_removed.iter().map(|(_, w)| w))
            .collect();
        let attempted = outcomes
            .iter()
            .filter(|w| !matches!(w, Write::Skipped(_) | Write::Withheld(_)))
            .count();
        let failed = outcomes
            .iter()
            .filter(|w| matches!(w, Write::Failed(_)))
            .count();
        if failed > 0 {
            if attempted > 0 && failed == attempted {
                2
            } else {
                1
            }
        } else if outcomes.iter().any(|w| matches!(w, Write::Withheld(_))) {
            3
        } else {
            0
        }
    }
}

/// The malformed-notice text, exact and stable: the one-shot property is
/// carried by the string itself, read back from GitHub — no local marker
/// file, no ledger row — so a notice any earlier run posted, including one
/// from the review shell retired in GH-1061, is still recognized as posted.
fn notice_text(id: &str) -> String {
    format!("review: malformed verdict comment {id}")
}

/// Perform (or correctly withhold) every write for one `(pr, sha)` round.
///
/// `union` is the caller's own `gate::union(gate::from_lines(...))` over
/// `extracted.lines` — computed once by the caller and threaded through
/// rather than recomputed here, so this module never re-decides what a
/// verdict means (GH-769 stays the union rule's only owner).
pub(crate) fn deliver(
    gh: &dyn Gh,
    pr: u64,
    sha: &str,
    comments: &[Comment],
    extracted: &Extracted,
    union: Union,
) -> Delivery {
    let mut out = Delivery::default();

    let mut new_notice = false;
    for id in &extracted.malformed {
        let text = notice_text(id);
        if comments.iter().any(|c| c.body.trim() == text) {
            out.notices
                .push((id.clone(), Write::Skipped("already noticed")));
            continue;
        }
        // Attempting a not-yet-noticed id withholds status/label for this
        // run regardless of whether the post itself succeeds — mirroring
        // the retired review shell, whose `new_notice=1` sat outside the
        // if/else on the `gh pr comment` call. A failed post must not be
        // masked by a status write that proceeds as if nothing happened.
        new_notice = true;
        match gh.post_comment(pr, &text) {
            Ok(()) => out.notices.push((id.clone(), Write::Done)),
            Err(error) => out
                .notices
                .push((id.clone(), Write::Failed(error.to_string()))),
        }
    }

    // The only standing signal on this SHA is a self-declared SHADOW round
    // (or a malformed comment already noticed on an earlier run): R22 /
    // REVIEW.md §8 — zero further writes, whether or not a notice was just
    // posted.
    let shadow_only = extracted.lines.is_empty() && !extracted.shadow.is_empty();

    if new_notice {
        // R23 (#917): a run that posts a new malformed-comment notice
        // withholds status/label for THIS run regardless of `extracted.lines`
        // — mirroring the retired review shell's `post_review_status`, whose
        // `new_notice=1` sits outside the if/else on the `gh pr comment`
        // call and whose caller returns 3 ("status withheld this poll") so
        // the next poll retries. A failed notice post already withholds
        // (this branch fires either way) and already reports non-zero via
        // the notice's own `Write::Failed`, so status/label there stay
        // `None` (nothing to add) rather than `Withheld`; a successfully
        // posted notice must still not read as fully delivered when a real
        // status (and, per the union, a label) was due — see
        // `Delivery::exit_code`.
        let notice_failed = out
            .notices
            .iter()
            .any(|(_, w)| matches!(w, Write::Failed(_)));
        if !notice_failed && !shadow_only {
            const REASON: &str = "withheld: new malformed notice posted this run (R23/#917)";
            out.status = Some(Write::Withheld(REASON));
            if let Some(label) = label_for(union) {
                // GH-1081 Round 2 P2: the label is only actually due when
                // the PR's current head still matches the reviewed SHA —
                // exactly the gate the main branch below applies
                // (`Skipped("head moved")`). Reporting `Withheld` here
                // without that check told a moved head a label was held
                // back that was never due this run. A head-read failure
                // stays conservative (`Withheld`, as before this fix) since
                // it cannot positively confirm the head has moved.
                let outcome = match gh.head(pr) {
                    Ok(current) if current == sha => Write::Withheld(REASON),
                    Ok(_moved) => Write::Skipped("head moved"),
                    Err(_) => Write::Withheld(REASON),
                };
                out.label = Some((label.to_owned(), outcome));
            }
        }
        return out;
    }
    if shadow_only {
        return out;
    }

    let state = match union {
        Union::Pass => "success",
        Union::Fail => "failure",
        Union::None => "error",
    };
    let description = match union {
        Union::Pass => format!("LGTM · {} verdict(s) on this SHA", extracted.lines.len()),
        Union::Fail => format!(
            "Changes Requested · {} verdict(s) on this SHA",
            extracted.lines.len()
        ),
        Union::None => "no verdict on this SHA".to_owned(),
    };
    out.status = Some(match gh.latest_status(sha) {
        Ok(Some(current)) if current == state => Write::Skipped("already applied"),
        Ok(_) => match gh.post_status(sha, state, &description) {
            Ok(()) => Write::Done,
            Err(error) => Write::Failed(error.to_string()),
        },
        Err(error) => Write::Failed(format!("read latest status: {error}")),
    });

    if let Some(label) = label_for(union) {
        // The sibling review:* label (GH-1081/P1-1, tightened in Round 2
        // P1): the shell this verb replaces removes it right after a
        // successful add (its own sibling cleanup) so a later verdict
        // on the same SHA never leaves both labels standing. The sibling
        // condition is now evaluated independently of whether `label` was
        // freshly added or already applied — Round 1's fix only reached the
        // removal from the fresh-add arm, so a single transient
        // `remove_label` failure (or any split state left by an older
        // writer) could never self-heal: every later rerun took the
        // `Skipped("already applied")` arm and never looked at the sibling
        // again. A clean state (desired label present, sibling absent)
        // still makes zero new calls either way.
        let sibling = if label == LABEL_LGTM {
            LABEL_CHANGES
        } else {
            LABEL_LGTM
        };
        let outcome = match gh.head(pr) {
            Ok(current) if current == sha => match gh.labels(pr) {
                Ok(existing) => {
                    let sibling_stands = existing.iter().any(|l| l == sibling);
                    let label_outcome = if existing.iter().any(|l| l == label) {
                        Write::Skipped("already applied")
                    } else {
                        match gh.add_label(pr, label) {
                            Ok(()) => Write::Done,
                            Err(error) => Write::Failed(error.to_string()),
                        }
                    };
                    // Only attempt the removal when the desired label
                    // itself did not just fail — a failed add changed
                    // nothing about the PR's label state, so there is
                    // nothing new to clean up on top of an already-failed
                    // round.
                    if sibling_stands && !matches!(label_outcome, Write::Failed(_)) {
                        // GH-1081 Round 2 P1: no longer `let _ =` — a
                        // failed removal is recorded as `Write::Failed` so
                        // it surfaces on stdout/JSON and contributes to
                        // `exit_code()`, instead of a delivered label
                        // silently leaving both `review:*` labels standing.
                        out.label_removed = Some((
                            sibling.to_owned(),
                            match gh.remove_label(pr, sibling) {
                                Ok(()) => Write::Done,
                                Err(error) => Write::Failed(error.to_string()),
                            },
                        ));
                    }
                    label_outcome
                }
                Err(error) => Write::Failed(format!("read labels: {error}")),
            },
            Ok(_moved) => Write::Skipped("head moved"),
            Err(error) => Write::Failed(format!("read current head: {error}")),
        };
        out.label = Some((label.to_owned(), outcome));
    }

    out
}

/// The `review:*` label the union rule calls for, if any.
fn label_for(union: Union) -> Option<&'static str> {
    match union {
        Union::Pass => Some(LABEL_LGTM),
        Union::Fail => Some(LABEL_CHANGES),
        Union::None => None,
    }
}

/// The real `gh`-backed [`Gh`]. `head`/`labels`/`latest_status` are reads;
/// `add_label`/`post_status`/`post_comment` are writes via
/// [`github::gh_write`] — `gh`'s own stdout for these is a bare URL or
/// nothing, never JSON, so they cannot go through [`github::gh`].
pub(crate) struct GhCli<'a> {
    pub repo: &'a Path,
}

impl Gh for GhCli<'_> {
    fn head(&self, pr: u64) -> Result<String> {
        github::pr_head(self.repo, pr)
    }

    fn labels(&self, pr: u64) -> Result<Vec<String>> {
        let value = github::gh(
            self.repo,
            &["pr", "view", &pr.to_string(), "--json", "labels"],
        )?;
        Ok(value["labels"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|label| label["name"].as_str().map(str::to_owned))
            .collect())
    }

    fn add_label(&self, pr: u64, label: &str) -> Result<()> {
        github::gh_write(
            self.repo,
            &["pr", "edit", &pr.to_string(), "--add-label", label],
        )
    }

    fn remove_label(&self, pr: u64, label: &str) -> Result<()> {
        github::gh_write(
            self.repo,
            &["pr", "edit", &pr.to_string(), "--remove-label", label],
        )
    }

    fn latest_status(&self, sha: &str) -> Result<Option<String>> {
        let value = github::gh(
            self.repo,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/commits/{sha}/status"),
            ],
        )?;
        Ok(value["statuses"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .find(|status| status["context"].as_str() == Some(STATUS_CONTEXT))
            .and_then(|status| status["state"].as_str())
            .map(str::to_owned))
    }

    fn post_status(&self, sha: &str, state: &str, description: &str) -> Result<()> {
        github::gh_write(
            self.repo,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/statuses/{sha}"),
                "-f",
                &format!("state={state}"),
                "-f",
                &format!("context={STATUS_CONTEXT}"),
                "-f",
                &format!("description={description}"),
            ],
        )
    }

    fn post_comment(&self, pr: u64, body: &str) -> Result<()> {
        github::gh_write(
            self.repo,
            &["pr", "comment", &pr.to_string(), "--body", body],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// An injectable, network-free [`Gh`]: every read is pre-seeded, every
    /// write is recorded (so a test can assert "zero new writes") and can be
    /// made to fail (the gh-failure-contract fixture).
    ///
    /// No `#[derive(Default)]`: `Result<String, String>` is not `Default`,
    /// so [`FakeGh::new`] lists every field explicitly instead.
    struct FakeGh {
        head: RefCell<Result<String, String>>,
        labels: RefCell<Vec<String>>,
        latest_status: RefCell<Result<Option<String>, String>>,
        fail_add_label: bool,
        fail_post_status: bool,
        fail_post_comment: bool,
        fail_remove_label: bool,

        label_calls: RefCell<Vec<(u64, String)>>,
        status_calls: RefCell<Vec<(String, String, String)>>,
        comment_calls: RefCell<Vec<(u64, String)>>,
        remove_label_calls: RefCell<Vec<(u64, String)>>,
    }

    impl FakeGh {
        fn new(head: &str) -> Self {
            Self {
                head: RefCell::new(Ok(head.to_owned())),
                labels: RefCell::new(Vec::new()),
                latest_status: RefCell::new(Ok(None)),
                fail_add_label: false,
                fail_post_status: false,
                fail_post_comment: false,
                fail_remove_label: false,
                label_calls: RefCell::new(Vec::new()),
                status_calls: RefCell::new(Vec::new()),
                comment_calls: RefCell::new(Vec::new()),
                remove_label_calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Gh for FakeGh {
        fn head(&self, _pr: u64) -> Result<String> {
            self.head.borrow().clone().map_err(anyhow::Error::msg)
        }
        fn labels(&self, _pr: u64) -> Result<Vec<String>> {
            Ok(self.labels.borrow().clone())
        }
        fn add_label(&self, pr: u64, label: &str) -> Result<()> {
            self.label_calls.borrow_mut().push((pr, label.to_owned()));
            anyhow::ensure!(!self.fail_add_label, "simulated add_label failure");
            self.labels.borrow_mut().push(label.to_owned());
            Ok(())
        }
        fn remove_label(&self, pr: u64, label: &str) -> Result<()> {
            self.remove_label_calls
                .borrow_mut()
                .push((pr, label.to_owned()));
            anyhow::ensure!(!self.fail_remove_label, "simulated remove_label failure");
            self.labels.borrow_mut().retain(|l| l != label);
            Ok(())
        }
        fn latest_status(&self, _sha: &str) -> Result<Option<String>> {
            self.latest_status
                .borrow()
                .clone()
                .map_err(anyhow::Error::msg)
        }
        fn post_status(&self, sha: &str, state: &str, description: &str) -> Result<()> {
            self.status_calls.borrow_mut().push((
                sha.to_owned(),
                state.to_owned(),
                description.to_owned(),
            ));
            anyhow::ensure!(!self.fail_post_status, "simulated post_status failure");
            *self.latest_status.borrow_mut() = Ok(Some(state.to_owned()));
            Ok(())
        }
        fn post_comment(&self, pr: u64, body: &str) -> Result<()> {
            self.comment_calls.borrow_mut().push((pr, body.to_owned()));
            anyhow::ensure!(!self.fail_post_comment, "simulated post_comment failure");
            Ok(())
        }
    }

    fn extracted(lines: &[&str]) -> Extracted {
        Extracted {
            lines: lines.iter().map(|s| (*s).to_owned()).collect(),
            malformed: vec![],
            shadow: vec![],
            untrusted: vec![],
        }
    }

    fn union_of(extracted: &Extracted) -> Union {
        super::super::gate::union(&super::super::gate::from_lines(&extracted.lines.join("\n")))
    }

    // ---- union-rule matrix drives the posted status state -------------------

    #[test]
    fn lgtm_only_posts_success_and_applies_the_lgtm_label() {
        let gh = FakeGh::new(SHA);
        let ext = extracted(&["LGTM\t0\t0"]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "success");
        assert_eq!(d.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
        assert_eq!(d.exit_code(), 0);
    }

    #[test]
    fn lgtm_then_changes_requested_same_sha_posts_failure_gh742() {
        // GH-742: a later LGTM never overrides an earlier Changes Requested.
        let gh = FakeGh::new(SHA);
        let ext = extracted(&["LGTM\t0\t0", "Changes Requested\t0\t1"]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "failure");
        assert_eq!(d.label, Some((LABEL_CHANGES.to_owned(), Write::Done)));

        // Order reversed: still failure, never re-flipped to success.
        let gh2 = FakeGh::new(SHA);
        let ext2 = extracted(&["Changes Requested\t0\t1", "LGTM\t0\t0"]);
        let d2 = deliver(&gh2, 1, SHA, &[], &ext2, union_of(&ext2));
        assert_eq!(gh2.status_calls.borrow()[0].1, "failure");
        assert_eq!(d2.label, Some((LABEL_CHANGES.to_owned(), Write::Done)));
    }

    #[test]
    fn a_later_changes_requested_on_the_same_sha_removes_the_standing_lgtm_label_gh1081_p1_1() {
        // GH-1081 Round 1 P1-1: the label carrier used to be add-only. Run 1
        // delivers a lone LGTM (review:lgtm applied). A Changes Requested
        // comment then lands on the SAME sha, so run 2's union flips to
        // Fail (GH-742) and must apply review:changes-requested — and, per
        // the retired shell's sibling cleanup, remove the now-stale review:lgtm
        // sibling so the two labels never stand together.
        let gh = FakeGh::new(SHA);
        let ext1 = extracted(&["LGTM\t0\t0"]);
        let d1 = deliver(&gh, 1, SHA, &[], &ext1, union_of(&ext1));
        assert_eq!(d1.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
        assert_eq!(gh.labels.borrow().as_slice(), [LABEL_LGTM.to_owned()]);
        assert!(gh.remove_label_calls.borrow().is_empty());

        let ext2 = extracted(&["LGTM\t0\t0", "Changes Requested\t0\t1"]);
        let d2 = deliver(&gh, 1, SHA, &[], &ext2, union_of(&ext2));
        assert_eq!(d2.label, Some((LABEL_CHANGES.to_owned(), Write::Done)));
        assert_eq!(
            gh.remove_label_calls.borrow().as_slice(),
            [(1, LABEL_LGTM.to_owned())],
            "the sibling review:lgtm must be removed, not left standing"
        );
        assert_eq!(
            gh.labels.borrow().as_slice(),
            [LABEL_CHANGES.to_owned()],
            "only the new label stands after the removal"
        );
    }

    #[test]
    fn reapplying_the_same_label_in_a_clean_state_makes_zero_new_writes() {
        // Clean-state rerun (GH-1081 Round 2 P1 fixture c): the target label
        // is already applied and the sibling is absent — idempotency must
        // still hold, exactly like before the Round 2 fix, just now also
        // covering the new `label_removed` field (GH-1030 doneWhen:
        // idempotent rerun, zero new writes).
        let gh = FakeGh::new(SHA);
        let ext = extracted(&["LGTM\t0\t0"]);
        let d1 = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(d1.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
        assert_eq!(d1.label_removed, None);

        let d2 = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(
            d2.label,
            Some((LABEL_LGTM.to_owned(), Write::Skipped("already applied")))
        );
        assert_eq!(d2.label_removed, None);
        assert!(gh.remove_label_calls.borrow().is_empty());
    }

    #[test]
    fn a_rerun_with_the_desired_label_applied_and_the_sibling_still_standing_removes_it_gh1081_round2_p1(
    ) {
        // GH-1081 Round 2 P1 fixture (b): Round 1's fix only reached the
        // sibling from the fresh-add arm, so a rerun where the desired
        // label was already applied (e.g. an earlier removal attempt
        // failed, or some other writer left both labels standing) could
        // never retry the cleanup — `Skipped("already applied")` at :302
        // returned before the sibling was even looked at. The condition
        // must now be evaluated independently of freshness.
        let gh = FakeGh::new(SHA);
        gh.labels.borrow_mut().push(LABEL_LGTM.to_owned());
        gh.labels.borrow_mut().push(LABEL_CHANGES.to_owned());
        let ext = extracted(&["LGTM\t0\t0"]);

        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(
            d.label,
            Some((LABEL_LGTM.to_owned(), Write::Skipped("already applied"))),
            "the desired label was already standing — no add_label call"
        );
        assert!(gh.label_calls.borrow().is_empty());
        assert_eq!(
            d.label_removed,
            Some((LABEL_CHANGES.to_owned(), Write::Done)),
            "the still-standing sibling must be removed even on the already-applied path"
        );
        assert_eq!(
            gh.remove_label_calls.borrow().as_slice(),
            [(1, LABEL_CHANGES.to_owned())]
        );
        assert_eq!(gh.labels.borrow().as_slice(), [LABEL_LGTM.to_owned()]);
        assert_eq!(d.exit_code(), 0);
    }

    #[test]
    fn a_failed_sibling_removal_is_recorded_and_makes_the_exit_code_non_zero_gh1081_round2_p1() {
        // GH-1081 Round 2 P1 fixture (a): Round 1 discarded the removal's
        // Result via `let _ =` — a failed `gh pr edit --remove-label` still
        // reported the label `Write::Done` and exit 0, with nothing on
        // stdout/JSON to say the sibling never came off. The removal must
        // now be recorded as its own `Write::Failed` and count toward
        // `exit_code()` like any other attempted write.
        let gh = FakeGh {
            fail_remove_label: true,
            ..FakeGh::new(SHA)
        };
        gh.labels.borrow_mut().push(LABEL_CHANGES.to_owned());
        let ext = extracted(&["LGTM\t0\t0"]);

        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(
            d.label,
            Some((LABEL_LGTM.to_owned(), Write::Done)),
            "the desired label itself was freshly, successfully applied"
        );
        assert!(
            matches!(&d.label_removed, Some((sib, Write::Failed(_))) if sib == LABEL_CHANGES),
            "the failed removal must be recorded, not swallowed: {:?}",
            d.label_removed
        );
        assert_eq!(
            gh.labels.borrow().as_slice(),
            [LABEL_CHANGES.to_owned(), LABEL_LGTM.to_owned()],
            "a failed removal must not be reflected as removed"
        );
        assert_ne!(
            d.exit_code(),
            0,
            "a failed write must never read as fully delivered"
        );
    }

    #[test]
    fn no_verdicts_at_all_posts_error_and_applies_no_label() {
        let gh = FakeGh::new(SHA);
        let ext = extracted(&[]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "error");
        assert_eq!(d.label, None);
        assert!(gh.label_calls.borrow().is_empty());
    }

    // ---- moved head: label withheld, status still written -------------------

    #[test]
    fn moved_head_withholds_the_label_but_still_posts_status() {
        let gh = FakeGh::new("ffffffffffffffffffffffffffffffffffffffff"); // current head != reviewed sha
        let ext = extracted(&["LGTM\t0\t0"]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "success");
        assert_eq!(
            d.label,
            Some((LABEL_LGTM.to_owned(), Write::Skipped("head moved")))
        );
        assert!(gh.label_calls.borrow().is_empty());
        assert_eq!(d.exit_code(), 0);
    }

    // ---- SHADOW: zero writes when it is the only signal ---------------------

    #[test]
    fn shadow_only_round_performs_zero_github_writes() {
        let gh = FakeGh::new(SHA);
        let ext = Extracted {
            lines: vec![],
            malformed: vec![],
            shadow: vec!["9".to_owned()],
            untrusted: vec![],
        };
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(d.status, None);
        assert_eq!(d.label, None);
        assert!(d.notices.is_empty());
        assert!(gh.status_calls.borrow().is_empty());
        assert!(gh.label_calls.borrow().is_empty());
        assert!(gh.comment_calls.borrow().is_empty());
        assert_eq!(d.exit_code(), 0);
    }

    #[test]
    fn a_real_verdict_alongside_a_shadow_comment_still_delivers_normally() {
        // SHADOW contributes nothing to `lines`, but must not poison a real,
        // authoritative verdict standing on the same SHA (REVIEW.md §8: a
        // SHADOW round is calibration evidence, not a gate on its own SHA).
        let gh = FakeGh::new(SHA);
        let ext = Extracted {
            lines: vec!["LGTM\t0\t0".to_owned()],
            malformed: vec![],
            shadow: vec!["9".to_owned()],
            untrusted: vec![],
        };
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "success");
        assert_eq!(d.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
    }

    // ---- malformed heading: one notice, status/label withheld ---------------

    #[test]
    fn malformed_heading_posts_one_notice_and_withholds_status_and_label() {
        let gh = FakeGh::new(SHA);
        let ext = Extracted {
            lines: vec![],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
            untrusted: vec![],
        };
        let d = deliver(&gh, 7, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.comment_calls.borrow().len(), 1);
        assert_eq!(
            gh.comment_calls.borrow()[0].1,
            "review: malformed verdict comment 42"
        );
        assert_eq!(d.notices, vec![("42".to_owned(), Write::Done)]);
        // No lines stood on this SHA, but an "error" status was still due
        // (see no_verdicts_at_all_posts_error_and_applies_no_label) — R23
        // withholds it rather than skipping it outright, so exit_code must
        // say "come back", not "delivered" (GH-1081 P1-2).
        assert!(matches!(d.status, Some(Write::Withheld(_))));
        assert_eq!(d.label, None, "Union::None calls for no label either way");
        assert!(gh.status_calls.borrow().is_empty());
        assert_eq!(d.exit_code(), 3);
    }

    #[test]
    fn malformed_notice_alongside_a_standing_verdict_withholds_status_and_label_gh1081_p1_2() {
        // GH-1081 Round 1 P1-2: a not-yet-noticed malformed id used to take
        // an early return regardless of `extracted.lines`, so a standing
        // LGTM got no status that run, yet exit_code() answered 0
        // ("delivered"). the retired review shell's post_review_status returns 3
        // ("status withheld this poll") in exactly this case, which is what
        // makes its caller come back on the next poll instead of treating
        // the SHA as settled.
        let gh = FakeGh::new(SHA);
        let ext = Extracted {
            lines: vec!["LGTM\t0\t0".to_owned()],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
            untrusted: vec![],
        };
        let d = deliver(&gh, 7, SHA, &[], &ext, union_of(&ext));
        assert_eq!(d.notices, vec![("42".to_owned(), Write::Done)]);
        assert!(
            matches!(d.status, Some(Write::Withheld(_))),
            "a standing LGTM makes the status due, not merely absent: {:?}",
            d.status
        );
        assert!(
            matches!(d.label, Some((_, Write::Withheld(_)))),
            "the union (LGTM) calls for review:lgtm, withheld not skipped: {:?}",
            d.label
        );
        assert!(
            gh.status_calls.borrow().is_empty(),
            "no status call — withheld means no gh write, not a failed one"
        );
        assert!(
            gh.label_calls.borrow().is_empty(),
            "no label call — withheld means no gh write, not a failed one"
        );
        assert_ne!(
            d.exit_code(),
            0,
            "a withheld status must not read as delivered"
        );
        assert_eq!(
            d.exit_code(),
            3,
            "withheld is its own outcome, distinct from partial (1) or full (2) failure"
        );
    }

    #[test]
    fn a_new_malformed_notice_on_a_moved_head_does_not_report_the_label_withheld_gh1081_round2_p2()
    {
        // GH-1081 Round 2 P2: the main branch gates the label on
        // `gh.head(pr) == sha`, answering `Skipped("head moved")` rather
        // than attempting anything. The withhold branch (a new malformed
        // notice posted this run) used to report the label `Withheld`
        // unconditionally, telling a moved head a label was held back that
        // was never due this run at all.
        let gh = FakeGh::new("ffffffffffffffffffffffffffffffffffffffff"); // current head != reviewed sha
        let ext = Extracted {
            lines: vec!["LGTM\t0\t0".to_owned()],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
            untrusted: vec![],
        };
        let d = deliver(&gh, 7, SHA, &[], &ext, union_of(&ext));
        assert_eq!(d.notices, vec![("42".to_owned(), Write::Done)]);
        assert!(
            matches!(d.status, Some(Write::Withheld(_))),
            "status is unconditional on head — still withheld: {:?}",
            d.status
        );
        assert_eq!(
            d.label,
            Some((LABEL_LGTM.to_owned(), Write::Skipped("head moved"))),
            "the label was never due this run on a moved head — not Withheld: {:?}",
            d.label
        );
        assert!(gh.label_calls.borrow().is_empty());
        assert!(gh.remove_label_calls.borrow().is_empty());
    }

    #[test]
    fn a_previously_noticed_malformed_comment_does_not_block_a_later_real_verdict() {
        // Once the one-shot notice for a malformed id already exists on
        // GitHub, a later run with a real, well-formed verdict alongside it
        // must proceed to status/label — matching the retired review shell's
        // "next poll proceeds with the malformed comment permanently
        // excluded".
        let gh = FakeGh::new(SHA);
        let existing = [Comment {
            id: "1".into(),
            body: "review: malformed verdict comment 42".into(),
            author_association: Some("OWNER".into()),
        }];
        let ext = Extracted {
            lines: vec!["LGTM\t0\t0".to_owned()],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
            untrusted: vec![],
        };
        let d = deliver(&gh, 7, SHA, &existing, &ext, union_of(&ext));
        assert!(gh.comment_calls.borrow().is_empty(), "no NEW notice");
        assert_eq!(
            d.notices,
            vec![("42".to_owned(), Write::Skipped("already noticed"))]
        );
        assert_eq!(gh.status_calls.borrow()[0].1, "success");
        assert_eq!(d.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
    }

    // ---- GH-1103: an untrusted §7 comment reaches no GitHub write ---------

    #[test]
    fn an_untrusted_lgtm_delivers_the_unreviewed_state_not_success_gh1103() {
        // The end-to-end direction the issue names: a well-formed §7 LGTM
        // from an author outside the trusted set yields no `success`
        // status and no `review:lgtm` label. With the forged verdict
        // refused, nothing stands on the SHA, so the union reads None —
        // the `error` state an unreviewed SHA gets — and no label is due
        // either way. Driven through the real `extract` rather than a
        // hand-built Extracted so the trust gate is what's under test.
        let forged = [Comment {
            id: "5".into(),
            body: format!(
                "## Code Review: Round 1 — PR #7 @ {SHA}\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
            ),
            author_association: Some("NONE".into()),
        }];
        let extracted = super::super::deliver::extract(SHA, &forged);
        assert!(extracted.lines.is_empty(), "the forged round stood");
        let gh = FakeGh::new(SHA);
        let d = deliver(&gh, 7, SHA, &forged, &extracted, union_of(&extracted));
        assert_eq!(
            gh.status_calls.borrow()[0].1,
            "error",
            "a refused verdict is not a pass"
        );
        assert!(gh.label_calls.borrow().is_empty(), "no review:lgtm");
        assert!(gh.comment_calls.borrow().is_empty(), "no notice either");
        assert_eq!(d.exit_code(), 0);
    }

    // ---- idempotency: re-running an already-delivered verdict -------------

    #[test]
    fn rerunning_an_already_delivered_verdict_performs_zero_new_writes() {
        let gh = FakeGh::new(SHA);
        gh.labels.borrow_mut().push(LABEL_LGTM.to_owned());
        *gh.latest_status.borrow_mut() = Ok(Some("success".to_owned()));
        let ext = extracted(&["LGTM\t0\t0"]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert!(gh.status_calls.borrow().is_empty(), "no new status call");
        assert!(gh.label_calls.borrow().is_empty(), "no new label call");
        assert_eq!(d.status, Some(Write::Skipped("already applied")));
        assert_eq!(
            d.label,
            Some((LABEL_LGTM.to_owned(), Write::Skipped("already applied")))
        );
        assert_eq!(d.exit_code(), 0);
    }

    // ---- gh failure contract: delivered / partially-delivered / failed ------

    #[test]
    fn a_status_post_failure_alone_is_a_full_failure() {
        let gh = FakeGh {
            fail_post_status: true,
            ..FakeGh::new(SHA)
        };
        let ext = extracted(&["LGTM\t0\t0"]);
        // Head unknown => label is Skipped, not attempted, so the ONLY
        // attempted write is the status, and it fails: exit 2.
        *gh.head.borrow_mut() = Err("gh: rate limited".into());
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert!(matches!(d.status, Some(Write::Failed(_))));
        assert!(matches!(d.label, Some((_, Write::Failed(_)))));
        assert_eq!(d.exit_code(), 2, "nothing was delivered");
    }

    #[test]
    fn a_label_failure_alongside_a_successful_status_is_partial() {
        let gh = FakeGh {
            fail_add_label: true,
            ..FakeGh::new(SHA)
        };
        let ext = extracted(&["LGTM\t0\t0"]);
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(d.status, Some(Write::Done));
        assert!(matches!(d.label, Some((_, Write::Failed(_)))));
        assert_eq!(d.exit_code(), 1, "status delivered, label did not");
    }

    #[test]
    fn a_malformed_notice_post_failure_withholds_status_and_label_and_is_a_full_failure() {
        let gh = FakeGh {
            fail_post_comment: true,
            ..FakeGh::new(SHA)
        };
        let ext = Extracted {
            lines: vec![],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
            untrusted: vec![],
        };
        let d = deliver(&gh, 7, SHA, &[], &ext, union_of(&ext));
        assert!(matches!(d.notices[0].1, Write::Failed(_)));
        // Attempting an as-yet-unnoticed id withholds status/label this run
        // even when the post itself fails — a failed notice must not be
        // masked by a status write that proceeds as if nothing happened.
        assert_eq!(d.status, None);
        assert_eq!(d.label, None);
        assert!(gh.status_calls.borrow().is_empty());
        assert_eq!(d.exit_code(), 2, "the only attempted write failed");
    }
}
