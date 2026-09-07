//! The one liveness criterion for peer sessions (GH-617).
//!
//! A session is live exactly when its last heartbeat is no older than
//! [`stale_secs`] (x15 for parented sub-agents, mirroring peer discovery).
//! `edda peers` and `edda claim check` must both go through this module so
//! the two verbs can never grow a second, disagreeing notion of "dead".
//! Session inference (`discovery::infer_session_id`) observes the same
//! criterion (GH-705), so a session that is live for those two verbs is
//! inferable there too.
//!
//! There is one liveness surface — the heartbeat file — and one criterion —
//! [`liveness_from_heartbeat`]. Do not add a parallel one.
//!
//! ## Claims are a second fact, not a second criterion
//!
//! A board claim carries its own timestamp, and for a bare CLI session
//! (`cli-*`) that timestamp is the only judgeable thing about it: nothing ever
//! refreshes a heartbeat for a one-shot process, so the criterion above can
//! only ever call it dead (GH-705, GH-1018). [`claim_age_secs_at`] is the one
//! place that age is computed.
//!
//! Two questions are asked of that one age, and they take **different**
//! thresholds on purpose:
//!
//! | Question | Function | Threshold |
//! |---|---|---|
//! | Is this claim fresh? (display) | [`claim_is_stale_at`] | [`stale_secs`] |
//! | May it still refuse a writer? | [`claim_guard_expired_at`] | [`claim_ttl_secs`] |
//!
//! Collapsing those into one threshold is not simplification, it is a bug:
//! [`stale_secs`] is calibrated for something refreshed every 30 seconds, and
//! a claim is written once. The rule against parallel criteria is about
//! answering one question two ways — not about refusing to notice that these
//! are two questions.

use serde::Serialize;

use super::claim_ttl_secs;
use super::read_heartbeat;
use super::stale_secs;
use crate::parse::now_rfc3339;
use edda_store::SessionHeartbeat;

use super::helpers::parse_rfc3339_to_epoch;

/// Liveness verdict for one session under the shared criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveness {
    /// Heartbeat no older than the staleness threshold.
    Live {
        /// Seconds since the last heartbeat.
        age_secs: u64,
    },
    /// Heartbeat older than the staleness threshold.
    Stale {
        /// Seconds since the last heartbeat.
        age_secs: u64,
    },
    /// No heartbeat file exists (or it is unreadable/unparseable): the
    /// session was never heard from.
    NoHeartbeat,
}

impl SessionLiveness {
    pub fn is_live(&self) -> bool {
        matches!(self, SessionLiveness::Live { .. })
    }
}

/// The shared criterion, pure over a heartbeat snapshot.
///
/// Mirrors peer discovery exactly: `age <= stale_secs()` is live, and a
/// parented sub-agent heartbeat gets the same 15x multiplier discovery
/// applies (no hook events fire during a sub-agent's run, so a heartbeat
/// written once at spawn would otherwise age out mid-run).
pub fn liveness_from_heartbeat(hb: &SessionHeartbeat, now_epoch: u64) -> SessionLiveness {
    let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
    let age = now_epoch.saturating_sub(hb_epoch);
    let stale_threshold = stale_secs();
    let effective_threshold = if hb.parent_session_id.is_some() {
        stale_threshold * 15
    } else {
        stale_threshold
    };
    if age > effective_threshold {
        SessionLiveness::Stale { age_secs: age }
    } else {
        SessionLiveness::Live { age_secs: age }
    }
}

/// Classify one session's liveness against a caller-supplied now (testable).
pub fn classify_session_liveness_at(
    project_id: &str,
    session_id: &str,
    now_epoch: u64,
) -> SessionLiveness {
    match read_heartbeat(project_id, session_id) {
        Some(hb) => liveness_from_heartbeat(&hb, now_epoch),
        None => SessionLiveness::NoHeartbeat,
    }
}

/// Classify one session's liveness using the current clock.
pub fn classify_session_liveness(project_id: &str, session_id: &str) -> SessionLiveness {
    classify_session_liveness_at(project_id, session_id, now_epoch())
}

/// The clock reading both criteria are measured against.
pub fn now_epoch() -> u64 {
    parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0)
}

