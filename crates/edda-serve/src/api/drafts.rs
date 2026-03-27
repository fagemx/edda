use std::path::Path;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::agent_phase::{mobile_context_summary, AgentPhaseState};
use edda_core::event::{new_approval_event, ApprovalEventParams};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;

use crate::error::AppError;
use crate::state::AppState;

// ── GET /api/drafts ──

#[derive(Serialize)]
struct DraftItem {
    draft_id: String,
    title: String,
    stage_id: String,
    role: String,
    approved: usize,
    min_approvals: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
}

#[derive(Serialize)]
struct DraftsResponse {
    drafts: Vec<DraftItem>,
}

#[derive(Deserialize)]
struct MinimalDraft {
    #[serde(default)]
    draft_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    stages: Vec<MinimalStage>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    branch: String,
}

#[derive(Deserialize)]
struct MinimalStage {
    #[serde(default)]
    stage_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    min_approvals: usize,
    #[serde(default)]
    approved_by: Vec<String>,
    #[serde(default)]
    status: String,
}

async fn get_drafts(State(state): State<Arc<AppState>>) -> Result<Json<DraftsResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let drafts_dir = &ledger.paths.drafts_dir;

    if !drafts_dir.exists() {
        return Ok(Json(DraftsResponse { drafts: vec![] }));
    }

    // Load agent phase states for context enrichment
    let phase_states = load_agent_phase_states(&state.repo_root);

    // Load recent decisions/commits for context summary
    let head = ledger.head_branch().unwrap_or_default();
    let recent_decisions = recent_decision_summaries(&ledger, &head, 3);
    let recent_commits = recent_commit_summaries(&ledger, &head, 3);

    let mut items = Vec::new();
    for entry in std::fs::read_dir(drafts_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_stem().and_then(|s| s.to_str()) == Some("latest") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;
        let draft: MinimalDraft = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if draft.status == "applied" {
            continue;
        }

        // Try to find a matching agent phase state (by branch or label)
        let matched_phase = phase_states.iter().find(|ps| {
            ps.branch.as_deref() == Some(&draft.branch) || ps.session_id == draft.draft_id
        });

        let (phase, agent, issue, context_summary) = if let Some(ps) = matched_phase {
            let summary = mobile_context_summary(ps, &recent_decisions, &recent_commits, 200);
            (
                Some(ps.phase.to_string()),
                Some(ps.session_id.clone()),
                ps.issue,
                Some(summary),
            )
        } else {
            (None, None, None, None)
        };

        // Derive risk_level from labels if present
        let risk_level = draft
            .labels
            .iter()
            .find(|l| l.starts_with("risk:") || l.contains("risk"))
            .map(|l| l.strip_prefix("risk:").unwrap_or(l).to_string())
            .or_else(|| {
                if draft.labels.iter().any(|l| l == "high-risk") {
                    Some("high".to_string())
                } else {
                    None
                }
            });

        for stage in &draft.stages {
            if stage.status != "pending" {
                continue;
            }
            items.push(DraftItem {
                draft_id: draft.draft_id.clone(),
                title: draft.title.clone(),
                stage_id: stage.stage_id.clone(),
                role: stage.role.clone(),
                approved: stage.approved_by.len(),
                min_approvals: stage.min_approvals,
                risk_level: risk_level.clone(),
                phase: phase.clone(),
                agent: agent.clone(),
                issue,
                context_summary: context_summary.clone(),
                requested_at: draft.created_at.clone(),
                labels: draft.labels.clone(),
            });
        }
    }

    Ok(Json(DraftsResponse { drafts: items }))
}

/// Load agent phase state files from `.edda/agent-phases/`.
fn load_agent_phase_states(repo_root: &Path) -> Vec<AgentPhaseState> {
    let phases_dir = repo_root.join(".edda").join("agent-phases");
    if !phases_dir.exists() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&phases_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut states = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(ps) = serde_json::from_str::<AgentPhaseState>(&content) {
                states.push(ps);
            }
        }
    }
    states
}

