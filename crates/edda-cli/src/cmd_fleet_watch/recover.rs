//! Recovery half of `edda fleet watch` (GH-573).
//!
//! Split out of the parent module when the file-length ratchet was reached;
//! the parent owns observation and rendering, this file owns the writes the
//! verb makes. It is never called without `--apply`.

use super::*;

// ── Recovery ─────────────────────────────────────────────────────────────────

/// What `--apply` did (or, in a dry run, what it would do) for one orphan.
#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct Recovered {
    pub(super) session_id: String,
    pub(super) plan: String,
    pub(super) phase: String,
    pub(super) steps: Vec<RecoveryStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) resume: Option<String>,
    pub(super) terminal_recorded: bool,
    pub(super) claim_released: bool,
    pub(super) redispatch_recorded: bool,
    pub(super) stop_loss_recorded: bool,
}

/// The store a recovery writes into: the one that actually holds this lane's
/// work.
///
/// Every git worktree of a repository shares one `project_id`
/// (`edda_store::project_id` resolves to the main root), so `lane.stores`
/// holds every candidate store with the invocation root first. Writing the
/// ledger note and the phase transition into the invocation root while the
/// plan state lives in a worktree would skip the transition and lose the
/// redispatch, so the store holding this plan's state wins. A stateless lane
/// (no state anywhere) records into the first store that has a ledger at all.
fn recovery_store(lane: &Lane) -> Option<&Path> {
    if edda_conductor::state::persist::validate_plan_name(&lane.plan).is_ok() {
        if let Some(store_path) = lane
            .stores
            .iter()
            .find(|s| state_path(s, &lane.plan).is_file())
        {
            return Some(store_path.as_path());
        }
    }
    lane.stores
        .iter()
        .find(|s| s.join(".edda").join("ledger.db").exists())
        .map(PathBuf::as_path)
}

fn append_fleet_note(
    ledger: &Ledger,
    text: &str,
    tags: &[String],
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let _lock = WorkspaceLock::acquire(&ledger.paths).context("acquiring workspace lock")?;
    let branch = ledger.head_branch().context("reading HEAD branch")?;
    let parent_hash = ledger
        .last_event_hash()
        .context("reading last event hash")?;
    let (safe_text, hits) = edda_core::secret_guard::redact(text);
    if !hits.is_empty() {
        eprintln!(
            "⚠ fleet watch: secret-guard redacted {} pattern(s) before writing the note",
            hits.len()
        );
    }
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent_hash.as_deref(),
        "fleet watch",
        &safe_text,
        tags,
    )
    .context("building note event")?;
    event.payload[PAYLOAD_KEY] = payload;
    // Embedding the structured payload changes the body; re-hash exactly like
    // the conductor's phase notes do, or the append rejects the event.
    edda_core::event::finalize_event(&mut event).context("re-finalizing note event")?;
    ledger
        .append_event(&event)
        .context("appending note event")?;
    if let Err(e) = edda_derive::rebuild_branch(ledger, &branch) {
        // Same best-effort convention as `edda note` and the conductor's notes,
        // but never silent: a shrinking derived view stays observable.
        eprintln!(
            "⚠ fleet watch: derived-view rebuild failed after writing a note (the ledger write stands): {e:#}"
        );
    }
    Ok(())
}

fn fleet_payload(action: &str, lane: &Lane, reason: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "action": action,
        "plan": lane.plan,
        "phase": lane.phase,
        "session_id": lane.session_id,
        "attempt": lane.attempt,
    });
    if let Some(reason) = reason {
        value["reason"] = serde_json::Value::String(reason.to_string());
    }
    value
}

/// Step 1 — write the terminal record.
///
/// The lane died; its work must not stay `Running` forever. Two writes, both
/// into surfaces that already exist: the same `Running`/`Checking → Stale`
/// transition `detect_stale_phases` applies on resume ("phase was running when
/// conductor stopped"), and a ledger note keyed by (plan, phase, session) —
/// the record that makes *this* lane terminal for the next run.
///
/// The state transition goes **first**. The note alone would make every later
/// run read the lane `finished` (so the recovery is never retried) while a
/// crash between the two writes left the phase `Running` — the exact state the
/// transition exists to clear (REVIEW round 3, finding 2). This order can only
/// leave a `Stale` phase with no note, which is a terminal record in its own
/// right.
///
/// The transition is assigned directly for the same reason
/// `detect_stale_phases` assigns it directly: `Checking → Stale` has no edge in
/// the state machine, so there is no `transition` call to make.
fn write_terminal(store_path: &Path, lane: &Lane) -> anyhow::Result<bool> {
    let reason = format!(
        "lane {} terminated abnormally: heartbeat stale for {}s with no terminal record",
        lane.session_id, lane.age_secs
    );

    let marked = if edda_conductor::state::persist::validate_plan_name(&lane.plan).is_err()
        || !state_path(store_path, &lane.plan).is_file()
    {
        false
    } else {
        update_state(store_path, &lane.plan, |state| {
            let Some(phase) = state.phases.iter_mut().find(|p| p.id == lane.phase) else {
                return Ok(false);
            };
            if phase.status != PhaseStatus::Running && phase.status != PhaseStatus::Checking {
                return Ok(false);
            }
            phase.status = PhaseStatus::Stale;
            phase.error = Some(ErrorInfo {
                error_type: ErrorType::Timeout,
                message: "phase was running when conductor stopped; lane heartbeat expired with \
                          no terminal record (edda fleet watch)"
                    .to_string(),
                retryable: true,
                check_index: None,
                timestamp: now_rfc3339(),
            });
            edda_conductor::state::derive::update_plan_status(state);
            Ok(true)
        })?
    };

    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: lane \"{}\" ({}/{}) terminated abnormally — {reason}",
        lane.session_id, lane.plan, lane.phase
    );
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "orphan".to_string()],
        fleet_payload("terminal", lane, Some(&reason)),
    )?;
    Ok(marked)
}