/// Age of a board claim, from the claim's own recorded timestamp.
///
/// This is a different fact from the claimant session's heartbeat age. A
/// claim records when it was written; a heartbeat records when the session
/// was last heard from. For a bare CLI session (`cli-*`) only the first
/// exists — nothing ever refreshes a heartbeat for a one-shot process — so
/// the claim's own age is the only judgeable thing about it.
///
/// An unparseable timestamp reads as epoch 0, i.e. maximally old. That is the
/// same reading `edda peers --json` has always given it, and it fails towards
/// "this claim is ancient" rather than "this claim is fresh".
pub fn claim_age_secs_at(claim_ts: &str, now_epoch: u64) -> u64 {
    now_epoch.saturating_sub(parse_rfc3339_to_epoch(claim_ts).unwrap_or(0))
}

/// Is this board claim stale by its own timestamp?
///
/// The rule `edda peers --json` publishes for every claim (GH-569): older
/// than [`stale_secs`] is stale, so a 55-day-old zombie claim and a
/// 37-second-old live one are distinguishable to a program.
///
/// This answers a **display** question — "is this claim still fresh?" — and it
/// is not the question a write guard asks. See [`claim_guard_expired_at`].
pub fn claim_is_stale_at(claim_ts: &str, now_epoch: u64) -> bool {
    claim_age_secs_at(claim_ts, now_epoch) > stale_secs()
}

