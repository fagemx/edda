//! `edda review due` — the trigger policy, in the product (GH-763).
//!
//! Whether a PR is worth reviewing again is a judgement, not forge glue
//! (#766 D8), and it is the main cost switch in the whole review system. The
//! watcher's `decide()` used to answer it in awk with one rule — *head differs
//! from the last reviewed SHA* — which means **one review per push**. A round-1
//! Opus review measured $1.28–$2.57 on #754; the resumed delta round of the
//! same PR measured $0.22 and then $0.02. Five pushes on one PR bought five
//! round-1 prices for what a debounce plus a resume would have charged once.
//!
//! So this verb decides, and the daemon only gathers forge facts and acts on
//! the exit code. It reads the ledger and writes nothing.
//!
//! ## The policy, in order
//!
//! | Condition | Answer |
//! |---|---|
//! | `--draft` | `SKIP draft` |
//! | `--ready` (left draft this cycle) | `REVIEW ready` |
//! | `--response-at` newer than the last verdict | `REVIEW response` |
//! | head moved and the push has settled | `REVIEW push` |
//! | head moved, still inside the debounce | `SKIP debounce <n>s` |
//! | otherwise | `SKIP reviewed` |
//!
//! Draft comes first because a draft is not a subject at all. `ready` and
//! `response` are events rather than states, so neither is debounced: they
//! happen once and the operator is waiting on them. Only `push` is, because
//! only `push` repeats.

use super::config::DueConfig;
use anyhow::{Context, Result};
use edda_core::ReviewVerdictPayload;
use edda_ledger::Ledger;
use std::path::Path;

/// Flags for `edda review due`.
#[derive(clap::Args)]
pub struct DueArgs {
    /// The PR's current head, as a full lowercase 40-hex SHA
    #[arg(long, value_name = "SHA")]
    pub head: String,
    /// When that head was pushed (RFC3339); without it a moved head waits
    #[arg(long, value_name = "RFC3339")]
    pub pushed_at: Option<String>,
    /// The PR left draft during this cycle
    #[arg(long)]
    pub ready: bool,
    /// Timestamp of the newest `Review Response: Round N` comment (RFC3339)
    #[arg(long, value_name = "RFC3339")]
    pub response_at: Option<String>,
    /// When the caller last reviewed this PR (RFC3339), from its own record
    ///
    /// A ledger verdict answers the same question but only when one exists:
    /// verdicts do not cross machines (GH-671) and a round published through
    /// the §7 comment path writes none at all. Without this, a response older
    /// than a review that did happen still reads as unanswered.
    #[arg(long, value_name = "RFC3339")]
    pub last_reviewed_at: Option<String>,
    /// The PR is a draft
    #[arg(long)]
    pub draft: bool,
    /// PR number, which is how the ledger's rounds are found
    #[arg(long, value_name = "N")]
    pub pr: Option<u64>,
    /// The SHA the caller last reviewed, when it knows better than the ledger
    ///
    /// The daemon keeps its own reviewed-head state, and it is the only record
    /// of a round that went through the §7 comment path rather than
    /// `edda review` — those write no `review_verdict` event, so the ledger
    /// would read them as "never reviewed" and make every cycle due.
    #[arg(long, value_name = "SHA")]
    pub last_reviewed: Option<String>,
    /// The PR carries `review:unreviewed`: a head the watcher already gave up on
    #[arg(long)]
    pub unreviewed_label: bool,
    /// Clock override, for tests
    #[arg(long, value_name = "RFC3339", hide = true)]
    pub now: Option<String>,
}

