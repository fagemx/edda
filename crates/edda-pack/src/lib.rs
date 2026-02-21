use edda_index::{fetch_store_line, read_index_tail, IndexRecordV1};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const DEFAULT_INDEX_TAIL_LINES: usize = 5000;
const DEFAULT_INDEX_TAIL_MAX_BYTES: u64 = 8 * 1024 * 1024; // 8MB
const DEFAULT_PACK_TURNS: usize = 12;
const DEFAULT_PACK_BUDGET_CHARS: usize = 12000;

// ── Turn + ToolUse structs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub user_uuid: String,
    pub assistant_uuid: String,
    pub user_text: String,
    pub assistant_texts: Vec<String>,
    pub tool_uses: Vec<ToolUse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: Option<String>,
    pub name: String,
    pub command: Option<String>,
    pub description: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackMetadata {
    pub project_id: String,
    pub session_id: String,
    pub git_branch: String,
    pub turn_count: usize,
    pub budget_chars: usize,
}

// ── Turn alignment via uuid/parentUuid ──

/// Build turns from index records by matching assistant.parentUuid → user.uuid.
pub fn build_turns(
    project_dir: &Path,
    session_id: &str,
    max_turns: usize,
) -> anyhow::Result<Vec<Turn>> {
    let tail_lines: usize = std::env::var("EDDA_INDEX_TAIL_LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INDEX_TAIL_LINES);
    let tail_bytes: u64 = std::env::var("EDDA_INDEX_TAIL_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INDEX_TAIL_MAX_BYTES);
    let pack_turns: usize = std::env::var("EDDA_PACK_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PACK_TURNS);
    let max_turns = max_turns.min(pack_turns);

    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let records = read_index_tail(&index_path, tail_lines, tail_bytes)?;

    if records.is_empty() {
        return Ok(vec![]);
    }

    // Build lookup by uuid
    let by_uuid: HashMap<String, &IndexRecordV1> =
        records.iter().map(|r| (r.uuid.clone(), r)).collect();

    // Collect assistant records in order
    let assistants: Vec<&IndexRecordV1> = records
        .iter()
        .filter(|r| r.record_type == "assistant")
        .collect();

    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    let mut turns = Vec::new();
    let mut seen_user_uuids = HashSet::new();

    // Process newest assistant first
    for asst_rec in assistants.iter().rev() {
        if turns.len() >= max_turns {
            break;
        }

        // Walk UP the parentUuid chain to find the real user prompt.
        // Claude Code transcript structure:
        //   user(STRING) → assistant(tool_use) → user(tool_result) → assistant(tool_use) → ... → assistant(text)
        // We start from the leaf assistant and walk up to find the root user with STRING content.
        let mut current_parent = asst_rec.parent_uuid.as_deref();
        let mut chain_tool_uses: Vec<ToolUse> = Vec::new();
        let mut real_user_uuid = String::new();
        let mut real_user_text = String::new();

        let mut depth = 0;
        const MAX_CHAIN_DEPTH: usize = 50;

        while let Some(parent_id) = current_parent {
            if depth >= MAX_CHAIN_DEPTH {
                break;
            }
            depth += 1;

            let parent_rec = match by_uuid.get(parent_id) {
                Some(r) => r,
                None => break,
            };

            if parent_rec.record_type == "user" {
                // Try to extract user text from this record
                if let Ok(raw) =
                    fetch_store_line(&store_path, parent_rec.store_offset, parent_rec.store_len)
                {
                    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                        let text = extract_user_text(&json);
                        if !text.is_empty() {
                            real_user_uuid = parent_rec.uuid.clone();
                            real_user_text = text;
                            break; // Found the real user prompt
                        }
                    }
                }
                // Content is array (tool_result) or empty → keep walking up
                current_parent = parent_rec.parent_uuid.as_deref();
            } else if parent_rec.record_type == "assistant" {
                // Intermediate assistant → collect its tool_uses
                if let Ok(raw) =
                    fetch_store_line(&store_path, parent_rec.store_offset, parent_rec.store_len)
                {
                    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                        let (_, tus) = parse_assistant_content(&json);
                        chain_tool_uses.extend(tus);
                    }
                }
                current_parent = parent_rec.parent_uuid.as_deref();
            } else {
                break; // unexpected record type
            }
        }

        if real_user_text.is_empty() || real_user_uuid.is_empty() {
            continue;
        }

        // Dedup: only one turn per real user prompt
        if !seen_user_uuids.insert(real_user_uuid.clone()) {
            continue;
        }

        // Parse final (leaf) assistant content
        let asst_raw =
            match fetch_store_line(&store_path, asst_rec.store_offset, asst_rec.store_len) {
                Ok(r) => r,
                Err(_) => continue,
            };
        let asst_json: serde_json::Value = match serde_json::from_slice(&asst_raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (assistant_texts, final_tool_uses) = parse_assistant_content(&asst_json);

        // Merge tool_uses: chain (reversed to chronological) + final assistant's
        chain_tool_uses.reverse();
        chain_tool_uses.extend(final_tool_uses);

        turns.push(Turn {
            user_uuid: real_user_uuid,
            assistant_uuid: asst_rec.uuid.clone(),
            user_text: real_user_text,
            assistant_texts,
            tool_uses: chain_tool_uses,
        });
    }

    Ok(turns)
}

/// Extract user text from a transcript user record.
/// Returns non-empty string only for real user prompts (STRING content).
/// Returns empty for tool_result arrays (these are tool execution results, not user input).
fn extract_user_text(user_json: &serde_json::Value) -> String {
    let content = match user_json.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return String::new(),
    };

    // String content → real user prompt
    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    // Array content → check block types
    if let Some(arr) = content.as_array() {
        // If any block is tool_result, this is NOT a real user prompt
        let has_tool_result = arr
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"));
        if has_tool_result {
            return String::new();
        }

        // Extract text from text blocks (handles ARRAY(text) format)
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join(" ");
        }
    }

    String::new()
}