/// Has this board claim outlived the window in which it may refuse a writer?
///
/// Two questions look alike here and are not the same:
///
/// - *Is this claim fresh?* — [`claim_is_stale_at`], measured against
///   [`stale_secs`] (120s), the window a **heartbeat** is refreshed inside
///   (every 30s). Past it, nobody has been heard from lately.
/// - *May this claim still refuse a writer?* — this function. A board claim is
///   written **once** and never refreshed, so measuring it against a
///   refresh-calibrated window would expire a claim its owner is still working
///   under. An `edda claim` + `edda conduct run` session from the operator
///   runbook occupies its surface for minutes to hours; two minutes in, the
///   surface would silently open.
///
/// So the guard gets its own window, [`claim_ttl_secs`]. Both questions share
/// one age computation ([`claim_age_secs_at`]) and one place to read the
/// difference from; what they must not share is a threshold calibrated for the
/// other one's fact (GH-1018).
pub fn claim_guard_expired_at(claim_ts: &str, now_epoch: u64) -> bool {
    claim_age_secs_at(claim_ts, now_epoch) > claim_ttl_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heartbeat(last_heartbeat: &str, parent: Option<&str>) -> SessionHeartbeat {
        SessionHeartbeat {
            session_id: "s".into(),
            started_at: last_heartbeat.into(),
            last_heartbeat: last_heartbeat.into(),
            label: "l".into(),
            focus_files: vec![],
            active_tasks: vec![],
            files_modified_count: 0,
            total_edits: 0,
            recent_commits: vec![],
            branch: None,
            current_phase: None,
            parent_session_id: parent.map(str::to_string),
            plan: None,
            phase: None,
            attempt: None,
            stage: None,
            pid: None,
        }
    }

    #[test]
    fn fresh_heartbeat_is_live() {
        // 60s-old heartbeat, default threshold 120s (env unset in tests).
        let hb = heartbeat("2026-09-02T12:00:00Z", None);
        assert_eq!(
            liveness_from_heartbeat(&hb, parse_rfc3339_to_epoch("2026-09-02T12:01:00Z").unwrap()),
            SessionLiveness::Live { age_secs: 60 }
        );
    }

    #[test]
    fn expired_heartbeat_is_stale() {
        let hb = heartbeat("2026-09-02T12:00:00Z", None);
        assert_eq!(
            liveness_from_heartbeat(&hb, parse_rfc3339_to_epoch("2026-09-02T12:10:00Z").unwrap()),
            SessionLiveness::Stale { age_secs: 600 }
        );
    }

    #[test]
    fn parented_sub_agent_gets_15x_threshold() {
        // 25 min old: stale for a normal session (threshold 120s), live at
        // 15x (1800s) for a parented sub-agent.
        let hb = heartbeat("2026-09-02T12:00:00Z", Some("parent"));
        let now = parse_rfc3339_to_epoch("2026-09-02T12:25:00Z").unwrap();
        assert_eq!(
            liveness_from_heartbeat(&hb, now),
            SessionLiveness::Live { age_secs: 1500 }
        );
        let orphan = heartbeat("2026-09-02T12:00:00Z", None);
        assert!(matches!(
            liveness_from_heartbeat(&orphan, now),
            SessionLiveness::Stale { .. }
        ));
    }

    #[test]
    fn unparsable_timestamp_counts_as_ancient() {
        let hb = heartbeat("", None);
        assert!(matches!(
            liveness_from_heartbeat(&hb, 1000),
            SessionLiveness::Stale { .. }
        ));
    }

    #[test]
    fn stale_verdict_is_what_discovery_filters_on() {
        // Pin the boundary the two verbs share: age == threshold is live,
        // age == threshold + 1 is stale (discovery uses `age > threshold`).
        let hb = heartbeat("2026-09-02T12:00:00Z", None);
        let t0 = parse_rfc3339_to_epoch("2026-09-02T12:00:00Z").unwrap();
        assert_eq!(
            liveness_from_heartbeat(&hb, t0 + stale_secs()),
            SessionLiveness::Live {
                age_secs: stale_secs()
            }
        );
        assert!(matches!(
            liveness_from_heartbeat(&hb, t0 + stale_secs() + 1),
            SessionLiveness::Stale { .. }
        ));
    }

    #[test]
    fn claim_staleness_shares_the_session_boundary() {
        // The claim rule reads its own timestamp, but against the same
        // threshold and the same `age > threshold` boundary as the session
        // criterion above — that shared edge is the point (GH-1018).
        let t0 = parse_rfc3339_to_epoch("2026-09-02T12:00:00Z").unwrap();
        let ts = "2026-09-02T12:00:00Z";
        assert_eq!(claim_age_secs_at(ts, t0 + 30), 30);
        assert!(!claim_is_stale_at(ts, t0 + stale_secs()));
        assert!(claim_is_stale_at(ts, t0 + stale_secs() + 1));
    }

    #[test]
    fn an_unparseable_claim_timestamp_reads_as_ancient() {
        // Fail towards "ancient", never towards "fresh": a claim nobody can
        // date must not be able to stand forever by being unreadable.
        assert!(claim_is_stale_at("not a timestamp", 1_000_000));
        assert!(claim_is_stale_at("", 1_000_000));
    }

    #[test]
    fn the_guard_window_is_not_the_display_window() {
        // The two questions this module answers about one claim age take
        // different thresholds on purpose, and the gap between them is where
        // the bug lives: a claim two minutes old is no longer *fresh*, and it
        // must still refuse a writer. Collapsing them opens an operator's
        // claimed surface two minutes after they claimed it (GH-1018).
        let t0 = parse_rfc3339_to_epoch("2026-09-02T12:00:00Z").unwrap();
        let ts = "2026-09-02T12:00:00Z";
        assert!(claim_is_stale_at(ts, t0 + stale_secs() + 1));
        assert!(!claim_guard_expired_at(ts, t0 + stale_secs() + 1));
    }

    #[test]
    fn the_guard_shares_the_boundary_every_criterion_here_uses() {
        // `age > threshold`, same edge as the session criterion and the
        // display rule: at exactly the TTL the claim still stands.
        let t0 = parse_rfc3339_to_epoch("2026-09-02T12:00:00Z").unwrap();
        let ts = "2026-09-02T12:00:00Z";
        assert!(!claim_guard_expired_at(ts, t0 + claim_ttl_secs()));
        assert!(claim_guard_expired_at(ts, t0 + claim_ttl_secs() + 1));
    }

    #[test]
    fn an_undateable_claim_cannot_refuse_a_writer_forever() {
        // The guard fails the same direction the display rule does. A claim
        // nobody can date is the one shape that could otherwise hold a
        // surface permanently — exactly the standstill GH-1018 found.
        assert!(claim_guard_expired_at("not a timestamp", 1_000_000));
        assert!(claim_guard_expired_at("", 1_000_000));
    }
}