/// Step 2 — release the dead session's claim, if one is on the board. A
/// session that never claimed is a no-op, so a second run writes nothing.
fn release_claim(lane: &Lane) -> bool {
    let Ok(claims) = crate::cmd_claim::read_active_claims(&lane.project_id) else {
        eprintln!(
            "⚠ fleet watch: board unreadable for project {}; claim not released",
            lane.project_id
        );
        return false;
    };
    if !claims.iter().any(|c| c.session_id == lane.session_id) {
        return false;
    }
    peers::write_unclaim(&lane.project_id, &lane.session_id);
    true
}

/// Prior redispatches this verb recorded for (plan, phase), read back from the
/// ledger notes themselves — the cap needs no side file.
pub(super) fn prior_redispatches(lane: &Lane) -> u32 {
    let mut count = 0;
    for store_path in &lane.stores {
        let Ok(Some(ledger)) = store_ledger(store_path) else {
            continue;
        };
        let Ok(events) = ledger.iter_events() else {
            continue;
        };
        for event in events {
            let Some(fw) = event.payload.get(PAYLOAD_KEY) else {
                continue;
            };
            if fw.get("action").and_then(|v| v.as_str()) == Some("redispatch")
                && fw.get("plan").and_then(|v| v.as_str()) == Some(lane.plan.as_str())
                && fw.get("phase").and_then(|v| v.as_str()) == Some(lane.phase.as_str())
            {
                count += 1;
            }
        }
    }
    count
}

/// The worktree facts the takeover instruction must carry, read **before** the
/// redispatch is recorded (the issue's doneWhen item 3: re-dispatch must read
/// the worktree's current state first, and an existing uncommitted/unpushed
/// change means "continue on top of it", never "redo it").
#[derive(Debug, Clone, serde::Serialize)]
struct WorktreeState {
    dirty_files: usize,
    unpushed_commits: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sample_paths: Vec<String>,
}

impl WorktreeState {
    fn describe(&self) -> String {
        if self.dirty_files == 0 && self.unpushed_commits == 0 {
            return "worktree is clean (no uncommitted files, no unpushed commits);".to_string();
        }
        let sample = if self.sample_paths.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.sample_paths.join("; "))
        };
        format!(
            "worktree has {} uncommitted file(s){sample} and {} unpushed commit(s);",
            self.dirty_files, self.unpushed_commits
        )
    }
}

/// Best-effort read of one worktree's current state. A non-git or unreadable
/// worktree reports zeroes rather than failing the recovery: the observation
/// plane must never block the work plane.
fn worktree_state(store_path: &Path) -> WorktreeState {
    let status = git_capture(store_path, &["status", "--porcelain"]);
    let mut dirty_files = 0usize;
    let mut sample_paths = Vec::new();
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }
        dirty_files += 1;
        if sample_paths.len() < 3 {
            sample_paths.push(line.trim().to_string());
        }
    }
    let unpushed_commits = git_capture(store_path, &["rev-list", "--count", "@{u}..HEAD"])
        .trim()
        .parse::<usize>()
        .unwrap_or(0);
    WorktreeState {
        dirty_files,
        unpushed_commits,
        sample_paths,
    }
}

fn git_capture(cwd: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default()
}

