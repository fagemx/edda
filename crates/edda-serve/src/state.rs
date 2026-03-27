use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use edda_ledger::Ledger;

// ── Config ──

pub struct ServeConfig {
    pub bind: String,
    pub port: u16,
}

// ── App State ──

pub(crate) struct AppState {
    pub(crate) repo_root: PathBuf,
    pub(crate) chronicle: Option<ChronicleContext>,
    pub(crate) pending_pairings: Mutex<HashMap<String, PairingRequest>>,
}

pub(crate) struct PairingRequest {
    pub(crate) device_name: String,
    pub(crate) expires_at: std::time::Instant,
}

pub(crate) struct ChronicleContext {
    pub(crate) _store_root: PathBuf,
}

impl AppState {
    pub(crate) fn open_ledger(&self) -> anyhow::Result<Ledger> {
        Ledger::open(&self.repo_root)
    }
}
