//! `edda review gate <sha>` — the union rule and the window check, in the
//! product (GH-769).
//!
//! Two questions decide whether a reviewed SHA has passed, and both are
//! verdict semantics rather than forge glue:
//!
//! - **Union.** While any non-qualifying verdict stands on the SHA, the answer
//!   is no. A later LGTM never overrides an earlier Changes Requested on the
//!   same SHA (GH-742).
//! - **Window.** If the base advanced over a file the PR changed, the reviewed
//!   tree is no longer the tree that would merge.
//!
//! `scripts/pr-review-watch.sh` carried both in shell, marked `D8-debt(#769)`.
//! This module is where that debt is paid: one rule, one implementation.
//!
//! ## Two sources, one rule
//!
//! The rule is a pure function over [`Standing`] values. Two adapters produce
//! them:
//!
//! - the ledger's `review_verdict` events (default), and
//! - `--verdicts` (a file, or `-` for stdin) in the tab-separated shape the
//!   watcher already derives from §7 verdict comments.
//!
//! The second exists because the verdict *source* is a different debt from the
//! union *rule*. `D8-debt(#671)` — reading verdicts out of PR comments — stands
//! until the ledger carries verdicts across machines, and it does not yet:
//! `review_verdict` is not among the event types the committed mirror imports.
//! A fleet that reviews on one machine and merges from another would, on the
//! ledger alone, see no verdict at all. Feeding the caller's facts to the same
//! rule keeps that correct today without a second copy of the rule, and the
//! ledger path is ready for the day #671 is paid.
//!
//! This verb never writes the ledger, never launches a lane, and never touches
//! GitHub.

use super::git::git;
use anyhow::{Context, Result};
use edda_core::ReviewVerdictPayload;
use edda_ledger::Ledger;
use std::path::Path;

/// Flags for `edda review gate <sha>`.
#[derive(clap::Args)]
pub struct GateArgs {
    /// The reviewed commit, as a full 40-character SHA
    #[arg(value_name = "SHA")]
    pub sha: String,
    /// Comparison base; when given, the review window is checked too
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,
    /// Read verdict facts from this path (`-` for stdin) instead of the ledger
    ///
    /// One record per line, tab separated: `<verdict>\t<p0>\t<p1>`, where
    /// verdict is `LGTM`, `Provisional`, or anything else (which cannot
    /// qualify). A missing or non-numeric count reads as non-zero.
    #[arg(long, value_name = "PATH")]
    pub verdicts: Option<String>,
    /// Emit the gate report as JSON
    #[arg(long)]
    pub json: bool,
}

/// One verdict standing on the reviewed SHA, reduced to what the rule needs.
#[derive(Debug, Clone)]
pub(crate) struct Standing {
    /// `lgtm` or anything else; only `lgtm` can contribute a pass.
    verdict: String,
    /// An unqualified LGTM is Provisional: pending, never a pass on its own.
    qualified: bool,
    /// `None` is an unstated count, which reads as non-zero (fail closed).
    p0: Option<u64>,
    p1: Option<u64>,
    reviewer_model: Option<String>,
    round: Option<u32>,
    cost_usd: Option<f64>,
    cost_measured: bool,
}

impl Standing {
    /// P0 and P1 are both stated and both zero.
    fn clean(&self) -> bool {
        self.p0 == Some(0) && self.p1 == Some(0)
    }
}

/// What the union rule says about the verdicts standing on one SHA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Union {
    /// At least one qualifying LGTM at P0=P1=0, and nothing standing against it.
    Pass,
    /// A non-qualifying verdict stands, or only Provisional rounds do.
    Fail,
    /// No verdict at all — not a judgement, an inability to make one.
    None,
}

