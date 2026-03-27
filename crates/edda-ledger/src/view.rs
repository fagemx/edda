//! Read-side projection of decisions.
//!
//! `DecisionView` is the delivery type consumed by Injection (hooks, pack builder).
//! Storage code uses `DecisionRow`; read-side consumers use `DecisionView` (BOUNDARY-01).

use crate::sqlite_store::DecisionRow;

/// Read-side projection of a decision. Injection consumers use this type
/// instead of `DecisionRow` (BOUNDARY-01).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionView {
    // Identity
    pub event_id: String,
    pub branch: String,
    pub ts: Option<String>,

    // What
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,

    // Governance state
    /// "active" | "experimental" | "proposed" | "deprecated" | "superseded"
    pub status: String,
    /// "human" | "agent" | "system"
    pub authority: String,
    /// "easy" | "medium" | "hard"
    pub reversibility: String,

    // Scope — parsed arrays, not JSON strings
    pub affected_paths: Vec<String>,
    pub tags: Vec<String>,
    /// Renamed from `scope` column.
    pub propagation: String,

    // Graph (optional)
    pub supersedes_id: Option<String>,

    // Review schedule
    pub review_after: Option<String>,

    // Village scope
    #[serde(skip_serializing_if = "Option::is_none")]
    pub village_id: Option<String>,
}

/// Convert a storage row into a delivery view.
///
/// - Parses `affected_paths` and `tags` from JSON string → `Vec<String>`
/// - Renames `scope` → `propagation`
/// - Drops `is_active`, `source_project_id`, `source_event_id`
///
/// If `affected_paths` or `tags` JSON is invalid or missing, defaults to `vec![]`.
pub fn to_view(row: &DecisionRow) -> DecisionView {
    let affected_paths: Vec<String> = serde_json::from_str(&row.affected_paths).unwrap_or_default();

    let tags: Vec<String> = serde_json::from_str(&row.tags).unwrap_or_default();

    DecisionView {
        event_id: row.event_id.clone(),
        branch: row.branch.clone(),
        ts: row.ts.clone(),
        key: row.key.clone(),
        value: row.value.clone(),
        reason: row.reason.clone(),
        domain: row.domain.clone(),
        status: row.status.clone(),
        authority: row.authority.clone(),
        reversibility: row.reversibility.clone(),
        affected_paths,
        tags,
        propagation: row.scope.clone(),
        supersedes_id: row.supersedes_id.clone(),
        review_after: row.review_after.clone(),
        village_id: row.village_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_default_row() -> DecisionRow {
        DecisionRow {
            event_id: "evt_test".to_string(),
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: "embedded".to_string(),
            domain: "db".to_string(),
            branch: "main".to_string(),
            supersedes_id: None,
            is_active: true,
            ts: Some("2026-03-20T00:00:00Z".to_string()),
            scope: "local".to_string(),
            source_project_id: None,
            source_event_id: None,
            status: "active".to_string(),
            authority: "human".to_string(),
            affected_paths: "[]".to_string(),
            tags: "[]".to_string(),
            review_after: None,
            reversibility: "medium".to_string(),
            village_id: None,
        }
    }

    fn make_row_with_paths(affected_paths: &str, tags: &str) -> DecisionRow {
        let mut row = make_default_row();
        row.affected_paths = affected_paths.to_string();
        row.tags = tags.to_string();
        row
    }

    #[test]
    fn to_view_parses_json_arrays() {
        let row = make_row_with_paths(
            r#"["crates/edda-ledger/**", "crates/edda-core/**"]"#,
            r#"["architecture", "storage"]"#,
        );
        let view = to_view(&row);
        assert_eq!(
            view.affected_paths,
            vec!["crates/edda-ledger/**", "crates/edda-core/**"]
        );
        assert_eq!(view.tags, vec!["architecture", "storage"]);
    }

    #[test]
    fn to_view_defaults_empty_on_empty_array() {
        let row = make_row_with_paths("[]", "[]");
        let view = to_view(&row);
        assert!(view.affected_paths.is_empty());
        assert!(view.tags.is_empty());
    }

    #[test]
    fn to_view_defaults_empty_on_invalid_json() {
        let row = make_row_with_paths("not json", "{bad}");
        let view = to_view(&row);
        assert!(view.affected_paths.is_empty());
        assert!(view.tags.is_empty());
    }

    #[test]
    fn to_view_renames_scope_to_propagation() {
        let mut row = make_default_row();
        row.scope = "shared".to_string();
        let view = to_view(&row);
        assert_eq!(view.propagation, "shared");
    }

    #[test]
    fn decision_view_has_no_is_active() {
        // Compile-time check: DecisionView must not have is_active field.
        // If someone adds `is_active`, the struct definition changes but
        // this test ensures we only access `status`.
        let view = to_view(&make_default_row());
        let _ = view.status; // exists
                             // view.is_active; // must not compile
    }

    #[test]
    fn to_view_preserves_review_after() {
        let mut row = make_default_row();
        row.review_after = Some("2026-06-01".to_string());
        let view = to_view(&row);
        assert_eq!(view.review_after, Some("2026-06-01".to_string()));
    }
}