/// Step 3a — record the redispatch and re-arm the conductor phase through the
/// existing retry transition (Running/Checking → Stale → Pending), so the
/// existing `edda conduct run <plan_file> --cwd <store>` resume re-runs it.
/// Returns the resume hint, or `None` when there is no plan state to re-arm
/// (a stateless dispatch lane), which the caller records as a stop-loss.
fn redispatch(store_path: &Path, lane: &Lane) -> anyhow::Result<Option<String>> {
    if !state_path(store_path, &lane.plan).is_file() {
        return Ok(None);
    }
    // Read the worktree BEFORE re-arming, so the instruction the redispatch
    // carries is evidence-based (the issue's doneWhen item 3).
    let worktree = worktree_state(store_path);
    let takeover = format!(
        "A previous attempt at this phase (lane {}) terminated abnormally. {} \
         Read the worktree's current state first and continue on top of it — do not \
         redo work that is already there.",
        lane.session_id,
        worktree.describe()
    );

    let plan_file = update_state(store_path, &lane.plan, |state| {
        let plan_file = state.plan_file.clone();
        let Some(current) = state
            .phases
            .iter()
            .find(|p| p.id == lane.phase)
            .map(|p| p.status)
        else {
            return Ok(None);
        };
        // Through the state machine's own retry edges (`Stale → Pending`,
        // `Failed → Pending`), exactly as `edda conduct retry` does: that is
        // what clears the transient gate/verdict state and bumps the state
        // version the runner reconciles against. A phase that is not on one of
        // those edges (e.g. already Pending after a previous stop-loss) is
        // left alone rather than forced.
        if current != PhaseStatus::Stale && current != PhaseStatus::Failed {
            return Ok(None);
        }
        // `retry_context` is the channel the runner actually consumes: it is
        // injected into the phase prompt by `build_phase_prompt` on the next
        // attempt. Writing the instruction only into a ledger note would leave
        // the re-dispatched worker without the takeover brief (REVIEW round 2
        // finding 3), so the note and the prompt carry the same text.
        edda_conductor::state::machine::transition(
            state,
            &lane.phase,
            current,
            PhaseStatus::Pending,
            Some(edda_conductor::state::machine::PhaseUpdate {
                retry_context: Some(Some(takeover.clone())),
                ..Default::default()
            }),
        )?;
        edda_conductor::state::derive::update_plan_status(state);
        Ok(Some(plan_file))
    })?;
    let Some(plan_file) = plan_file else {
        return Ok(None);
    };

    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: redispatch lane \"{}\" ({}/{}) — the phase was re-armed to Pending with \
         this takeover instruction in its retry context: {takeover}",
        lane.session_id, lane.plan, lane.phase
    );
    let mut payload = fleet_payload("redispatch", lane, None);
    payload["worktree"] = serde_json::to_value(&worktree)?;
    payload["takeover_instruction"] = serde_json::Value::String(takeover);
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "redispatch".to_string()],
        payload,
    )?;

    // Quoted: this is a copy-pasteable hint, and a store path with a space (the
    // Windows norm) would otherwise split into two arguments.
    let plan = if plan_file.is_empty() || !Path::new(&plan_file).is_absolute() {
        "<plan.yaml>".to_string()
    } else {
        plan_file
    };
    Ok(Some(format!(
        "edda conduct run \"{plan}\" --cwd \"{}\"",
        store_path.display()
    )))
}

/// Step 3b — record why the ladder stopped.
fn record_stop_loss(store_path: &Path, lane: &Lane, reason: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: stop-loss for lane \"{}\" ({}/{}): {reason}",
        lane.session_id, lane.plan, lane.phase
    );
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "stop_loss".to_string()],
        fleet_payload("stop_loss", lane, Some(reason)),
    )
}

/// Apply the planned steps to one orphan.
pub(super) fn recover(lane: &Lane, steps: &[RecoveryStep], cap: u32) -> anyhow::Result<Recovered> {
    let mut out = Recovered {
        session_id: lane.session_id.clone(),
        plan: lane.plan.clone(),
        phase: lane.phase.clone(),
        steps: steps.to_vec(),
        reason: None,
        resume: None,
        terminal_recorded: false,
        claim_released: false,
        redispatch_recorded: false,
        stop_loss_recorded: false,
    };
    let Some(store_path) = recovery_store(lane) else {
        out.reason = Some(format!(
            "no edda workspace among {} to record into",
            lane.stores
                .iter()
                .map(|s| s.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return Ok(out);
    };
    if steps.contains(&RecoveryStep::WriteTerminal) {
        write_terminal(store_path, lane)?;
        out.terminal_recorded = true;
    }
    if steps.contains(&RecoveryStep::ReleaseClaim) {
        out.claim_released = release_claim(lane);
    }
    match steps.last() {
        Some(RecoveryStep::Redispatch) => match redispatch(store_path, lane)? {
            Some(resume) => {
                out.redispatch_recorded = true;
                out.resume = Some(resume);
            }
            None => {
                let reason = format!(
                    "no recorded plan/brief for {}/{}; redispatch belongs to the caller",
                    lane.plan, lane.phase
                );
                record_stop_loss(store_path, lane, &reason)?;
                out.stop_loss_recorded = true;
                out.reason = Some(reason);
            }
        },
        Some(RecoveryStep::StopLoss) => {
            let reason = format!(
                "redispatch cap reached ({cap} prior redispatch(es) for {}/{})",
                lane.plan, lane.phase
            );
            record_stop_loss(store_path, lane, &reason)?;
            out.stop_loss_recorded = true;
            out.reason = Some(reason);
        }
        _ => {}
    }
    Ok(out)
}
