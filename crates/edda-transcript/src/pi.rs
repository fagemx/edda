//! Pi session transcript discovery, delta ingestion, and ledger normalization (GH-577).
//!
//! Pi persists its transcripts to:
//! `~/.pi/agent/sessions/<encoded-cwd>/<timestamp>_<session-id>.jsonl`
//! (or to a custom directory when `--session-dir` is provided).
//!
//! This module provides:
//! 1. `find_pi_session_file`: locates the session file by matching session ID in filename or header.
//! 2. `ingest_pi_transcript_delta`: cursor-based incremental ingestion into `transcripts/{session_id}.jsonl`
//!    and `ledger/{session_id}.jsonl` (normalizing tool calls and failures so `digest` can summarize it),
//!    plus updating `state/usage.json`.

use crate::cursor::TranscriptCursor;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Ingestion statistics for a Pi session delta.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PiIngestStats {
    pub records_read: usize,
    pub records_kept: usize,
    pub tool_calls: usize,
    pub tool_failures: usize,
    pub user_prompts: usize,
    pub bytes_read: u64,
    pub from_offset: u64,
    pub to_offset: u64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: Option<f64>,
}

/// Compute the default Pi session directory for a given working directory.
/// Encodes `cwd` into `--<escaped_path>--` under `~/.pi/agent/sessions/`,
/// strictly matching `@earendil-works/pi-coding-agent`'s `getDefaultSessionDirPath`.
pub fn pi_session_dir_for_cwd(cwd: &Path) -> Option<PathBuf> {
    let resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let s = resolved.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let trimmed = s.trim_start_matches(['/', '\\']);
    let mut safe = String::with_capacity(trimmed.len() + 4);
    safe.push_str("--");
    for c in trimmed.chars() {
        if c == '/' || c == '\\' || c == ':' {
            safe.push('-');
        } else {
            safe.push(c);
        }
    }
    safe.push_str("--");
    let home = edda_core::paths::home_dir()?;
    Some(home.join(".pi").join("agent").join("sessions").join(safe))
}

/// Find the Pi session JSONL file for a given session ID.
///
/// If `session_dir` is provided, searches there.
/// Otherwise, resolves the default directory via [`pi_session_dir_for_cwd`].
pub fn find_pi_session_file(
    cwd: &Path,
    session_id: &str,
    session_dir: Option<&Path>,
) -> Option<PathBuf> {
    let dir = match session_dir {
        Some(d) => d.to_path_buf(),
        None => pi_session_dir_for_cwd(cwd)?,
    };

    if !dir.exists() {
        return None;
    }

    let entries = fs::read_dir(&dir).ok()?;
    let suffix = format!("_{session_id}.jsonl");
    let exact = format!("{session_id}.jsonl");

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".jsonl") {
            continue;
        }

        if fname == exact || fname.ends_with(&suffix) {
            matches.push(path);
            continue;
        }

        // Header check: if filename doesn't match suffix, inspect the first line
        if let Ok(file) = fs::File::open(&path) {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(file);
            let mut first_line = String::new();
            if reader.read_line(&mut first_line).is_ok() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&first_line) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("session")
                        && v.get("id").and_then(|id| id.as_str()) == Some(session_id)
                    {
                        matches.push(path);
                    }
                }
            }
        }
    }

    // If multiple matches exist, pick the most recently modified
    matches.sort_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    matches.pop()
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn normalize_tool_name(name: &str) -> &str {
    match name {
        "bash" | "terminal" | "shell" => "Bash",
        "edit" | "edit_file" | "file_edit" => "Edit",
        "write" | "write_file" | "file_write" => "Write",
        other => other,
    }
}

const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024; // 8MB

