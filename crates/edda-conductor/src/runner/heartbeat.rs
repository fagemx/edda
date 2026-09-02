//! Lane heartbeat (GH-566/GH-569).
//!
//! While a conductor phase runs — whether dispatched by `edda conduct` or
//! `edda dispatch` — the runner periodically refreshes the session's existing
//! heartbeat file so `edda peers` (and any future status plane) can see the
//! lane is alive and what it is doing. This is the SAME surface the Claude
//! hook path writes (the type and IO live in `edda-store`); there is exactly
//! one liveness surface, no parallel format.
//!
//! The heartbeat is an observation plane, never a control plane (decision
//! `fleet.lane-dispatch`): a write failure degrades to a warning and can
//! never fail the phase. Writes are atomic (temp + rename) over a bounded
//! single file, and the lane goes stale naturally once it stops — there is
//! deliberately no delete step.

use crate::agent::launcher::{AgentLauncher, PhaseResult};
use crate::plan::schema::Phase;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Heartbeat refresh interval: env-configurable, default ~30s. Must stay well
/// under the peer staleness threshold (`EDDA_PEER_STALE_SECS`, default 120s).
pub fn lane_heartbeat_interval_secs() -> u64 {
    std::env::var("EDDA_LANE_HEARTBEAT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(30)
}

/// Identity of one running lane, enough to answer "what is it doing".
#[derive(Debug, Clone)]
pub struct LaneHeartbeat {
    /// The cwd the heartbeat is rooted at — resolved through
    /// `edda_store::project_id`, the same root resolution every reader uses,
    /// so worktree-launched plans land where readers look.
    pub cwd: PathBuf,
    pub session_id: String,
    pub plan: String,
    pub phase: String,
    pub attempt: u32,
}

impl LaneHeartbeat {
    /// Refresh the heartbeat once with the given stage. Updates only the
    /// fields the lane owns (stage/plan/phase/attempt/pid/label/timestamps)
    /// and preserves hook-produced telemetry — see `edda_store::
    /// update_heartbeat`. Best-effort: callers own error policy.
    pub fn try_write(&self, stage: &str) -> anyhow::Result<()> {
        let project_id = edda_store::project_id(&self.cwd);
        let now = now_rfc3339();
        edda_store::update_heartbeat(&project_id, &self.session_id, |hb| {
            // Preserve started_at across refreshes so age-of-session stays
            // true; a fresh record starts its clock here.
            if hb.started_at.is_empty() {
                hb.started_at = now.clone();
            }
            hb.last_heartbeat = now;
            // The claim for a conduct phase uses the phase id as its label
            // (`write_phase_claim`); matching it keeps `edda request`
            // routing and the peers view coherent.
            hb.label = self.phase.clone();
            hb.current_phase = Some(stage.to_string());
            hb.plan = Some(self.plan.clone());
            hb.phase = Some(self.phase.clone());
            hb.attempt = Some(self.attempt);
            hb.stage = Some(stage.to_string());
            hb.pid = Some(std::process::id());
            // focus_files, active_tasks, edit counts, recent commits, branch
            // and the parent link belong to the hook producer — untouched.
        })
    }

    /// Refresh the heartbeat once, degrading any failure to a warning on
    /// stderr — the observation plane must not be able to kill the work
    /// plane.
    pub fn write(&self, stage: &str) {
        if let Err(e) = self.try_write(stage) {
            eprintln!(
                "warning: lane heartbeat write failed for {} (continuing): {e}",
                self.session_id
            );
        }
    }

    /// Start refreshing the heartbeat every interval until the token is
    /// cancelled (or the handle is aborted at turn end). The FIRST write
    /// happens here, synchronously on the caller's task — never inside the
    /// spawned task, which a fast caller could finish before the scheduler
    /// ever polls.
    pub fn spawn(
        &self,
        stage: &'static str,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        self.write(stage);
        let hb = self.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs(lane_heartbeat_interval_secs());
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => hb.write(stage),
                    _ = cancel.cancelled() => break,
                }
            }
        })
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Run one agent turn with a live lane heartbeat: the first write happens
/// synchronously before the turn can complete, then a background task
/// refreshes the heartbeat every interval while the turn runs, and is
/// stopped when the turn returns. The phase's checks are covered separately
/// by `process_phase_result` (stage "checking"), so the heartbeat spans the
/// whole phase lifetime (GH-566). Afterwards the heartbeat ages out through
/// the normal staleness threshold — no delete step, a stopped lane is
/// simply stale.
///
/// This is the single write site serving `edda conduct` (both the main phase
/// loop and post-rejection redispatch turns) and `edda dispatch`.
///
/// `hb` carries the heartbeat identity (rooted at the plan cwd, matching
/// `write_phase_claim`); `turn_cwd` is where the agent itself runs.
pub async fn run_phase_with_heartbeat(
    launcher: &dyn AgentLauncher,
    phase: &Phase,
    prompt: &str,
    plan_context: &str,
    turn_cwd: &Path,
    cancel: &CancellationToken,
    hb: &LaneHeartbeat,
) -> Result<PhaseResult> {
    let writer = hb.spawn("running", cancel.child_token());
    let result = launcher
        .run_phase(
            phase,
            prompt,
            plan_context,
            &hb.session_id,
            turn_cwd,
            cancel.child_token(),
        )
        .await;
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::sequential::tests::{make_repo, ClaimEnvGuard};

    #[test]
    fn interval_defaults_to_30s_and_reads_env() {
        let previous = std::env::var("EDDA_LANE_HEARTBEAT_SECS");
        std::env::remove_var("EDDA_LANE_HEARTBEAT_SECS");
        assert_eq!(lane_heartbeat_interval_secs(), 30);
        std::env::set_var("EDDA_LANE_HEARTBEAT_SECS", "5");
        assert_eq!(lane_heartbeat_interval_secs(), 5);
        match previous {
            Ok(v) => std::env::set_var("EDDA_LANE_HEARTBEAT_SECS", v),
            Err(_) => std::env::remove_var("EDDA_LANE_HEARTBEAT_SECS"),
        }
    }

    /// P1-2 regression (review round 1): the runner refresh must update only
    /// the fields the lane owns and preserve hook-produced telemetry
    /// (branch, focus_files, active_tasks, edit counts, recent commits) —
    /// a Claude conduct lane has both producers racing on one file.
    #[test]
    fn lane_refresh_preserves_hook_telemetry_it_does_not_own() {
        // ClaimEnvGuard serializes EDDA_STORE_ROOT mutation with the other
        // heartbeat tests via CLAIM_ENV_LOCK.
        let guard = ClaimEnvGuard::new();
        let cwd = make_repo(guard._store_root.path());
        let project_id = edda_store::project_id(&cwd);

        // Hook telemetry pre-seeded by the Claude hook producer.
        let seeded = edda_store::SessionHeartbeat {
            session_id: "s".into(),
            started_at: "2026-09-01T00:00:00Z".into(),
            last_heartbeat: "2026-09-01T00:00:10Z".into(),
            label: "hook-label".into(),
            focus_files: vec!["src/lib.rs".into(), "src/main.rs".into()],
            active_tasks: vec![edda_store::TaskSnapshot {
                id: "t1".into(),
                subject: "ship it".into(),
                status: "in_progress".into(),
            }],
            files_modified_count: 3,
            total_edits: 7,
            recent_commits: vec!["abc1234 fix things".into()],
            branch: Some("feat/x".into()),
            current_phase: None,
            parent_session_id: None,
            plan: None,
            phase: None,
            attempt: None,
            stage: None,
            pid: None,
        };
        edda_store::write_heartbeat(&project_id, &seeded).unwrap();

        let hb = LaneHeartbeat {
            cwd: cwd.clone(),
            session_id: "s".into(),
            plan: "p".into(),
            phase: "a".into(),
            attempt: 1,
        };
        hb.write("running");

        let after = edda_store::read_heartbeat(&project_id, "s").expect("heartbeat exists");
        assert_eq!(
            after.focus_files, seeded.focus_files,
            "focus_files preserved"
        );
        assert_eq!(after.active_tasks.len(), 1, "active_tasks preserved");
        assert_eq!(
            after.files_modified_count, 3,
            "files_modified_count preserved"
        );
        assert_eq!(after.total_edits, 7, "total_edits preserved");
        assert_eq!(
            after.recent_commits, seeded.recent_commits,
            "commits preserved"
        );
        assert_eq!(after.branch.as_deref(), Some("feat/x"), "branch preserved");
        assert_eq!(
            after.started_at, "2026-09-01T00:00:00Z",
            "started_at preserved"
        );
        // The lane still stamps the fields it owns.
        assert_eq!(after.plan.as_deref(), Some("p"));
        assert_eq!(after.phase.as_deref(), Some("a"));
        assert_eq!(after.attempt, Some(1));
        assert_eq!(after.stage.as_deref(), Some("running"));
        assert!(after.pid.is_some());
    }

    #[test]
    fn write_failure_is_swallowed_not_fatal() {
        // Serialize env mutation with the other heartbeat tests via
        // ClaimEnvGuard's shared CLAIM_ENV_LOCK. A private lock here (as an
        // earlier draft had) is what flaked `lane_refresh_preserves_hook_
        // telemetry_it_does_not_own`: this test relocated EDDA_STORE_ROOT
        // under a lock that test never saw, so its seeded write landed in
        // one store and its read-modify-write polled another — persist
        // os error 2/5 when the stolen root's tempdir was deleted, and
        // "focus_files preserved: left []" when the RMW read a blank
        // record. Same defect as PR #588's Windows CI failure in edda-cli.
        let guard = ClaimEnvGuard::new();
        let root = guard._store_root.path();
        let cwd = root.join("file-as-project-dir-repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let project_dir = edda_store::project_dir(&edda_store::project_id(&cwd));
        let _ = std::fs::remove_dir_all(&project_dir);
        std::fs::create_dir_all(project_dir.parent().unwrap()).unwrap();
        std::fs::write(&project_dir, b"file").unwrap();
        let hb = LaneHeartbeat {
            cwd,
            session_id: "s".into(),
            plan: "p".into(),
            phase: "a".into(),
            attempt: 1,
        };
        // The write IS attempted and its failure is surfaced: a store whose
        // project dir is a regular file must make `try_write` return Err
        // (the `write` wrapper turns that into the promised warning), not
        // silently skip the attempt.
        let result = hb.try_write("running");
        assert!(
            result.is_err(),
            "expected the heartbeat write to fail against a file-as-project-dir store, got Ok"
        );
        // The guard restores EDDA_STORE_ROOT on drop.
    }
}