/// Fetch recent decision summaries from the ledger for context generation.
fn recent_decision_summaries(ledger: &Ledger, branch: &str, limit: usize) -> Vec<String> {
    let events = ledger
        .iter_events_filtered(branch, Some("decision"), None, None, None, limit)
        .unwrap_or_default();
    events
        .iter()
        .filter_map(|e| {
            let key = e.payload.get("key")?.as_str()?;
            let value = e.payload.get("value")?.as_str()?;
            Some(format!("{key}={value}"))
        })
        .collect()
}

/// Fetch recent commit summaries from the ledger for context generation.
fn recent_commit_summaries(ledger: &Ledger, branch: &str, limit: usize) -> Vec<String> {
    let events = ledger
        .iter_events_filtered(branch, Some("commit"), None, None, None, limit)
        .unwrap_or_default();
    events
        .iter()
        .filter_map(|e| {
            e.payload
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

// ── POST /api/drafts/:id/approve ──

#[derive(Deserialize)]
struct ApproveRequest {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    stage: Option<String>,
}

#[derive(Serialize)]
struct ApprovalResponse {
    event_id: String,
    draft_status: String,
    stage_status: String,
}

async fn post_draft_approve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;
    handle_draft_action(&state, &headers, &draft_id, "approve", &body).await
}

// ── POST /api/drafts/:id/deny ──

async fn post_draft_deny(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;
    handle_draft_action(&state, &headers, &draft_id, "deny", &body).await
}

/// Shared handler for approve/deny actions on drafts.
async fn handle_draft_action(
    state: &AppState,
    headers: &HeaderMap,
    draft_id: &str,
    action: &str,
    body: &ApproveRequest,
) -> Result<Response, AppError> {
    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    // Read the draft
    let draft_path = ledger.paths.drafts_dir.join(format!("{draft_id}.json"));
    if !draft_path.exists() {
        return Err(AppError::NotFound(format!("draft not found: {draft_id}")));
    }
    let content = std::fs::read_to_string(&draft_path)?;
    let mut draft: serde_json::Value = serde_json::from_str(&content)?;

    // Check draft status
    let draft_status = draft
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("proposed");
    if draft_status == "applied" || draft_status == "rejected" {
        return Err(AppError::Conflict(format!(
            "draft {draft_id} is already {draft_status}"
        )));
    }

    let actor = body.actor.as_deref().unwrap_or("human");
    let reason = body.reason.as_deref().unwrap_or("");
    let device_id = headers
        .get("x-edda-device-id")
        .and_then(|v| v.to_str().ok());

    let decision = if action == "approve" {
        "approve"
    } else {
        "reject"
    };

    let head = ledger.head_branch()?;

    // Compute draft SHA256
    let draft_sha256 = {
        use sha2::Digest;
        let bytes = std::fs::read(&draft_path)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };

    let parent_hash = ledger.last_event_hash()?;

    // Handle stage-aware drafts
    let stages = draft
        .get("stages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let (stage_id, stage_role, stage_status) = if !stages.is_empty() {
        // Determine which stage to act on
        let requested_stage = body.stage.as_deref();
        let target_stage = if let Some(sid) = requested_stage {
            stages
                .iter()
                .find(|s| s.get("stage_id").and_then(|v| v.as_str()) == Some(sid))
                .ok_or_else(|| AppError::NotFound(format!("stage not found: {sid}")))?
        } else {
            // Auto-select the first pending stage
            stages
                .iter()
                .find(|s| s.get("status").and_then(|v| v.as_str()) == Some("pending"))
                .ok_or_else(|| AppError::Conflict("no pending stages remain".to_string()))?
        };

        let sid = target_stage
            .get("stage_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let role = target_stage
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let st_status = target_stage
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .to_string();

        if st_status != "pending" {
            return Err(AppError::Conflict(format!(
                "stage '{sid}' is already {st_status}"
            )));
        }

        (sid, role, st_status)
    } else {
        (String::new(), String::new(), "pending".to_string())
    };

    // Replay protection: stage already acted on
    if stage_status != "pending" {
        return Err(AppError::Conflict(format!(
            "draft {draft_id} stage '{}' is already {stage_status}",
            stage_id
        )));
    }

    // Create approval event
    let event = new_approval_event(&ApprovalEventParams {
        branch: &head,
        parent_hash: parent_hash.as_deref(),
        draft_id,
        draft_sha256: &draft_sha256,
        decision,
        actor,
        note: reason,
        stage_id: &stage_id,
        role: &stage_role,
        device_id,
    })?;
    ledger.append_event(&event)?;

    // Update draft JSON
    let ts = event.ts.clone();
    let approval_record = serde_json::json!({
        "ts": ts,
        "actor": actor,
        "decision": decision,
        "note": reason,
        "approval_event_id": event.event_id,
        "stage_id": stage_id,
        "role": stage_role,
    });

    // Append to approvals array
    if let Some(approvals) = draft.get_mut("approvals") {
        if let Some(arr) = approvals.as_array_mut() {
            arr.push(approval_record);
        }
    } else {
        draft["approvals"] = serde_json::json!([approval_record]);
    }

    // Update stage status
    let mut new_stage_status = "pending".to_string();
    if let Some(stages_arr) = draft.get_mut("stages").and_then(|v| v.as_array_mut()) {
        for stage in stages_arr.iter_mut() {
            let sid = stage.get("stage_id").and_then(|v| v.as_str()).unwrap_or("");
            if sid == stage_id {
                if decision == "reject" {
                    stage["status"] = serde_json::Value::String("rejected".to_string());
                    new_stage_status = "rejected".to_string();
                } else {
                    // Read min_approvals first to avoid borrow conflict
                    let min = stage
                        .get("min_approvals")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as usize;
                    // Add actor to approved_by
                    if let Some(ab) = stage.get_mut("approved_by") {
                        if let Some(arr) = ab.as_array_mut() {
                            let actor_val = serde_json::Value::String(actor.to_string());
                            if !arr.contains(&actor_val) {
                                arr.push(actor_val);
                            }
                            if arr.len() >= min {
                                new_stage_status = "approved".to_string();
                            }
                        }
                    }
                    if new_stage_status == "approved" {
                        stage["status"] = serde_json::Value::String("approved".to_string());
                    }
                }
                break;
            }
        }

        // Update draft-level status
        let all_approved = stages_arr
            .iter()
            .all(|s| s.get("status").and_then(|v| v.as_str()) == Some("approved"));
        let any_rejected = stages_arr
            .iter()
            .any(|s| s.get("status").and_then(|v| v.as_str()) == Some("rejected"));

        if any_rejected {
            draft["status"] = serde_json::Value::String("rejected".to_string());
        } else if all_approved {
            draft["status"] = serde_json::Value::String("approved".to_string());
        }
    } else {
        // Flat (no stages) draft
        if decision == "reject" {
            draft["status"] = serde_json::Value::String("rejected".to_string());
            new_stage_status = "rejected".to_string();
        } else {
            let min = draft
                .get("policy_min_approvals")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;
            let count = draft
                .get("approvals")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter(|r| r.get("decision").and_then(|v| v.as_str()) == Some("approve"))
                        .count()
                })
                .unwrap_or(0);
            if count >= min.max(1) {
                draft["status"] = serde_json::Value::String("approved".to_string());
                new_stage_status = "approved".to_string();
            }
        }
    }

    let final_draft_status = draft
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("proposed")
        .to_string();

    // Write updated draft
    std::fs::write(&draft_path, serde_json::to_string_pretty(&draft)?)?;

    // Rebuild derived state
    let snap_branch = ledger.head_branch()?;
    let _ = edda_derive::rebuild_branch(&ledger, &snap_branch);

    let resp = ApprovalResponse {
        event_id: event.event_id,
        draft_status: final_draft_status,
        stage_status: new_stage_status,
    };

    Ok((StatusCode::OK, Json(resp)).into_response())
}

/// Draft-related routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/drafts", get(get_drafts))
        .route("/api/drafts/{id}/approve", post(post_draft_approve))
        .route("/api/drafts/{id}/deny", post(post_draft_deny))
}
