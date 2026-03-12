use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedContent {
    pub session_id: String,
    pub timestamp: String,
    pub snippet: String,
    pub relevance_score: f64,
    pub source: String,
}

pub fn find_related_content(
    _query: &str,
    project_root: &std::path::Path,
    _max_results: usize,
) -> Result<Vec<RelatedContent>> {
    let search_dir = project_root.join("search").join("tantivy");

    if !search_dir.exists() {
        return Ok(vec![]);
    }

    // For now, return empty results if FTS index doesn't exist
    // In a full implementation, this would use edda-search-fts to perform BM25 search
    // and handle the case where the index needs to be built

    Ok(vec![])
}

pub fn build_search_index_if_needed(project_root: &std::path::Path) -> Result<()> {
    let search_dir = project_root.join("search").join("tantivy");

    if !search_dir.exists() {
        // Index doesn't exist - would need to trigger index build
        // For now, just log a warning
        tracing::warn!(path = ?search_dir, "FTS index not found, BM25 relations will be skipped");
    }

    Ok(())
}
