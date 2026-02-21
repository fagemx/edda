//! Public render API for external integrations (CLI consumers).
//!
//! Thin wrappers around internal dispatch/peers render functions,
//! providing a stable public interface for `edda bridge claude render-*` commands.

/// Static write-back protocol text that teaches agents to use `edda decide` and `edda note`.
pub fn writeback() -> String {
    crate::dispatch::render_write_back_protocol("").unwrap_or_default()
}

/// Workspace context rendered from the `.edda/` ledger in `cwd`.
///
/// Returns `None` if no workspace exists at `cwd`.
pub fn workspace(cwd: &str, budget: usize) -> Option<String> {
    crate::dispatch::render_workspace_section(cwd, budget)
}

/// Full L2 coordination protocol (peers, claims, bindings, requests).
///
/// Returns `None` in solo mode with no bindings.
pub fn coordination(project_id: &str, session_id: &str) -> Option<String> {
    crate::peers::render_coordination_protocol(project_id, session_id, "")
}

/// Read the existing hot pack file (recent turns summary).
///
/// Returns `None` if no pack has been built yet for this project.
/// Note: this reads the last-built pack, not a fresh build.
pub fn pack(project_id: &str) -> Option<String> {
    crate::dispatch::read_hot_pack(project_id)
}

/// Active plan excerpt from `.claude/plans/*.md`.
///
/// Returns `None` if no plan file exists.
pub fn plan(project_id: Option<&str>) -> Option<String> {
    crate::dispatch::render_active_plan(project_id)
}
