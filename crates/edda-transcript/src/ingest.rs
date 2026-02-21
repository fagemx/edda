use crate::cursor::TranscriptCursor;
use crate::filter::{classify_record, update_progress_last, FilterAction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024; // 4MB

/// Callback type for index generation during ingest.
pub type IndexWriterFn = dyn Fn(&str, u64, u64, &serde_json::Value) -> anyhow::Result<()>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IngestStats {
    pub records_read: usize,
    pub records_kept: usize,
    pub records_dropped: usize,
    pub bytes_read: u64,
    pub kept_by_type: HashMap<String, usize>,
    pub dropped_by_type: HashMap<String, usize>,
    pub from_offset: u64,
    pub to_offset: u64,
}

/// Perform cursor-based delta ingest from a Claude transcript JSONL file.
///
/// Reads from `transcript_path` starting at the cursor offset (or 0 if new),
/// classifies records, writes kept records verbatim to the store,
/// and returns ingest statistics.
///
/// If `index_writer` is Some, calls it for each kept record with
/// (raw_line, store_offset, store_len, parsed_json) for index generation.
pub fn ingest_transcript_delta(
    project_dir: &Path,
    session_id: &str,
    transcript_path: &Path,
    index_writer: Option<&IndexWriterFn>,
) -> anyhow::Result<IngestStats> {
    let state_dir = project_dir.join("state");
    std::fs::create_dir_all(&state_dir)?;

    // Session-level lock
    let lock_path = state_dir.join(format!("ingest.{session_id}.lock"));
    let _lock = edda_store::lock_file(&lock_path)?;

    // Load or create cursor
    let mut cursor = TranscriptCursor::load(&state_dir, session_id)?.unwrap_or(TranscriptCursor {
        offset: 0,
        file_size: 0,
        mtime_unix: 0,
        updated_at_unix: 0,
    });

    // Check file metadata
    let meta = std::fs::metadata(transcript_path)?;
    let file_size = meta.len();

    // Truncation detection
    cursor.detect_truncation(file_size);

    if cursor.offset >= file_size {
        // Nothing new to read
        return Ok(IngestStats {
            records_read: 0,
            records_kept: 0,
            records_dropped: 0,
            bytes_read: 0,
            kept_by_type: HashMap::new(),
            dropped_by_type: HashMap::new(),
            from_offset: cursor.offset,
            to_offset: cursor.offset,
        });
    }

    let max_bytes: u64 = std::env::var("EDDA_TRANSCRIPT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_BYTES);

    // Open and seek
    let mut file = std::fs::File::open(transcript_path)?;
    file.seek(SeekFrom::Start(cursor.offset))?;

    let bytes_to_read = (file_size - cursor.offset).min(max_bytes);
    let mut buf = vec![0u8; bytes_to_read as usize];
    let actually_read = file.read(&mut buf)?;
    buf.truncate(actually_read);

    // Partial line protection: only consume up to the last newline
    let consumable_len = match buf.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => 0, // no complete line
    };

    if consumable_len == 0 {
        return Ok(IngestStats {
            records_read: 0,
            records_kept: 0,
            records_dropped: 0,
            bytes_read: 0,
            kept_by_type: HashMap::new(),
            dropped_by_type: HashMap::new(),
            from_offset: cursor.offset,
            to_offset: cursor.offset,
        });
    }

    let from_offset = cursor.offset;
    let data = &buf[..consumable_len];

    // Prepare store path (verbatim append)
    let transcripts_dir = project_dir.join("transcripts");
    std::fs::create_dir_all(&transcripts_dir)?;
    let store_path = transcripts_dir.join(format!("{session_id}.jsonl"));
    let mut store_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&store_path)?;

    // Load progress_last map
    let progress_path = state_dir.join(format!("progress_last.{session_id}.json"));
    let mut progress_map: HashMap<String, serde_json::Value> = if progress_path.exists() {
        let content = std::fs::read_to_string(&progress_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let mut stats = IngestStats {
        records_read: 0,
        records_kept: 0,
        records_dropped: 0,
        bytes_read: consumable_len as u64,
        kept_by_type: HashMap::new(),
        dropped_by_type: HashMap::new(),
        from_offset,
        to_offset: from_offset + consumable_len as u64,
    };

    // Process line by line
    for raw_line in data.split(|&b| b == b'\n') {
        if raw_line.is_empty() {
            continue;
        }

        stats.records_read += 1;

        let parsed: serde_json::Value = match serde_json::from_slice(raw_line) {
            Ok(v) => v,
            Err(_) => {
                stats.records_dropped += 1;
                *stats
                    .dropped_by_type
                    .entry("parse_error".into())
                    .or_insert(0) += 1;
                continue;
            }
        };

        let record_type = parsed
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        match classify_record(&parsed) {
            FilterAction::Keep => {
                // Record store_offset before write
                let store_offset = store_file.seek(SeekFrom::End(0)).unwrap_or(0);

                // Write raw line verbatim (CONTRACT BRIDGE-03)
                store_file.write_all(raw_line)?;
                store_file.write_all(b"\n")?;

                let store_len = raw_line.len() as u64 + 1; // +1 for newline

                // Call index writer if provided
                if let Some(writer) = index_writer {
                    let raw_str = std::str::from_utf8(raw_line).unwrap_or("");
                    writer(raw_str, store_offset, store_len, &parsed)?;
                }

                stats.records_kept += 1;
                *stats.kept_by_type.entry(record_type).or_insert(0) += 1;
            }
            FilterAction::Progress => {
                update_progress_last(&mut progress_map, &parsed);
                stats.records_dropped += 1;
                *stats.dropped_by_type.entry(record_type).or_insert(0) += 1;
            }
            FilterAction::Drop => {
                stats.records_dropped += 1;
                *stats.dropped_by_type.entry(record_type).or_insert(0) += 1;
            }
        }
    }

    // Save progress_last map
    let progress_json = serde_json::to_string_pretty(&progress_map)?;
    edda_store::write_atomic(&progress_path, progress_json.as_bytes())?;

    // Update and save cursor
    cursor.offset = stats.to_offset;
    cursor.file_size = file_size;
    cursor.updated_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    cursor.save(&state_dir, session_id)?;

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_transcript(dir: &Path, lines: &[&str]) -> std::path::PathBuf {
        let path = dir.join("transcript.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn ingest_basic_keep_and_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        let transcript = write_transcript(
            tmp.path(),
            &[
                r#"{"type":"user","uuid":"u1","message":{"content":"hello"}}"#,
                r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"content":[{"type":"text","text":"hi"}]}}"#,
                r#"{"type":"progress","toolUseID":"t1","data":{"output":"running"}}"#,
                r#"{"type":"system","subtype":"turn_duration","duration_ms":100}"#,
            ],
        );

        let stats = ingest_transcript_delta(&project_dir, "sess1", &transcript, None).unwrap();

        assert_eq!(stats.records_read, 4);
        assert_eq!(stats.records_kept, 2); // user + assistant
        assert_eq!(stats.records_dropped, 2); // progress + turn_duration

        // Verify verbatim store
        let store = project_dir.join("transcripts").join("sess1.jsonl");
        let content = std::fs::read_to_string(&store).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"type\":\"user\""));
        assert!(lines[1].contains("\"type\":\"assistant\""));
    }

    #[test]
    fn ingest_cursor_based_delta() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        let transcript_path = tmp.path().join("transcript.jsonl");

        // First write
        {
            let mut f = std::fs::File::create(&transcript_path).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","uuid":"u1","message":{{"content":"first"}}}}"#
            )
            .unwrap();
        }
        let stats1 =
            ingest_transcript_delta(&project_dir, "sess1", &transcript_path, None).unwrap();
        assert_eq!(stats1.records_kept, 1);

        // Append more
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript_path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"user","uuid":"u2","message":{{"content":"second"}}}}"#
            )
            .unwrap();
        }
        let stats2 =
            ingest_transcript_delta(&project_dir, "sess1", &transcript_path, None).unwrap();
        assert_eq!(stats2.records_kept, 1); // only the new line
        assert_eq!(stats2.from_offset, stats1.to_offset);

        // Store should have 2 lines total
        let store = project_dir.join("transcripts").join("sess1.jsonl");
        let content = std::fs::read_to_string(&store).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn ingest_with_index_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        let transcript = write_transcript(
            tmp.path(),
            &[r#"{"type":"user","uuid":"u1","message":{"content":"hello"}}"#],
        );

        let called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let called_clone = called.clone();

        let writer = move |_raw: &str,
                           _offset: u64,
                           _len: u64,
                           _json: &serde_json::Value|
              -> anyhow::Result<()> {
            called_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        };

        ingest_transcript_delta(&project_dir, "sess1", &transcript, Some(&writer)).unwrap();

        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
