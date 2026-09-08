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
//! already present on GitHub — posted by whatever ran the review, today
//! `scripts/pr-review-watch.sh`'s `settle_pending`. The issue's own list of
//! what the follow-up adapter PR converts into calls on this verb —
//! `post_review_status`, `settle_pending`, `verdict_body_lines` — never
//! names the shell's `gh pr comment --body-file` verdict post, so that call
//! stays exactly where it is; this module owns only the writes derived from
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
//! files the way `pr-review-watch.sh`'s `verdict_engine` / `classify_surface`
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
//! carries no head-equality clause either. `pr-review-watch.sh`'s
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
}

impl Write {
    pub(crate) fn tag(&self) -> &'static str {
        match self {
            Write::Skipped(_) => "skipped",
            Write::Done => "done",
            Write::Failed(_) => "failed",
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Write::Skipped(r) => Some(r),
            Write::Failed(e) => Some(e.as_str()),
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
    /// `None` only when withheld by rule: a new notice was just posted this
    /// run, or the only standing signal on the SHA is SHADOW.
    pub status: Option<Write>,
    /// `(label, outcome)`; `None` when the union is `Union::None` (no label
    /// is due either way).
    pub label: Option<(String, Write)>,
}

impl Delivery {
    /// 0 delivered, 1 partially delivered, 2 failed — the exit-code
    /// contract doneWhen requires: nothing here is silently dropped, every
    /// attempted write's outcome feeds this. A run whose intended action
    /// (a notice, or a status+label) fully succeeds, or whose intended
    /// action was correctly "nothing" (SHADOW-only, an idempotent rerun),
    /// is `delivered`.
    pub(crate) fn exit_code(&self) -> i32 {
        let outcomes: Vec<&Write> = self
            .notices
            .iter()
            .map(|(_, w)| w)
            .chain(self.status.iter())
            .chain(self.label.iter().map(|(_, w)| w))
            .collect();
        let attempted = outcomes
            .iter()
            .filter(|w| !matches!(w, Write::Skipped(_)))
            .count();
        let failed = outcomes
            .iter()
            .filter(|w| matches!(w, Write::Failed(_)))
            .count();
        if failed == 0 {
            0
        } else if attempted > 0 && failed == attempted {
            2
        } else {
            1
        }
    }
}

/// The malformed-notice text, exact and stable so a notice posted by either
/// `edda review deliver` or `pr-review-watch.sh` (during the transition
/// before the follow-up adapter PR) is recognized by the other — no local
/// marker file, no ledger row, just this string read back from GitHub.
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
        // pr-review-watch.sh, whose `new_notice=1` sits outside the
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
    if new_notice {
        return out;
    }
    // The only standing signal on this SHA is a self-declared SHADOW round
    // (or a malformed comment already noticed on an earlier run): R22 /
    // REVIEW.md §8 — zero further writes.
    if extracted.lines.is_empty() && !extracted.shadow.is_empty() {
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

    let label_target = match union {
        Union::Pass => Some(LABEL_LGTM),
        Union::Fail => Some(LABEL_CHANGES),
        Union::None => None,
    };
    if let Some(label) = label_target {
        let outcome = match gh.head(pr) {
            Ok(current) if current == sha => match gh.labels(pr) {
                Ok(existing) if existing.iter().any(|l| l == label) => {
                    Write::Skipped("already applied")
                }
                Ok(_) => match gh.add_label(pr, label) {
                    Ok(()) => Write::Done,
                    Err(error) => Write::Failed(error.to_string()),
                },
                Err(error) => Write::Failed(format!("read labels: {error}")),
            },
            Ok(_moved) => Write::Skipped("head moved"),
            Err(error) => Write::Failed(format!("read current head: {error}")),
        };
        out.label = Some((label.to_owned(), outcome));
    }

    out
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

        label_calls: RefCell<Vec<(u64, String)>>,
        status_calls: RefCell<Vec<(String, String, String)>>,
        comment_calls: RefCell<Vec<(u64, String)>>,
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
                label_calls: RefCell::new(Vec::new()),
                status_calls: RefCell::new(Vec::new()),
                comment_calls: RefCell::new(Vec::new()),
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
        };
        let d = deliver(&gh, 1, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.status_calls.borrow()[0].1, "success");
        assert_eq!(d.label, Some((LABEL_LGTM.to_owned(), Write::Done)));
    }

    // ---- malformed heading: one notice, no status, no label -----------------

    #[test]
    fn malformed_heading_posts_one_notice_and_withholds_status_and_label() {
        let gh = FakeGh::new(SHA);
        let ext = Extracted {
            lines: vec![],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
        };
        let d = deliver(&gh, 7, SHA, &[], &ext, union_of(&ext));
        assert_eq!(gh.comment_calls.borrow().len(), 1);
        assert_eq!(
            gh.comment_calls.borrow()[0].1,
            "review: malformed verdict comment 42"
        );
        assert_eq!(d.notices, vec![("42".to_owned(), Write::Done)]);
        assert_eq!(d.status, None);
        assert_eq!(d.label, None);
        assert!(gh.status_calls.borrow().is_empty());
        assert_eq!(d.exit_code(), 0);
    }

    #[test]
    fn a_previously_noticed_malformed_comment_does_not_block_a_later_real_verdict() {
        // Once the one-shot notice for a malformed id already exists on
        // GitHub, a later run with a real, well-formed verdict alongside it
        // must proceed to status/label — matching pr-review-watch.sh's
        // "next poll proceeds with the malformed comment permanently
        // excluded".
        let gh = FakeGh::new(SHA);
        let existing = [Comment {
            id: "1".into(),
            body: "review: malformed verdict comment 42".into(),
        }];
        let ext = Extracted {
            lines: vec!["LGTM\t0\t0".to_owned()],
            malformed: vec!["42".to_owned()],
            shadow: vec![],
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
