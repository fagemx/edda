use std::sync::Arc;

use anyhow::Context;
use axum::extract::{Path as AxumPath, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::policy;
use edda_ledger::Ledger;

use crate::error::AppError;
use crate::state::AppState;

// ── POST /api/scope/check ──

#[derive(Deserialize)]
struct ScopeCheckBody {
    project_id: String,
    session_id: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct ScopeCheckResult {
    path: String,
    allowed: bool,
}

#[derive(Serialize)]
struct ScopeCheckResponse {
    session_id: String,
    label: String,
    scope: Vec<String>,
    no_claim: bool,
    all_allowed: bool,
    results: Vec<ScopeCheckResult>,
}

// SECURITY: `project_id` is caller-supplied and not validated against any
// ACL. Acceptable because edda is a single-user local tool; revisit if
// multi-tenant isolation is ever required.
async fn post_scope_check(
    Json(body): Json<ScopeCheckBody>,
) -> Result<Json<ScopeCheckResponse>, AppError> {
    let board = edda_bridge_claude::peers::compute_board_state(&body.project_id);
    let claim = board
        .claims
        .iter()
        .find(|c| c.session_id == body.session_id);

    match claim {
        None => {
            // Permissive default: no claim means all files allowed
            let results = body
                .files
                .iter()
                .map(|f| ScopeCheckResult {
                    path: f.clone(),
                    allowed: true,
                })
                .collect();
            Ok(Json(ScopeCheckResponse {
                session_id: body.session_id,
                label: String::new(),
                scope: vec![],
                no_claim: true,
                all_allowed: true,
                results,
            }))
        }
        Some(claim) => {
            // Build glob set from claim patterns
            let mut builder = globset::GlobSetBuilder::new();
            for pattern in &claim.paths {
                if let Ok(glob) = globset::GlobBuilder::new(pattern)
                    .literal_separator(false)
                    .build()
                {
                    builder.add(glob);
                }
            }
            let glob_set = builder
                .build()
                .map_err(|e| anyhow::anyhow!("invalid glob patterns: {}", e))?;

            let results: Vec<ScopeCheckResult> = body
                .files
                .iter()
                .map(|f| ScopeCheckResult {
                    path: f.clone(),
                    allowed: glob_set.is_match(f),
                })
                .collect();

            let all_allowed = results.iter().all(|r| r.allowed);

            Ok(Json(ScopeCheckResponse {
                session_id: body.session_id,
                label: claim.label.clone(),
                scope: claim.paths.clone(),
                no_claim: false,
                all_allowed,
                results,
            }))
        }
    }
}

// ── GET /api/scope/whitelist ──

#[derive(Deserialize)]
struct WhitelistQuery {
    project_id: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Serialize)]
struct WhitelistClaim {
    session_id: String,
    label: String,
    patterns: Vec<String>,
    ts: String,
}

#[derive(Serialize)]
struct WhitelistResponse {
    claims: Vec<WhitelistClaim>,
}

// SECURITY: `project_id` is caller-supplied and not validated against any
// ACL. Acceptable because edda is a single-user local tool; revisit if
// multi-tenant isolation is ever required.
async fn get_scope_whitelist(
    Query(query): Query<WhitelistQuery>,
) -> Result<Json<WhitelistResponse>, AppError> {
    let board = edda_bridge_claude::peers::compute_board_state(&query.project_id);

    let claims: Vec<WhitelistClaim> = board
        .claims
        .iter()
        .filter(|c| {
            query
                .session_id
                .as_ref()
                .is_none_or(|sid| &c.session_id == sid)
        })
        .map(|c| WhitelistClaim {
            session_id: c.session_id.clone(),
            label: c.label.clone(),
            patterns: c.paths.clone(),
            ts: c.ts.clone(),
        })
        .collect();

    Ok(Json(WhitelistResponse { claims }))
}

// ── POST /api/authz/check ──

async fn post_authz_check(
    State(state): State<Arc<AppState>>,
    Json(body): Json<policy::AuthzRequest>,
) -> Result<Json<policy::AuthzResult>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let pol = policy::load_policy_from_dir(&edda_dir)?;
    let actors = policy::load_actors_from_dir(&edda_dir)?;
    let result = policy::evaluate_authz(&body, &pol, &actors);
    Ok(Json(result))
}

