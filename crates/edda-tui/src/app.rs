use std::path::PathBuf;

use edda_bridge_claude::peers::{BoardState, PeerSummary};
use edda_bridge_claude::watch;

/// Which panel is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Peers,
    Events,
    Decisions,
}

impl Panel {
    pub fn next(self) -> Self {
        match self {
            Panel::Peers => Panel::Events,
            Panel::Events => Panel::Decisions,
            Panel::Decisions => Panel::Peers,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Panel::Peers => Panel::Decisions,
            Panel::Events => Panel::Peers,
            Panel::Decisions => Panel::Events,
        }
    }
}

/// Application state for the TUI.
pub struct App {
    pub project_id: String,
    pub repo_root: PathBuf,
    pub should_quit: bool,
    pub active_panel: Panel,
    pub paused: bool,

    // Data
    pub peers: Vec<PeerSummary>,
    pub board: BoardState,
    pub events: Vec<edda_core::types::Event>,

    // Scroll positions (per panel)
    pub peer_scroll: usize,
    pub event_scroll: usize,
    pub decision_scroll: usize,
}

impl App {
    pub fn new(project_id: String, repo_root: PathBuf) -> Self {
        Self {
            project_id,
            repo_root,
            should_quit: false,
            active_panel: Panel::Peers,
            paused: false,
            peers: Vec::new(),
            board: BoardState {
                claims: Vec::new(),
                bindings: Vec::new(),
                requests: Vec::new(),
            },
            events: Vec::new(),
            peer_scroll: 0,
            event_scroll: 0,
            decision_scroll: 0,
        }
    }

    /// Refresh data from disk (unless paused).
    pub fn refresh_data(&mut self) -> anyhow::Result<()> {
        if self.paused {
            return Ok(());
        }
        let data = watch::snapshot(&self.project_id, &self.repo_root, 200)?;
        self.peers = data.peers;
        self.board = data.board;
        self.events = data.events;
        Ok(())
    }

    /// Handle a key press.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.active_panel = self.active_panel.next(),
            KeyCode::BackTab => self.active_panel = self.active_panel.prev(),
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char('j') | KeyCode::Down => self.scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_up(),
            _ => {}
        }
    }

    fn scroll_down(&mut self) {
        let (scroll, max) = self.active_scroll_and_max();
        if scroll < max.saturating_sub(1) {
            *self.active_scroll_mut() += 1;
        }
    }

    fn scroll_up(&mut self) {
        let scroll = self.active_scroll_mut();
        *scroll = scroll.saturating_sub(1);
    }

    fn active_scroll_and_max(&self) -> (usize, usize) {
        match self.active_panel {
            Panel::Peers => (self.peer_scroll, self.peers.len()),
            Panel::Events => (self.event_scroll, self.events.len()),
            Panel::Decisions => (self.decision_scroll, self.board.bindings.len()),
        }
    }

    fn active_scroll_mut(&mut self) -> &mut usize {
        match self.active_panel {
            Panel::Peers => &mut self.peer_scroll,
            Panel::Events => &mut self.event_scroll,
            Panel::Decisions => &mut self.decision_scroll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_has_empty_data() {
        let app = App::new("test-project".into(), PathBuf::from("/tmp"));
        assert!(app.peers.is_empty());
        assert!(app.board.claims.is_empty());
        assert!(app.events.is_empty());
        assert!(!app.should_quit);
        assert!(!app.paused);
        assert_eq!(app.active_panel, Panel::Peers);
    }

    #[test]
    fn panel_cycling() {
        assert_eq!(Panel::Peers.next(), Panel::Events);
        assert_eq!(Panel::Events.next(), Panel::Decisions);
        assert_eq!(Panel::Decisions.next(), Panel::Peers);
        assert_eq!(Panel::Peers.prev(), Panel::Decisions);
    }

    #[test]
    fn quit_on_q() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::empty(),
        );
        app.handle_key(key);
        assert!(app.should_quit);
    }

    #[test]
    fn space_toggles_pause() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        assert!(!app.paused);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::empty(),
        );
        app.handle_key(key);
        assert!(app.paused);
        app.handle_key(key);
        assert!(!app.paused);
    }

    #[test]
    fn tab_switches_panel() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        let tab = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::empty(),
        );
        app.handle_key(tab);
        assert_eq!(app.active_panel, Panel::Events);
        app.handle_key(tab);
        assert_eq!(app.active_panel, Panel::Decisions);
    }
}
