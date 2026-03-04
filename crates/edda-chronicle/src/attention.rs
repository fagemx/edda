use anyhow::Result;
use edda_ledger::Ledger;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionItem {
    pub item_type: String,
    pub description: String,
    pub priority: String,
    pub source: String,
    pub event_id: Option<String>,
}

pub fn get_attention_items(
    ledger: &Ledger,
    _project_filter: Option<&str>,
) -> Result<Vec<AttentionItem>> {
    let mut items = Vec::new();

    let events = ledger.iter_events()?;

    // Find blocked tasks (simplified - look for notes with "blocker" or "blocked")
    for event in events.iter().rev() {
        if event.event_type == "note" {
            let text = event
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let text_lower = text.to_lowercase();

            if text_lower.contains("blocker") || text_lower.contains("blocked") {
                items.push(AttentionItem {
                    item_type: "blocker".to_string(),
                    description: truncate_text(text, 100),
                    priority: "high".to_string(),
                    source: "ledger".to_string(),
                    event_id: Some(event.event_id.clone()),
                });
            }
        }
    }

    // TODO: Add more attention sources:
    // - Stale decisions (not updated in X days)
    // - Open PRs with failing CI
    // - Decisions made without recent commits
    // - GitHub API enrichment (#177)

    // Sort by priority
    items.sort_by(|a, b| {
        let priority_order = |p: &str| match p {
            "high" => 0,
            "medium" => 1,
            "low" => 2,
            _ => 3,
        };
        priority_order(&a.priority).cmp(&priority_order(&b.priority))
    });

    Ok(items)
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max_len - 3);
        format!("{}...", &text[..end])
    }
}
