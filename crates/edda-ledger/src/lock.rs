use crate::paths::EddaPaths;
use fs2::FileExt;
use std::fs::{File, OpenOptions};

/// True when the error is SQLite busy/locked contention — a transient
/// condition where retrying the same read later succeeds (GH-541). Any other
/// ledger error (corrupt database, permission failure, missing workspace) is
/// persistent and must surface to the operator instead of being swallowed as
/// "no verdict yet" forever.
pub fn is_busy_error(err: &anyhow::Error) -> bool {
    use rusqlite::ffi::ErrorCode;
    err.chain().any(|cause| {
        cause.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
            matches!(
                e,
                rusqlite::Error::SqliteFailure(ffi, _)
                    if ffi.code == ErrorCode::DatabaseBusy
                        || ffi.code == ErrorCode::DatabaseLocked
            )
        })
    })
}

/// Exclusive workspace lock backed by `.edda/LOCK`.
/// Automatically released when dropped.
pub struct WorkspaceLock {
    _file: File,
}

impl WorkspaceLock {
    /// Try to acquire the workspace lock (non-blocking).
    /// Returns an error if already locked by another process.
    pub fn acquire(paths: &EddaPaths) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.lock_file)
            .map_err(|e| {
                anyhow::anyhow!("cannot open lock file {}: {}", paths.lock_file.display(), e)
            })?;

        file.try_lock_exclusive().map_err(|_| {
            anyhow::anyhow!(
                "workspace is locked by another process ({})",
                paths.lock_file.display()
            )
        })?;

        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_errors_are_transient_others_are_not() {
        let busy = anyhow::Error::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".into()),
        ));
        assert!(is_busy_error(&busy));
        let wrapped = anyhow::Error::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".into()),
        ))
        .context("verdict query failed");
        assert!(
            is_busy_error(&wrapped),
            "classification must walk the chain"
        );
        let corrupt = anyhow::anyhow!("file is not a database");
        assert!(!is_busy_error(&corrupt));
        let missing = anyhow::anyhow!("not an edda workspace");
        assert!(!is_busy_error(&missing));
    }

    #[test]
    fn acquire_and_drop() {
        let tmp = std::env::temp_dir().join(format!("edda_lock_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let lock = WorkspaceLock::acquire(&p).unwrap();
        // Second acquire should fail while first is held
        assert!(WorkspaceLock::acquire(&p).is_err());
        drop(lock);
        // After drop, should succeed again
        let _lock2 = WorkspaceLock::acquire(&p).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
