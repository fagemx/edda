use edda_core::Event;
use edda_ledger::sqlite_store::DecisionRow;
use edda_ledger::Ledger;
use serde::Serialize;

// ── Input type detection ─────────────────────────────────────────────

/// Detected input type for a query string.
#[derive(Debug, PartialEq)]
pub enum InputType {
    /// "db.engine" — contains '.' matching word.word
    ExactKey(String),
    /// "db" — matches a known domain
    Domain(String),
    /// "postgres" — default keyword search
    Keyword(String),
    /// Empty query — show all active decisions
    Overview,
}

/// Classify a query string into one of the four input types.
pub fn detect_input_type(query: &str, known_domains: &[String]) -> InputType {
    let q = query.trim();
    if q.is_empty() {
        return InputType::Overview;
    }
    // Check for exact key pattern: word.word (e.g. "db.engine")
    if q.contains('.') && q.split('.').count() >= 2 && q.split('.').all(|p| !p.is_empty()) {
        return InputType::ExactKey(q.to_string());
    }
    // Check if query matches a known domain (case-insensitive)
    let q_lower = q.to_lowercase();
    if known_domains.iter().any(|d| d.to_lowercase() == q_lower) {
        return InputType::Domain(q.to_string());
    }
    InputType::Keyword(q.to_string())
}

