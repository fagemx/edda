//! Karvi-specific controls patch adapter.
//!
//! Translates [`ControlsSuggestion`]s from the L2 threshold engine into
//! [`ControlsPatch`] proposals with file-based CRUD storage and audit logging.
//! Approved patches can be posted to the Karvi `/api/controls` endpoint.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use edda_aggregate::controls::{
    evaluate_controls_rules, ControlsSuggestion, MetricKind, ThresholdOp, ThresholdRule,
};

// ── Types ──

/// Status of a controls patch proposal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PatchStatus {
    Pending,
    Approved,
    Dismissed,
    Applied,
}

/// A controls patch proposal targeting Karvi automation controls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlsPatch {
    pub patch_id: String,
    pub created_at: String,
    pub controls: HashMap<String, serde_json::Value>,
    pub suggestions: Vec<ControlsSuggestion>,
    pub status: PatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    patch_id: String,
    action: String,
    actor: Option<String>,
    detail: Option<String>,
}

// ── Default Rules ──

/// Built-in threshold rules for Karvi integration.
pub fn default_rules() -> Vec<ThresholdRule> {
    vec![
        ThresholdRule {
            name: "low-success-disable-dispatch".to_string(),
            metric: MetricKind::SuccessRate,
            operator: ThresholdOp::Lt,
            threshold: 0.60,
            action: "disable_auto_dispatch".to_string(),
            reason_template: "Success rate {value} below {threshold} threshold".to_string(),
        },
        ThresholdRule {
            name: "high-cost-reduce-concurrency".to_string(),
            metric: MetricKind::AvgCostUsd,
            operator: ThresholdOp::Gt,
            threshold: 0.50,
            action: "reduce_concurrency".to_string(),
            reason_template: "Average cost {value} exceeds {threshold}".to_string(),
        },
        ThresholdRule {
            name: "high-latency-flag".to_string(),
            metric: MetricKind::AvgLatencyMs,
            operator: ThresholdOp::Gt,
            threshold: 30000.0,
            action: "flag_slow_model".to_string(),
            reason_template: "Average latency {value} exceeds {threshold}".to_string(),
        },
    ]
}

// ── Suggestion Generation ──

/// Evaluate rules against a quality report and build a `ControlsPatch` if any fire.
///
/// Returns `None` if no rules are triggered or if a pending patch already exists
/// for the same actions within the cooldown period (24h).
pub fn suggest_controls_patch(
    project_id: &str,
    report: &edda_aggregate::quality::QualityReport,
    rules: &[ThresholdRule],
    min_samples: Option<u64>,
) -> Result<Option<ControlsPatch>> {
    let suggestions = evaluate_controls_rules(rules, report, min_samples);
    if suggestions.is_empty() {
        return Ok(None);
    }

    // Cooldown check: skip if a pending patch exists with overlapping actions
    // created within the last 24 hours.
    if let Ok(existing) = list_patches(project_id, Some(&PatchStatus::Pending)) {
        let now = now_rfc3339();
        let cutoff = cooldown_cutoff(&now);
        for patch in &existing {
            if patch.created_at > cutoff {
                let existing_actions: Vec<&str> = patch
                    .suggestions
                    .iter()
                    .map(|s| s.action.as_str())
                    .collect();
                let new_actions: Vec<&str> =
                    suggestions.iter().map(|s| s.action.as_str()).collect();
                let overlap = new_actions.iter().any(|a| existing_actions.contains(a));
                if overlap {
                    return Ok(None);
                }
            }
        }
    }

    // Build controls map from suggestions.
    let mut controls = HashMap::new();
    for s in &suggestions {
        let value = match s.action.as_str() {
            "disable_auto_dispatch" => serde_json::json!(false),
            "reduce_concurrency" => serde_json::json!(1),
            "flag_slow_model" => serde_json::json!(true),
            _ => serde_json::json!(s.action.clone()),
        };
        controls.insert(s.action.clone(), value);
    }

    let patch = ControlsPatch {
        patch_id: new_patch_id(),
        created_at: now_rfc3339(),
        controls,
        suggestions,
        status: PatchStatus::Pending,
        approved_by: None,
        approved_at: None,
        dismiss_reason: None,
        applied_at: None,
    };

    Ok(Some(patch))
}

