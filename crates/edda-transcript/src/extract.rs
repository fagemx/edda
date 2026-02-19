use std::io::{BufRead, BufReader};
use std::path::Path;

/// Extract the last assistant text message from a stored transcript JSONL.
///
/// Scans the file line by line (forward), keeping track of the last assistant
/// text seen. Returns the final one, truncated to `max_chars`.
///
/// Expected format per line:
/// ```json
/// {"type":"assistant","message":{"content":[{"type":"text","text":"..."},{"type":"tool_use",...}]}}
/// ```
///
/// Only extracts `content` blocks with `"type":"text"`. Skips `tool_use` blocks.
pub fn extract_last_assistant_text(store_path: &Path, max_chars: usize) -> Option<String> {
    let file = std::fs::File::open(store_path).ok()?;
    let reader = BufReader::new(file);

    let mut last_text: Option<String> = None;

    for line in reader.lines() {
        let line = line.ok()?;
        if line.is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if parsed.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        // Extract text from message.content array
        if let Some(content_arr) = parsed
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            let mut texts: Vec<&str> = Vec::new();
            for block in content_arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        texts.push(text);
                    }
                }
            }
            if !texts.is_empty() {
                last_text = Some(texts.join("\n"));
            }
        }
    }

    // Truncate to max_chars
    last_text.map(|t| {
        if t.len() > max_chars {
            let truncated = &t[..t.floor_char_boundary(max_chars)];
            format!("{truncated}...")
        } else {
            t
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_store(dir: &Path, lines: &[&str]) -> std::path::PathBuf {
        let path = dir.join("test-session.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn extract_basic_assistant_text() {
        let tmp = tempfile::tempdir().unwrap();
        let store = write_store(
            tmp.path(),
            &[
                r#"{"type":"user","uuid":"u1","message":{"content":"hello"}}"#,
                r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"Hi there!"}]}}"#,
            ],
        );

        let result = extract_last_assistant_text(&store, 500);
        assert_eq!(result.as_deref(), Some("Hi there!"));
    }

    #[test]
    fn extract_skips_tool_use_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let store = write_store(
            tmp.path(),
            &[
                r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"Let me check"},{"type":"tool_use","id":"tu1","name":"Bash"}]}}"#,
            ],
        );

        let result = extract_last_assistant_text(&store, 500);
        assert_eq!(result.as_deref(), Some("Let me check"));
    }

    #[test]
    fn extract_returns_last_assistant() {
        let tmp = tempfile::tempdir().unwrap();
        let store = write_store(
            tmp.path(),
            &[
                r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"first"}]}}"#,
                r#"{"type":"user","uuid":"u2","message":{"content":"more"}}"#,
                r#"{"type":"assistant","uuid":"a2","message":{"content":[{"type":"text","text":"second and final"}]}}"#,
            ],
        );

        let result = extract_last_assistant_text(&store, 500);
        assert_eq!(result.as_deref(), Some("second and final"));
    }

    #[test]
    fn extract_truncates_long_text() {
        let tmp = tempfile::tempdir().unwrap();
        let long_text = "x".repeat(1000);
        let line = format!(
            r#"{{"type":"assistant","uuid":"a1","message":{{"content":[{{"type":"text","text":"{long_text}"}}]}}}}"#
        );
        let store = write_store(tmp.path(), &[&line]);

        let result = extract_last_assistant_text(&store, 100).unwrap();
        assert!(result.len() <= 104); // 100 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn extract_no_assistant_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = write_store(
            tmp.path(),
            &[r#"{"type":"user","uuid":"u1","message":{"content":"hello"}}"#],
        );

        let result = extract_last_assistant_text(&store, 500);
        assert!(result.is_none());
    }

    #[test]
    fn extract_missing_file_returns_none() {
        let result = extract_last_assistant_text(Path::new("/nonexistent/file.jsonl"), 500);
        assert!(result.is_none());
    }
}