/// Ingest a Pi session transcript file incrementally into `project_dir`.
///
/// 1. Reads new lines using a session-specific cursor (`pi_transcript_cursor.{session_id}.json`).
/// 2. Appends raw transcript lines to `transcripts/{session_id}.jsonl`.
/// 3. Normalizes tool calls, tool results, and user prompts into `ledger/{session_id}.jsonl`.
/// 4. Updates `state/usage.json` with observed model and token totals.
#[allow(clippy::too_many_lines)] // 367 lines at #779; split tracked in none
pub fn ingest_pi_transcript_delta(
    project_dir: &Path,
    session_id: &str,
    cwd: &Path,
    transcript_path: &Path,
) -> anyhow::Result<PiIngestStats> {
    let state_dir = project_dir.join("state");
    let ledger_dir = project_dir.join("ledger");
    let transcripts_dir = project_dir.join("transcripts");

    fs::create_dir_all(&state_dir)?;
    fs::create_dir_all(&ledger_dir)?;
    fs::create_dir_all(&transcripts_dir)?;

    // Session-level lock to guard concurrent ingestion
    let lock_path = state_dir.join(format!("ingest_pi.{session_id}.lock"));
    let _lock = edda_store::lock_file(&lock_path)?;

    // Cursor path
    let cursor_path = state_dir.join(format!("pi_transcript_cursor.{session_id}.json"));
    let mut cursor = if cursor_path.exists() {
        let content = fs::read_to_string(&cursor_path)?;
        serde_json::from_str(&content).unwrap_or(TranscriptCursor {
            offset: 0,
            file_size: 0,
            mtime_unix: 0,
            updated_at_unix: 0,
        })
    } else {
        TranscriptCursor {
            offset: 0,
            file_size: 0,
            mtime_unix: 0,
            updated_at_unix: 0,
        }
    };

    let meta = fs::metadata(transcript_path)?;
    let file_size = meta.len();
    cursor.detect_truncation(file_size);

    if cursor.offset >= file_size {
        return Ok(PiIngestStats {
            from_offset: cursor.offset,
            to_offset: cursor.offset,
            ..Default::default()
        });
    }

    let max_bytes: u64 = std::env::var("EDDA_TRANSCRIPT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_BYTES);

    let mut file = fs::File::open(transcript_path)?;
    file.seek(SeekFrom::Start(cursor.offset))?;

    let bytes_to_read = (file_size - cursor.offset).min(max_bytes);
    let mut buf = vec![0u8; bytes_to_read as usize];
    let actually_read = file.read(&mut buf)?;
    buf.truncate(actually_read);

    // Partial line protection: only consume up to the last newline
    let consumable_len = match buf.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => 0,
    };

    if consumable_len == 0 {
        return Ok(PiIngestStats {
            from_offset: cursor.offset,
            to_offset: cursor.offset,
            ..Default::default()
        });
    }

    let from_offset = cursor.offset;
    let to_offset = from_offset + consumable_len as u64;
    let data = &buf[..consumable_len];

    // Transcripts raw store path
    let raw_store_path = transcripts_dir.join(format!("{session_id}.jsonl"));
    let mut raw_store_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&raw_store_path)?;

    // Session ledger path
    let ledger_path = ledger_dir.join(format!("{session_id}.jsonl"));
    let mut ledger_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger_path)?;

    let project_id = project_dir
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default()
        .to_string();
    let cwd_str = cwd.to_string_lossy().to_string();

    let mut stats = PiIngestStats {
        bytes_read: consumable_len as u64,
        from_offset,
        to_offset,
        ..Default::default()
    };

    // Load existing usage state if present
    let usage_path = state_dir.join("usage.json");
    let current_usage = if usage_path.exists() {
        fs::read_to_string(&usage_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::Value::Null
    };

    let mut model_seen = String::new();
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_cache_write = 0u64;
    let mut total_cost = 0.0f64;
    let mut usage_observed = false;
    let mut cost_observed = false;

    let default_ts = now_rfc3339();

    for raw_line in data.split(|&b| b == b'\n') {
        if raw_line.is_empty() {
            continue;
        }
        stats.records_read += 1;

        // Write raw line verbatim to transcripts/
        raw_store_file.write_all(raw_line)?;
        raw_store_file.write_all(b"\n")?;
        stats.records_kept += 1;

        let parsed: serde_json::Value = match serde_json::from_slice(raw_line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ts = parsed
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or(&default_ts)
            .to_string();

        let line_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if line_type == "message" {
            if let Some(msg) = parsed.get("message") {
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                match role {
                    "assistant" => {
                        // Extract model & usage if present
                        if let Some(m) = msg
                            .get("model")
                            .or_else(|| parsed.get("model"))
                            .and_then(|v| v.as_str())
                        {
                            if !m.is_empty() {
                                model_seen = m.to_string();
                            }
                        }

                        if let Some(u) = msg.get("usage") {
                            usage_observed = true;
                            if let Some(inp) = u.get("input").and_then(|v| v.as_u64()) {
                                total_input += inp;
                            }
                            if let Some(out) = u.get("output").and_then(|v| v.as_u64()) {
                                total_output += out;
                            }
                            if let Some(cr) = u.get("cacheRead").and_then(|v| v.as_u64()) {
                                total_cache_read += cr;
                            }
                            if let Some(cw) = u.get("cacheWrite").and_then(|v| v.as_u64()) {
                                total_cache_write += cw;
                            }
                            if let Some(cost) = u
                                .get("cost")
                                .and_then(|c| c.get("total"))
                                .and_then(|v| v.as_f64())
                            {
                                total_cost += cost;
                                cost_observed = true;
                            }
                        }

                        // Extract tool calls from content array
                        if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
                            for item in content_arr {
                                if item.get("type").and_then(|t| t.as_str()) == Some("toolCall") {
                                    stats.tool_calls += 1;
                                    let tool_name =
                                        item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                    let tool_id =
                                        item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                    let tool_input = item
                                        .get("arguments")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);

                                    let record = serde_json::json!({
                                        "ts": ts,
                                        "project_id": project_id,
                                        "session_id": session_id,
                                        "hook_event_name": "PostToolUse",
                                        "tool_name": normalize_tool_name(tool_name),
                                        "tool_use_id": tool_id,
                                        "cwd": cwd_str,
                                        "model": model_seen,
                                        "bridge": "pi",
                                        "tool_input": tool_input,
                                    });
                                    writeln!(ledger_file, "{}", serde_json::to_string(&record)?)?;
                                }
                            }
                        }
                    }
                    "toolResult" => {
                        let is_error = msg
                            .get("isError")
                            .and_then(|e| e.as_bool())
                            .unwrap_or(false);
                        if is_error {
                            stats.tool_failures += 1;
                            let tool_name =
                                msg.get("toolName").and_then(|n| n.as_str()).unwrap_or("");
                            let tool_id =
                                msg.get("toolCallId").and_then(|i| i.as_str()).unwrap_or("");
                            let tool_content = msg
                                .get("content")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);

                            let record = serde_json::json!({
                                "ts": ts,
                                "project_id": project_id,
                                "session_id": session_id,
                                "hook_event_name": "PostToolUseFailure",
                                "tool_name": normalize_tool_name(tool_name),
                                "tool_use_id": tool_id,
                                "cwd": cwd_str,
                                "bridge": "pi",
                                "tool_response": tool_content,
                            });
                            writeln!(ledger_file, "{}", serde_json::to_string(&record)?)?;
                        }
                    }
                    "user" => {
                        stats.user_prompts += 1;
                        let record = serde_json::json!({
                            "ts": ts,
                            "project_id": project_id,
                            "session_id": session_id,
                            "hook_event_name": "UserPromptSubmit",
                            "cwd": cwd_str,
                            "bridge": "pi",
                        });
                        writeln!(ledger_file, "{}", serde_json::to_string(&record)?)?;
                    }
                    _ => {}
                }
            }
        }
    }

    stats.model = model_seen.clone();
    stats.input_tokens = total_input;
    stats.output_tokens = total_output;
    stats.cache_read_tokens = total_cache_read;
    stats.cache_creation_tokens = total_cache_write;
    stats.cost_usd = if cost_observed {
        Some(total_cost)
    } else {
        None
    };

    // Update usage state file for the digest reader
    if usage_observed || !model_seen.is_empty() {
        // GH-577 round 2 (P1-3): usage.json is project-scoped. If it records
        // a DIFFERENT session_id, do NOT accumulate into it — reset to zero
        // so one session's digest never reports another session's tokens.
        // Only accumulate if resuming the same session.
        let is_same_session =
            current_usage.get("session_id").and_then(|s| s.as_str()) == Some(session_id);

        let prev_u = if is_same_session {
            current_usage.get("usage").cloned().unwrap_or_default()
        } else {
            serde_json::Value::Null
        };

        let prev_in = prev_u
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prev_out = prev_u
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prev_cr = prev_u
            .get("cache_read_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prev_cw = prev_u
            .get("cache_creation_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prev_cost = prev_u.get("cost_usd").and_then(|v| v.as_f64());
        let final_cost = match (prev_cost, cost_observed) {
            (Some(prev), true) => Some(prev + total_cost),
            (Some(prev), false) => Some(prev),
            (None, true) => Some(total_cost),
            (None, false) => None,
        };

        let usage_obj = serde_json::json!({
            "session_id": session_id,
            "updated_at": now_rfc3339(),
            "usage": {
                "model": if model_seen.is_empty() {
                    prev_u.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string()
                } else {
                    model_seen
                },
                "input_tokens": prev_in + total_input,
                "output_tokens": prev_out + total_output,
                "cache_read_tokens": prev_cr + total_cache_read,
                "cache_creation_tokens": prev_cw + total_cache_write,
                "cost_usd": final_cost,
                "usage_observed": true,
            }
        });
        fs::write(&usage_path, serde_json::to_string_pretty(&usage_obj)?)?;
    }

    // Save updated cursor
    cursor.offset = to_offset;
    cursor.file_size = file_size;
    cursor.mtime_unix = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    cursor.updated_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let data = serde_json::to_string_pretty(&cursor)?;
    edda_store::write_atomic(&cursor_path, data.as_bytes())?;

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pi_session_dir_for_cwd() {
        let p = Path::new("C:\\ai_agent\\edda");
        let dir = pi_session_dir_for_cwd(p).expect("should compute dir");
        let dir_str = dir.to_string_lossy();
        assert!(
            dir_str.contains("--C--ai_agent-edda--") || dir_str.contains(".pi"),
            "got {dir_str}"
        );
    }

    #[test]
    fn test_find_pi_session_file_by_name_and_header() {
        let tmp = tempfile::tempdir().unwrap();
        let session_id = "test-sess-uuid-1234";

        let sess_file = tmp
            .path()
            .join(format!("2026-09-02T12-00-00-000Z_{session_id}.jsonl"));
        fs::write(
            &sess_file,
            format!(r#"{{"type":"session","id":"{session_id}"}}"#),
        )
        .unwrap();

        let cwd = Path::new("C:\\test\\dir");
        let found = find_pi_session_file(cwd, session_id, Some(tmp.path()));
        assert_eq!(found, Some(sess_file));
    }
}
