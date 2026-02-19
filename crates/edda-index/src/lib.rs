use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

// ── IndexRecordV1 ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexRecordV1 {
    pub v: u32,
    pub session_id: String,
    pub uuid: String,
    pub parent_uuid: Option<String>,
    #[serde(rename = "type")]
    pub record_type: String,
    pub ts: String,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub store_offset: u64,
    pub store_len: u64,
    pub assistant: Option<AssistantMeta>,
    pub usage: Option<UsageMeta>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssistantMeta {
    #[serde(default)]
    pub tool_use_ids: Vec<String>,
    #[serde(default)]
    pub tool_use_names: Vec<String>,
    #[serde(default)]
    pub bash_commands: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageMeta {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

// ── Index operations ──

/// Append a single IndexRecordV1 line to the index JSONL file.
pub fn append_index(index_path: &Path, record: &IndexRecordV1) -> anyhow::Result<()> {
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(record)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(index_path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Read the tail of an index file (last N lines, up to max_bytes).
pub fn read_index_tail(
    index_path: &Path,
    max_lines: usize,
    max_bytes: u64,
) -> anyhow::Result<Vec<IndexRecordV1>> {
    if !index_path.exists() {
        return Ok(vec![]);
    }

    let meta = std::fs::metadata(index_path)?;
    let file_size = meta.len();
    let read_from = file_size.saturating_sub(max_bytes);

    let mut file = std::fs::File::open(index_path)?;
    file.seek(SeekFrom::Start(read_from))?;

    let mut buf = String::new();
    file.read_to_string(&mut buf)?;

    // If we started mid-file, drop the first partial line
    let lines_str = if read_from > 0 {
        if let Some(pos) = buf.find('\n') {
            &buf[pos + 1..]
        } else {
            ""
        }
    } else {
        &buf
    };

    let all_lines: Vec<&str> = lines_str.lines().collect();
    let start = if all_lines.len() > max_lines {
        all_lines.len() - max_lines
    } else {
        0
    };

    let mut records = Vec::new();
    for line in &all_lines[start..] {
        if line.is_empty() {
            continue;
        }
        if let Ok(rec) = serde_json::from_str::<IndexRecordV1>(line) {
            records.push(rec);
        }
    }
    Ok(records)
}

// ── Deterministic fetch ──

/// Fetch a raw line from the store file at the given offset and length.
/// Returns the raw bytes (trailing newline stripped).
pub fn fetch_store_line(store_path: &Path, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
    let mut file = std::fs::File::open(store_path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)?;
    // Strip trailing newline
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    Ok(buf)
}

// ── Build IndexRecordV1 from raw JSON ──

/// Build an IndexRecordV1 from a parsed transcript record JSON.
pub fn build_index_record(
    session_id: &str,
    store_offset: u64,
    store_len: u64,
    parsed: &serde_json::Value,
) -> IndexRecordV1 {
    let uuid = parsed
        .get("uuid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let parent_uuid = parsed
        .get("parentUuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let record_type = parsed
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let ts = parsed
        .get("timestamp")
        .or_else(|| parsed.get("ts"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let git_branch: Option<String> = None;
    let cwd = parsed
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract assistant metadata if this is an assistant record
    let assistant = if record_type == "assistant" {
        Some(extract_assistant_meta(parsed))
    } else {
        None
    };

    // Extract usage metadata
    let usage = parsed.get("usage").map(|u| UsageMeta {
        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_read_input_tokens: u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
    });

    IndexRecordV1 {
        v: 1,
        session_id: session_id.to_string(),
        uuid,
        parent_uuid,
        record_type,
        ts,
        git_branch,
        cwd,
        store_offset,
        store_len,
        assistant,
        usage,
    }
}

fn extract_assistant_meta(parsed: &serde_json::Value) -> AssistantMeta {
    let mut tool_use_ids = Vec::new();
    let mut tool_use_names = Vec::new();
    let mut bash_commands = Vec::new();

    if let Some(content) = parsed
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for block in content {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if block_type == "tool_use" {
                if let Some(id) = block.get("id").and_then(|v| v.as_str()) {
                    tool_use_ids.push(id.to_string());
                }
                if let Some(name) = block.get("name").and_then(|v| v.as_str()) {
                    tool_use_names.push(name.to_string());
                    if name == "Bash" || name == "bash" {
                        if let Some(cmd) = block
                            .get("input")
                            .and_then(|i| i.get("command"))
                            .and_then(|c| c.as_str())
                        {
                            bash_commands.push(cmd.to_string());
                        }
                    }
                }
            }
        }
    }

    AssistantMeta {
        tool_use_ids,
        tool_use_names,
        bash_commands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_index() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.jsonl");

        let record = IndexRecordV1 {
            v: 1,
            session_id: "s1".into(),
            uuid: "uuid1".into(),
            parent_uuid: None,
            record_type: "user".into(),
            ts: "2025-01-01T00:00:00Z".into(),
            git_branch: None,
            cwd: None,
            store_offset: 0,
            store_len: 100,
            assistant: None,
            usage: None,
        };

        append_index(&path, &record).unwrap();
        let records = read_index_tail(&path, 100, 1024 * 1024).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].uuid, "uuid1");
        assert_eq!(records[0].store_offset, 0);
        assert_eq!(records[0].store_len, 100);
    }

    #[test]
    fn fetch_store_line_works() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store.jsonl");

        // Write two lines
        let line1 = r#"{"type":"user","uuid":"u1"}"#;
        let line2 = r#"{"type":"assistant","uuid":"a1"}"#;
        let mut f = std::fs::File::create(&store).unwrap();
        writeln!(f, "{line1}").unwrap();
        writeln!(f, "{line2}").unwrap();

        let offset1 = 0u64;
        let len1 = line1.len() as u64 + 1; // +newline

        let fetched = fetch_store_line(&store, offset1, len1).unwrap();
        let fetched_str = std::str::from_utf8(&fetched).unwrap();
        assert_eq!(fetched_str, line1);

        let offset2 = len1;
        let len2 = line2.len() as u64 + 1;
        let fetched2 = fetch_store_line(&store, offset2, len2).unwrap();
        let fetched_str2 = std::str::from_utf8(&fetched2).unwrap();
        assert_eq!(fetched_str2, line2);
    }

    #[test]
    fn build_index_record_extracts_fields() {
        let parsed = serde_json::json!({
            "type": "assistant",
            "uuid": "a1",
            "parentUuid": "u1",
            "message": {
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "ls"}}
                ]
            },
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });

        let record = build_index_record("s1", 0, 200, &parsed);
        assert_eq!(record.uuid, "a1");
        assert_eq!(record.parent_uuid, Some("u1".into()));
        assert_eq!(record.record_type, "assistant");
        let asst = record.assistant.unwrap();
        assert_eq!(asst.tool_use_ids, vec!["tu1"]);
        assert_eq!(asst.tool_use_names, vec!["Bash"]);
        assert_eq!(asst.bash_commands, vec!["ls"]);
        let usage = record.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }
}