// ── CRUD ──

/// Generate a new patch ID.
pub fn new_patch_id() -> String {
    format!(
        "cpatch_{}",
        &ulid::Ulid::new().to_string()[..12].to_lowercase()
    )
}

/// Save a controls patch to disk.
pub fn save_patch(project_id: &str, patch: &ControlsPatch) -> Result<()> {
    let path = patch_path(project_id, &patch.patch_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(patch)?;
    fs::write(&path, json)?;

    append_audit(project_id, &patch.patch_id, "created", None, None)?;
    Ok(())
}

/// Load a single patch by ID.
pub fn load_patch(project_id: &str, patch_id: &str) -> Result<ControlsPatch> {
    let path = patch_path(project_id, patch_id);
    let content =
        fs::read_to_string(&path).with_context(|| format!("Patch not found: {patch_id}"))?;
    let patch: ControlsPatch = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse patch: {}", path.display()))?;
    Ok(patch)
}

/// List patches, optionally filtered by status.
pub fn list_patches(
    project_id: &str,
    status_filter: Option<&PatchStatus>,
) -> Result<Vec<ControlsPatch>> {
    let dir = patches_dir(project_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        let patch: ControlsPatch = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse patch: {}", path.display()))?;

        if let Some(filter) = status_filter {
            if &patch.status != filter {
                continue;
            }
        }
        results.push(patch);
    }

    results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(results)
}

/// Approve a patch: mark as approved, return it.
pub fn approve_patch(project_id: &str, patch_id: &str, by: &str) -> Result<ControlsPatch> {
    let mut patch = load_patch(project_id, patch_id)?;
    if patch.status != PatchStatus::Pending {
        anyhow::bail!(
            "Patch {patch_id} is not pending (status: {:?})",
            patch.status
        );
    }

    patch.status = PatchStatus::Approved;
    patch.approved_by = Some(by.to_string());
    patch.approved_at = Some(now_rfc3339());

    let path = patch_path(project_id, patch_id);
    let json = serde_json::to_string_pretty(&patch)?;
    fs::write(&path, json)?;

    append_audit(project_id, patch_id, "approved", Some(by), None)?;
    Ok(patch)
}

/// Dismiss a patch with optional reason.
pub fn dismiss_patch(project_id: &str, patch_id: &str, reason: Option<&str>) -> Result<()> {
    let mut patch = load_patch(project_id, patch_id)?;
    if patch.status != PatchStatus::Pending {
        anyhow::bail!(
            "Patch {patch_id} is not pending (status: {:?})",
            patch.status
        );
    }

    patch.status = PatchStatus::Dismissed;
    patch.dismiss_reason = reason.map(String::from);

    let path = patch_path(project_id, patch_id);
    let json = serde_json::to_string_pretty(&patch)?;
    fs::write(&path, json)?;

    append_audit(project_id, patch_id, "dismissed", None, reason)?;
    Ok(())
}

/// Mark an approved patch as applied (after successfully posting to Karvi).
pub fn mark_applied(project_id: &str, patch_id: &str) -> Result<ControlsPatch> {
    let mut patch = load_patch(project_id, patch_id)?;
    if patch.status != PatchStatus::Approved {
        anyhow::bail!(
            "Patch {patch_id} is not approved (status: {:?})",
            patch.status
        );
    }

    patch.status = PatchStatus::Applied;
    patch.applied_at = Some(now_rfc3339());

    let path = patch_path(project_id, patch_id);
    let json = serde_json::to_string_pretty(&patch)?;
    fs::write(&path, json)?;

    append_audit(project_id, patch_id, "applied", None, None)?;
    Ok(patch)
}

/// Post an approved patch to Karvi `/api/controls`.
///
/// Returns `Ok(())` on success, `Err` if the request fails or the patch
/// is not in the `Approved` state.
pub fn apply_patch_to_karvi(project_id: &str, patch_id: &str, karvi_url: &str) -> Result<()> {
    let patch = load_patch(project_id, patch_id)?;
    if patch.status != PatchStatus::Approved {
        anyhow::bail!(
            "Patch {patch_id} must be approved before applying (status: {:?})",
            patch.status
        );
    }

    let url = format!("{}/api/controls", karvi_url.trim_end_matches('/'));
    let body = serde_json::to_string(&patch.controls)?;

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();

    if let Err(e) = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
    {
        anyhow::bail!("Failed to POST to Karvi {url}: {e}");
    }

    mark_applied(project_id, patch_id)?;
    Ok(())
}

/// Return the current threshold rules.
///
/// Placeholder: always returns `default_rules()`. A future enhancement
/// could load from `.edda/controls-rules.yaml`.
pub fn load_rules() -> Vec<ThresholdRule> {
    default_rules()
}

// ── Storage Helpers ──

fn patches_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("controls_patches")
}