/// What the ledger knows about the rounds already spent on this PR.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// Head SHA of the newest verdict, in ledger insertion order.
    pub(crate) last_sha: Option<String>,
    /// Timestamp of that verdict's event.
    pub(crate) last_ts: Option<String>,
    /// Reviewer session of the lowest round present — what a later round
    /// resumes. Round 1 when this ledger has it.
    pub(crate) first_session: Option<String>,
    /// The round `first_session` came from; `u32::MAX` until one is seen.
    pub(crate) first_round: u32,
    /// Rounds that produced a verdict.
    pub(crate) rounds: u32,
    /// Summed cost of those rounds, and whether every one of them was measured.
    pub(crate) cost_usd: f64,
    pub(crate) all_measured: bool,
}

impl History {
    /// `review cost so far: …` — the line that makes the cost switch visible
    /// at the moment it is thrown.
    ///
    /// An unmeasured round makes the whole total unmeasured rather than
    /// quietly contributing zero: a sum that silently drops a round reads as
    /// cheaper than the truth, which is the one direction a cost display must
    /// never fail in.
    /// The ` --resume <session>` suffix on a REVIEW line, or empty.
    ///
    /// A second round resumes the first round's session: the reviewer already
    /// holds the diff, the spec and its own findings, which is the difference
    /// between a $2 round and a $0.02 one. Round 1's session is the one to
    /// resume — a later round's reviewer holds a delta, not the whole subject.
    pub(crate) fn resume_suffix(&self) -> String {
        match (self.rounds >= 1, self.first_session.as_deref()) {
            (true, Some(session)) => format!(" --resume {session}"),
            _ => String::new(),
        }
    }

    pub(crate) fn cost_line(&self) -> String {
        let rounds = if self.rounds == 1 { "round" } else { "rounds" };
        if self.rounds == 0 {
            return format!("review cost so far: $0.00 over 0 {rounds}");
        }
        if !self.all_measured {
            return format!(
                "review cost so far: unmeasured over {} {rounds}",
                self.rounds
            );
        }
        format!(
            "review cost so far: ${:.2} over {} {rounds}",
            self.cost_usd, self.rounds
        )
    }
}

/// Rounds already spent on `pr`, from the ledger's `review_verdict` events.
///
/// Ordered by insertion (`ORDER BY rowid`), like every other ledger fold —
/// RFC3339 strings with mixed precision do not sort chronologically, so the
/// newest verdict is the last row, not the largest timestamp.
///
/// `unreviewed` events are skipped: they record that no review could be made,
/// so they consume no round, carry no cost, and cannot be resumed.
pub(crate) fn history(repo: &Path, pr: u64) -> Result<History> {
    let ledger = Ledger::open(repo)?;
    let mut history = History {
        all_measured: true,
        first_round: u32::MAX,
        ..Default::default()
    };
    for event in ledger.iter_events_by_type("review_verdict")? {
        // Scope by PR *before* deserializing. A `review_verdict` this build
        // cannot read is fatal for the PR it belongs to — a policy that
        // silently drops the event it cannot parse is a policy that decides
        // while blind — but it must not be fatal for every other PR. Reading
        // `refs.pr` from the raw JSON keeps one malformed event from turning
        // the whole daemon into `SKIP due-unknown` until the ledger is
        // repaired.
        // Skip only what is *positively* another PR's. An event whose
        // `refs.pr` is absent or unreadable falls through to the full
        // deserialize below, which either reads it or fails loudly — skipping
        // it here would be the same blindness this gate exists to avoid, just
        // moved one line up.
        match event.payload.pointer("/refs/pr").and_then(|v| v.as_u64()) {
            Some(other) if other != pr => continue,
            _ => {}
        }
        let payload: ReviewVerdictPayload = serde_json::from_value(event.payload.clone())
            .with_context(|| format!("review_verdict event {}", event.event_id))?;
        if payload.verdict == "unreviewed" {
            continue;
        }
        history.rounds += 1;
        history.last_sha = Some(payload.subject.head_sha.clone());
        history.last_ts = Some(event.ts.clone());
        // Round 1's reviewer is the one to resume — it holds the whole
        // subject, where a later round holds a delta — and the payload says
        // which round it is. Selecting by ledger position instead would name
        // whichever round happens to sit earliest here, which is not the same
        // thing on a machine that imported only later rounds.
        //
        // When no round 1 is present at all, the lowest round that *is*
        // present is the best available resume target; answering "no session"
        // would send a continuing review back to round-1 price for want of an
        // event this machine never received.
        let round = payload.refs.round.unwrap_or(u32::MAX);
        if round < history.first_round {
            history.first_round = round;
            history.first_session = Some(payload.reviewer.session_id.clone());
        }
        match (payload.cost.measured, payload.cost.usd) {
            (true, Some(usd)) => history.cost_usd += usd,
            _ => history.all_measured = false,
        }
    }
    Ok(history)
}

