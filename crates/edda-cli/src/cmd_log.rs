use edda_core::Event;
use edda_ledger::Ledger;
use std::path::Path;

pub struct LogParams<'a> {
    pub repo_root: &'a Path,
    pub event_type: Option<&'a str>,
    pub family: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub keyword: Option<&'a str>,
    pub after: Option<&'a str>,
    pub before: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub limit: usize,
    pub json: bool,
}

pub fn execute(params: &LogParams<'_>) -> anyhow::Result<()> {
    let ledger = Ledger::open(params.repo_root)?;
    let events = ledger.iter_events()?;

    let mut matched: Vec<&Event> = events
        .iter()
        .rev() // newest first
        .filter(|e| matches_filter(e, params))
        .collect();

    if params.limit > 0 {
        matched.truncate(params.limit);
    }

    if matched.is_empty() {
        println!("No events match the filter.");
        return Ok(());
    }

    if params.json {
        for e in &matched {
            println!("{}", serde_json::to_string(e)?);
        }
    } else {
        for e in &matched {
            print_event_line(e);
        }
        println!("\n({} events shown)", matched.len());
    }

    Ok(())
}

fn matches_filter(event: &Event, params: &LogParams<'_>) -> bool {
    // --type filter
    if let Some(t) = params.event_type {
        if event.event_type != t {
            return false;
        }
    }

    // --family filter
    if let Some(f) = params.family {
        match &event.event_family {
            Some(ef) if ef == f => {}
            _ => return false,
        }
    }

    // --branch filter
    if let Some(b) = params.branch {
        if event.branch != b {
            return false;
        }
    }

    // --after filter (ISO 8601 prefix comparison)
    if let Some(after) = params.after {
        if event.ts.as_str() < after {
            return false;
        }
    }

    // --before filter
    if let Some(before) = params.before {
        if event.ts.as_str() > before {
            return false;
        }
    }

    // --tag filter (check payload.tags array)
    if let Some(tag) = params.tag {
        let has_tag = event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some(tag)))
            .unwrap_or(false);
        if !has_tag {
            return false;
        }
    }

    // --keyword filter (full-text search in payload)
    if let Some(kw) = params.keyword {
        let payload_str = serde_json::to_string(&event.payload).unwrap_or_default();
        if !payload_str.to_lowercase().contains(&kw.to_lowercase()) {
            return false;
        }
    }

    true
}

fn print_event_line(event: &Event) {
    // Format: [2026-02-14 03:42] note  main  evt_01jk...  "text"  #tag1 #tag2
    let ts_short = if event.ts.len() >= 16 {
        // "2026-02-14T03:42:00Z" -> "2026-02-14 03:42"
        format!("{} {}", &event.ts[..10], &event.ts[11..16])
    } else {
        event.ts.clone()
    };

    let eid_short = if event.event_id.len() > 14 {
        format!("{}...", &event.event_id[..14])
    } else {
        event.event_id.clone()
    };

    let detail = format_event_detail(event);
    let tags = format_tags(event);

    println!(
        "[{ts_short}] {:<15} {:<10} {eid_short}  {detail}{tags}",
        event.event_type, event.branch
    );
}

fn format_event_detail(event: &Event) -> String {
    match event.event_type.as_str() {
        "note" => {
            let text = event
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let truncated = if text.len() > 60 {
                format!("{}...", &text[..57])
            } else {
                text.to_string()
            };
            format!("\"{truncated}\"")
        }
        "cmd" => {
            let exit = event
                .payload
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            let argv = event
                .payload
                .get("argv")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let argv_short = if argv.len() > 40 {
                format!("{}...", &argv[..37])
            } else {
                argv
            };
            format!("exit={exit} \"{argv_short}\"")
        }
        "commit" => {
            let title = event
                .payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let labels = event
                .payload
                .get("labels")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| format!("[{s}]"))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            format!("\"{title}\" {labels}")
        }
        "merge" => {
            let src = event
                .payload
                .get("src")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let dst = event
                .payload
                .get("dst")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{src} -> {dst}")
        }
        "branch_create" => {
            let name = event
                .payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("create {name}")
        }
        "branch_switch" => {
            let to = event
                .payload
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("switch -> {to}")
        }
        "approval" => {
            let decision = event
                .payload
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let actor = event
                .payload
                .get("actor")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{decision} by {actor}")
        }
        _ => String::new(),
    }
}

fn format_tags(event: &Event) -> String {
    event
        .payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let tags: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!(" #{s}"))
                .collect();
            tags.join("")
        })
        .unwrap_or_default()
}
