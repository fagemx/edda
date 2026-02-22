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

    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let all_events = ledger.iter_events()?;
    let events: Vec<_> = all_events.into_iter().rev().take(event_limit).collect();

    Ok(WatchData {
        peers,
        board,
        events,
    })
}