fn patch_path(project_id: &str, patch_id: &str) -> PathBuf {
    patches_dir(project_id).join(format!("{patch_id}.json"))
}

fn audit_log_path(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("controls_patch_audit.jsonl")
}

fn append_audit(
    project_id: &str,
    patch_id: &str,
    action: &str,
    actor: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    use std::io::Write;
    let path = audit_log_path(project_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let entry = AuditEntry {
        ts: now_rfc3339(),
        patch_id: patch_id.to_string(),
        action: action.to_string(),
        actor: actor.map(String::from),
        detail: detail.map(String::from),
    };
    let line = serde_json::to_string(&entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Calculate cooldown cutoff (24 hours before the given timestamp).
/// Returns an ISO 8601 string that can be compared lexicographically.
fn cooldown_cutoff(now: &str) -> String {
    // Parse the timestamp and subtract 24h. On parse failure, return epoch
    // so that cooldown check is effectively skipped.
    time::OffsetDateTime::parse(now, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|t| {
            t.checked_sub(time::Duration::hours(24)).and_then(|t2| {
                t2.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use edda_aggregate::quality::{ModelQuality, QualityReport};

    fn make_report(success_rate: f64, total_steps: u64, cost: f64) -> QualityReport {
        QualityReport {
            models: vec![ModelQuality {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                total_steps,
                success_count: (total_steps as f64 * success_rate) as u64,
                failed_count: total_steps - (total_steps as f64 * success_rate) as u64,
                cancelled_count: 0,
                success_rate,
                avg_cost_usd: if total_steps > 0 {
                    cost / total_steps as f64
                } else {
                    0.0
                },
                avg_latency_ms: 1000.0,
                total_cost_usd: cost,
                total_tokens_in: 0,
                total_tokens_out: 0,
            }],
            total_steps,
            overall_success_rate: success_rate,
            total_cost_usd: cost,
        }
    }

    #[test]
    fn patch_serde_roundtrip() {
        let patch = ControlsPatch {
            patch_id: "cpatch_test123".to_string(),
            created_at: "2026-03-13T10:00:00Z".to_string(),
            controls: {
                let mut m = HashMap::new();
                m.insert("disable_auto_dispatch".to_string(), serde_json::json!(true));
                m
            },
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };
        let json = serde_json::to_string(&patch).unwrap();
        let parsed: ControlsPatch = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.patch_id, "cpatch_test123");
        assert_eq!(parsed.status, PatchStatus::Pending);
    }

    #[test]
    fn suggest_returns_none_when_no_triggers() {
        let report = make_report(0.80, 20, 2.0);
        let rules = default_rules();
        let result = suggest_controls_patch("test_suggest_none", &report, &rules, None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn suggest_returns_patch_when_triggered() {
        let report = make_report(0.30, 20, 2.0);
        let rules = default_rules();
        let result = suggest_controls_patch("test_suggest_some", &report, &rules, None).unwrap();
        assert!(result.is_some());
        let patch = result.unwrap();
        assert_eq!(patch.status, PatchStatus::Pending);
        assert!(!patch.suggestions.is_empty());
        assert!(patch.controls.contains_key("disable_auto_dispatch"));
    }

    #[test]
    fn crud_roundtrip() {
        let pid = "test_controls_crud";
        let _ = edda_store::ensure_dirs(pid);

        let patch = ControlsPatch {
            patch_id: "cpatch_crud1".to_string(),
            created_at: "2026-03-13T10:00:00Z".to_string(),
            controls: HashMap::new(),
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };

        save_patch(pid, &patch).unwrap();
        let loaded = load_patch(pid, "cpatch_crud1").unwrap();
        assert_eq!(loaded.patch_id, "cpatch_crud1");

        let all = list_patches(pid, None).unwrap();
        assert_eq!(all.len(), 1);

        let pending = list_patches(pid, Some(&PatchStatus::Pending)).unwrap();
        assert_eq!(pending.len(), 1);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn approve_and_dismiss_transitions() {
        let pid = "test_controls_transitions";
        let _ = edda_store::ensure_dirs(pid);

        // Approve path
        let p1 = ControlsPatch {
            patch_id: "cpatch_trans1".to_string(),
            created_at: "2026-03-13T10:00:00Z".to_string(),
            controls: HashMap::new(),
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };
        save_patch(pid, &p1).unwrap();

        let approved = approve_patch(pid, "cpatch_trans1", "alice").unwrap();
        assert_eq!(approved.status, PatchStatus::Approved);
        assert_eq!(approved.approved_by.as_deref(), Some("alice"));

        // Cannot approve again
        assert!(approve_patch(pid, "cpatch_trans1", "bob").is_err());

        // Dismiss path
        let p2 = ControlsPatch {
            patch_id: "cpatch_trans2".to_string(),
            created_at: "2026-03-13T10:01:00Z".to_string(),
            controls: HashMap::new(),
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };
        save_patch(pid, &p2).unwrap();

        dismiss_patch(pid, "cpatch_trans2", Some("not needed")).unwrap();
        let dismissed = load_patch(pid, "cpatch_trans2").unwrap();
        assert_eq!(dismissed.status, PatchStatus::Dismissed);
        assert_eq!(dismissed.dismiss_reason.as_deref(), Some("not needed"));

        // Cannot dismiss again
        assert!(dismiss_patch(pid, "cpatch_trans2", None).is_err());

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn mark_applied_requires_approved() {
        let pid = "test_controls_applied";
        let _ = edda_store::ensure_dirs(pid);

        let p1 = ControlsPatch {
            patch_id: "cpatch_app1".to_string(),
            created_at: "2026-03-13T10:00:00Z".to_string(),
            controls: HashMap::new(),
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };
        save_patch(pid, &p1).unwrap();

        // Cannot mark pending as applied
        assert!(mark_applied(pid, "cpatch_app1").is_err());

        // Approve then apply
        approve_patch(pid, "cpatch_app1", "human").unwrap();
        let applied = mark_applied(pid, "cpatch_app1").unwrap();
        assert_eq!(applied.status, PatchStatus::Applied);
        assert!(applied.applied_at.is_some());

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn audit_log_records_actions() {
        let pid = "test_controls_audit";
        let _ = edda_store::ensure_dirs(pid);

        let patch = ControlsPatch {
            patch_id: "cpatch_audit1".to_string(),
            created_at: "2026-03-13T10:00:00Z".to_string(),
            controls: HashMap::new(),
            suggestions: vec![],
            status: PatchStatus::Pending,
            approved_by: None,
            approved_at: None,
            dismiss_reason: None,
            applied_at: None,
        };
        save_patch(pid, &patch).unwrap();
        approve_patch(pid, "cpatch_audit1", "human").unwrap();

        let path = audit_log_path(pid);
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("created"));
        assert!(lines[1].contains("approved"));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn new_patch_id_format() {
        let id = new_patch_id();
        assert!(id.starts_with("cpatch_"));
        assert_eq!(id.len(), 19); // "cpatch_" (7) + 12 chars
    }

    #[test]
    fn cooldown_cutoff_works() {
        let cutoff = cooldown_cutoff("2026-03-13T12:00:00Z");
        assert_eq!(cutoff, "2026-03-12T12:00:00Z");
    }

    #[test]
    fn patch_status_serde() {
        for (status, expected) in [
            (PatchStatus::Pending, "\"pending\""),
            (PatchStatus::Approved, "\"approved\""),
            (PatchStatus::Dismissed, "\"dismissed\""),
            (PatchStatus::Applied, "\"applied\""),
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, expected);
            let parsed: PatchStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }
}
