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
    /// Refresh the heartbeat once with the given stage. Best-effort: any
    /// failure is a warning on stderr, never an error — the observation plane
    /// must not be able to kill the work plane.
    pub fn write(&self, stage: &str) {
        let project_id = edda_store::project_id(&self.cwd);
        let now = now_rfc3339();
        // Preserve started_at across refreshes so age-of-session stays true.
        let started_at = edda_store::read_heartbeat(&project_id, &self.session_id)
            .map(|h| h.started_at)
            .unwrap_or_else(|| now.clone());
        let hb = edda_store::SessionHeartbeat {
            session_id: self.session_id.clone(),
            started_at,
            last_heartbeat: now,
            // The claim for a conduct phase uses the phase id as its label
            // (`write_phase_claim`); matching it keeps `edda request`
            // routing and the peers view coherent.
            label: self.phase.clone(),
            focus_files: Vec::new(),
            active_tasks: Vec::new(),
            files_modified_count: 0,
            total_edits: 0,
            recent_commits: Vec::new(),
            branch: None,
            current_phase: Some(stage.to_string()),
            parent_session_id: None,
            plan: Some(self.plan.clone()),
            phase: Some(self.phase.clone()),
            attempt: Some(self.attempt),
            stage: Some(stage.to_string()),
            pid: Some(std::process::id()),
        };
        if let Err(e) = edda_store::write_heartbeat(&project_id, &hb) {
            eprintln!(
                "warning: lane heartbeat write failed for {} (continuing): {e}",
                self.session_id
            );
        }
    }

    /// Spawn a background task that refreshes the heartbeat every interval
    /// until the token is cancelled or the task is aborted at turn end.
    pub fn spawn(
        &self,
        stage: &'static str,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let hb = self.clone();
        tokio::spawn(async move {
            let interval = lane_heartbeat_interval_secs();
            hb.write(stage);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(interval)) => hb.write(stage),
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

/// Run one agent turn with a live lane heartbeat: a background task refreshes
/// the heartbeat every interval while the turn runs, and is stopped when the
/// turn returns (the heartbeat then ages out through the normal staleness
/// threshold — no delete step, a stopped lane is simply stale).
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

    #[test]
    fn write_failure_is_swallowed_not_fatal() {
        // Serialize env mutation with the other heartbeat test.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("EDDA_STORE_ROOT");
        std::env::set_var("EDDA_STORE_ROOT", tmp.path());
        let cwd = tmp.path().join("repo");
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
        hb.write("running");
        match previous {
            Some(v) => std::env::set_var("EDDA_STORE_ROOT", v),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
    }
}
