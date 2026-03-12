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
    pub tool: Option<&'a str>,
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
    // --type filter (with "session" alias for session_digest tagged events)
    if let Some(t) = params.event_type {
        if t == "session" {
            let is_session_digest = event
                .payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("session_digest")))
                .unwrap_or(false);
            if !is_session_digest {
                return false;
            }
        } else if event.event_type != t {
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

    // --tool filter (check session_stats.tool_call_breakdown for a specific tool)
    if let Some(tool) = params.tool {
        let has_tool = event
            .payload
            .get("session_stats")
            .and_then(|ss| ss.get("tool_call_breakdown"))
            .and_then(|tb| tb.get(tool))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0;
        if !has_tool {
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

fn is_session_digest(event: &Event) -> bool {
    event
        .payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|t| t.as_str() == Some("session_digest")))
        .unwrap_or(false)
}

fn format_event_detail(event: &Event) -> String {
    // Session digest events get special formatting regardless of event_type
    if is_session_digest(event) {
        return format_session_digest_detail(event);
    }

    match event.event_type.as_str() {
        "note" => {
            let text = event
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let truncated = if text.len() > 60 {
                format!("{}...", &text[..text.floor_char_boundary(57)])
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
                format!("{}...", &argv[..argv.floor_char_boundary(37)])
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

fn format_session_digest_detail(event: &Event) -> String {
    let ss = event.payload.get("session_stats");
    let tool_calls = ss
        .and_then(|s| s.get("tool_calls"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let duration = ss
        .and_then(|s| s.get("duration_minutes"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let outcome = ss
        .and_then(|s| s.get("outcome"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let activity = ss
        .and_then(|s| s.get("activity"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let activity_tag = if activity == "unknown" {
        String::new()
    } else {
        format!(" [{activity}]")
    };

    let mut parts = vec![format!(
        "{tool_calls} calls, {duration}m, {outcome}{activity_tag}"
    )];

    // Tool breakdown
    if let Some(tb) = ss
        .and_then(|s| s.get("tool_call_breakdown"))
        .and_then(|v| v.as_object())
    {
        let breakdown: Vec<String> = tb
            .iter()
            .filter(|(_, v)| v.as_u64().unwrap_or(0) > 0)
            .map(|(k, v)| format!("{}:{}", k, v.as_u64().unwrap_or(0)))
            .collect();
        if !breakdown.is_empty() {
            parts.push(format!("[{}]", breakdown.join(" ")));
        }
    }

    // Edit/search ratios
    if let Some(edit_ratio) = ss
        .and_then(|s| s.get("edit_ratio"))
        .and_then(|v| v.as_f64())
    {
        if edit_ratio > 0.0 {
            parts.push(format!("edit:{:.0}%", edit_ratio * 100.0));
        }
    }
    if let Some(search_ratio) = ss
        .and_then(|s| s.get("search_ratio"))
        .and_then(|v| v.as_f64())
    {
        if search_ratio > 0.0 {
            parts.push(format!("search:{:.0}%", search_ratio * 100.0));
        }
    }

    // Model/tokens/cost
    let model = ss
        .and_then(|s| s.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let input_tokens = ss
        .and_then(|s| s.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = ss
        .and_then(|s| s.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cost = ss
        .and_then(|s| s.get("estimated_cost_usd"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    if input_tokens > 0 || output_tokens > 0 {
        let model_short = if model.is_empty() {
            "unknown".to_string()
        } else {
            shorten_model_name(model)
        };
        let total = format_token_count(input_tokens + output_tokens);
        let cost_str = if cost > 0.0 {
            format!(" ${:.4}", cost)
        } else {
            String::new()
        };
        parts.push(format!("{model_short} {total}{cost_str}"));
    }

    parts.join("  ")
}

/// Shorten Claude model names for display.
/// "claude-sonnet-4-20250514" -> "sonnet-4"
/// "claude-opus-4-20250514" -> "opus-4"
fn shorten_model_name(model: &str) -> String {
    let lower = model.to_lowercase();
    // Match patterns like "claude-{family}-{version}-{date}"
    if lower.starts_with("claude-") {
        let without_prefix = &model[7..]; // skip "claude-"
                                          // Remove trailing date (YYYYMMDD)
        let trimmed = if without_prefix.len() > 9 {
            let last_dash = without_prefix.rfind('-').unwrap_or(without_prefix.len());
            let after_dash = &without_prefix[last_dash + 1..];
            // Check if it looks like a date (8 digits)
            if after_dash.len() == 8 && after_dash.chars().all(|c| c.is_ascii_digit()) {
                &without_prefix[..last_dash]
            } else {
                without_prefix
            }
        } else {
            without_prefix
        };
        return trimmed.to_string();
    }
    model.to_string()
}

/// Format a token count for human-readable display.
/// <1000 -> exact, 1k-999k -> "X.Xk", >=1M -> "X.XM"
fn format_token_count(tokens: u64) -> String {
    if tokens < 1_000 {
        format!("{tokens}")
    } else if tokens < 1_000_000 {
        let k = tokens as f64 / 1_000.0;
        if k >= 10.0 {
            format!("{:.0}k", k)
        } else {
            format!("{:.1}k", k)
        }
    } else {
        let m = tokens as f64 / 1_000_000.0;
        format!("{:.1}M", m)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_token_count() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1.0k");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(10_000), "10k");
        assert_eq!(format_token_count(150_000), "150k");
        assert_eq!(format_token_count(1_000_000), "1.0M");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }

    #[test]
    fn test_shorten_model_name() {
        assert_eq!(shorten_model_name("claude-sonnet-4-20250514"), "sonnet-4");
        assert_eq!(shorten_model_name("claude-opus-4-20250514"), "opus-4");
        assert_eq!(shorten_model_name("claude-haiku-3-5-20250514"), "haiku-3-5");
        assert_eq!(shorten_model_name("gpt-4o"), "gpt-4o");
        assert_eq!(shorten_model_name("claude-sonnet-4"), "sonnet-4");
    }

    #[test]
    fn test_session_digest_with_usage() {
        let event = Event {
            event_id: "evt_test123456789".into(),
            event_type: "note".into(),
            ts: "2026-03-01T10:00:00Z".into(),
            branch: "main".into(),
            event_family: Some("milestone".into()),
            payload: serde_json::json!({
                "tags": ["session_digest"],
                "session_stats": {
                    "tool_calls": 42,
                    "duration_minutes": 15,
                    "outcome": "completed",
                    "tool_call_breakdown": {
                        "Read": 10,
                        "Edit": 8,
                        "Bash": 6,
                        "Grep": 5
                    },
                    "edit_ratio": 0.19,
                    "search_ratio": 0.12,
                    "model": "claude-sonnet-4-20250514",
                    "input_tokens": 150000,
                    "output_tokens": 30000,
                    "estimated_cost_usd": 0.9
                }
            }),
            refs: edda_core::types::Refs::default(),
            hash: String::new(),
            parent_hash: None,
            digests: vec![],
            event_level: None,
            schema_version: 1,
        };
        assert!(is_session_digest(&event));
        let detail = format_event_detail(&event);
        assert!(detail.contains("42 calls"), "detail: {detail}");
        assert!(detail.contains("15m"), "detail: {detail}");
        assert!(detail.contains("completed"), "detail: {detail}");
        assert!(detail.contains("sonnet-4"), "detail: {detail}");
        assert!(detail.contains("180k"), "detail: {detail}"); // 150k+30k
        assert!(detail.contains("$0.9000"), "detail: {detail}");
    }

    #[test]
    fn test_session_digest_without_usage() {
        let event = Event {
            event_id: "evt_test123456789".into(),
            event_type: "note".into(),
            ts: "2026-03-01T10:00:00Z".into(),
            branch: "main".into(),
            event_family: Some("milestone".into()),
            payload: serde_json::json!({
                "tags": ["session_digest"],
                "session_stats": {
                    "tool_calls": 10,
                    "duration_minutes": 5,
                    "outcome": "completed",
                    "model": "",
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "estimated_cost_usd": 0.0
                }
            }),
            refs: edda_core::types::Refs::default(),
            hash: String::new(),
            parent_hash: None,
            digests: vec![],
            event_level: None,
            schema_version: 1,
        };
        let detail = format_event_detail(&event);
        assert!(detail.contains("10 calls"), "detail: {detail}");
        // Should NOT contain model/token info when tokens are 0
        assert!(!detail.contains("unknown"), "detail: {detail}");
    }

    #[test]
    fn test_session_digest_with_usage_no_cost() {
        let event = Event {
            event_id: "evt_test123456789".into(),
            event_type: "note".into(),
            ts: "2026-03-01T10:00:00Z".into(),
            branch: "main".into(),
            event_family: Some("milestone".into()),
            payload: serde_json::json!({
                "tags": ["session_digest"],
                "session_stats": {
                    "tool_calls": 20,
                    "duration_minutes": 8,
                    "outcome": "completed",
                    "model": "gpt-4o",
                    "input_tokens": 50000,
                    "output_tokens": 10000,
                    "estimated_cost_usd": 0.0
                }
            }),
            refs: edda_core::types::Refs::default(),
            hash: String::new(),
            parent_hash: None,
            digests: vec![],
            event_level: None,
            schema_version: 1,
        };
        let detail = format_event_detail(&event);
        assert!(detail.contains("gpt-4o"), "detail: {detail}");
        assert!(detail.contains("60k"), "detail: {detail}"); // 50k+10k
                                                             // Should NOT contain $ when cost is 0
        assert!(!detail.contains("$"), "detail: {detail}");
    }
}