/// The answer, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Due {
    /// Review, for this reason.
    Review(String),
    /// Do not review, for this reason.
    Skip(String),
}

/// The facts the policy runs on, with every clock already resolved to epoch
/// seconds so the rule itself has no I/O and no ambient time.
pub(crate) struct Facts<'a> {
    pub(crate) head: &'a str,
    pub(crate) draft: bool,
    pub(crate) ready: bool,
    /// The PR carries `review:unreviewed`.
    pub(crate) unreviewed_label: bool,
    pub(crate) pushed_at: Option<i64>,
    pub(crate) response_at: Option<i64>,
    pub(crate) now: i64,
    pub(crate) last_sha: Option<&'a str>,
    pub(crate) last_verdict_at: Option<i64>,
    /// When the caller itself last reviewed, if it keeps such a record.
    pub(crate) last_reviewed_at: Option<i64>,
}

/// The trigger policy. Pure: same facts, same answer, on every machine.
pub(crate) fn decide(facts: &Facts, config: &DueConfig) -> Due {
    // A draft is not a subject. This is first and is not configurable: an
    // enabled trigger cannot make a draft reviewable.
    if facts.draft {
        return Due::Skip("draft".into());
    }

    // `review:unreviewed` marks a head the watcher already gave up on. It is
    // released by the head moving — and only by that, so a PR whose reviewed
    // SHA was never recorded stays skipped rather than looping: without a
    // recorded SHA there is no way to tell that the head moved, and guessing
    // in the reviewing direction buys a round-1 price per cycle.
    if facts.unreviewed_label {
        let moved_from_known = facts.last_sha.is_some_and(|last| last != facts.head);
        if !moved_from_known {
            return Due::Skip("review-unreviewed".into());
        }
    }

    // Leaving draft is a one-time event the operator is waiting on, so it is
    // never debounced.
    if facts.ready && config.enabled("ready") {
        return Due::Review("ready".into());
    }

    // A Review Response is the implementer saying "look again", and it counts
    // only if it arrived after the review it answers. An older one is the
    // previous round's, already answered.
    //
    // This branch is not debounced and nothing marks a response as consumed,
    // so getting "already answered" wrong does not cost one wasted round — it
    // costs one per poll, forever, each at round-1 price. It therefore
    // measures against **every** record of having reviewed that the caller
    // has, and fires only if the response is newer than all of them:
    //
    // - the ledger verdict, which exists only sometimes (verdicts do not cross
    //   machines — GH-671 — and a round published through the §7 comment path
    //   writes no event at all), and
    // - the caller's own `--last-reviewed-at`, which is the record that does
    //   exist for a round this daemon ran.
    //
    // With neither, there is nothing to be newer than and this is not a
    // trigger: the PR falls through to the push rule below, which is debounced
    // and reads the caller's recorded head.
    if config.enabled("response") {
        let reviewed_at = match (facts.last_verdict_at, facts.last_reviewed_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (only, None) | (None, only) => only,
        };
        if let (Some(response_at), Some(reviewed_at)) = (facts.response_at, reviewed_at) {
            if response_at > reviewed_at {
                return Due::Review("response".into());
            }
        }
    }

    let moved = facts.last_sha != Some(facts.head);
    if !moved {
        return Due::Skip("reviewed".into());
    }
    if !config.enabled("push") {
        return Due::Skip("push-trigger-disabled".into());
    }

    // The debounce is the cost switch. A head that moved 20 seconds ago is
    // usually mid-sequence — the fixup, the second thought, the CI-driven
    // amend — and reviewing it buys a round-1 price for a tree about to
    // change again.
    //
    // An unknown push time waits rather than proceeding. The caller cannot
    // tell "this PR has no commit date" from "the forge call failed", and the
    // second is the dangerous one: a rate limit or an expired token would
    // otherwise turn the cost switch off for every PR at once, which is
    // exactly when the daemon is polling hardest. Holding costs one cycle;
    // proceeding costs a round.
    let debounce = i64::try_from(config.debounce_seconds).unwrap_or(i64::MAX);
    let Some(pushed_at) = facts.pushed_at else {
        return Due::Skip("debounce push-time-unknown".into());
    };
    let settled_for = facts.now.saturating_sub(pushed_at);
    if settled_for >= debounce {
        Due::Review("push".into())
    } else {
        Due::Skip(format!(
            "debounce {}s",
            debounce.saturating_sub(settled_for)
        ))
    }
}

