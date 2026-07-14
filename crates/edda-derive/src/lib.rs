mod context;
mod evidence;
mod snapshot;
mod types;
mod writers;

pub use context::render_context;
pub use evidence::{build_auto_evidence, last_commit_contribution, AutoEvidenceResult};
pub use types::*;
pub use writers::{rebuild_all, rebuild_branch};

#[cfg(test)]
pub(crate) mod test_support {
    use edda_core::types::Event;
    use edda_ledger::{EddaPaths, Ledger};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TestLedger(Ledger);

    impl std::ops::Deref for TestLedger {
        type Target = Ledger;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl TestLedger {
        pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
            let mut chained = event.clone();
            chained.parent_hash = self.0.last_event_hash()?;
            edda_core::event::finalize_event(&mut chained)?;
            self.0.append_event(&chained)
        }
    }

    pub fn setup_workspace() -> (std::path::PathBuf, TestLedger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_derive_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = TestLedger(Ledger::open(&tmp).unwrap());
        (tmp, ledger)
    }
}
