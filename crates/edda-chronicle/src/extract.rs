use crate::classify::SessionType;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
    project_root: &std::path::Path,
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
        .map(serde_json::from_str)
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
                let has_bash = tool_names.contains(&"Bash");

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

    turns.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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

                let has_write = tool_names.contains(&"Write");

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

    turns.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_index_jsonl(dir: &std::path::Path, session_id: &str, records: &[serde_json::Value]) {
        let index_dir = dir.join("index");
        std::fs::create_dir_all(&index_dir).unwrap();
        let lines: Vec<String> = records.iter().map(|r| r.to_string()).collect();
        std::fs::write(
            index_dir.join(format!("{}.jsonl", session_id)),
            lines.join("\n"),
        )
        .unwrap();
    }

    fn assistant_record(tools: &[&str], offset: u64, len: u64) -> serde_json::Value {
        serde_json::json!({
            "record_type": "assistant",
            "assistant": {
                "tool_use_names": tools,
            },
            "store_offset": offset,
            "store_len": len,
        })
    }

    fn user_record(offset: u64, len: u64) -> serde_json::Value {
        serde_json::json!({
            "record_type": "user",
            "store_offset": offset,
            "store_len": len,
        })
    }

    #[test]
    fn test_missing_index_file() {
        let tmp = tempfile::tempdir().unwrap();
        let result =
            extract_key_turns("nonexistent", &SessionType::Coding, tmp.path(), 5).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_coding_turns_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let records = vec![
            assistant_record(&["Edit"], 0, 100),
            assistant_record(&["Read"], 100, 50),
            assistant_record(&["Write"], 150, 80),
        ];
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::Coding, tmp.path(), 5).unwrap();
        // Edit and Write both match has_edit (Edit || Write)
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].reason, "Edit operation");
        assert_eq!(turns[0].relevance_score, 3.0);
    }

    #[test]
    fn test_coding_turns_bash() {
        let tmp = tempfile::tempdir().unwrap();
        let records = vec![
            assistant_record(&["Bash"], 0, 100),
            assistant_record(&["Read"], 100, 50),
        ];
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::Coding, tmp.path(), 5).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].reason, "Bash command");
        assert_eq!(turns[0].relevance_score, 2.0);
    }

    #[test]
    fn test_research_turns_write() {
        let tmp = tempfile::tempdir().unwrap();
        let records = vec![
            assistant_record(&["Write"], 0, 100),
            user_record(100, 50),
            assistant_record(&["Read"], 150, 80),
        ];
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::Research, tmp.path(), 5).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].relevance_score, 3.0);
        assert_eq!(turns[0].reason, "Write operation (documentation)");
    }

    #[test]
    fn test_discussion_turns_head_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let records: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({
                    "record_type": "user",
                    "store_offset": i * 100,
                    "store_len": 100,
                })
            })
            .collect();
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::Discussion, tmp.path(), 10).unwrap();
        // head(3) + tail(3), indices 0-2 and 7-9
        assert_eq!(turns.len(), 6);
        let head_turns: Vec<_> = turns.iter().filter(|t| t.reason == "Session start").collect();
        let tail_turns: Vec<_> = turns.iter().filter(|t| t.reason == "Session end").collect();
        assert_eq!(head_turns.len(), 3);
        assert_eq!(tail_turns.len(), 3);
    }

    #[test]
    fn test_automated_turns_last() {
        let tmp = tempfile::tempdir().unwrap();
        let records = vec![
            assistant_record(&["Bash"], 0, 100),
            assistant_record(&["Bash"], 100, 50),
            assistant_record(&["Bash"], 150, 80),
        ];
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::Automated, tmp.path(), 5).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_index, 2);
        assert_eq!(turns[0].reason, "Final result");
    }

    #[test]
    fn test_quick_ops_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let records = vec![assistant_record(&["Edit"], 0, 100)];
        write_index_jsonl(tmp.path(), "s1", &records);

        let turns = extract_key_turns("s1", &SessionType::QuickOps, tmp.path(), 5).unwrap();
        assert!(turns.is_empty());
    }
}
