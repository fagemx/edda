use edda_ask::{
    affected_paths_for_hits, ask, format_human, staleness::annotate_hits, AskOptions,
    ConversationHit, TranscriptSearchFn,
};
use edda_ledger::Ledger;
use std::path::Path;

/// `edda ask [query]` — query project decisions, history, and conversations.
pub fn execute(
    repo_root: &Path,
    query: Option<&str>,
    limit: usize,
    json: bool,
    all: bool,
    branch: Option<&str>,
    impact: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let q = query.unwrap_or("");

    let opts = AskOptions {
        limit,
        include_superseded: all,
        branch: branch.map(|s| s.to_string()),
        impact,
        ..Default::default()
    };

    // Build transcript search callback
    let transcript_cb = build_transcript_callback(repo_root);
    let transcript_ref: Option<&edda_ask::TranscriptSearchFn> =
        transcript_cb.as_ref().map(|f| f.as_ref());

    let mut result = ask(&ledger, q, &opts, transcript_ref)?;

    // EDDA-STALENESS1 q334: annotate decisions whose affected_paths have
    // shifted since the decision was recorded. Query-time derivation; ledger
    // untouched. Best-effort: file-system errors leave staleness=None.
    let decisions_paths = affected_paths_for_hits(&ledger, &result.decisions);
    annotate_hits(&mut result.decisions, &decisions_paths, Some(repo_root));
    let timeline_paths = affected_paths_for_hits(&ledger, &result.timeline);
    annotate_hits(&mut result.timeline, &timeline_paths, Some(repo_root));

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", format_human(&result));
    }

    Ok(())
}

/// Build a transcript search callback using Tantivy, if index exists.
fn build_transcript_callback(repo_root: &Path) -> Option<Box<TranscriptSearchFn>> {
    let project_id = edda_store::project_id(repo_root);
    let index_dir = edda_store::project_dir(&project_id)
        .join("search")
        .join("tantivy");

    if !index_dir.exists() {
        return None;
    }
    // GH-402: don't search a stale-schema index — its CJK results would be
    // silently wrong. Skip transcript search and hint the rebuild instead.
    if edda_search_fts::schema::index_is_outdated(&index_dir) {
        eprintln!("(search index is out of date; run `edda search index` to include transcripts)");
        return None;
    }

    let index = edda_search_fts::schema::open_index(&index_dir)?;
    let pid = project_id.clone();

    Some(Box::new(move |query: &str, limit: usize| {
        let opts = edda_search_fts::search::SearchOptions {
            project_id: Some(&pid),
            doc_type: Some("turn"),
            ..Default::default()
        };
        let results =
            edda_search_fts::search::search(&index, query, &opts, limit).unwrap_or_default();
        results
            .into_iter()
            .map(|r| ConversationHit {
                doc_id: r.doc_id,
                session_id: r.session_id,
                ts: r.ts,
                snippet: r.snippet,
                rank: r.rank,
            })
            .collect()
    }))
}
