//! Row structs, enums, and parameter types for the SQLite store.
//!
//! Only storage-internal types are defined here. Public domain types
//! live in `crate::domain` and are re-exported for internal use.

// Re-export domain types so internal sqlite_store code can use them unchanged.
pub use crate::domain::{
    BundleRow, DayCount, DecideSnapshotRow, DependencyEdge, DetectedPattern, DeviceTokenRow,
    DomainCount, ExecutionLinked, ImportParams, OutcomeMetrics, PatternType, SuggestionRow,
    TaskBriefRow, VillageStats, VillageStatsPeriod,
};

/// Backwards-compatible alias: `DepRow` → `DependencyEdge`.
pub type DepRow = DependencyEdge;

/// A row from the `decisions` table (storage-internal).
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

/// An entry in a causal chain traversal result (storage-internal).
#[derive(Debug, Clone)]
pub struct ChainEntry {
    pub decision: DecisionRow,
    pub relation: String,
    pub depth: usize,
}
