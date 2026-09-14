//! Process-scoped workspace roots for the control-effect test suite (GH-1235).
//!
//! The control-effect suite re-execs this same test binary as real OS
//! contender processes, and every process shares the platform temp directory.
//! Leaving `.edda/LOCK` identity to the platform's mkdtemp naming lets an
//! unrelated parallel test and a spawned contender resolve the same lock
//! file, which flaked `cargo test -p edda-ledger` on macOS CI. Embedding the
//! process id and a process-local ordinal makes the lock path distinct by
//! construction: no two processes can ever name the same fixture, however the
//! platform names temp directories.
//!
//! Compiled only for tests (`#[cfg(test)]` in `lib.rs`); the contender harness
//! still shares one workspace with its parent on purpose through
//! `EDDA_CONTROL_TEST_ROOT`.

use std::sync::atomic::{AtomicU64, Ordering};

/// A control-effect workspace root that is unique to this process.
pub(crate) fn process_scoped_workspace() -> tempfile::TempDir {
    static ORDINAL: AtomicU64 = AtomicU64::new(0);
    let ordinal = ORDINAL.fetch_add(1, Ordering::SeqCst);
    tempfile::Builder::new()
        .prefix(&format!("edda-ctl-{}-{ordinal}-", std::process::id()))
        .tempdir()
        .expect("control-effect workspace tempdir")
}
