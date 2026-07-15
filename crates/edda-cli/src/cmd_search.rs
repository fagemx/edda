use clap::Subcommand;
use edda_index::fetch_store_line;
use edda_ledger::Ledger;
use edda_search_fts::{schema, search, sync};
use edda_store::project_dir;
use std::path::{Path, PathBuf};

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum SearchCmd {
    /// Build or update search index (Tantivy)
    Index {
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
        /// Session ID (index single session instead of all)
        #[arg(long)]
        session: Option<String>,
    },
    /// Search for events and transcript turns
    Query {
        /// Search query (fuzzy for ASCII; "exact"; /regex/ over indexed terms —
        /// note: regex matches tokenized terms, so CJK regex only spans 2 chars)
        query: String,
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
        /// Session ID filter
        #[arg(long)]
        session: Option<String>,
        /// Filter by document type: event or turn
        #[arg(long, name = "type")]
        doc_type: Option<String>,
        /// Filter by event type: note, commit, merge, etc.
        #[arg(long)]
        event_type: Option<String>,
        /// Exact match (disable fuzzy)
        #[arg(long)]
        exact: bool,
        /// Maximum results (default: 20)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show full content of a specific turn
    Show {
        /// Turn ID (from search results)
        #[arg(long)]
        turn: String,
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
    },
}

// ── Dispatch ──

pub fn run_cmd(cmd: SearchCmd, repo_root: &Path) -> anyhow::Result<()> {
    let default_pid = resolve_project_id(repo_root);
    match cmd {
        SearchCmd::Index { project, session } => {
            let pid = project.as_deref().unwrap_or(&default_pid);
            index(repo_root, pid, session.as_deref())
        }
        SearchCmd::Query {
            query: q,
            project,
            session,
            doc_type,
            event_type,
            exact,
            limit,
        } => {
            let pid = project.as_deref().unwrap_or(&default_pid);
            query(
                repo_root,
                pid,
                &q,
                session.as_deref(),
                doc_type.as_deref(),
                event_type.as_deref(),
                exact,
                limit,
            )
        }
        SearchCmd::Show { turn, project } => {
            let pid = project.as_deref().unwrap_or(&default_pid);
            show(pid, &turn)
        }
    }
}

// ── Command Implementations ──

/// Execute `edda search <query>` — full-text search over the Tantivy index.
#[allow(clippy::too_many_arguments)]
pub fn query(
    repo_root: &Path,
    project_id: &str,
    query_str: &str,
    session_id: Option<&str>,
    doc_type: Option<&str>,
    event_type: Option<&str>,
    exact: bool,
    limit: usize,
) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let index_dir = proj_dir.join("search").join("tantivy");

    // GH-403: an unusable index is not a dead end. Announce, then fix it — a
    // silent 25s stall reads as a hang, and telling the user to go run another
    // command is the complaint this issue exists to answer.
    let missing = !index_dir.exists();
    let outdated = schema::index_is_outdated(&index_dir);
    if missing || outdated {
        if missing {
            println!("No search index — building now (one-time)…");
        } else {
            println!("Search index schema is outdated — rebuilding now (one-time)…");
        }
        // Build from the ledger that actually backs this project, which may not
        // be the repo we are standing in (GH-414). Degrade rather than hard-fail:
        // a query that cannot work out where to build from should say so, not
        // abort with an error.
        let ledger_root = match ledger_root_for(repo_root, project_id, |pid| {
            edda_store::registry::get_project(pid).map(|e| e.path)
        }) {
            Ok(root) => root,
            Err(e) => {
                eprintln!("{e}");
                return Ok(());
            }
        };
        let ledger = Ledger::open(&ledger_root)?;
        let stats = sync::sync(&proj_dir, project_id, None, |after| {
            ledger.events_after_rowid(after)
        })?;
        println!(
            "Indexed {} event(s) + {} turn(s).\n",
            stats.events, stats.turns
        );
    }

    // Read-only open: answering a query must never wipe/recreate the index.
    let Some(index) = schema::open_index(&index_dir) else {
        eprintln!("Search index could not be opened. Run `edda search index` to rebuild.");
        return Ok(());
    };
    let opts = search::SearchOptions {
        project_id: Some(project_id),
        session_id,
        doc_type,
        event_type,
        exact,
    };
    let results = search::search(&index, query_str, &opts, limit)?;

    if results.is_empty() {
        println!("No results found for: {query_str}");
        print_watermark(repo_root, &proj_dir, project_id);
        return Ok(());
    }

    println!("Found {} result(s) for: {query_str}\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let type_label = if r.doc_type == "event" {
            format!("[{}]", r.event_type)
        } else {
            "[turn]".to_string()
        };
        let sid_display = if r.session_id.is_empty() {
            String::new()
        } else {
            format!(" session={}", &r.session_id[..r.session_id.len().min(8)])
        };
        println!(
            "  {}. {} {}{} ts={}",
            i + 1,
            type_label,
            r.doc_id,
            sid_display,
            r.ts,
        );
        if !r.snippet.is_empty() {
            println!("     {}\n", r.snippet.replace('\n', " "));
        } else {
            println!();
        }
    }

    print_watermark(repo_root, &proj_dir, project_id);
    Ok(())
}

