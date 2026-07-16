use edda_ask::{
    affected_paths_for_hits, ask, format_human, staleness::annotate_hits, AskOptions,
    ConversationHit, TranscriptSearchFn,
};
use edda_ledger::Ledger;
use std::path::Path;

/// `edda ask [query]` — query project decisions, history, and conversations.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    repo_root: &Path,
    query: Option<&str>,
    limit: usize,
    json: bool,
    all: bool,
    branch: Option<&str>,
    impact: bool,
    fleet: bool,
) -> anyhow::Result<()> {
    let q = query.unwrap_or("");

    let opts = AskOptions {
        limit,
        include_superseded: all,
        branch: branch.map(|s| s.to_string()),
        impact,
        ..Default::default()
    };

    if fleet {
        return execute_fleet(repo_root, q, &opts, json);
    }

    let ledger = Ledger::open(repo_root)?;

    // Build transcript search callback
    let transcript_cb = build_transcript_callback(repo_root, None);
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

/// `edda ask --fleet` — the same question, asked of every project in scope.
///
/// Rendered as per-project sections rather than one merged, ranked list. That is
/// deliberate: relevance scores are TF-IDF against a *corpus*, so a score from
/// the foundry ledger and one from the edda ledger are not comparable, and
/// interleaving them by rank would invent an ordering that means nothing.
/// Sections also keep every hit unambiguously attributed to its home.
fn execute_fleet(repo_root: &Path, q: &str, opts: &AskOptions, json: bool) -> anyhow::Result<()> {
    let scope = edda_store::registry::fleet_scope(repo_root);

    let (hits, misses) = crate::fleet::fan_out(&scope, |entry| {
        let root = Path::new(&entry.path);
        let ledger = Ledger::open(root)?;
        let cb = build_transcript_callback(root, Some(&entry.name));
        let cb_ref: Option<&TranscriptSearchFn> = cb.as_ref().map(|f| f.as_ref());
        let mut result = ask(&ledger, q, opts, cb_ref)?;

        let decisions_paths = affected_paths_for_hits(&ledger, &result.decisions);
        annotate_hits(&mut result.decisions, &decisions_paths, Some(root));
        let timeline_paths = affected_paths_for_hits(&ledger, &result.timeline);
        annotate_hits(&mut result.timeline, &timeline_paths, Some(root));

        Ok(vec![result])
    });

    if json {
        let payload = serde_json::json!({
            "fleet": hits.iter().map(|h| serde_json::json!({
                "project": h.project,
                "result": h.item,
            })).collect::<Vec<_>>(),
            "unreadable": misses.iter().map(|m| serde_json::json!({
                "project": m.project,
                "reason": m.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let mut answered = 0;
    for hit in &hits {
        let body = format_human(&hit.item);
        if body.trim().is_empty() {
            continue;
        }
        answered += 1;
        println!("── [{}] ──────────────────────────", hit.project);
        print!("{body}");
    }

    // Misses are printed as results, not swallowed: a fleet read that quietly
    // skipped a repo would answer "nothing there" when the truth is "did not
    // look", which is the failure this whole verb exists to remove.
    for miss in &misses {
        println!("  [{}] unreadable: {}", miss.project, miss.reason);
    }

    if answered == 0 && misses.is_empty() {
        println!("No results across {} project(s) for: {q}", scope.len());
    }

    Ok(())
}

/// Build a transcript search callback using Tantivy, if index exists.
///
/// `label` names the project when fanning out. It must be `Some` for every
/// fleet call: the notice below was written for a single workspace, where "the
/// index" is unambiguous — fanned out over 16 projects an unattributed line says
/// nothing about which one is stale, repeats once per project, and prescribes a
/// remedy that (post-GH-414) needs the very project id it failed to mention.
fn build_transcript_callback(
    repo_root: &Path,
    label: Option<&str>,
) -> Option<Box<TranscriptSearchFn>> {
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
        match label {
            Some(name) => eprintln!(
                "  [{name}] search index is out of date; \
                 run `edda search index --project {project_id}` to include its transcripts"
            ),
            None => eprintln!(
                "(search index is out of date; run `edda search index` to include transcripts)"
            ),
        }
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
