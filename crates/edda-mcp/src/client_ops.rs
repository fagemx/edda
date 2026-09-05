//! Client-contract operations behind MCP tools (GH-611).
//!
//! Every function here reuses the validated service path:
//! - task verbs drive `edda_ledger::task_actions` — the same state machine the
//!   CLI uses (start/done pairing, blocked-dependency checks, receipt
//!   requirement, idempotency dedup). No state rules are duplicated here.
//! - claim writes go through `edda_bridge_claude::peers` — the known temporary
//!   MCP→Claude dependency edge; the ledger never depends on the bridge.
//! - verify reads the ledger read-only via `Ledger::open_existing` +
//!   `verify_chain_report`, the same calls `edda verify` makes.
//!
//! These are sync fns on the blocking paths; the MCP tool methods in `lib.rs`
//! call them (they run on the server's worker context).

use std::path::Path;

use edda_ledger::task_actions::{self, NewTaskArgs};
use edda_ledger::Ledger;

/// The rebuild step every task write shares (derived views per branch).
fn rebuild(ledger: &Ledger, branch: &str) {
    let _ = edda_derive::rebuild_branch(ledger, branch);
}

fn notify_task_assigned(repo_root: &Path, task_id: u64, title: &str, assignee: &str) {
    // Same presentation dispatch the CLI performs after `task new`.
    let paths = edda_ledger::paths::EddaPaths::discover(repo_root);
    let notify_config = edda_notify::NotifyConfig::load(&paths);
    if !notify_config.channels.is_empty() {
        edda_notify::dispatch(
            &notify_config,
            &edda_notify::NotifyEvent::TaskAssigned {
                task_id,
                title: title.to_string(),
                assignee: assignee.to_string(),
            },
        );
    }
}

/// `edda_task_new` — create a task on the rail (idempotent by key).
pub fn task_new(repo_root: &Path, args: &NewTaskArgs<'_>) -> anyhow::Result<serde_json::Value> {
    let outcome = task_actions::new_task(repo_root, args, &rebuild)?;
    if !outcome.deduped {
        if let Some(assignee) = args.assignee {
            notify_task_assigned(repo_root, outcome.task_id, args.title, assignee);
        }
    }
    Ok(serde_json::json!({
        "task_id": outcome.task_id,
        "status": outcome.status.to_string(),
        "deduped": outcome.deduped,
    }))
}

/// `edda_task_start` — take the lease and mark running.
pub fn task_start(
    repo_root: &Path,
    id: u64,
    lease_ttl_s: u64,
) -> anyhow::Result<serde_json::Value> {
    let outcome = task_actions::start_task(repo_root, id, lease_ttl_s, &rebuild)?;
    Ok(serde_json::json!({ "attempt": outcome.attempt }))
}

/// `edda_task_done` — complete with a receipt; successors become ready.
pub fn task_done(
    repo_root: &Path,
    id: u64,
    receipt: &str,
    evidence_paths: &[String],
) -> anyhow::Result<serde_json::Value> {
    let outcome = task_actions::done_task(repo_root, id, receipt, evidence_paths, &rebuild)?;
    let unlocked: Vec<serde_json::Value> = outcome
        .unlocked
        .into_iter()
        .map(|(task_id, title, assignee)| {
            serde_json::json!({ "task_id": task_id, "title": title, "assignee": assignee })
        })
        .collect();
    Ok(serde_json::json!({ "unlocked": unlocked }))
}

/// `edda_task_fail` — mark a running task failed.
pub fn task_fail(repo_root: &Path, id: u64, reason: &str) -> anyhow::Result<serde_json::Value> {
    task_actions::fail_task(repo_root, id, reason, &rebuild)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// `edda_receipt` — read the recorded receipt of a task (derived view).
///
/// Receipts are written by `task done` only; there is no separate write path
/// ("done without a receipt does not exist"). This is the read side.
pub fn receipt(repo_root: &Path, task_id: u64) -> anyhow::Result<serde_json::Value> {
    let ledger = Ledger::open_existing(repo_root)?;
    let views = ledger.task_views()?;
    let v = views
        .iter()
        .find(|v| v.task_id == task_id)
        .ok_or_else(|| anyhow::anyhow!("task #{task_id} not found — see `edda task list`"))?;
    Ok(serde_json::json!({
        "task_id": v.task_id,
        "status": v.status.to_string(),
        "receipt": v.receipt,
        "title": v.title,
    }))
}

/// `edda_verify` — verify the hash chain, read-only (same payload as
/// `edda verify --json`; an unreadable ledger is an error, never a repair).
pub fn verify(repo_root: &Path) -> anyhow::Result<serde_json::Value> {
    let ledger = Ledger::open_existing(repo_root)?;
    let report = ledger.verify_chain_report()?;
    Ok(serde_json::json!({
        "ok": report.first_bad_event.is_none(),
        "events": report.events,
        "first_bad_event": report.first_bad_event,
    }))
}

/// `edda_claim` — claim a coordination scope on the session board.
///
/// Reuses the bridge's validated write (`write_claim_with_subject`) and
/// verifies the fold actually recorded the claim before reporting success —
/// a lost write must not look like a claimed scope (GH-705 behavior).
///
/// Session id: callers pass their own; when omitted a deterministic
/// `mcp-<label>` id is minted (mirrors the CLI's `cli-<label>` tier-4 shape).
/// Board claims fold one-per-session: a second claim replaces the first.
pub fn claim(
    repo_root: &Path,
    label: &str,
    paths: &[String],
    subject: Option<&str>,
    session: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    if label.trim().is_empty() {
        anyhow::bail!("claim label must not be empty");
    }
    let project_id = edda_store::project_id(repo_root);
    let session_id = session
        .map(str::to_string)
        .unwrap_or_else(|| format!("mcp-{label}"));

    let replaced = edda_bridge_claude::peers::compute_board_state(&project_id)
        .claims
        .into_iter()
        .find(|c| c.session_id == session_id)
        .map(|c| serde_json::json!({ "label": c.label, "paths": c.paths }));

    edda_bridge_claude::peers::write_claim_with_subject(
        &project_id,
        &session_id,
        label,
        paths,
        subject,
    );

    let current = edda_bridge_claude::peers::compute_board_state(&project_id)
        .claims
        .into_iter()
        .find(|c| c.session_id == session_id);
    let recorded = current
        .as_ref()
        .is_some_and(|c| c.label == label && c.paths == paths && c.subject.as_deref() == subject);
    if !recorded {
        anyhow::bail!(
            "claim write not visible on the board — refusing to report success for '{label}'"
        );
    }
    Ok(serde_json::json!({
        "session_id": session_id,
        "label": label,
        "paths": paths,
        "replaced": replaced,
    }))
}
