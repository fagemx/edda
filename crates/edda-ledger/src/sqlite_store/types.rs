//! Row structs, enums, and parameter types for the SQLite store.

/// A row from the `decisions` table.
#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,
    pub branch: String,
    pub supersedes_id: Option<String>,
    pub is_active: bool,
    pub ts: Option<String>,
    /// Decision propagation scope: "local", "shared", or "global".
    pub scope: String,
    /// Source project ID if this decision was imported from another project.
    pub source_project_id: Option<String>,
    /// Source event ID if this decision was imported from another project.
    pub source_event_id: Option<String>,
    /// Lifecycle status: "proposed", "active", "experimental", "deprecated", "superseded"
    pub status: String,
    /// Decision authority: "human", "agent", "system"
    pub authority: String,
    /// JSON array of glob patterns for guarded file paths
    pub affected_paths: String,
    /// JSON array of tag strings
    pub tags: String,
    /// Optional ISO-8601 date for scheduled re-evaluation
    pub review_after: Option<String>,
    /// Reversibility level: "easy", "medium", "hard"
    pub reversibility: String,
    /// Village scope identifier
    pub village_id: Option<String>,
}

/// An entry in a causal chain traversal result.
#[derive(Debug, Clone)]
pub struct ChainEntry {
    pub decision: DecisionRow,
    pub relation: String,
    pub depth: usize,
}

/// A row from the `review_bundles` table.
#[derive(Debug, Clone)]
pub struct BundleRow {
    pub event_id: String,
    pub bundle_id: String,
    pub status: String,
    pub risk_level: String,
    pub total_added: i64,
    pub total_deleted: i64,
    pub files_changed: i64,
    pub tests_passed: i64,
    pub tests_failed: i64,
    pub suggested_action: String,
    pub branch: String,
    pub created_at: String,
}

/// A row from the `decision_deps` table.
#[derive(Debug, Clone)]
pub struct DepRow {
    pub source_key: String,
    pub target_key: String,
    pub dep_type: String,
    pub created_event: Option<String>,
    pub created_at: String,
}

/// Statistics for a village's decisions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VillageStats {
    pub village_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<VillageStatsPeriod>,
    pub total_decisions: usize,
    pub decisions_per_day: f64,
    pub by_status: std::collections::HashMap<String, usize>,
    pub by_authority: std::collections::HashMap<String, usize>,
    pub top_domains: Vec<DomainCount>,
    pub rollback_rate: f64,
    pub trend: Vec<DayCount>,
}

/// Time period for village stats.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VillageStatsPeriod {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// The type of recurring pattern detected.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    RecurringDecision,
    ChiefRepeatedAction,
    RollbackTrend,
}

/// A single detected pattern in a village's decision history.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedPattern {
    pub pattern_type: PatternType,
    pub key: String,
    pub domain: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    pub occurrences: usize,
    pub first_seen: String,
    pub last_seen: String,
    pub dates: Vec<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trending_up: Option<bool>,
}

/// Result of pattern detection for a village.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternDetectionResult {
    pub village_id: String,
    pub lookback_days: u32,
    pub after: String,
    pub total_patterns: usize,
    pub patterns: Vec<DetectedPattern>,
}

/// Domain with decision count.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DomainCount {
    pub domain: String,
    pub count: usize,
}

/// Daily decision count.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DayCount {
    pub date: String,
    pub count: usize,
}

/// Parameters for inserting an imported decision from another project.
pub struct ImportParams<'a> {
    pub event: &'a edda_core::types::Event,
    pub key: &'a str,
    pub value: &'a str,
    pub reason: &'a str,
    pub domain: &'a str,
    pub scope: &'a str,
    pub source_project_id: &'a str,
    pub source_event_id: &'a str,
    pub is_active: bool,
}

/// A row from the `task_briefs` table.
#[derive(Debug, Clone)]
pub struct TaskBriefRow {
    pub task_id: String,
    pub intake_event_id: String,
    pub title: String,
    pub intent: edda_core::types::TaskBriefIntent,
    pub source_url: String,
    pub status: edda_core::types::TaskBriefStatus,
    pub branch: String,
    pub iterations: i64,
    pub artifacts: String,
    pub decisions: String,
    pub last_feedback: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A row from the `device_tokens` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceTokenRow {
    pub token_hash: String,
    pub device_name: String,
    pub paired_at: String,
    pub paired_from_ip: String,
    pub revoked_at: Option<String>,
    pub pair_event_id: String,
    pub revoke_event_id: Option<String>,
}

/// A row from the `decide_snapshots` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecideSnapshotRow {
    pub event_id: String,
    pub context_hash: String,
    pub engine_version: String,
    pub schema_version: String,
    pub redaction_level: String,
    pub village_id: Option<String>,
    pub cycle_id: Option<String>,
    pub has_blobs: bool,
    pub created_at: String,
}

/// A row from the `suggestions` table.
#[derive(Debug, Clone)]
pub struct SuggestionRow {
    pub id: String,
    pub event_type: String,
    pub source_layer: String,
    pub source_refs: String,
    pub summary: String,
    pub suggested_because: String,
    pub detail: String,
    pub tags: String,
    pub status: String,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

/// Aggregated outcome metrics for a decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutcomeMetrics {
    pub decision_event_id: String,
    pub decision_key: String,
    pub decision_value: String,
    pub decision_ts: String,
    pub total_executions: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub cancelled_count: u64,
    pub success_rate: f64,
    pub total_cost_usd: f64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub avg_latency_ms: f64,
    pub first_execution_ts: Option<String>,
    pub last_execution_ts: Option<String>,
}

/// An execution event linked to a decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLinked {
    pub event_id: String,
    pub ts: String,
    pub status: String,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub token_in: Option<u64>,
    pub token_out: Option<u64>,
    pub latency_ms: Option<u64>,
}
