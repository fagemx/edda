//! `edda verdict` — issue a verdict on a gated subject (GH-519 D1).
//!
//! An external actor approves or rejects a subject bound to (subject, full
//! SHA); each verdict is a first-class `verdict.recorded` ledger event. The
//! conductor consumes verdicts, it does not own them. A verdict for a
//! mismatched SHA is findable but never satisfies a gate waiting on another
//! SHA. `<subject>` is a free-form string; for conductor gates it is
//! `<plan-name>/<phase-id>`.

use clap::Subcommand;
use edda_core::event::new_verdict_event;
use edda_core::secret_guard::redact;
use edda_core::{VerdictDecision, VerdictPayload};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

#[derive(Debug, Subcommand)]
pub enum VerdictCmd {
    /// Approve a gated subject — the waiting gate may resume
    Approve {
        /// Free-form subject; for conductor gates `<plan-name>/<phase-id>`
        subject: String,
        /// Full 40-hex git SHA the verdict applies to
        #[arg(long)]
        sha: String,
        /// Optional context recorded with the approval
        #[arg(long)]
        comment: Option<String>,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Reject a gated subject — the comment feeds back into the gated agent session
    Reject {
        /// Free-form subject; for conductor gates `<plan-name>/<phase-id>`
        subject: String,
        /// Full 40-hex git SHA the verdict applies to
        #[arg(long)]
        sha: String,
        /// Required: why the gate was rejected (fed back to the agent as the next turn)
        #[arg(long)]
        comment: String,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
}

/// Arguments for recording a verdict (mirrors the `verdict.recorded` payload).
pub struct RecordVerdictArgs<'a> {
    pub subject: &'a str,
    pub decision: VerdictDecision,
    pub sha: &'a str,
    pub comment: Option<&'a str>,
    pub cli_session: Option<&'a str>,
}

#[derive(Debug)]
pub struct RecordOutcome {
    pub event_id: String,
    pub decision: VerdictDecision,
}

/// A verdict binds to a full 40-hex git SHA. A short or malformed SHA would
/// silently never match a gate's `gate_sha`, so it is refused at the door.
pub(crate) fn validate_sha(sha: &str) -> anyhow::Result<()> {
    if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        anyhow::bail!("--sha must be a full 40-hex git SHA (got \"{sha}\")")
    }
}

pub fn run(cmd: VerdictCmd, repo_root: &Path) -> anyhow::Result<()> {
    let (subject, sha, decision, comment, cli_session) = match cmd {
        VerdictCmd::Approve {
            subject,
            sha,
            comment,
            session,
        } => (subject, sha, VerdictDecision::Approved, comment, session),
        VerdictCmd::Reject {
            subject,
            sha,
            comment,
            session,
        } => (
            subject,
            sha,
            VerdictDecision::Rejected,
            Some(comment),
            session,
        ),
    };
    let outcome = do_record(
        repo_root,
        &RecordVerdictArgs {
            subject: &subject,
            decision,
            sha: &sha,
            comment: comment.as_deref(),
            cli_session: cli_session.as_deref(),
        },
    )?;
    println!("Verdict recorded: {} {subject} @ {sha}", outcome.decision);
    println!("  event: {}", outcome.event_id);
    if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
        println!("  comment: {c}");
    }
    Ok(())
}

