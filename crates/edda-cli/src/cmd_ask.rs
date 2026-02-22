use edda_ask::{ask, format_human, AskOptions, ConversationHit, TranscriptSearchFn};
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
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let q = query.unwrap_or("");

    let opts = AskOptions {
        limit,
        include_superseded: all,
        branch: branch.map(|s| s.to_string()),
    };

    // Build transcript search callback
    let transcript_cb = build_transcript_callback(repo_root);
    let transcript_ref: Option<&edda_ask::TranscriptSearchFn> =
        transcript_cb.as_ref().map(|f| f.as_ref());

    let result = ask(&ledger, q, &opts, transcript_ref)?;

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

    let index = edda_search_fts::schema::ensure_index(&index_dir).ok()?;
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