/// The union rule.
///
/// A Provisional round (unqualified LGTM at P0=P1=0, REVIEW.md §6.4) is
/// *pending*: never a pass on its own, and once its escalation is adjudicated
/// it does not hold a later qualified LGTM on the same SHA at fail. With any
/// P0/P1 it is a standing non-qualifying verdict like any other (§8, GH-742).
pub(crate) fn union(standing: &[Standing]) -> Union {
    if standing.is_empty() {
        return Union::None;
    }
    let mut pass = false;
    for entry in standing {
        if entry.verdict == "lgtm" && entry.clean() {
            if entry.qualified {
                pass = true;
            }
            // A clean Provisional neither passes nor blocks.
            continue;
        }
        return Union::Fail;
    }
    if pass {
        Union::Pass
    } else {
        Union::Fail
    }
}

/// Did the base advance over a file this subject changed?
///
/// The reviewed tree is the merge of `sha` into `base` as it stood. If `base`
/// has since moved on any file the subject touches, that merge is a different
/// tree and the review no longer covers it.
///
/// Both arguments are resolved commits, not refs: two `git` calls read `base`
/// here, and a name that moved between them would compare two different trees.
fn window_moved(repo: &Path, sha: &str, base: &str) -> Result<bool> {
    let changed = git(repo, &["diff", "--name-only", &format!("{base}...{sha}")])?;
    let files: Vec<&str> = changed.lines().filter(|line| !line.is_empty()).collect();
    if files.is_empty() {
        return Ok(false);
    }
    let merge_base = git(repo, &["merge-base", sha, base])?;
    let range = format!("{merge_base}..{base}");
    let mut args = vec!["diff", "--name-only", range.as_str(), "--"];
    args.extend(files);
    Ok(!git(repo, &args)?.trim().is_empty())
}

/// Verdicts on `sha`, from the ledger's `review_verdict` events.
///
/// An `unreviewed` event is not a verdict: it records that a review could not
/// be made. Skipping it is what makes "no verdict at all" reachable, and it
/// matches how `subject::history` reads the same events.
///
/// A payload that cannot be deserialized is fatal rather than skipped: a gate
/// that silently drops the event it cannot read is a gate that passes because
/// it went blind.
fn from_ledger(repo: &Path, sha: &str) -> Result<Vec<Standing>> {
    let ledger = Ledger::open(repo)?;
    let mut standing = Vec::new();
    for event in ledger.iter_events_by_type("review_verdict")? {
        let payload: ReviewVerdictPayload = serde_json::from_value(event.payload.clone())
            .with_context(|| format!("review_verdict event {}", event.event_id))?;
        if payload.verdict == "unreviewed" || payload.subject.head_sha != sha {
            continue;
        }
        let count = |severity: &str| {
            payload
                .findings
                .iter()
                .filter(|finding| finding.severity == severity)
                .count() as u64
        };
        standing.push(Standing {
            p0: Some(count("P0")),
            p1: Some(count("P1")),
            qualified: payload.qualified,
            verdict: payload.verdict.clone(),
            reviewer_model: Some(payload.reviewer.model_observed.clone()),
            round: payload.refs.round,
            cost_usd: payload.cost.usd,
            cost_measured: payload.cost.measured,
        });
    }
    Ok(standing)
}

/// Verdicts supplied by the caller, in the watcher's tab-separated shape.
pub(crate) fn from_lines(text: &str) -> Vec<Standing> {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let label = fields.next().unwrap_or("").trim();
            let count = |field: Option<&str>| field.and_then(|v| v.trim().parse::<u64>().ok());
            let (verdict, qualified) = match label {
                "LGTM" => ("lgtm", true),
                "Provisional" => ("lgtm", false),
                other => (other, false),
            };
            Standing {
                verdict: verdict.to_owned(),
                qualified,
                p0: count(fields.next()),
                p1: count(fields.next()),
                reviewer_model: None,
                round: None,
                cost_usd: None,
                cost_measured: false,
            }
        })
        .collect()
}