pub(crate) fn do_record(
    repo_root: &Path,
    args: &RecordVerdictArgs<'_>,
) -> anyhow::Result<RecordOutcome> {
    validate_sha(args.sha)?;
    if args.subject.trim().is_empty() {
        anyhow::bail!("subject must not be empty");
    }
    // Comment is REQUIRED on reject: it is what feeds back into the gated
    // agent session as its next turn. Enforced here, not just at the CLI
    // layer, so every writer of the ledger object honors the invariant.
    if args.decision == VerdictDecision::Rejected
        && args.comment.is_none_or(|c| c.trim().is_empty())
    {
        anyhow::bail!(
            "a rejection without a comment does not exist — the comment is what feeds \
             back into the gated agent session"
        );
    }

    // EDDA-SECRET-GUARD1 q331: scrub the comment before any persistence,
    // same deterministic zero-LLM pass decide/note run.
    let comment = match args.comment {
        Some(c) => {
            let (safe, hits) = redact(c);
            if !hits.is_empty() {
                eprintln!(
                    "⚠ secret-guard: redacted {} secret pattern(s) before writing verdict",
                    hits.len()
                );
            }
            Some(safe)
        }
        None => None,
    };

    // Same identity source as other edda writes (decide/claim/request).
    let project_id = edda_store::project_id(repo_root);
    let (_session_id, label) =
        crate::cmd_bridge::resolve_session_id(args.cli_session, &project_id, "cli")?;

    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let payload = VerdictPayload {
        subject: args.subject.to_string(),
        decision: args.decision,
        sha: args.sha.to_string(),
        comment: comment.filter(|c| !c.trim().is_empty()),
        actor: label,
    };
    let event = new_verdict_event(&branch, parent_hash.as_deref(), &payload)?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(RecordOutcome {
        event_id: event.event_id,
        decision: args.decision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(clap::Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: VerdictCmd,
    }

    fn parse(args: &[&str]) -> Result<VerdictCmd, clap::Error> {
        use clap::Parser;
        let cli = TestCli::try_parse_from(std::iter::once("edda").chain(args.iter().copied()))?;
        Ok(cli.cmd)
    }

    fn sha(ch: char) -> String {
        std::iter::repeat_n(ch, 40).collect()
    }

    fn temp_ws(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("edda_cmdverdict_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Ledger::ensure_initialized(&dir).unwrap();
        dir
    }

    // ── CLI parse ──

    #[test]
    fn parses_approve_with_optional_comment() {
        let cmd = parse(&["approve", "plan-x/phase-1", "--sha", &sha('a')]).unwrap();
        match cmd {
            VerdictCmd::Approve {
                subject,
                sha: s,
                comment,
                ..
            } => {
                assert_eq!(subject, "plan-x/phase-1");
                assert_eq!(s, sha('a'));
                assert_eq!(comment, None);
            }
            other => panic!("expected approve, got {other:?}"),
        }
    }

    #[test]
    fn parses_reject_with_comment() {
        let cmd = parse(&[
            "reject",
            "plan-x/phase-1",
            "--sha",
            &sha('b'),
            "--comment",
            "tests fail",
        ])
        .unwrap();
        match cmd {
            VerdictCmd::Reject {
                subject, comment, ..
            } => {
                assert_eq!(subject, "plan-x/phase-1");
                assert_eq!(comment, "tests fail");
            }
            other => panic!("expected reject, got {other:?}"),
        }
    }

    #[test]
    fn reject_requires_comment_at_parse_time() {
        let err = parse(&["reject", "plan-x/phase-1", "--sha", &sha('b')])
            .expect_err("missing --comment must be a parse error");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn short_sha_is_refused() {
        let ws = temp_ws("shortsha");
        let err = do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Approved,
                sha: "abc123",
                comment: None,
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("40-hex"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    // ── Write/read roundtrip ──

    #[test]
    fn approve_roundtrip_is_queryable() {
        let ws = temp_ws("roundtrip");
        let s = sha('a');
        let outcome = do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Approved,
                sha: &s,
                comment: Some("looks good"),
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap();
        assert_eq!(outcome.decision, VerdictDecision::Approved);
        assert!(outcome.event_id.starts_with("evt_"));

        let ledger = Ledger::open(&ws).unwrap();
        let verdict = ledger
            .latest_verdict("plan-x/phase-1", &s)
            .unwrap()
            .expect("verdict should be found");
        assert_eq!(verdict.payload.decision, VerdictDecision::Approved);
        assert!(!verdict.payload.actor.is_empty());
        assert_eq!(verdict.payload.comment.as_deref(), Some("looks good"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sha_mismatch_does_not_satisfy_query() {
        let ws = temp_ws("mismatch");
        let s = sha('a');
        do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Approved,
                sha: &s,
                comment: None,
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap();

        let ledger = Ledger::open(&ws).unwrap();
        // The verdict is findable in the listing...
        assert_eq!(ledger.iter_verdicts().unwrap().len(), 1);
        // ...but never satisfies a gate waiting on a different SHA.
        let other = sha('b');
        assert!(ledger
            .latest_verdict("plan-x/phase-1", &other)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn reject_without_comment_is_refused() {
        let ws = temp_ws("rejectnocomment");
        let s = sha('c');
        let err = do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Rejected,
                sha: &s,
                comment: Some("   "),
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("comment"), "unexpected error: {err}");
        let ledger = Ledger::open(&ws).unwrap();
        assert_eq!(ledger.iter_verdicts().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn later_verdict_supersedes_for_same_subject_and_sha() {
        let ws = temp_ws("supersede");
        let s = sha('d');
        do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Rejected,
                sha: &s,
                comment: Some("no"),
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap();
        do_record(
            &ws,
            &RecordVerdictArgs {
                subject: "plan-x/phase-1",
                decision: VerdictDecision::Approved,
                sha: &s,
                comment: None,
                cli_session: Some("test-session-id"),
            },
        )
        .unwrap();

        let ledger = Ledger::open(&ws).unwrap();
        let latest = ledger
            .latest_verdict("plan-x/phase-1", &s)
            .unwrap()
            .unwrap();
        assert_eq!(latest.payload.decision, VerdictDecision::Approved);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
