use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranscriptCursor {
    pub offset: u64,
    pub file_size: u64,
    pub mtime_unix: i64,
    pub updated_at_unix: i64,
}

impl TranscriptCursor {
    pub fn load(state_dir: &Path, session_id: &str) -> anyhow::Result<Option<Self>> {
        let path = state_dir.join(format!("transcript_cursor.{session_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let cursor: Self = serde_json::from_str(&content)?;
        Ok(Some(cursor))
    }

    pub fn save(&self, state_dir: &Path, session_id: &str) -> anyhow::Result<()> {
        let path = state_dir.join(format!("transcript_cursor.{session_id}.json"));
        let data = serde_json::to_string_pretty(self)?;
        edda_store::write_atomic(&path, data.as_bytes())
    }

    /// Check for truncation: if file shrank, reset offset to 0.
    pub fn detect_truncation(&mut self, current_file_size: u64) {
        if current_file_size < self.offset {
            self.offset = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let cursor = TranscriptCursor {
            offset: 100,
            file_size: 5000,
            mtime_unix: 1700000000,
            updated_at_unix: 1700000001,
        };
        cursor.save(tmp.path(), "sess1").unwrap();
        let loaded = TranscriptCursor::load(tmp.path(), "sess1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.offset, 100);
        assert_eq!(loaded.file_size, 5000);
    }

    #[test]
    fn cursor_load_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = TranscriptCursor::load(tmp.path(), "missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn truncation_detection() {
        let mut cursor = TranscriptCursor {
            offset: 5000,
            file_size: 5000,
            mtime_unix: 0,
            updated_at_unix: 0,
        };
        cursor.detect_truncation(3000);
        assert_eq!(cursor.offset, 0);
    }
}
