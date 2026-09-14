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
    #[cfg(test)]
    _fork_gate: std::sync::RwLockReadGuard<'static, ()>,
}

/// Test-only admission gate for forked contender processes (GH-1235).
///
/// A process that holds a workspace lock must not `fork()`: the child inherits
/// the open `.edda/LOCK` descriptor and keeps the `flock` alive even after the
/// parent closes it, so the very next acquisition in the parent fails with
/// "workspace is locked by another process". The control-effect contender
/// harness takes this gate exclusively around `Command::spawn`, so no workspace
/// lock is open at fork time. The child then `exec`s (which resets the gate)
/// and acquires its own workspace lock normally.
#[cfg(test)]
pub(crate) mod fork_gate {
    use std::sync::RwLock;

    pub(crate) static WORKSPACE_LOCK_FORK_GATE: RwLock<()> = RwLock::new(());
}

impl WorkspaceLock {
    /// Try to acquire the workspace lock (non-blocking).
    /// Returns an error if already locked by another process.
    pub fn acquire(paths: &EddaPaths) -> anyhow::Result<Self> {
        // Take the shared gate before the file lock: a fork between the two
        // would otherwise inherit the lock descriptor (GH-1235).
        #[cfg(test)]
        let fork_gate = fork_gate::WORKSPACE_LOCK_FORK_GATE
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

        Ok(Self {
            _file: file,
            #[cfg(test)]
            _fork_gate: fork_gate,
        })
    }
}

/// Per-task generation lock held across an ACP turn.
///
/// `task.started` and `task.failed` appends take the same lock, so a running
/// prompt cannot become stale through a fail/restart transition while it can
/// still receive action authority. This lock is deliberately separate from
/// [`WorkspaceLock`]: ACP permission audits must remain able to append while a
/// turn is in progress.
pub struct TaskDispatchLock {
    _file: File,
    task_id: u64,
}

impl TaskDispatchLock {
    pub fn acquire(paths: &EddaPaths, task_id: u64) -> anyhow::Result<Self> {
        let file = open_task_lock(paths, task_id)?;
        file.lock_exclusive().map_err(|error| {
            anyhow::anyhow!("cannot lock ACP task #{task_id} dispatch generation: {error}")
        })?;
        Ok(Self {
            _file: file,
            task_id,
        })
    }

    pub fn try_acquire(paths: &EddaPaths, task_id: u64) -> anyhow::Result<Self> {
        let file = open_task_lock(paths, task_id)?;
        file.try_lock_exclusive().map_err(|_| {
            anyhow::anyhow!(
                "task #{task_id} has an active ACP turn; start/fail generation change refused"
            )
        })?;
        Ok(Self {
            _file: file,
            task_id,
        })
    }

    pub fn task_id(&self) -> u64 {
        self.task_id
    }
}

pub(crate) fn task_generation_guard(
    paths: &EddaPaths,
    event: &edda_core::Event,
) -> anyhow::Result<Option<TaskDispatchLock>> {
    if !matches!(event.event_type.as_str(), "task.started" | "task.failed") {
        return Ok(None);
    }
    let task_id = event
        .payload
        .get("task_id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("{} omits task_id", event.event_type))?;
    TaskDispatchLock::try_acquire(paths, task_id).map(Some)
}

fn open_task_lock(paths: &EddaPaths, task_id: u64) -> anyhow::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.edda_dir.join(format!("acp-task-{task_id}.lock")))
        .map_err(|error| anyhow::anyhow!("cannot open ACP task dispatch lock: {error}"))
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
