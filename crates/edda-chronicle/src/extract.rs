use crate::classify::SessionType;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyTurn {
    pub turn_index: usize,
    pub offset: u64,
    pub length: u64,
    pub relevance_score: f64,
    pub reason: String,
}

pub fn extract_key_turns(
    session_id: &str,
    session_type: &SessionType,
    project_root: &PathBuf,
    max_turns: usize,
) -> Result<Vec<KeyTurn>> {
    let index_path = project_root
        .join("index")
        .join(format!("{}.jsonl", session_id));

    if !index_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&index_path)
        .with_context(|| format!("Failed to read index file: {:?}", index_path))?;

    let records: Vec<serde_json::Value> = content
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line))
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| "Failed to parse index records")?;

    let turns = match session_type {
        SessionType::Coding => extract_coding_turns(&records, max_turns),
        SessionType::Research => extract_research_turns(&records, max_turns),
        SessionType::Discussion => extract_discussion_turns(&records, max_turns),
        SessionType::Analysis => extract_analysis_turns(&records, max_turns),
        SessionType::Debugging => extract_debugging_turns(&records, max_turns),
        SessionType::Automated => extract_automated_turns(&records, max_turns),
        SessionType::QuickOps => extract_quick_ops_turns(&records, max_turns),
    };

    Ok(turns)
}

fn extract_coding_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    let mut turns = Vec::new();

    for (idx, record) in records.iter().enumerate() {
        let record_type = record
            .get("record_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let assistant = record.get("assistant");

        if record_type == "assistant" {
            if let Some(meta) = assistant {
                let tool_names = meta
                    .get("tool_use_names")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                    .unwrap_or_default();

                let has_edit = tool_names.iter().any(|t| *t == "Edit" || *t == "Write");
                let has_bash = tool_names.iter().any(|t| *t == "Bash");

                if has_edit || has_bash {
                    let offset = record
                        .get("store_offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let length = record
                        .get("store_len")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    turns.push(KeyTurn {
                        turn_index: idx,
                        offset,
                        length,
                        relevance_score: if has_edit { 3.0 } else { 2.0 },
                        reason: if has_edit {
                            "Edit operation".to_string()
                        } else {
                            "Bash command".to_string()
                        },
                    });
                }
            }
        }
    }

    turns.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap());
    turns.into_iter().take(max_turns).collect()
}

fn extract_research_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    let mut turns = Vec::new();

    for (idx, record) in records.iter().enumerate() {
        let record_type = record
            .get("record_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let assistant = record.get("assistant");

        if record_type == "assistant" {
            if let Some(meta) = assistant {
                let tool_names = meta
                    .get("tool_use_names")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                    .unwrap_or_default();

                let has_write = tool_names.iter().any(|t| *t == "Write");

                if has_write {
                    let offset = record
                        .get("store_offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let length = record
                        .get("store_len")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    turns.push(KeyTurn {
                        turn_index: idx,
                        offset,
                        length,
                        relevance_score: 3.0,
                        reason: "Write operation (documentation)".to_string(),
                    });
                }
            }
        } else if record_type == "user" {
            let offset = record
                .get("store_offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let length = record
                .get("store_len")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            turns.push(KeyTurn {
                turn_index: idx,
                offset,
                length,
                relevance_score: 2.0,
                reason: "User query".to_string(),
            });
        }
    }

    turns.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap());
    turns.into_iter().take(max_turns).collect()
}

fn extract_discussion_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    let mut turns = Vec::new();
    let total = records.len();

    // head(2-3) + tail(2-3) + pivot detection
    let head_count = 3.min(total / 2);
    let tail_count = 3.min(total / 2);

    for (idx, record) in records.iter().enumerate() {
        let is_head = idx < head_count;
        let is_tail = idx >= total.saturating_sub(tail_count);

        if is_head || is_tail {
            let offset = record
                .get("store_offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let length = record
                .get("store_len")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            turns.push(KeyTurn {
                turn_index: idx,
                offset,
                length,
                relevance_score: if is_head { 3.0 } else { 2.5 },
                reason: if is_head {
                    "Session start".to_string()
                } else {
                    "Session end".to_string()
                },
            });
        }
    }

    turns.into_iter().take(max_turns).collect()
}

fn extract_analysis_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    extract_research_turns(records, max_turns)
}

fn extract_debugging_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    extract_coding_turns(records, max_turns)
}

fn extract_automated_turns(records: &[serde_json::Value], max_turns: usize) -> Vec<KeyTurn> {
    let mut turns = Vec::new();

    // Just get the last turn (result)
    if let Some(last_record) = records.last() {
        let offset = last_record
            .get("store_offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let length = last_record
            .get("store_len")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        turns.push(KeyTurn {
            turn_index: records.len() - 1,
            offset,
            length,
            relevance_score: 3.0,
            reason: "Final result".to_string(),
        });
    }

    turns.into_iter().take(max_turns).collect()
}

fn extract_quick_ops_turns(_records: &[serde_json::Value], _max_turns: usize) -> Vec<KeyTurn> {
    // Skip quick ops - use digest only
    vec![]
}
