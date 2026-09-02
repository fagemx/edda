//! Verdict ledger object (GH-519 D1) — read-side helpers for
//! `verdict.recorded` events.
//!
//! A verdict is written by `edda verdict approve|reject` (see edda-cli
//! `cmd_verdict`) and consumed by external readers such as the conductor's
//! AWAITING_VERDICT wait. The conductor consumes verdicts; it does not own
//! them. Every verdict is bound to `(subject, sha)`: a verdict recorded for
//! one SHA remains findable but never matches a query for another SHA.

use edda_core::{Event, VerdictPayload};

/// One parsed `verdict.recorded` event.
#[derive(Debug, Clone, PartialEq)]
pub struct VerdictRecord {
    pub event_id: String,
    pub ts: String,
    pub payload: VerdictPayload,
}

/// Parse a `verdict.recorded` event into a [`VerdictRecord`].
/// Returns `None` for any other event type or a malformed payload.
pub fn parse_verdict_event(event: &Event) -> Option<VerdictRecord> {
    if event.event_type != "verdict.recorded" {
        return None;
    }
    let payload: VerdictPayload = serde_json::from_value(event.payload.clone()).ok()?;
    Some(VerdictRecord {
        event_id: event.event_id.clone(),
        ts: event.ts.clone(),
        payload,
    })
}

/// The latest verdict (by ledger insertion order) for `(subject, sha)`.
///
/// Events are expected in insertion (rowid) order, which is the ledger's
/// reliable chronology — RFC3339 strings with mixed precision do not sort
/// chronologically. Later verdicts for the same `(subject, sha)` supersede
/// earlier ones.
pub fn latest_verdict<'a>(
    verdicts: &'a [VerdictRecord],
    subject: &str,
    sha: &str,
) -> Option<&'a VerdictRecord> {
    verdicts
        .iter()
        .rev()
        .find(|v| v.payload.subject == subject && v.payload.sha == sha)
}

