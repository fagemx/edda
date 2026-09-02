//! The one liveness criterion for peer sessions (GH-617).
//!
//! A session is live exactly when its last heartbeat is no older than
//! [`stale_secs`] (x15 for parented sub-agents, mirroring peer discovery).
//! `edda peers` and `edda claim check` must both go through this module so
//! the two verbs can never grow a second, disagreeing notion of "dead".
//!
//! There is one liveness surface — the heartbeat file — and one criterion —
//! [`liveness_from_heartbeat`]. Do not add a parallel one.

use serde::Serialize;

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
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);
    classify_session_liveness_at(project_id, session_id, now)
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
}