/// Report how current the index is (GH-403), so silence is never mistaken for
/// absence. Best-effort: a broken watermark must not fail a query that already
/// produced results.
///
/// `indexed through` is read from the project's own meta.sqlite and is therefore
/// correct for any project. The "newer events" count is not: it can only be
/// computed against a ledger, and the only ledger we have is `repo_root`'s. For
/// a `--project` that is not this repo, that count would compare an unrelated
/// project's cursor against these events — a fabricated number. Since the point
/// of this line is honesty about staleness, a confidently wrong count is worse
/// than none, so it is omitted rather than guessed.
fn print_watermark(repo_root: &Path, proj_dir: &Path, project_id: &str) {
    let meta_path = proj_dir.join("search").join("meta.sqlite");
    let Ok(conn) = schema::ensure_meta_db(&meta_path) else {
        return;
    };
    let Ok(cursor) = schema::read_events_cursor(&conn, project_id) else {
        return;
    };
    let Some(ts) = cursor.ts else {
        return;
    };
    if project_id != resolve_project_id(repo_root) {
        println!("  (indexed through {ts})");
        return;
    }
    let newer = Ledger::open(repo_root)
        .and_then(|l| l.events_after_rowid(cursor.rowid))
        .map(|v| v.len())
        .unwrap_or(0);
    if newer > 0 {
        println!("  (indexed through {ts}; {newer} newer event(s) not yet indexed)");
    } else {
        println!("  (indexed through {ts})");
    }
}

/// Decide whose ledger backs `project_id` (GH-414).
///
/// `--project` names a project independently of where the user is standing, so
/// the two can diverge. Pairing this repo's ledger with another project's index
/// writes these events into that project — silently, and reported as success.
///
/// The registry knows where each project lives, so a foreign project is resolved
/// to its own repo rather than refused. `lookup` is injected (production passes
/// `edda_store::get_project`) because resolving it for real reads the
/// process-wide store root, which a test cannot redirect without corrupting its
/// siblings.
fn ledger_root_for(
    repo_root: &Path,
    project_id: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> anyhow::Result<PathBuf> {
    if project_id == resolve_project_id(repo_root) {
        return Ok(repo_root.to_path_buf());
    }

    let Some(path) = lookup(project_id) else {
        anyhow::bail!(
            "Project {project_id} is not in the registry, so there is no way to tell \
             whose events it holds. Run `edda search index` from that project's repo."
        );
    };

    // Same staleness rule the registry itself uses: a live repo has a `.edda/`.
    // Falling back to repo_root here would be the bug this exists to fix.
    let root = PathBuf::from(path);
    if !root.join(".edda").is_dir() {
        anyhow::bail!(
            "Project {project_id} is registered at {} but that path no longer holds \
             an edda ledger. Run `edda search index` from that project's repo.",
            root.display()
        );
    }
    Ok(root)
}

/// Execute `edda search index` — build/update the Tantivy index for a project.
pub fn index(repo_root: &Path, project_id: &str, session_id: Option<&str>) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    if !proj_dir.exists() {
        anyhow::bail!("Project directory not found: {}", proj_dir.display());
    }

    let ledger_root = ledger_root_for(repo_root, project_id, |pid| {
        edda_store::registry::get_project(pid).map(|e| e.path)
    })?;
    let ledger = Ledger::open(&ledger_root)?;
    let stats = sync::sync(&proj_dir, project_id, session_id, |after| {
        ledger.events_after_rowid(after)
    })?;

    if stats.rebuilt {
        println!("Rebuilt index from scratch.");
    }
    println!(
        "Indexed {} event(s) + {} turn(s) for project {project_id}",
        stats.events, stats.turns
    );
    Ok(())
}

