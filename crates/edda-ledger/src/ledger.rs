use crate::paths::EddaPaths;
use edda_core::Event;
use std::io::{BufRead, Write};
use std::path::Path;

/// The append-only event ledger backed by `events.jsonl`.
pub struct Ledger {
    pub paths: EddaPaths,
}

impl Ledger {
    /// Open an existing workspace. Fails if `.edda/` does not exist.
    pub fn open(repo_root: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let paths = EddaPaths::discover(repo_root);
        if !paths.is_initialized() {
            anyhow::bail!(
                "not a edda workspace ({}/.edda not found). Run `edda init` first.",
                paths.root.display()
            );
        }
        Ok(Self { paths })
    }

    /// Read the current HEAD branch name.
    pub fn head_branch(&self) -> anyhow::Result<String> {
        let content = std::fs::read_to_string(&self.paths.head_file)
            .map_err(|e| anyhow::anyhow!("cannot read HEAD: {e}"))?;
        Ok(content.trim().to_string())
    }

    /// Write the HEAD branch name.
    pub fn set_head_branch(&self, name: &str) -> anyhow::Result<()> {
        std::fs::write(&self.paths.head_file, format!("{name}\n"))?;
        Ok(())
    }

    /// Append an event to `events.jsonl`. Append-only (CONTRACT LEDGER-02).
    pub fn append_event(&self, event: &Event, fsync: bool) -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.events_jsonl)?;
        let json = serde_json::to_string(event)?;
        writeln!(file, "{json}")?;
        if fsync {
            file.sync_all()?;
        }
        Ok(())
    }

    /// Get the hash of the last event, or `None` if the ledger is empty.
    pub fn last_event_hash(&self) -> anyhow::Result<Option<String>> {
        if !self.paths.events_jsonl.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&self.paths.events_jsonl)?;
        let last_line = content.lines().rev().find(|l| !l.trim().is_empty());
        match last_line {
            None => Ok(None),
            Some(line) => {
                let event: Event = serde_json::from_str(line)?;
                Ok(Some(event.hash))
            }
        }
    }

    /// Iterate over all events in the ledger.
    pub fn iter_events(&self) -> anyhow::Result<Vec<Event>> {
        if !self.paths.events_jsonl.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.paths.events_jsonl)?;
        let reader = std::io::BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(&line)?;
            events.push(event);
        }
        Ok(events)
    }
}

/// Initialize a new workspace from `EddaPaths`. Used by `cmd_init`.
pub fn init_workspace(paths: &EddaPaths) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    // Create branches/main/
    std::fs::create_dir_all(paths.branch_dir("main"))?;
    Ok(())
}

/// Write the initial HEAD file if it doesn't exist.
pub fn init_head(paths: &EddaPaths, branch: &str) -> anyhow::Result<()> {
    if !paths.head_file.exists() {
        if let Some(parent) = paths.head_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&paths.head_file, format!("{branch}\n"))?;
    }
    Ok(())
}

/// Write initial branches.json if it doesn't exist.
pub fn init_branches_json(paths: &EddaPaths, branch: &str) -> anyhow::Result<()> {
    if !paths.branches_json.exists() {
        let now = time_now_rfc3339();
        let json = serde_json::json!({
            "branches": {
                branch: {
                    "created_at": now
                }
            }
        });
        std::fs::write(&paths.branches_json, serde_json::to_string_pretty(&json)?)?;
    }
    Ok(())
}

fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

impl Ledger {
    /// Convenience: open from a Path ref (avoids Into<PathBuf> ambiguity).
    pub fn open_path(repo_root: &Path) -> anyhow::Result<Self> {
        Self::open(repo_root.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_ledger_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        init_workspace(&paths).unwrap();
        init_head(&paths, "main").unwrap();
        init_branches_json(&paths, "main").unwrap();
        let ledger = Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn empty_ledger_has_no_hash() {
        let (tmp, ledger) = setup_workspace();
        assert_eq!(ledger.last_event_hash().unwrap(), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn append_and_read_back() {
        let (tmp, ledger) = setup_workspace();
        let e1 = new_note_event("main", None, "system", "init", &[]).unwrap();
        ledger.append_event(&e1, false).unwrap();
        assert_eq!(ledger.last_event_hash().unwrap(), Some(e1.hash.clone()));

        let e2 = new_note_event("main", Some(&e1.hash), "user", "hello", &[]).unwrap();
        ledger.append_event(&e2, false).unwrap();
        assert_eq!(ledger.last_event_hash().unwrap(), Some(e2.hash.clone()));

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, e1.event_id);
        assert_eq!(events[1].event_id, e2.event_id);
        assert_eq!(events[1].parent_hash.as_deref(), Some(e1.hash.as_str()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn head_branch_read_write() {
        let (tmp, ledger) = setup_workspace();
        assert_eq!(ledger.head_branch().unwrap(), "main");
        ledger.set_head_branch("feat/x").unwrap();
        assert_eq!(ledger.head_branch().unwrap(), "feat/x");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn open_without_init_fails() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_no_init_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(Ledger::open(&tmp).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
