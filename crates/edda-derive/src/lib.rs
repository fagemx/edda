mod types;
mod snapshot;
mod writers;
mod context;
mod evidence;

pub use types::*;
pub use writers::{rebuild_branch, rebuild_all};
pub use context::render_context;
pub use evidence::{build_auto_evidence, last_commit_contribution, AutoEvidenceResult};

#[cfg(test)]
pub(crate) mod test_support {
    use edda_ledger::{EddaPaths, Ledger};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    pub fn setup_workspace() -> (std::path::PathBuf, Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "edda_derive_test_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }
}