/// Execute `edda search show` — retrieve full turn content by turn_id.
pub fn show(project_id: &str, turn_id: &str) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let meta_db_path = proj_dir.join("search").join("meta.sqlite");
    if !meta_db_path.exists() {
        anyhow::bail!("No search metadata found. Run `edda search index` first.");
    }

    let meta_conn = schema::ensure_meta_db(&meta_db_path)?;
    let meta = match search::get_turn_meta(&meta_conn, turn_id)? {
        Some(m) => m,
        None => {
            println!("Turn not found: {turn_id}");
            return Ok(());
        }
    };

    let store_path = proj_dir
        .join("transcripts")
        .join(format!("{}.jsonl", meta.session_id));

    println!("Turn: {}", meta.turn_id);
    println!("Session: {}", meta.session_id);
    println!("Timestamp: {}", meta.ts.as_deref().unwrap_or("?"));
    println!("---");

    // Fetch and display user message
    if meta.user_store_len > 0 {
        if let Ok(raw) = fetch_store_line(
            &store_path,
            meta.user_store_offset as u64,
            meta.user_store_len as u64,
        ) {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                let text = extract_message_text(&json);
                println!("USER:\n{text}\n---");
            }
        }
    }

    // Fetch and display assistant message
    if meta.assistant_store_len > 0 {
        if let Ok(raw) = fetch_store_line(
            &store_path,
            meta.assistant_store_offset as u64,
            meta.assistant_store_len as u64,
        ) {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                let text = extract_message_text(&json);
                println!("ASSISTANT:\n{text}");
            }
        }
    }

    Ok(())
}

/// Resolve project ID from repo root (convenience for CLI).
pub fn resolve_project_id(repo_root: &Path) -> String {
    edda_store::project_id(repo_root)
}

/// Extract readable text from a transcript message JSON.
fn extract_message_text(json: &serde_json::Value) -> String {
    let content = match json.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                "tool_use" => {
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let cmd = block
                        .get("input")
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str());
                    if let Some(c) = cmd {
                        parts.push(format!("[ToolUse: {name} `{c}`]"));
                    } else {
                        parts.push(format!("[ToolUse: {name}]"));
                    }
                }
                "tool_result" => {
                    parts.push("[tool_result]".to_string());
                }
                _ => {}
            }
        }
        return parts.join("\n");
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry must not even be consulted for the project we are standing
    /// in — that is the overwhelmingly common case and it must not depend on
    /// being registered.
    #[test]
    fn local_project_uses_this_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pid = resolve_project_id(root);

        let got = ledger_root_for(root, &pid, |_| {
            panic!("registry must not be consulted for the local project")
        })
        .unwrap();

        assert_eq!(got, root);
    }

    /// GH-414: `--project X` names a project independently of where we stand, so
    /// its events must come from ITS repo. Using this repo's ledger writes these
    /// events into that project's index.
    #[test]
    fn foreign_project_uses_its_own_registered_repo() {
        let here = tempfile::tempdir().unwrap();
        let there = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(there.path().join(".edda")).unwrap();
        let there_path = there.path().to_string_lossy().into_owned();

        let got = ledger_root_for(here.path(), "other-pid", |_| Some(there_path.clone())).unwrap();

        assert_eq!(
            got,
            there.path(),
            "a foreign project must be indexed from its own repo, never ours"
        );
    }

    #[test]
    fn foreign_project_not_in_the_registry_is_refused() {
        let here = tempfile::tempdir().unwrap();

        let err = ledger_root_for(here.path(), "unknown-pid", |_| None).unwrap_err();

        assert!(
            err.to_string().contains("not in the registry"),
            "unhelpful error: {err}"
        );
    }

    /// The registry is full of entries whose repos are gone (see #417). A stale
    /// one must refuse, not fall back to whatever repo we happen to be in.
    #[test]
    fn foreign_project_whose_repo_is_gone_is_refused() {
        let here = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let gone_path = gone.path().to_string_lossy().into_owned();
        drop(gone);

        let err = ledger_root_for(here.path(), "stale-pid", |_| Some(gone_path)).unwrap_err();

        assert!(
            err.to_string().contains("no longer holds"),
            "unhelpful error: {err}"
        );
    }
}
