use serde_json::Value;
use std::collections::HashMap;

/// Return the largest byte index `<= i` that is a valid char boundary.
/// Equivalent to `str::floor_char_boundary` (unstable nightly API).
fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut pos = i;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterAction {
    Keep,
    Progress,
    Drop,
}

/// Classify a transcript JSONL record.
pub fn classify_record(json: &Value) -> FilterAction {
    let record_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match record_type {
        "user" | "assistant" => FilterAction::Keep,
        "system" => {
            let subtype = json.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "turn_duration" {
                FilterAction::Drop
            } else {
                FilterAction::Keep
            }
        }
        "progress" => FilterAction::Progress,
        "file-history-snapshot" | "queue-operation" => FilterAction::Keep,
        _ => FilterAction::Drop,
    }
}

/// Progress Strategy 3: per-toolUseID, keep only the latest record.
/// Truncate data.output to max chars and limit total entries.
pub fn update_progress_last(progress_map: &mut HashMap<String, Value>, record: &Value) {
    let tool_use_id = record
        .get("toolUseID")
        .or_else(|| record.get("tool_use_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if tool_use_id.is_empty() {
        return;
    }

    let max_output_chars: usize = std::env::var("EDDA_PROGRESS_OUTPUT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);

    let max_tools: usize = std::env::var("EDDA_PROGRESS_MAX_TOOLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    // Truncate data.output if present
    let mut record = record.clone();
    if let Some(data) = record.get_mut("data") {
        let needs_truncate = data
            .get("output")
            .and_then(|v| v.as_str())
            .map(|s| s.len() > max_output_chars)
            .unwrap_or(false);
        if needs_truncate {
            let output = data["output"].as_str().unwrap();
            // Find a valid char boundary at or before max_output_chars
            let end = floor_char_boundary(output, max_output_chars);
            let truncated = output[..end].to_string();
            data.as_object_mut()
                .unwrap()
                .insert("output".into(), Value::String(truncated));
        }
    }

    progress_map.insert(tool_use_id, record);

    // Enforce map size limit
    while progress_map.len() > max_tools {
        // Remove oldest (arbitrary key since HashMap is unordered)
        if let Some(key) = progress_map.keys().next().cloned() {
            progress_map.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_user_keep() {
        let v = serde_json::json!({"type": "user", "message": "hello"});
        assert_eq!(classify_record(&v), FilterAction::Keep);
    }

    #[test]
    fn classify_assistant_keep() {
        let v = serde_json::json!({"type": "assistant"});
        assert_eq!(classify_record(&v), FilterAction::Keep);
    }

    #[test]
    fn classify_system_keep() {
        let v = serde_json::json!({"type": "system", "subtype": "init"});
        assert_eq!(classify_record(&v), FilterAction::Keep);
    }

    #[test]
    fn classify_system_turn_duration_drop() {
        let v = serde_json::json!({"type": "system", "subtype": "turn_duration"});
        assert_eq!(classify_record(&v), FilterAction::Drop);
    }

    #[test]
    fn classify_progress() {
        let v = serde_json::json!({"type": "progress"});
        assert_eq!(classify_record(&v), FilterAction::Progress);
    }

    #[test]
    fn classify_unknown_drop() {
        let v = serde_json::json!({"type": "unknown_type"});
        assert_eq!(classify_record(&v), FilterAction::Drop);
    }

    #[test]
    fn truncate_respects_char_boundary() {
        let mut map = HashMap::new();
        // Build a string that has multi-byte chars near the truncation point.
        // '後' is 3 bytes (E5 BE 8C). Place it so byte index 600 lands mid-char.
        let prefix = "x".repeat(598); // 598 ASCII bytes
        let output = format!("{prefix}後後後 tail"); // byte 598..601 = '後'
        assert!(!output.is_char_boundary(600)); // confirm the setup

        let r = serde_json::json!({
            "toolUseID": "t_utf8",
            "data": { "output": output }
        });
        // Default max_output_chars = 600 → should NOT panic
        update_progress_last(&mut map, &r);
        let stored = map["t_utf8"]["data"]["output"].as_str().unwrap();
        // floor_char_boundary(600) → 598 (before '後' at bytes 598..601)
        assert_eq!(stored.len(), 598);
        assert!(stored.chars().all(|c| c == 'x'));
    }

    #[test]
    fn floor_char_boundary_basic() {
        assert_eq!(super::floor_char_boundary("hello", 3), 3);
        assert_eq!(super::floor_char_boundary("hello", 100), 5);
        // '後' = 3 bytes
        let s = "ab後cd"; // b'a'=0, b'b'=1, '後'=2..5, b'c'=5, b'd'=6
        assert_eq!(super::floor_char_boundary(s, 3), 2); // mid-'後' → back to 2
        assert_eq!(super::floor_char_boundary(s, 4), 2); // still mid-'後'
        assert_eq!(super::floor_char_boundary(s, 5), 5); // at 'c', valid boundary
    }

    #[test]
    fn progress_last_keeps_latest() {
        let mut map = HashMap::new();
        let r1 = serde_json::json!({"toolUseID": "t1", "data": {"output": "old"}});
        let r2 = serde_json::json!({"toolUseID": "t1", "data": {"output": "new"}});
        update_progress_last(&mut map, &r1);
        update_progress_last(&mut map, &r2);
        assert_eq!(map.len(), 1);
        assert_eq!(map["t1"]["data"]["output"], "new");
    }
}