fn report(sha: &str, standing: &[Standing], outcome: Union, reason: Option<&str>) -> String {
    let verdicts: Vec<serde_json::Value> = standing
        .iter()
        .map(|entry| {
            serde_json::json!({
                "verdict": entry.verdict,
                "qualified": entry.qualified,
                "p0": entry.p0,
                "p1": entry.p1,
                "reviewer_model": entry.reviewer_model,
                "round": entry.round,
                // A cost nobody measured is `unmeasured`, never 0 — printing a
                // zero would claim a free round that was simply not weighed.
                "cost_usd": match (entry.cost_measured, entry.cost_usd) {
                    (true, Some(usd)) => serde_json::json!(usd),
                    _ => serde_json::json!("unmeasured"),
                },
            })
        })
        .collect();
    serde_json::json!({
        "sha": sha,
        "state": match outcome {
            Union::Pass => "pass",
            Union::Fail => "fail",
            Union::None => "none",
        },
        "reason": reason,
        "verdicts": verdicts,
    })
    .to_string()
}

/// CLI entry point. Exit: 0 pass, 1 fail, 2 cannot judge.
pub fn run(args: GateArgs, cwd: &Path) -> Result<()> {
    if args.sha.len() != 40 || !args.sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        eprintln!("edda review gate: expected a full 40-character SHA");
        std::process::exit(2);
    }
    // The repo is resolved only where it is needed: `--verdicts` judges facts
    // the caller supplies, so it must work outside a checkout too.
    let standing = match args.verdicts.as_deref() {
        Some("-") => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .context("read verdicts from stdin")?;
            from_lines(&text)
        }
        Some(path) => from_lines(
            &std::fs::read_to_string(path).with_context(|| format!("read verdicts {path}"))?,
        ),
        None => from_ledger(&super::git::repo_root_from(cwd)?, &args.sha)?,
    };

    let mut outcome = union(&standing);
    let mut reason = match outcome {
        Union::Fail => Some("union"),
        _ => None,
    };
    // The window is only meaningful once the verdicts themselves pass: a SHA
    // that has not been approved is not blocked *because* its base moved.
    if outcome == Union::Pass {
        if let Some(base) = args.base.as_deref() {
            // Resolve the base through this module's own guard: `commit` runs
            // `rev-parse --verify --end-of-options` and returns a full SHA, so
            // a ref shaped like a flag cannot reach `git` as one, and the
            // ranges below name one commit rather than whatever the ref means
            // by the time the second `git` call runs.
            let repo = super::git::repo_root_from(cwd)?;
            let base = super::git::commit(&repo, base)?;
            if window_moved(&repo, &args.sha, &base)? {
                outcome = Union::Fail;
                reason = Some("window");
            }
        }
    }

    if args.json {
        println!("{}", report(&args.sha, &standing, outcome, reason));
    } else {
        match (outcome, reason) {
            (Union::Pass, _) => println!("PASS {} verdicts={}", args.sha, standing.len()),
            (Union::Fail, reason) => {
                println!("FAIL {} {}", args.sha, reason.unwrap_or("union"))
            }
            (Union::None, _) => println!("NONE {}", args.sha),
        }
    }
    match outcome {
        Union::Pass => Ok(()),
        Union::Fail => std::process::exit(1),
        Union::None => std::process::exit(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_review::git::testrepo;

    fn standing(verdict: &str, qualified: bool, p0: Option<u64>, p1: Option<u64>) -> Standing {
        Standing {
            verdict: verdict.into(),
            qualified,
            p0,
            p1,
            reviewer_model: None,
            round: None,
            cost_usd: None,
            cost_measured: false,
        }
    }

    fn lgtm() -> Standing {
        standing("lgtm", true, Some(0), Some(0))
    }

    fn provisional() -> Standing {
        standing("lgtm", false, Some(0), Some(0))
    }

    fn changes() -> Standing {
        standing("changes-requested", false, Some(1), Some(0))
    }

    #[test]
    fn union_matrix_matches_the_rule_the_shell_carried() {
        assert_eq!(union(&[]), Union::None);
        assert_eq!(union(&[lgtm()]), Union::Pass);
        assert_eq!(union(&[changes()]), Union::Fail);
        // GH-742: a later LGTM never overrides an earlier Changes Requested.
        assert_eq!(union(&[changes(), lgtm()]), Union::Fail);
        assert_eq!(union(&[lgtm(), changes()]), Union::Fail);
        // An LGTM carrying a blocking finding is not clean.
        assert_eq!(
            union(&[standing("lgtm", true, Some(0), Some(1))]),
            Union::Fail
        );
        assert_eq!(
            union(&[standing("lgtm", true, Some(1), Some(0))]),
            Union::Fail
        );
        // An unstated count reads as non-zero.
        assert_eq!(union(&[standing("lgtm", true, None, Some(0))]), Union::Fail);
    }

    #[test]
    fn a_provisional_round_is_pending_not_a_verdict_either_way() {
        // Never a pass on its own...
        assert_eq!(union(&[provisional()]), Union::Fail);
        // ...and does not hold a later qualified LGTM on the same SHA at fail.
        assert_eq!(union(&[provisional(), lgtm()]), Union::Pass);
        // With a blocking finding it is a standing non-qualifying verdict —
        // alone, and holding a later qualified LGTM on the same SHA at fail.
        assert_eq!(
            union(&[standing("lgtm", false, Some(0), Some(2))]),
            Union::Fail
        );
        assert_eq!(
            union(&[standing("lgtm", false, Some(0), Some(2)), lgtm()]),
            Union::Fail
        );
    }

    #[test]
    fn caller_supplied_lines_parse_into_the_same_rule() {
        assert_eq!(union(&from_lines("LGTM\t0\t0\n")), Union::Pass);
        assert_eq!(union(&from_lines("Changes Requested\t1\t0\n")), Union::Fail);
        assert_eq!(union(&from_lines("Provisional\t0\t0\n")), Union::Fail);
        assert_eq!(
            union(&from_lines("Provisional\t0\t0\nLGTM\t0\t0\n")),
            Union::Pass
        );
        // Missing and non-numeric counts read as non-zero.
        assert_eq!(union(&from_lines("LGTM\t\t\n")), Union::Fail);
        assert_eq!(union(&from_lines("LGTM\tmany\t0\n")), Union::Fail);
        // Blank lines and CRLF are not records.
        assert_eq!(union(&from_lines("\r\n\nLGTM\t0\t0\r\n")), Union::Pass);
        assert_eq!(union(&from_lines("   \n")), Union::None);
    }

    #[test]
    fn window_is_clear_until_the_base_moves_on_a_changed_file() {
        let (_temp, root) = testrepo::init();
        testrepo::commit_file(&root, "shared.txt", "base\n", "base file");
        let base_start = testrepo::run(&root, &["rev-parse", "HEAD"]);
        testrepo::run(&root, &["checkout", "-q", "-b", "subject"]);
        let sha = testrepo::commit_file(&root, "shared.txt", "subject\n", "subject edit");
        testrepo::run(&root, &["checkout", "-q", "main"]);

        // The base has not moved at all.
        assert!(!window_moved(&root, &sha, &base_start).unwrap());

        // The base moves, but only on a file the subject never touched.
        testrepo::commit_file(&root, "unrelated.txt", "other\n", "unrelated advance");
        assert!(!window_moved(&root, &sha, "main").unwrap());

        // The base moves on the file the subject changed.
        testrepo::commit_file(&root, "shared.txt", "moved\n", "base advance");
        assert!(window_moved(&root, &sha, "main").unwrap());
    }

    /// Build a `review_verdict` payload through serde so the fixture cannot
    /// drift from the struct the product actually writes.
    fn payload(head: &str, verdict: &str, qualified: bool) -> ReviewVerdictPayload {
        serde_json::from_value(serde_json::json!({
            "schema": "review_verdict/0",
            "subject": {
                "base_sha": "base", "head_sha": head,
                "files": 1, "lines": 1, "coverage": "full",
            },
            "refs": { "round": 2 },
            "spec": { "mode": "spec-backed", "source": "issue#769", "trust": "local" },
            "brief": { "core": "scope", "classes": [] },
            "reviewer": {
                "agent": "pi", "transport": "rpc",
                "model_requested": "openai-codex/gpt-5.6-sol",
                "model_observed": "openai-codex/gpt-5.6-sol",
                "observed_via": "provider",
                "session_id": "s", "session_label": "review", "tool_policy": "hard",
            },
            "independence": "verified",
            "independence_policy": "session",
            "gates": { "status": "verified", "declared_by": [], "read": [], "ran": [] },
            "verdict": verdict,
            "outcome": "done",
            "qualified": qualified,
            "cost": { "usd": 2.17, "measured": true, "duration_ms": 1 },
            "parse": "ok",
        }))
        .expect("review_verdict payload fixture")
    }

    fn append(ledger: &edda_ledger::Ledger, payload: &ReviewVerdictPayload) {
        let event = edda_core::event::new_review_verdict_event(
            "main",
            ledger.last_event_hash().unwrap().as_deref(),
            payload,
            None,
            None,
            &[],
        )
        .expect("verdict event");
        ledger.append_event(&event).expect("append verdict");
    }

    #[test]
    fn the_ledger_source_reads_only_the_verdicts_standing_on_this_sha() {
        let (_temp, root) = testrepo::init();
        let ledger = edda_ledger::Ledger::open_or_init(&root).expect("ledger");
        let sha = "a".repeat(40);
        let other = "b".repeat(40);

        append(&ledger, &payload(&sha, "lgtm", true));
        // Another SHA's verdict is not this SHA's business.
        append(&ledger, &payload(&other, "changes-requested", false));
        // An `unreviewed` event records that no review could be made. Counting
        // it would make "no verdict at all" unreachable, and `subject::history`
        // skips it for the same reason.
        append(&ledger, &payload(&sha, "unreviewed", false));

        let standing = from_ledger(&root, &sha).expect("read ledger");
        assert_eq!(standing.len(), 1, "{standing:?}");
        assert_eq!(standing[0].verdict, "lgtm");
        assert_eq!(standing[0].round, Some(2));
        assert_eq!(
            standing[0].reviewer_model.as_deref(),
            Some("openai-codex/gpt-5.6-sol")
        );
        assert_eq!(union(&standing), Union::Pass);
    }

    #[test]
    fn a_self_check_is_structurally_invisible_to_the_gate() {
        // `pr-review-loop` self-checks are PR comments, never `review_verdict`
        // events. The gate cannot mistake one for a verdict because it never
        // looks anywhere a self-check can appear — so a PR carrying nothing
        // but a self-check is `NONE`, which the caller maps to exit 2.
        let (_temp, root) = testrepo::init();
        let ledger = edda_ledger::Ledger::open_or_init(&root).expect("ledger");
        let argv = ["edda", "review"].map(str::to_owned);
        let event = edda_core::event::new_cmd_event_with_git_context(
            &edda_core::event::CmdEventParams {
                branch: "main",
                parent_hash: ledger.last_event_hash().unwrap().as_deref(),
                argv: &argv,
                cwd: root.to_str().unwrap(),
                exit_code: 0,
                duration_ms: 1,
                stdout_blob: "",
                stderr_blob: "",
            },
            None,
            Some(false),
        )
        .expect("cmd event");
        ledger.append_event(&event).expect("append cmd event");

        let standing = from_ledger(&root, &"c".repeat(40)).expect("read ledger");
        assert!(standing.is_empty(), "{standing:?}");
        assert_eq!(union(&standing), Union::None);
    }

    #[test]
    fn json_reports_an_unmeasured_cost_rather_than_zero() {
        let mut measured = lgtm();
        measured.cost_usd = Some(2.17);
        measured.cost_measured = true;
        measured.reviewer_model = Some("openai-codex/gpt-5.6-sol".into());
        measured.round = Some(2);
        let text = report(
            "a".repeat(40).as_str(),
            &[measured, lgtm()],
            Union::Pass,
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["state"], "pass");
        assert_eq!(value["verdicts"][0]["cost_usd"], 2.17);
        assert_eq!(
            value["verdicts"][0]["reviewer_model"],
            "openai-codex/gpt-5.6-sol"
        );
        assert_eq!(value["verdicts"][0]["round"], 2);
        assert_eq!(value["verdicts"][1]["cost_usd"], "unmeasured");
    }
}