// ── GET /api/tool-tier/:tool_name ──

async fn get_tool_tier(
    State(state): State<Arc<AppState>>,
    AxumPath(tool_name): AxumPath<String>,
) -> Result<Json<edda_core::tool_tier::ToolTierResult>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let config = edda_core::tool_tier::load_tool_tiers_from_dir(&edda_dir)?;
    let result = edda_core::tool_tier::resolve_tool_tier(&config, &tool_name);
    Ok(Json(result))
}

// ── POST /api/approval/check ──

#[derive(Deserialize)]
struct ApprovalCheckRequest {
    step: String,
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    risk_level: Option<edda_core::bundle::RiskLevel>,
    #[serde(default)]
    files_changed: Option<u32>,
    #[serde(default)]
    tests_failed: Option<u32>,
    #[serde(default)]
    off_limits_touched: Option<bool>,
}

async fn post_approval_check(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ApprovalCheckRequest>,
) -> Result<Json<edda_core::approval::ApprovalDecision>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let policy = edda_core::approval::load_approval_policy(&edda_dir)?;

    // Build ReviewBundle from request or from ledger
    let bundle = if let Some(bundle_id) = &body.bundle_id {
        let ledger = Ledger::open(&state.repo_root).context("POST /api/approval/check")?;
        let Some(row) = ledger.get_bundle(bundle_id)? else {
            return Err(AppError::NotFound(format!(
                "Bundle '{}' not found",
                bundle_id
            )));
        };
        let Some(event) = ledger.get_event(&row.event_id)? else {
            return Err(AppError::NotFound(format!(
                "Event for bundle '{}' not found",
                bundle_id
            )));
        };
        serde_json::from_value::<edda_core::bundle::ReviewBundle>(event.payload)?
    } else {
        // Build a synthetic bundle from inline fields
        let risk = body
            .risk_level
            .unwrap_or(edda_core::bundle::RiskLevel::Medium);
        let file_count = body.files_changed.unwrap_or(0) as usize;
        let failed = body.tests_failed.unwrap_or(0);
        let files: Vec<edda_core::bundle::FileChange> = (0..file_count)
            .map(|i| edda_core::bundle::FileChange {
                path: format!("file_{i}"),
                added: 1,
                deleted: 0,
            })
            .collect();
        edda_core::bundle::ReviewBundle {
            bundle_id: "inline".to_string(),
            change_summary: edda_core::bundle::ChangeSummary {
                files,
                total_added: file_count as u32,
                total_deleted: 0,
                diff_ref: "inline".to_string(),
            },
            test_results: edda_core::bundle::TestResults {
                passed: 0,
                failed,
                ignored: 0,
                total: failed,
                failures: vec![],
                command: "inline".to_string(),
            },
            risk_assessment: edda_core::bundle::RiskAssessment {
                level: risk,
                factors: vec![],
            },
            suggested_action: edda_core::bundle::SuggestedAction::Review,
            suggested_reason: "inline check".to_string(),
        }
    };

    let phase_state = edda_core::agent_phase::AgentPhaseState {
        phase: edda_core::agent_phase::AgentPhase::Implement,
        session_id: "api-check".to_string(),
        label: None,
        issue: None,
        pr: None,
        branch: None,
        confidence: 1.0,
        detected_at: String::new(),
        signals: vec![],
    };

    let ctx = edda_core::approval::EvalContext {
        bundle: &bundle,
        phase: &phase_state,
        off_limits_touched: body.off_limits_touched.unwrap_or(false),
        consecutive_failures: 0,
        current_time: Some(time::OffsetDateTime::now_utc()),
    };

    let decision = policy.evaluate(&body.step, &ctx);
    Ok(Json(decision))
}

/// Policy-related routes (scope, authz, approval, tool-tier).
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/scope/check", post(post_scope_check))
        .route("/api/scope/whitelist", get(get_scope_whitelist))
        .route("/api/authz/check", post(post_authz_check))
        .route("/api/approval/check", post(post_approval_check))
        .route("/api/tool-tier/{tool_name}", get(get_tool_tier))
}