fn parse_assistant_content(asst_json: &serde_json::Value) -> (Vec<String>, Vec<ToolUse>) {
    let mut texts = Vec::new();
    let mut tool_uses = Vec::new();

    let content = asst_json.get("message").and_then(|m| m.get("content"));

    if let Some(arr) = content.and_then(|c| c.as_array()) {
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        texts.push(text.to_string());
                    }
                }
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let id = block.get("id").and_then(|v| v.as_str()).map(|s| s.into());
                    let input = block.get("input");
                    let command = input
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str())
                        .map(|s| s.into());
                    let description = input
                        .and_then(|i| i.get("description"))
                        .and_then(|d| d.as_str())
                        .map(|s| s.into());
                    let file_path = input
                        .and_then(|i| i.get("file_path"))
                        .and_then(|f| f.as_str())
                        .map(|s| s.into());

                    tool_uses.push(ToolUse {
                        id,
                        name,
                        command,
                        description,
                        file_path,
                    });
                }
                _ => {}
            }
        }
    } else if let Some(text) = content.and_then(|c| c.as_str()) {
        texts.push(text.to_string());
    }

    (texts, tool_uses)
}

// ── Pack rendering ──

/// Render turns into a markdown pack string with budget truncation.
pub fn render_pack(turns: &[Turn], metadata: &PackMetadata, budget_chars: usize) -> String {
    let budget = if budget_chars == 0 {
        std::env::var("EDDA_PACK_BUDGET_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_PACK_BUDGET_CHARS)
    } else {
        budget_chars
    };

    let mut out = String::new();
    out.push_str("# edda memory pack (hot)\n\n");
    out.push_str(&format!("- project_id: {}\n", metadata.project_id));
    out.push_str(&format!("- session_id: {}\n", metadata.session_id));
    out.push_str(&format!("- git_branch: {}\n", metadata.git_branch));
    out.push_str(&format!("- turns: {}\n\n", turns.len()));
    out.push_str("## Recent Turns (deterministic)\n\n");

    // Render turns, newest first, truncate from oldest if over budget
    for (i, turn) in turns.iter().enumerate() {
        let mut section = String::new();
        let user_preview = truncate_str(&turn.user_text, 200);
        section.push_str(&format!("### Turn {} (newest first)\n", i + 1));
        section.push_str(&format!("- User: {user_preview}\n"));

        for tu in &turn.tool_uses {
            let cmd_str = tu
                .command
                .as_deref()
                .map(|c| format!(" `{}`", truncate_str(c, 80)))
                .unwrap_or_default();
            let desc_str = tu
                .description
                .as_deref()
                .map(|d| format!(" ({})", truncate_str(d, 60)))
                .unwrap_or_default();
            let file_str = tu
                .file_path
                .as_deref()
                .map(|f| format!(" file={f}"))
                .unwrap_or_default();
            section.push_str(&format!(
                "  - ToolUse: {}{}{}{}\n",
                tu.name, cmd_str, desc_str, file_str
            ));
        }

        for text in &turn.assistant_texts {
            let preview = truncate_str(text, 300);
            section.push_str(&format!("  - Assistant: {preview}\n"));
        }
        section.push('\n');

        if out.len() + section.len() > budget {
            out.push_str(&format!(
                "... ({} more turns truncated by budget)\n",
                turns.len() - i
            ));
            break;
        }
        out.push_str(&section);
    }

    out
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.replace('\n', " ")
    } else {
        // Find the last char boundary at or before `max` bytes
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", s[..end].replace('\n', " "))
    }
}