// ── Result types ─────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AskResult {
    pub query: String,
    pub input_type: String,
    pub decisions: Vec<DecisionHit>,
    pub timeline: Vec<DecisionHit>,
    pub related_commits: Vec<CommitHit>,
    pub related_notes: Vec<NoteHit>,
    pub conversations: Vec<ConversationHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionHit {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,
    pub branch: String,
    pub ts: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitHit {
    pub event_id: String,
    pub title: String,
    pub purpose: String,
    pub ts: String,
    pub branch: String,
    pub match_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteHit {
    pub event_id: String,
    pub text: String,
    pub ts: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationHit {
    pub doc_id: String,
    pub session_id: String,
    pub ts: String,
    pub snippet: String,
    pub rank: f64,
}

// ── Options ──────────────────────────────────────────────────────────

pub struct AskOptions {
    pub limit: usize,
    pub include_superseded: bool,
    pub branch: Option<String>,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            include_superseded: false,
            branch: None,
        }
    }
}

/// Transcript search callback type.
pub type TranscriptSearchFn = dyn Fn(&str, usize) -> Vec<ConversationHit>;

// ── Core ask function ────────────────────────────────────────────────

pub fn ask(
    ledger: &Ledger,
    query: &str,
    opts: &AskOptions,
    transcript_search: Option<&TranscriptSearchFn>,
) -> anyhow::Result<AskResult> {
    let domains = ledger.list_domains()?;
    let input_type = detect_input_type(query, &domains);

    // Branch filter helper: keep only decisions matching the requested branch
    let branch_filter = |hits: Vec<DecisionHit>| -> Vec<DecisionHit> {
        match &opts.branch {
            Some(b) => hits.into_iter().filter(|d| d.branch == *b).collect(),
            None => hits,
        }
    };

    let (decisions, timeline) = match &input_type {
        InputType::ExactKey(key) => {
            let all = ledger
                .decision_timeline(key)?
                .into_iter()
                .map(|r| to_decision_hit(&r))
                .collect::<Vec<_>>();
            let all = branch_filter(all);
            let active: Vec<DecisionHit> = if opts.include_superseded {
                all.clone()
            } else {
                all.iter().filter(|d| d.is_active).cloned().collect()
            };
            (active, all)
        }
        InputType::Domain(domain) => {
            let active = branch_filter(
                ledger
                    .active_decisions(Some(domain), None)?
                    .into_iter()
                    .map(|r| to_decision_hit(&r))
                    .collect(),
            );
            let tl = branch_filter(
                ledger
                    .domain_timeline(domain)?
                    .into_iter()
                    .map(|r| to_decision_hit(&r))
                    .collect(),
            );
            (active, tl)
        }
        InputType::Keyword(kw) => {
            let mut hits = branch_filter(
                ledger
                    .active_decisions(None, Some(kw))?
                    .into_iter()
                    .map(|r| to_decision_hit(&r))
                    .collect(),
            );
            if opts.include_superseded {
                // Also scan all events for superseded decisions matching keyword
                let events = ledger.iter_events()?;
                let kw_lower = kw.to_lowercase();
                for event in &events {
                    if let Some(ref b) = opts.branch {
                        if event.branch != *b {
                            continue;
                        }
                    }
                    if event.event_type == "note"
                        && edda_core::decision::is_decision(&event.payload)
                    {
                        if let Some(dp) = edda_core::decision::extract_decision(&event.payload) {
                            let reason_str = dp.reason.as_deref().unwrap_or("").to_string();
                            if (dp.key.to_lowercase().contains(&kw_lower)
                                || dp.value.to_lowercase().contains(&kw_lower)
                                || reason_str.to_lowercase().contains(&kw_lower))
                                && !hits.iter().any(|h| h.event_id == event.event_id)
                            {
                                let domain = edda_core::decision::extract_domain(&dp.key);
                                hits.push(DecisionHit {
                                    event_id: event.event_id.clone(),
                                    key: dp.key,
                                    value: dp.value,
                                    reason: reason_str,
                                    domain,
                                    branch: event.branch.clone(),
                                    ts: event.ts.clone(),
                                    is_active: false,
                                });
                            }
                        }
                    }
                }
            }
            (hits, vec![])
        }
        InputType::Overview => {
            let active = branch_filter(
                ledger
                    .active_decisions(None, None)?
                    .into_iter()
                    .map(|r| to_decision_hit(&r))
                    .collect(),
            );
            (active, vec![])
        }
    };

    // Collect decision event_ids for evidence chain matching
    let decision_event_ids: Vec<&str> = decisions
        .iter()
        .map(|d| d.event_id.as_str())
        .chain(timeline.iter().map(|d| d.event_id.as_str()))
        .collect();

    let q = query.trim();
    // Load events once for both commit and note searches
    let events = ledger.iter_events()?;
    let related_commits =
        find_related_commits(&events, &decision_event_ids, q, opts.limit, &opts.branch);
    let related_notes = find_related_notes(&events, q, opts.limit, &opts.branch);

    let conversations = match transcript_search {
        Some(search_fn) if !q.is_empty() => search_fn(q, opts.limit),
        _ => vec![],
    };

    let input_type_str = match &input_type {
        InputType::ExactKey(_) => "exact_key",
        InputType::Domain(_) => "domain",
        InputType::Keyword(_) => "keyword",
        InputType::Overview => "overview",
    };

    Ok(AskResult {
        query: q.to_string(),
        input_type: input_type_str.to_string(),
        decisions,
        timeline,
        related_commits,
        related_notes,
        conversations,
    })
}

// ── Related event helpers ────────────────────────────────────────────

fn find_related_commits(
    events: &[Event],
    decision_event_ids: &[&str],
    query: &str,
    limit: usize,
    branch: &Option<String>,
) -> Vec<CommitHit> {
    let mut hits: Vec<CommitHit> = Vec::new();
    let q_lower = query.to_lowercase();

    for event in events.iter().rev() {
        if event.event_type != "commit" {
            continue;
        }
        if let Some(b) = branch {
            if event.branch != *b {
                continue;
            }
        }

        let title = event
            .payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let purpose = event
            .payload
            .get("purpose")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Check evidence chain: does this commit reference any of our decision events?
        let mut match_type = None;

        // Check refs.events for evidence links
        for ref_id in &event.refs.events {
            if decision_event_ids.contains(&ref_id.as_str()) {
                match_type = Some("evidence");
                break;
            }
        }
        // Also check provenance
        if match_type.is_none() {
            for prov in &event.refs.provenance {
                if decision_event_ids.contains(&prov.target.as_str()) {
                    match_type = Some("evidence");
                    break;
                }
            }
        }

        // Title/purpose keyword match
        if match_type.is_none()
            && !query.is_empty()
            && (title.to_lowercase().contains(&q_lower)
                || purpose.to_lowercase().contains(&q_lower))
        {
            match_type = Some("title");
        }

        if let Some(mt) = match_type {
            // Deduplicate
            if !hits.iter().any(|h| h.event_id == event.event_id) {
                hits.push(CommitHit {
                    event_id: event.event_id.clone(),
                    title: title.to_string(),
                    purpose: purpose.to_string(),
                    ts: event.ts.clone(),
                    branch: event.branch.clone(),
                    match_type: mt.to_string(),
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }
    }

    hits
}

fn find_related_notes(
    events: &[Event],
    query: &str,
    limit: usize,
    branch: &Option<String>,
) -> Vec<NoteHit> {
    if query.is_empty() {
        return vec![];
    }

    let q_lower = query.to_lowercase();
    let mut hits: Vec<NoteHit> = Vec::new();

    for event in events.iter().rev() {
        if event.event_type != "note" {
            continue;
        }
        if let Some(b) = branch {
            if event.branch != *b {
                continue;
            }
        }
        // Skip decision notes — those are already in decisions section
        if event.event_type == "note" && edda_core::decision::is_decision(&event.payload) {
            continue;
        }

        let text = event
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if text.to_lowercase().contains(&q_lower) {
            hits.push(NoteHit {
                event_id: event.event_id.clone(),
                text: text.to_string(),
                ts: event.ts.clone(),
                branch: event.branch.clone(),
            });
            if hits.len() >= limit {
                break;
            }
        }
    }

    hits
}

// ── Human-readable formatting ────────────────────────────────────────

pub fn format_human(result: &AskResult) -> String {
    let mut out = String::new();

    if !result.decisions.is_empty() {
        out.push_str("── Decisions ──────────────────────────\n");
        for d in &result.decisions {
            let status = if d.is_active { "active" } else { "superseded" };
            out.push_str(&format!(
                "  {} = {} — {}\n  branch: {} | {} | {}\n\n",
                d.key, d.value, d.reason, d.branch, d.ts, status
            ));
        }
    }

    if !result.timeline.is_empty() {
        out.push_str("── Timeline ───────────────────────────\n");
        for d in &result.timeline {
            let status = if d.is_active { "active" } else { "superseded" };
            out.push_str(&format!(
                "  {}  {} = {}  ({})\n",
                d.ts, d.key, d.value, status
            ));
        }
        out.push('\n');
    }

    if !result.related_commits.is_empty() {
        out.push_str("── Related Commits ────────────────────\n");
        for c in &result.related_commits {
            out.push_str(&format!(
                "  {} ({}, {})\n  match: {}\n\n",
                c.title, c.ts, c.branch, c.match_type
            ));
        }
    }

    if !result.related_notes.is_empty() {
        out.push_str("── Related Notes ──────────────────────\n");
        for n in &result.related_notes {
            out.push_str(&format!("  \"{}\" ({}, {})\n\n", n.text, n.ts, n.branch));
        }
    }

    if !result.conversations.is_empty() {
        out.push_str("── Conversations ──────────────────────\n");
        for c in &result.conversations {
            out.push_str(&format!(
                "  [{}] {}\n  rank: {:.2}\n\n",
                c.session_id, c.snippet, c.rank
            ));
        }
    }

    if out.is_empty() {
        out.push_str("No results found.\n");
    }

    out
}

// ── Internal helpers ─────────────────────────────────────────────────

fn to_decision_hit(row: &DecisionRow) -> DecisionHit {
    DecisionHit {
        event_id: row.event_id.clone(),
        key: row.key.clone(),
        value: row.value.clone(),
        reason: row.reason.clone(),
        domain: row.domain.clone(),
        branch: row.branch.clone(),
        ts: row.ts.clone().unwrap_or_default(),
        is_active: row.is_active,
    }
}

// Decision helpers centralized in edda_core::decision.

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{finalize_event, new_note_event};
    use edda_core::Provenance;
    use edda_ledger::ledger::{init_branches_json, init_head, init_workspace};
    use edda_ledger::paths::EddaPaths;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup() -> (std::path::PathBuf, Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_ask_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        init_workspace(&paths).unwrap();
        init_head(&paths, "main").unwrap();
        init_branches_json(&paths, "main").unwrap();
        let ledger = Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    fn make_decision(
        branch: &str,
        key: &str,
        value: &str,
        reason: Option<&str>,
        supersedes: Option<&str>,
    ) -> Event {
        let text = match reason {
            Some(r) => format!("{key}: {value} — {r}"),
            None => format!("{key}: {value}"),
        };
        let tags = vec!["decision".to_string()];
        let mut event = new_note_event(branch, None, "system", &text, &tags).unwrap();
        let decision_obj = match reason {
            Some(r) => serde_json::json!({"key": key, "value": value, "reason": r}),
            None => serde_json::json!({"key": key, "value": value}),
        };
        event.payload["decision"] = decision_obj;
        if let Some(target) = supersedes {
            event.refs.provenance.push(Provenance {
                target: target.to_string(),
                rel: "supersedes".to_string(),
                note: Some(format!("key '{key}' re-decided")),
            });
        }
        finalize_event(&mut event);
        event
    }

    fn make_commit(branch: &str, title: &str, purpose: &str, evidence: &[&str]) -> Event {
        use edda_core::event::finalize_event;
        let payload = serde_json::json!({
            "title": title,
            "purpose": purpose,
            "sha": "abc123",
        });
        let mut event = Event {
            event_id: format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase()),
            ts: time_now(),
            event_type: "commit".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: edda_core::Refs {
                events: evidence.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            schema_version: 1,
            digests: vec![],
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event);
        event
    }

    fn make_note(branch: &str, text: &str) -> Event {
        new_note_event(branch, None, "user", text, &[]).unwrap()
    }

    fn time_now() -> String {
        let now = time::OffsetDateTime::now_utc();
        now.format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339")
    }

    // ── detect_input_type tests ──────────────────────────────────────

    #[test]
    fn detect_exact_key() {
        let domains = vec!["db".into(), "auth".into()];
        assert_eq!(
            detect_input_type("db.engine", &domains),
            InputType::ExactKey("db.engine".into())
        );
    }

    #[test]
    fn detect_domain() {
        let domains = vec!["db".into(), "auth".into()];
        assert_eq!(
            detect_input_type("db", &domains),
            InputType::Domain("db".into())
        );
    }

    #[test]
    fn detect_domain_case_insensitive() {
        let domains = vec!["db".into()];
        assert_eq!(
            detect_input_type("DB", &domains),
            InputType::Domain("DB".into())
        );
    }

    #[test]
    fn detect_keyword() {
        let domains = vec!["db".into()];
        assert_eq!(
            detect_input_type("postgres", &domains),
            InputType::Keyword("postgres".into())
        );
    }

    #[test]
    fn detect_overview() {
        assert_eq!(detect_input_type("", &[]), InputType::Overview);
        assert_eq!(detect_input_type("  ", &[]), InputType::Overview);
    }

    // ── ask() tests ──────────────────────────────────────────────────

    #[test]
    fn ask_exact_key() {
        let (tmp, ledger) = setup();
        let d1 = make_decision("main", "db.engine", "sqlite", Some("MVP"), None);
        let d1_id = d1.event_id.clone();
        ledger.append_event(&d1).unwrap();

        let d2 = make_decision("main", "db.engine", "postgres", Some("JSONB"), Some(&d1_id));
        ledger.append_event(&d2).unwrap();

        let result = ask(&ledger, "db.engine", &AskOptions::default(), None).unwrap();
        assert_eq!(result.input_type, "exact_key");
        // decisions = only active (default)
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0].value, "postgres");
        // timeline = both
        assert_eq!(result.timeline.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_domain() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();
        ledger
            .append_event(&make_decision("main", "db.pool", "10", None, None))
            .unwrap();
        ledger
            .append_event(&make_decision("main", "auth.method", "JWT", None, None))
            .unwrap();

        let result = ask(&ledger, "db", &AskOptions::default(), None).unwrap();
        assert_eq!(result.input_type, "domain");
        assert_eq!(result.decisions.len(), 2);
        assert!(result.timeline.len() >= 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_keyword() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision(
                "main",
                "db.engine",
                "postgres",
                Some("JSONB"),
                None,
            ))
            .unwrap();
        ledger
            .append_event(&make_decision("main", "auth.method", "JWT", None, None))
            .unwrap();

        let result = ask(&ledger, "postgres", &AskOptions::default(), None).unwrap();
        assert_eq!(result.input_type, "keyword");
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0].key, "db.engine");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_overview() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();
        ledger
            .append_event(&make_decision("main", "auth.method", "JWT", None, None))
            .unwrap();

        let result = ask(&ledger, "", &AskOptions::default(), None).unwrap();
        assert_eq!(result.input_type, "overview");
        assert_eq!(result.decisions.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_with_transcript_callback() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();

        let callback = |_query: &str, _limit: usize| -> Vec<ConversationHit> {
            vec![ConversationHit {
                doc_id: "t1".into(),
                session_id: "s1".into(),
                ts: "2026-02-14T10:00:00Z".into(),
                snippet: "discussed postgres JSONB".into(),
                rank: 5.0,
            }]
        };

        let result = ask(&ledger, "postgres", &AskOptions::default(), Some(&callback)).unwrap();
        assert_eq!(result.conversations.len(), 1);
        assert_eq!(result.conversations[0].snippet, "discussed postgres JSONB");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_without_transcript_callback() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();

        let result = ask(&ledger, "postgres", &AskOptions::default(), None).unwrap();
        assert!(result.conversations.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Related events tests ─────────────────────────────────────────

    #[test]
    fn find_related_commits_evidence_chain() {
        let (tmp, ledger) = setup();
        let d1 = make_decision("main", "db.engine", "postgres", Some("JSONB"), None);
        let d1_id = d1.event_id.clone();
        ledger.append_event(&d1).unwrap();

        let c1 = make_commit(
            "main",
            "feat: migrate to postgres",
            "db migration",
            &[&d1_id],
        );
        ledger.append_event(&c1).unwrap();

        let result = ask(&ledger, "db.engine", &AskOptions::default(), None).unwrap();
        assert_eq!(result.related_commits.len(), 1);
        assert_eq!(result.related_commits[0].match_type, "evidence");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_related_commits_title_match() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();

        // Commit not linked via evidence, but title matches
        let c1 = make_commit("main", "fix: postgres connection pool", "pool fix", &[]);
        ledger.append_event(&c1).unwrap();

        let result = ask(&ledger, "postgres", &AskOptions::default(), None).unwrap();
        assert!(result
            .related_commits
            .iter()
            .any(|c| c.match_type == "title"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_related_notes_text_match() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision("main", "db.engine", "postgres", None, None))
            .unwrap();

        // Regular note (not a decision)
        ledger
            .append_event(&make_note(
                "main",
                "discussed mysql but rejected for licensing",
            ))
            .unwrap();

        let result = ask(&ledger, "mysql", &AskOptions::default(), None).unwrap();
        assert_eq!(result.related_notes.len(), 1);
        assert!(result.related_notes[0].text.contains("mysql"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_related_notes_excludes_decision_notes() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision(
                "main",
                "db.engine",
                "postgres",
                Some("JSONB"),
                None,
            ))
            .unwrap();

        let result = ask(&ledger, "postgres", &AskOptions::default(), None).unwrap();
        // Decision note should NOT appear in related_notes
        assert!(result.related_notes.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ask_branch_filter() {
        let (tmp, ledger) = setup();
        ledger
            .append_event(&make_decision(
                "main",
                "db.engine",
                "postgres",
                Some("prod"),
                None,
            ))
            .unwrap();
        ledger
            .append_event(&make_decision(
                "dev",
                "db.engine",
                "sqlite",
                Some("dev speed"),
                None,
            ))
            .unwrap();

        // No branch filter → both
        let result = ask(&ledger, "", &AskOptions::default(), None).unwrap();
        assert_eq!(result.decisions.len(), 2);

        // Filter to dev → only sqlite
        let opts = AskOptions {
            branch: Some("dev".into()),
            ..Default::default()
        };
        let result = ask(&ledger, "", &opts, None).unwrap();
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0].value, "sqlite");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn format_human_contains_sections() {
        let result = AskResult {
            query: "postgres".into(),
            input_type: "keyword".into(),
            decisions: vec![DecisionHit {
                event_id: "e1".into(),
                key: "db.engine".into(),
                value: "postgres".into(),
                reason: "JSONB".into(),
                domain: "db".into(),
                branch: "main".into(),
                ts: "2026-02-15".into(),
                is_active: true,
            }],
            timeline: vec![],
            related_commits: vec![CommitHit {
                event_id: "c1".into(),
                title: "feat: migrate".into(),
                purpose: "migration".into(),
                ts: "2026-02-15".into(),
                branch: "main".into(),
                match_type: "evidence".into(),
            }],
            related_notes: vec![],
            conversations: vec![],
        };

        let output = format_human(&result);
        assert!(output.contains("Decisions"));
        assert!(output.contains("postgres"));
        assert!(output.contains("Related Commits"));
        assert!(output.contains("feat: migrate"));
    }

    #[test]
    fn format_human_empty_result() {
        let result = AskResult {
            query: "nonexistent".into(),
            input_type: "keyword".into(),
            decisions: vec![],
            timeline: vec![],
            related_commits: vec![],
            related_notes: vec![],
            conversations: vec![],
        };

        let output = format_human(&result);
        assert!(output.contains("No results found"));
    }
}
