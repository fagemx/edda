//! SHA-pinned independent review payload (unstable schema review_verdict/0).
use serde::{Deserialize, Serialize};

/// Independent review verdict pinned to a git range (GH-652). Unstable
/// (outside spec v1); fields are only ever added.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewVerdictPayload {
    pub schema: String,
    pub subject: ReviewSubject,
    pub refs: ReviewRefs,
    pub spec: ReviewSpec,
    pub brief: ReviewBrief,
    pub reviewer: ReviewReviewer,
    pub independence: String,
    /// "session" (default) or "model" — which independence grades disqualify.
    pub independence_policy: String,
    pub gates: ReviewGates,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<ReviewProbe>,
    pub verdict: String,
    pub outcome: String,
    pub qualified: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disqualifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<ReviewFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<ReviewChecklistItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalations: Vec<String>,
    pub cost: ReviewCost,
    pub parse: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSubject {
    pub base_sha: String,
    pub head_sha: String,
    pub files: usize,
    pub lines: usize,
    pub coverage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_seen: Option<String>,
    /// Product-owned review worktree proof captured after the engine returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_check: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// `None` for unreviewed events (they do not consume a round number).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    #[serde(default)]
    pub history_rewritten: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSpec {
    pub mode: String,
    pub source: String,
    pub trust: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewBrief {
    pub core: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_md_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReviewer {
    pub agent: String,
    pub transport: String,
    pub model_requested: String,
    pub model_observed: String,
    pub observed_via: String,
    /// What the engine claimed to be. Recorded, never evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_self_report: Option<String>,
    /// UUID (backends such as claude require it); the human label is separate.
    pub session_id: String,
    pub session_label: String,
    pub tool_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewGates {
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<ReviewGateRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ran: Vec<ReviewGateRan>,
}

/// `result`: "green" | "red" | "pending" (pending only for CI check-runs still running).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewGateRead {
    pub kind: String,
    pub r#ref: String,
    pub cmd: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewGateRan {
    pub cmd: String,
    pub exit: i32,
    pub duration_ms: u64,
    /// `None` when the stdout tail could not be stored — recorded loudly in
    /// `notes`, and such a RAN never counts toward `gates.status = verified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_blob: Option<String>,
    /// Killed at the RAN deadline (exit is -1 then).
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewProbe {
    pub cmd: String,
    pub exit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub id: String,
    pub severity: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub claim: String,
    pub evidence: String,
    pub rule: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewChecklistItem {
    pub item: String,
    pub result: String,
    pub measure: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    pub measured: bool,
    pub duration_ms: u64,
}