/// Write hot.md and hot.meta.json to the packs directory.
pub fn write_pack(project_dir: &Path, pack_md: &str, meta: &PackMetadata) -> anyhow::Result<()> {
    let packs_dir = project_dir.join("packs");
    std::fs::create_dir_all(&packs_dir)?;

    edda_store::write_atomic(&packs_dir.join("hot.md"), pack_md.as_bytes())?;

    let meta_json = serde_json::to_string_pretty(meta)?;
    edda_store::write_atomic(&packs_dir.join("hot.meta.json"), meta_json.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_pack_basic() {
        let turns = vec![Turn {
            user_uuid: "u1".into(),
            assistant_uuid: "a1".into(),
            user_text: "How do I sort a list?".into(),
            assistant_texts: vec!["Use the sort() method.".into()],
            tool_uses: vec![ToolUse {
                id: Some("tu1".into()),
                name: "Bash".into(),
                command: Some("ls -la".into()),
                description: Some("List files".into()),
                file_path: None,
            }],
        }];

        let meta = PackMetadata {
            project_id: "abc123".into(),
            session_id: "s1".into(),
            git_branch: "main".into(),
            turn_count: 1,
            budget_chars: 12000,
        };

        let md = render_pack(&turns, &meta, 12000);
        assert!(md.contains("# edda memory pack (hot)"));
        assert!(md.contains("How do I sort a list?"));
        assert!(md.contains("ToolUse: Bash"));
        assert!(md.contains("Use the sort() method."));
    }

    #[test]
    fn render_pack_budget_truncation() {
        let turns: Vec<Turn> = (0..20)
            .map(|i| Turn {
                user_uuid: format!("u{i}"),
                assistant_uuid: format!("a{i}"),
                user_text: format!("Question {i} with some extra text padding to fill space"),
                assistant_texts: vec![format!(
                    "Answer {i} with a reasonably long response text to consume budget"
                )],
                tool_uses: vec![],
            })
            .collect();

        let meta = PackMetadata {
            project_id: "abc".into(),
            session_id: "s1".into(),
            git_branch: "main".into(),
            turn_count: 20,
            budget_chars: 500,
        };

        let md = render_pack(&turns, &meta, 500);
        assert!(md.contains("truncated by budget"));
        assert!(md.len() <= 600); // some slack for the truncation message
    }

    #[test]
    fn write_pack_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = PackMetadata {
            project_id: "test".into(),
            session_id: "s1".into(),
            git_branch: "main".into(),
            turn_count: 0,
            budget_chars: 12000,
        };

        write_pack(tmp.path(), "# pack content", &meta).unwrap();

        let hot = tmp.path().join("packs").join("hot.md");
        assert!(hot.exists());
        assert_eq!(std::fs::read_to_string(&hot).unwrap(), "# pack content");

        let meta_path = tmp.path().join("packs").join("hot.meta.json");
        assert!(meta_path.exists());
    }
}