/// Parse an RFC3339 timestamp to epoch seconds.
fn epoch(value: &str, what: &str) -> Result<i64> {
    Ok(
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            .with_context(|| format!("{what}: expected an RFC3339 timestamp, got {value:?}"))?
            .unix_timestamp(),
    )
}

/// CLI entry point. Exit: 0 due, 1 not due, 2 cannot judge.
///
/// Every internal failure leaves through exit 2. The daemon reads 1 as a
/// decision not to review; an unreadable ledger is not that decision, and
/// publishing it as one would silently stop reviewing the repository.
pub fn run(args: DueArgs, cwd: &Path) -> Result<()> {
    match judge(args, cwd) {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("edda review due: {error:#}");
            std::process::exit(2);
        }
    }
}

fn judge(args: DueArgs, cwd: &Path) -> Result<()> {
    if args.head.len() != 40
        || !args
            .head
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase() && byte <= b'f')
    {
        eprintln!("edda review due: expected a full lowercase 40-hex SHA");
        std::process::exit(2);
    }

    let now = match args.now.as_deref() {
        Some(value) => epoch(value, "--now")?,
        None => time::OffsetDateTime::now_utc().unix_timestamp(),
    };
    let pushed_at = args
        .pushed_at
        .as_deref()
        .map(|value| epoch(value, "--pushed-at"))
        .transpose()?;
    let response_at = args
        .response_at
        .as_deref()
        .map(|value| epoch(value, "--response-at"))
        .transpose()?;
    let last_reviewed_at = args
        .last_reviewed_at
        .as_deref()
        .map(|value| epoch(value, "--last-reviewed-at"))
        .transpose()?;

    // The repo is only needed once a PR number gives the ledger something to
    // look up, so the flag-only cases work outside a checkout.
    let (config, history) = match args.pr {
        Some(pr) => {
            let repo = super::git::repo_root_from(cwd)?;
            (DueConfig::load(&repo)?, history(&repo, pr)?)
        }
        None => (DueConfig::default(), History::default()),
    };
    let last_verdict_at = history
        .last_ts
        .as_deref()
        .map(|value| epoch(value, "ledger verdict timestamp"))
        .transpose()?;
    // The caller's record wins over the ledger's for "has this head been
    // reviewed": it is the one that also covers comment-path rounds.
    let last_sha = args
        .last_reviewed
        .as_deref()
        .or(history.last_sha.as_deref());

    let answer = decide(
        &Facts {
            head: &args.head,
            draft: args.draft,
            ready: args.ready,
            unreviewed_label: args.unreviewed_label,
            pushed_at,
            response_at,
            now,
            last_sha,
            last_verdict_at,
            last_reviewed_at,
        },
        &config,
    );

    match answer {
        Due::Review(reason) => {
            // A second round resumes the first round's session: the reviewer
            // already holds the diff, the spec and its own findings, which is
            // the difference between a $2 round and a $0.02 one.
            println!("REVIEW {reason}{}", history.resume_suffix());
            println!("{}", history.cost_line());
            Ok(())
        }
        Due::Skip(reason) => {
            println!("SKIP {reason}");
            println!("{}", history.cost_line());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn facts() -> Facts<'static> {
        Facts {
            head: HEAD,
            draft: false,
            ready: false,
            unreviewed_label: false,
            pushed_at: None,
            response_at: None,
            now: 10_000,
            last_sha: None,
            last_verdict_at: None,
            last_reviewed_at: None,
        }
    }

    #[test]
    fn a_response_older_than_the_callers_own_review_is_already_answered() {
        // Round 2's P1: requiring a verdict to *exist* is not the same as
        // requiring it to be *current*. `response` never reads the recorded
        // head, so a PR the daemon has already reviewed still re-fired on
        // every poll — undebounced, and nothing marks a response consumed.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD); // the daemon reviewed this very head
        f.last_verdict_at = Some(1_000); // an old verdict exists
        f.response_at = Some(2_000); // newer than the verdict...
        f.last_reviewed_at = Some(3_000); // ...but older than the review
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
    }

    #[test]
    fn a_response_newer_than_every_record_of_reviewing_still_fires() {
        // The trigger has to survive its own hardening: this is the case it
        // exists for — the implementer answered a round without pushing.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD);
        f.last_verdict_at = Some(1_000);
        f.last_reviewed_at = Some(3_000);
        f.response_at = Some(3_001);
        assert_eq!(decide(&f, &config), Due::Review("response".into()));
    }

    #[test]
    fn the_callers_own_record_counts_even_with_no_verdict_at_all() {
        // The common shape: a round published through the §7 comment path
        // writes no ledger event, so the caller's record is the only one.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD);
        f.last_verdict_at = None;
        f.last_reviewed_at = Some(3_000);
        f.response_at = Some(2_999);
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
        f.response_at = Some(3_001);
        assert_eq!(decide(&f, &config), Due::Review("response".into()));
    }

    #[test]
    fn a_response_with_no_verdict_to_answer_is_not_a_trigger() {
        // Round 1's P1: `is_none_or` read a missing verdict as "the response
        // is newer", so a PR carrying a Review Response and no ledger verdict
        // was due on every poll, forever, at round-1 price. Verdicts do not
        // cross machines (GH-671) and a §7-comment round writes no event, so
        // this is the ordinary shape, not an exotic one.
        let config = DueConfig::default();
        let mut f = facts();
        f.response_at = Some(9_000);
        f.last_verdict_at = None;
        f.last_sha = Some(HEAD); // head unmoved: nothing else can make it due
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
    }

    #[test]
    fn a_response_still_opens_the_next_round_when_a_verdict_exists() {
        // The trigger itself is unchanged for the case it was written for.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD);
        f.last_verdict_at = Some(9_000);
        f.response_at = Some(9_001);
        assert_eq!(decide(&f, &config), Due::Review("response".into()));
        // ...and an older response is the previous round's, already answered.
        f.response_at = Some(8_999);
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
    }

    #[test]
    fn an_unverdicted_response_falls_through_to_the_debounced_push_rule() {
        // Falling through is the point: the PR is still reviewable, but by the
        // rule that has a cost switch on it rather than the one that does not.
        let config = DueConfig::default();
        let mut f = facts();
        f.response_at = Some(9_000);
        f.last_verdict_at = None;
        f.last_sha = Some(OLD);
        f.pushed_at = Some(f.now - 10);
        assert_eq!(
            decide(&f, &config),
            Due::Skip(format!("debounce {}s", config.debounce_seconds - 10))
        );
        f.pushed_at = Some(f.now - config.debounce_seconds as i64);
        assert_eq!(decide(&f, &config), Due::Review("push".into()));
    }

    #[test]
    fn an_unknown_push_time_waits_rather_than_opening_the_switch() {
        // Round 1's other P1: the caller cannot tell "no commit date" from
        // "the forge call failed", and a rate limit would otherwise disable
        // the debounce for every PR at once — precisely when the daemon is
        // polling hardest. Holding costs a cycle; proceeding costs a round.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(OLD);
        f.pushed_at = None;
        assert_eq!(
            decide(&f, &config),
            Due::Skip("debounce push-time-unknown".into())
        );
    }

    #[test]
    fn an_enormous_configured_debounce_does_not_wrap_the_switch_open() {
        // `as i64` turned a debounce >= 2^63 negative, so `settled_for >=
        // debounce` always held and the switch was permanently open.
        let config = DueConfig {
            debounce_seconds: u64::MAX,
            ..DueConfig::default()
        };
        let mut f = facts();
        f.last_sha = Some(OLD);
        f.pushed_at = Some(f.now - 1);
        assert!(
            matches!(decide(&f, &config), Due::Skip(reason) if reason.starts_with("debounce ")),
            "a saturating debounce must hold, never open"
        );
    }

    /// A `review_verdict` payload with only the fields this policy reads.
    fn verdict_payload(pr: u64, round: u32, head: &str, session: &str) -> serde_json::Value {
        serde_json::json!({
            "schema": "review_verdict/0",
            "subject": {
                "base_sha": "base", "head_sha": head,
                "files": 1, "lines": 1, "coverage": "full"
            },
            "refs": { "pr": pr, "round": round },
            "spec": { "mode": "spec-backed", "source": "issue", "trust": "declared" },
            "brief": { "core": "review-spec-v1" },
            "reviewer": {
                "agent": "claude", "transport": "cli",
                "model_requested": "m", "model_observed": "m",
                "observed_via": "test", "session_id": session,
                "session_label": session, "tool_policy": "read-only"
            },
            "independence": "clean", "independence_policy": "session",
            "gates": { "status": "green" }, "verdict": "lgtm", "outcome": "pass",
            "qualified": true,
            "cost": { "usd": 1.0, "measured": true, "duration_ms": 1 },
            "parse": "ok"
        })
    }

    /// Append `review_verdict` events to a throwaway ledger and read them back
    /// through the real `history()`, so the round-selection and PR-scoping
    /// rules are exercised rather than assumed.
    fn ledger_history(events: &[(u64, u32, &str, &str)], pr: u64) -> (tempfile::TempDir, History) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let ledger = edda_ledger::Ledger::open_or_init(&root).expect("ledger");
        for (event_pr, round, head, session) in events {
            let payload: edda_core::ReviewVerdictPayload =
                serde_json::from_value(verdict_payload(*event_pr, *round, head, session))
                    .expect("payload");
            let event = edda_core::event::new_review_verdict_event(
                "main",
                ledger.last_event_hash().expect("hash").as_deref(),
                &payload,
                None,
                None,
                &[],
            )
            .expect("event");
            ledger.append_event(&event).expect("append");
        }
        let history = history(&root, pr).expect("history");
        (temp, history)
    }

    #[test]
    fn the_session_to_resume_is_round_ones_by_number_not_by_ledger_position() {
        // A machine that imported only later rounds would otherwise name
        // whichever verdict happens to sit earliest in its own ledger. Round 1
        // is the reviewer holding the whole subject; a later round holds a
        // delta.
        let (_temp, history) = ledger_history(
            &[
                (42, 2, "head2", "round-2-session"),
                (42, 1, "head1", "round-1-session"),
            ],
            42,
        );
        assert_eq!(history.rounds, 2);
        assert_eq!(history.resume_suffix(), " --resume round-1-session");
    }

    #[test]
    fn another_prs_verdicts_are_not_this_prs_history() {
        let (_temp, history) = ledger_history(
            &[
                (99, 1, "other", "other-session"),
                (42, 1, "head1", "round-1-session"),
            ],
            42,
        );
        assert_eq!(history.rounds, 1);
        assert_eq!(history.resume_suffix(), " --resume round-1-session");
        assert_eq!(history.last_sha.as_deref(), Some("head1"));
    }

    #[test]
    fn the_resume_suffix_names_round_ones_session_and_only_from_round_two() {
        // Round 2's P1: dropping ` --resume {session}` from the decision line
        // left every test green. The suffix is the whole point of the verb —
        // a resumed round measured $0.02 against round 1's $1.28 — so its
        // shape is pinned here rather than only in the ledger path.
        let mut history = History {
            rounds: 0,
            first_session: None,
            ..Default::default()
        };
        assert_eq!(
            history.resume_suffix(),
            "",
            "no round yet, nothing to resume"
        );
        history.rounds = 1;
        history.first_session = Some("round-1-session".into());
        assert_eq!(history.resume_suffix(), " --resume round-1-session");
        // A round that recorded no session id cannot be resumed by guessing.
        history.first_session = None;
        assert_eq!(history.resume_suffix(), "");
    }

    #[test]
    fn a_ledger_without_round_one_resumes_the_lowest_round_it_has() {
        // A machine that imported only later rounds still has a reviewer worth
        // resuming; answering "no session" would pay round-1 price for want of
        // an event this machine never received.
        let (_temp, history) = ledger_history(
            &[
                (42, 3, "head3", "round-3-session"),
                (42, 2, "head2", "round-2-session"),
            ],
            42,
        );
        assert_eq!(history.resume_suffix(), " --resume round-2-session");
    }

    #[test]
    fn a_draft_is_not_a_subject() {
        let config = DueConfig::default();
        let mut f = facts();
        f.draft = true;
        // Not even when everything else would fire — a draft is refused first.
        f.ready = true;
        f.response_at = Some(9_999);
        f.last_sha = Some(OLD);
        assert_eq!(decide(&f, &config), Due::Skip("draft".into()));
    }

    #[test]
    fn leaving_draft_and_answering_a_review_are_never_debounced() {
        let config = DueConfig::default();
        let mut ready = facts();
        ready.ready = true;
        ready.pushed_at = Some(9_999); // one second ago, deep inside the debounce
        assert_eq!(decide(&ready, &config), Due::Review("ready".into()));

        let mut response = facts();
        response.response_at = Some(9_999);
        response.pushed_at = Some(9_999);
        response.last_sha = Some(HEAD); // the head has not even moved
        response.last_verdict_at = Some(9_000);
        assert_eq!(decide(&response, &config), Due::Review("response".into()));
    }

    #[test]
    fn a_response_older_than_the_verdict_it_answers_is_the_previous_round() {
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD);
        f.last_verdict_at = Some(9_000);
        f.response_at = Some(8_000); // answered a round that is already closed
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
    }

    #[test]
    fn an_unmoved_head_is_already_reviewed() {
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(HEAD);
        f.pushed_at = Some(0); // long settled, and still not due
        assert_eq!(decide(&f, &config), Due::Skip("reviewed".into()));
    }

    #[test]
    fn a_moved_head_waits_out_the_debounce_then_becomes_due() {
        let config = DueConfig::default(); // 600s
        let mut f = facts();
        f.last_sha = Some(OLD);
        f.pushed_at = Some(10_000 - 599);
        assert_eq!(decide(&f, &config), Due::Skip("debounce 1s".into()));
        f.pushed_at = Some(10_000 - 600);
        assert_eq!(decide(&f, &config), Due::Review("push".into()));
    }

    #[test]
    fn three_pushes_inside_one_debounce_window_are_due_once() {
        // The whole point of the switch: a burst of pushes buys one review,
        // not one per push. Each push resets the wait, because each one
        // changes the tree the reviewer would read.
        let config = DueConfig::default();
        let mut f = facts();
        f.last_sha = Some(OLD);
        let mut due = 0;
        for (now, pushed) in [(10_000, 10_000), (10_100, 10_100), (10_200, 10_200)] {
            f.now = now;
            f.pushed_at = Some(pushed);
            if matches!(decide(&f, &config), Due::Review(_)) {
                due += 1;
            }
        }
        assert_eq!(due, 0, "a burst must not buy three round-1 reviews");
        // Once the last push settles, exactly one review is due.
        f.now = 10_200 + 600;
        assert_eq!(decide(&f, &config), Due::Review("push".into()));
    }

    #[test]
    fn an_unreviewed_label_is_released_only_by_a_head_that_demonstrably_moved() {
        let config = DueConfig::default();

        // Never recorded a reviewed SHA: nothing proves the head moved, so
        // the label holds. Guessing the other way buys a round-1 price every
        // cycle, forever.
        let mut unknown = facts();
        unknown.unreviewed_label = true;
        assert_eq!(
            decide(&unknown, &config),
            Due::Skip("review-unreviewed".into())
        );

        // Recorded, and equal: still the head that was given up on.
        let mut same = facts();
        same.unreviewed_label = true;
        same.last_sha = Some(HEAD);
        assert_eq!(
            decide(&same, &config),
            Due::Skip("review-unreviewed".into())
        );

        // Recorded, and different: the label is stale and the PR is due.
        // The push time is settled so this asserts the label rule rather than
        // the debounce — without one the answer would be "wait", which is
        // true but about a different rule.
        let mut moved = facts();
        moved.unreviewed_label = true;
        moved.last_sha = Some(OLD);
        moved.pushed_at = Some(moved.now - config.debounce_seconds as i64);
        assert_eq!(decide(&moved, &config), Due::Review("push".into()));

        // A draft still wins over everything.
        let mut draft = facts();
        draft.unreviewed_label = true;
        draft.last_sha = Some(OLD);
        draft.draft = true;
        assert_eq!(decide(&draft, &config), Due::Skip("draft".into()));
    }

    #[test]
    fn a_disabled_trigger_does_not_fire() {
        let config = DueConfig {
            debounce_seconds: 600,
            triggers: vec!["push".into()],
        };
        let mut ready = facts();
        ready.ready = true;
        ready.last_sha = Some(HEAD);
        assert_eq!(decide(&ready, &config), Due::Skip("reviewed".into()));

        let no_push = DueConfig {
            debounce_seconds: 600,
            triggers: vec!["ready".into()],
        };
        let mut moved = facts();
        moved.last_sha = Some(OLD);
        assert_eq!(
            decide(&moved, &no_push),
            Due::Skip("push-trigger-disabled".into())
        );
    }

    #[test]
    fn an_unmeasured_round_makes_the_total_unmeasured_not_cheaper() {
        let none = History::default();
        assert_eq!(none.cost_line(), "review cost so far: $0.00 over 0 rounds");

        let measured = History {
            rounds: 2,
            cost_usd: 1.5,
            all_measured: true,
            ..Default::default()
        };
        assert_eq!(
            measured.cost_line(),
            "review cost so far: $1.50 over 2 rounds"
        );

        let partial = History {
            rounds: 2,
            cost_usd: 1.5,
            all_measured: false,
            ..Default::default()
        };
        assert_eq!(
            partial.cost_line(),
            "review cost so far: unmeasured over 2 rounds"
        );

        let one = History {
            rounds: 1,
            cost_usd: 2.0,
            all_measured: true,
            ..Default::default()
        };
        assert_eq!(one.cost_line(), "review cost so far: $2.00 over 1 round");
    }
}
