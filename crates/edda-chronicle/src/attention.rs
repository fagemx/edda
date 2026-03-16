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

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;

    fn setup_ledger() -> (tempfile::TempDir, Ledger) {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_or_init(tmp.path()).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn test_empty_ledger() {
        let (_tmp, ledger) = setup_ledger();
        let items = get_attention_items(&ledger, None).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_blocker_detected() {
        let (_tmp, ledger) = setup_ledger();
        let event =
            new_note_event("main", None, "system", "This is a blocker for release", &[]).unwrap();
        ledger.append_event(&event).unwrap();

        let items = get_attention_items(&ledger, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, "blocker");
        assert_eq!(items[0].priority, "high");
        assert!(items[0].description.contains("blocker"));
    }

    #[test]
    fn test_non_blocker_ignored() {
        let (_tmp, ledger) = setup_ledger();
        let event = new_note_event("main", None, "system", "Normal progress update", &[]).unwrap();
        ledger.append_event(&event).unwrap();

        let items = get_attention_items(&ledger, None).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_truncate_text_long_input() {
        let long = "a".repeat(200);
        let result = truncate_text(&long, 100);
        assert!(result.len() <= 100);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_text_short_input() {
        let short = "hello world";
        let result = truncate_text(short, 100);
        assert_eq!(result, "hello world");
    }
}
