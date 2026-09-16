use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use edda_ledger::node::NodeConfig;
use edda_ledger::Ledger;

// ── Config ──

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub bind: String,
    pub port: u16,
    /// Node transport configuration. `None` (the default) keeps every existing
    /// route and test exactly as it was: `edda serve` alone does not expose the
    /// node endpoints.
    pub node: Option<NodeConfig>,
    /// The shared bearer token this listener accepts. `None` means no token is
    /// configured here, and then a request that also carries no token is a 401 —
    /// never an open door.
    pub node_token: Option<String>,
    /// Test-only: allow a non-tailnet bind and print a warning. Never used for
    /// the real two-machine proof.
    pub insecure_bind: bool,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 7433,
            node: None,
            node_token: None,
            insecure_bind: false,
        }
    }
}

// ── App State ──

pub(crate) struct AppState {
    pub(crate) repo_root: PathBuf,
    pub(crate) chronicle: Option<ChronicleContext>,
    pub(crate) pending_pairings: Mutex<HashMap<String, PairingRequest>>,
    /// Node config when `edda node start` wired the node routes.
    pub(crate) node: Option<NodeConfig>,
    /// Shared bearer token for the node routes. Never logged or rendered.
    pub(crate) node_token: Option<String>,
}

pub(crate) struct PairingRequest {
    pub(crate) device_name: String,
    pub(crate) expires_at: std::time::Instant,
}

pub(crate) struct ChronicleContext {
    pub(crate) _store_root: PathBuf,
}

impl AppState {
    pub(crate) fn open_ledger(&self) -> Result<Ledger, crate::error::AppError> {
        Ledger::open(&self.repo_root).map_err(crate::error::classify_open_error)
    }
}