/// Parse an RFC3339 timestamp; `None` on failure.
fn parse_rfc3339(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// The latest verdict for `(subject, sha)` that was **recorded after**
/// `not_before` (an RFC3339 timestamp, e.g. a gate's `gate_entered_at`) —
/// GH-519 D6 verdict freshness.
///
/// A redispatch turn is not guaranteed to produce a commit, so a re-entered
/// gate can wait on the *same* `(subject, gate_sha)` as the previous one;
/// without the freshness bound the stale rejected verdict still sitting in
/// the ledger would re-satisfy the gate forever. Comparison is done on
/// parsed instants, not string order (mixed-precision RFC3339 does not sort
/// chronologically).
///
/// `None` for `not_before` is an unbounded query (freshness not in force) —
/// the latest matching verdict wins. A **present but unparsable** bound
/// fails CLOSED (GH-541): no verdict satisfies, because admitting every
/// verdict regardless of age is the loop-prone direction D6 exists to kill
/// — the opposite of the sibling rule below, where an unparsable verdict
/// timestamp never satisfies a bounded query. The bound reaches this
/// predicate from a JSON state file on disk; a corrupt or hand-edited value
/// must degrade to "gate stays shut", not "gate admits anything".
pub fn latest_verdict_fresh<'a>(
    verdicts: &'a [VerdictRecord],
    subject: &str,
    sha: &str,
    not_before: Option<&str>,
) -> Option<&'a VerdictRecord> {
    let Some(raw_bound) = not_before else {
        // Unbounded query: same semantics as [`latest_verdict`].
        return latest_verdict(verdicts, subject, sha);
    };
    // Fail closed on an unparsable bound (GH-541): no verdict satisfies.
    let bound = parse_rfc3339(raw_bound)?;
    verdicts.iter().rev().find(|v| {
        v.payload.subject == subject
            && v.payload.sha == sha
            && parse_rfc3339(&v.ts).is_some_and(|ts| ts > bound)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_verdict_event;
    use edda_core::{VerdictDecision, VerdictPayload};

    fn verdict_payload(subject: &str, sha: &str, decision: VerdictDecision) -> VerdictPayload {
        VerdictPayload {
            subject: subject.to_string(),
            decision,
            sha: sha.to_string(),
            comment: None,
            actor: "tester".to_string(),
        }
    }

    fn append(ledger: &crate::Ledger, subject: &str, sha: &str, decision: VerdictDecision) {
        let branch = ledger.head_branch().unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let event = new_verdict_event(
            &branch,
            parent_hash.as_deref(),
            &verdict_payload(subject, sha, decision),
        )
        .unwrap();
        ledger.append_event(&event).unwrap();
    }

    fn temp_ledger(name: &str) -> (std::path::PathBuf, crate::Ledger) {
        let dir = std::env::temp_dir().join(format!("edda_verdict_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = crate::Ledger::open_or_init(&dir).unwrap();
        (dir, ledger)
    }

    #[test]
    fn parse_ignores_other_event_types() {
        let dir = std::env::temp_dir().join(format!("edda_verdict_parse_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = crate::Ledger::open_or_init(&dir).unwrap();
        let event = edda_core::event::new_note_event(
            &ledger.head_branch().unwrap(),
            None,
            "user",
            "hi",
            &[],
        )
        .unwrap();
        ledger.append_event(&event).unwrap();
        let events = ledger.iter_events().unwrap();
        assert!(!events.is_empty());
        assert!(parse_verdict_event(&events[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_write_then_read_by_subject_and_sha() {
        let (_dir, ledger) = temp_ledger("roundtrip");
        let sha = "a".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Approved);

        let verdicts = ledger.iter_verdicts().unwrap();
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].payload.subject, "plan-x/phase-1");
        assert_eq!(verdicts[0].payload.sha, sha);
        assert_eq!(verdicts[0].payload.decision, VerdictDecision::Approved);

        let latest = ledger.latest_verdict("plan-x/phase-1", &sha).unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().payload.decision, VerdictDecision::Approved);
    }

    #[test]
    fn sha_mismatch_never_matches() {
        let (_dir, ledger) = temp_ledger("shamismatch");
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha_a, VerdictDecision::Approved);

        // Findable as a general listing, but never satisfies another SHA.
        assert_eq!(ledger.iter_verdicts().unwrap().len(), 1);
        assert!(ledger
            .latest_verdict("plan-x/phase-1", &sha_b)
            .unwrap()
            .is_none());
    }

    #[test]
    fn latest_insertion_order_wins_not_timestamp_string() {
        let (_dir, ledger) = temp_ledger("latestwins");
        let sha = "c".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Rejected);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Approved);

        let latest = ledger
            .latest_verdict("plan-x/phase-1", &sha)
            .unwrap()
            .expect("a verdict exists");
        assert_eq!(latest.payload.decision, VerdictDecision::Approved);
    }

    #[test]
    fn different_subject_does_not_match() {
        let (_dir, ledger) = temp_ledger("subject");
        let sha = "d".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Approved);
        assert!(ledger
            .latest_verdict("plan-x/phase-2", &sha)
            .unwrap()
            .is_none());
    }

    #[test]
    fn stale_verdict_does_not_satisfy_fresh_query() {
        // D6: a verdict recorded BEFORE the bound must not satisfy a query
        // for the same (subject, sha) — the re-entered-gate scenario.
        let (_dir, ledger) = temp_ledger("stale");
        let sha = "e".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Rejected);

        let verdicts = ledger.iter_verdicts().unwrap();
        let after = time::OffsetDateTime::now_utc()
            .checked_add(time::Duration::minutes(1))
            .unwrap();
        let not_before = after
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        assert!(
            latest_verdict_fresh(&verdicts, "plan-x/phase-1", &sha, Some(&not_before)).is_none()
        );
        // Unbounded query still finds it (findable ≠ satisfying).
        assert!(latest_verdict(&verdicts, "plan-x/phase-1", &sha).is_some());
    }

    #[test]
    fn fresh_verdict_satisfies_fresh_query() {
        let (_dir, ledger) = temp_ledger("fresh");
        let sha = "f".repeat(40);
        append(&ledger, "plan-x/phase-1", &sha, VerdictDecision::Approved);

        let verdicts = ledger.iter_verdicts().unwrap();
        let before = time::OffsetDateTime::now_utc()
            .checked_sub(time::Duration::minutes(1))
            .unwrap();
        let not_before = before
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let fresh = latest_verdict_fresh(&verdicts, "plan-x/phase-1", &sha, Some(&not_before))
            .expect("verdict recorded after the bound must satisfy");
        assert_eq!(fresh.payload.decision, VerdictDecision::Approved);
    }

    #[test]
    fn fresh_query_compares_instants_not_strings() {
        // "…9Z" (0 fractional digits) sorts after "…0.1Z" as a string but is
        // an EARLIER instant; the fresh query must use parsed instants.
        let sha = "1".repeat(40);
        let mk = |ts: &str| VerdictRecord {
            event_id: "ev".into(),
            ts: ts.into(),
            payload: VerdictPayload {
                subject: "s".into(),
                decision: VerdictDecision::Rejected,
                sha: sha.clone(),
                comment: None,
                actor: "tester".into(),
            },
        };
        // 12:00:00.100Z (verdict) vs bound 12:00:00Z → later instant.
        let verdicts = vec![mk("2026-01-01T12:00:00.1Z")];
        assert!(latest_verdict_fresh(&verdicts, "s", &sha, Some("2026-01-01T12:00:00Z")).is_some());
        // Bound 12:00:01Z (string-smaller) is a LATER instant → stale.
        assert!(latest_verdict_fresh(&verdicts, "s", &sha, Some("2026-01-01T12:00:01Z")).is_none());
    }

    /// GH-541: a present-but-unparsable bound fails CLOSED — no verdict
    /// satisfies, even one that matches (subject, sha). Admitting every
    /// verdict regardless of age is the loop-prone direction D6 kills.
    #[test]
    fn fresh_query_unparsable_not_before_fails_closed() {
        let sha = "2".repeat(40);
        let verdicts = vec![VerdictRecord {
            event_id: "ev".into(),
            ts: "2026-01-01T12:00:00Z".into(),
            payload: VerdictPayload {
                subject: "s".into(),
                decision: VerdictDecision::Approved,
                sha: sha.clone(),
                comment: None,
                actor: "tester".into(),
            },
        }];
        assert!(latest_verdict_fresh(&verdicts, "s", &sha, Some("not-a-timestamp")).is_none());
    }

    /// GH-541: `None` remains the unbounded query — freshness not in force,
    /// the latest matching verdict wins (no regression in the no-bound case).
    #[test]
    fn fresh_query_none_bound_stays_unbounded() {
        let sha = "3".repeat(40);
        let verdicts = vec![VerdictRecord {
            event_id: "ev".into(),
            ts: "not-a-timestamp-either".into(),
            payload: VerdictPayload {
                subject: "s".into(),
                decision: VerdictDecision::Approved,
                sha: sha.clone(),
                comment: None,
                actor: "tester".into(),
            },
        }];
        let found = latest_verdict_fresh(&verdicts, "s", &sha, None)
            .expect("unbounded query must find the matching verdict");
        assert_eq!(found.payload.decision, VerdictDecision::Approved);
    }
}
