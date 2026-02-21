use crate::paths::EddaPaths;
use fs2::FileExt;
use std::fs::{File, OpenOptions};

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
