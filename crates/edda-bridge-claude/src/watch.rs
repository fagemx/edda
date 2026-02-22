//! Public API for edda watch TUI — snapshot of peers, board state, and events.

use std::path::Path;

use crate::peers::{self, BoardState, PeerSummary};

/// A point-in-time snapshot of workspace state for the TUI.
pub struct WatchData {
    pub peers: Vec<PeerSummary>,
    pub board: BoardState,
    pub events: Vec<edda_core::types::Event>,
}

/// Collect a snapshot of peers, coordination board, and recent ledger events.
pub fn snapshot(
    project_id: &str,
    repo_root: &Path,
    event_limit: usize,
) -> anyhow::Result<WatchData> {
    let peers = peers::discover_all_sessions(project_id);
    let board = peers::compute_board_state(project_id);

    let events = match edda_ledger::Ledger::open_or_init(repo_root) {
        Ok(ledger) => ledger
            .iter_events()
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(event_limit)
            .collect(),
        Err(_) => Vec::new(),
    };

    Ok(WatchData {
        peers,
        board,
        events,
    })
}
