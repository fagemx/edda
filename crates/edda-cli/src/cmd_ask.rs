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
        return Ok(());
    }

    // Emptiness is counted, not detected from the rendering: `format_human`
    // prints its own "No results found." for an empty result, so asking whether
    // the render is blank is a question that is never answered yes.
    print!("{}", format_human(&result));
    if hit_count(&result) == 0 {
        if let Some(hint) = fleet_hint_for_ask(repo_root, q, &opts) {
            println!("{hint}");
        }
    }

    Ok(())
}

/// Everything `format_human` would render as a hit.
///
/// The count has to span every collection, not the obvious two: a project whose
/// only hit is a commit or a transcript turn must not probe as empty, or the
/// hint reports absence where `--fleet` finds answers — the very failure this
/// hint exists to remove, one level down.
fn hit_count(r: &edda_ask::AskResult) -> usize {
    r.decisions.len()
        + r.timeline.len()
        + r.related_commits.len()
        + r.related_notes.len()
        + r.conversations.len()
        + r.dependents.len()
}

/// Ask the rest of the fleet whether a local miss is really absence (GH-407,
/// acceptance 4).
///
/// Probes exactly as `execute_fleet` reads, transcript callback included. A line
/// that says "rerun with --fleet" is a promise about what that command will
/// show, so anything the probe declines to look at is a promise it cannot keep.
fn fleet_hint_for_ask(repo_root: &Path, q: &str, opts: &AskOptions) -> Option<String> {
    if q.trim().is_empty() {
        return None; // no question was asked; there is nothing to look for elsewhere
    }
    let scope = edda_store::registry::fleet_scope(repo_root);
    let home = edda_store::project_id(repo_root);
    crate::fleet::elsewhere_hint(&scope, &home, "result", |entry| {
        let root = Path::new(&entry.path);
        let ledger = Ledger::open(root)?;
        let cb = build_transcript_callback(root, Some(&entry.name));
        let cb_ref: Option<&TranscriptSearchFn> = cb.as_ref().map(|f| f.as_ref());
        Ok(hit_count(&ask(&ledger, q, opts, cb_ref)?))
    })
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
        let projects = hits
            .iter()
            .map(|h| serde_json::json!({ "project": h.project, "result": h.item }))
            .collect();
        let payload = crate::fleet::json_envelope(projects, &misses);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    // Counted, not detected from the rendering: `format_human` prints its own
    // "No results found." for an empty result, so a blank-body test never fires
    // and every project — including the silent ones — got a section header and a
    // line saying it had nothing.
    let mut answered = 0;
    for hit in &hits {
        if hit_count(&hit.item) == 0 {
            continue;
        }
        answered += 1;
        println!("── [{}] ──────────────────────────", hit.project);
        print!("{}", format_human(&hit.item));
    }

    // Misses are printed as results, not swallowed: a fleet read that quietly
    // skipped a repo would answer "nothing there" when the truth is "did not
    // look", which is the failure this whole verb exists to remove.
    crate::fleet::print_misses(&misses);

    if answered == 0 {
        println!(
            "{}",
            crate::fleet::empty_summary("results", &format!(" for: {q}"), scope.len(), &misses)
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe decides whether the fleet gets mentioned at all, so it has to
    /// count every kind of hit `format_human` renders. Counting only the obvious
    /// two made a project whose hit was a note or a transcript turn probe as
    /// empty, which reports absence where `--fleet` finds answers — the exact
    /// failure the hint exists to remove, one level down.
    #[test]
    fn a_hit_of_any_kind_counts_not_just_decisions_and_timeline() {
        let mut r = edda_ask::AskResult {
            query: "q".to_string(),
            input_type: "keyword".to_string(),
            decisions: Vec::new(),
            timeline: Vec::new(),
            related_commits: Vec::new(),
            related_notes: Vec::new(),
            conversations: Vec::new(),
            dependents: Vec::new(),
            override_risk: None,
        };
        assert_eq!(hit_count(&r), 0, "an empty result is empty");

        r.related_notes.push(edda_ask::NoteHit {
            event_id: "evt_1".to_string(),
            ts: "2026-07-16T00:00:00Z".to_string(),
            text: "the answer is here".to_string(),
            branch: "main".to_string(),
        });
        assert_eq!(
            hit_count(&r),
            1,
            "a note-only hit must not probe as absence"
        );
    }

    /// The contract three branches were built on, and none of them checked.
    ///
    /// `format_human` prints its own "No results found." for an empty result, so
    /// `body.trim().is_empty()` is a question that is never answered yes. That
    /// one wrong assumption shipped three dead branches across three PRs — the
    /// local hint, `--fleet`'s empty-project skip, and `--fleet`'s empty summary
    /// — each of which read correctly and never ran. Emptiness is counted here,
    /// never inferred from the rendering.
    #[test]
    fn an_empty_result_still_renders_text_so_emptiness_must_be_counted() {
        let empty = edda_ask::AskResult {
            query: "q".to_string(),
            input_type: "keyword".to_string(),
            decisions: Vec::new(),
            timeline: Vec::new(),
            related_commits: Vec::new(),
            related_notes: Vec::new(),
            conversations: Vec::new(),
            dependents: Vec::new(),
            override_risk: None,
        };

        assert_eq!(hit_count(&empty), 0, "nothing was found");
        assert!(
            !format_human(&empty).trim().is_empty(),
            "the render is NOT blank — it says 'No results found.' — so any \
             branch gated on a blank body is dead code"
        );
    }
}
