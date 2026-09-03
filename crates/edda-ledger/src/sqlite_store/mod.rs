//! SQLite-backed storage for the edda ledger.
//!
//! Replaces the file-based storage (events.jsonl, refs/HEAD, refs/branches.json)
//! with a single `ledger.db` SQLite file using WAL mode.

mod decisions;
mod dependencies;
mod entities;
mod events;
mod mappers;
mod schema;
mod task_leases;
pub mod types;
mod village;

pub use schema::{UnsupportedSchemaVersionError, MAX_KNOWN_SCHEMA_VERSION};
pub use types::*;

use rusqlite::Connection;
use std::path::Path;

/// Map a decision status string to the legacy is_active boolean.
///
/// `is_active = true` iff status is "active" or "experimental".
/// This enforces CONTRACT COMPAT-01.
fn status_to_is_active(status: &str) -> bool {
    matches!(status, "active" | "experimental")
}

/// SQLite-backed storage engine.
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Open an existing ledger.db.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.apply_pragmas()?;
        Ok(store)
    }

    /// Open or create ledger.db with full schema.
    pub fn open_or_create(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.apply_pragmas()?;
        store.apply_schema()?;
        Ok(store)
    }

    /// Open an existing ledger.db without creating or migrating anything.
    ///
    /// Fails when the file does not exist. Unlike [`SqliteStore::open_or_create`],
    /// this never creates the database file, never applies the schema, and
    /// never runs migrations; the connection is opened `query_only`, so a
    /// read-only integrity consumer (`edda verify`, GH-647) cannot repair,
    /// migrate, or otherwise write to the ledger it is checking.
    pub fn open_existing(db_path: &Path) -> anyhow::Result<Self> {
        if !db_path.is_file() {
            anyhow::bail!(
                "ledger database not found: {} (run `edda init` to create a workspace)",
                db_path.display()
            );
        }
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        // Connection-level settings only, and `query_only` last: no schema
        // application, no `journal_mode` write, no checkpoint-on-drop write.
        store
            .conn
            .execute_batch("PRAGMA busy_timeout = 5000; PRAGMA query_only = ON;")?;
        store.check_schema_version_supported()?;
        Ok(store)
    }

    fn apply_pragmas(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        // Merge WAL back into main DB so users see a single file when idle.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

#[cfg(test)]
mod tests;
