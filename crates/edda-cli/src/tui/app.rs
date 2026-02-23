use std::collections::HashSet;
use std::path::PathBuf;

use edda_bridge_claude::peers::{BoardState, PeerSummary};
use edda_bridge_claude::watch;

/// Domains considered user-facing (shown expanded by default).
const USER_FACING_DOMAINS: &[&str] = &[
    "api", "auth", "ci", "db", "install", "readme", "release", "storage", "testing",
];

/// Check if a domain prefix is user-facing (shown expanded by default).
pub fn is_user_facing_domain(domain: &str) -> bool {
    USER_FACING_DOMAINS.contains(&domain)
}

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
    pub error: Option<String>,

    // Scroll positions (per panel)
    pub peer_scroll: usize,
    pub event_scroll: usize,
    pub decision_scroll: usize,

    // Filters
    pub show_cmd_events: bool,
    pub show_stale_peers: bool,
    pub expanded_domains: HashSet<String>,
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
            board: BoardState::default(),
            events: Vec::new(),
            error: None,
            peer_scroll: 0,
            event_scroll: 0,
            decision_scroll: 0,
            show_cmd_events: false,
            show_stale_peers: false,
            expanded_domains: HashSet::new(),
        }
    }

    /// Return only events that pass the current filter.
    pub fn visible_events(&self) -> Vec<&edda_core::types::Event> {
        self.events
            .iter()
            .filter(|e| {
                if !self.show_cmd_events && e.event_type == "cmd" {
                    return false;
                }
                true
            })
            .collect()
    }

    /// Return only peers that are active or have a label (hiding ghost sessions).
    pub fn active_peers(&self) -> Vec<&PeerSummary> {
        self.peers
            .iter()
            .filter(|p| {
                if self.show_stale_peers {
                    return true;
                }
                // Hide stale peers with no label
                !(p.label.is_empty() && p.age_secs > 120)
            })
            .collect()
    }

    /// Refresh data from disk (unless paused).
    /// Errors are stored in `self.error` instead of propagating.
    pub fn refresh_data(&mut self) {
        if self.paused {
            return;
        }
        match watch::snapshot(&self.project_id, &self.repo_root, 200) {
            Ok(data) => {
                self.peers = data.peers;
                self.board = data.board;
                self.events = data.events;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    /// Handle a key press.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.active_panel = self.active_panel.next(),
            KeyCode::BackTab => self.active_panel = self.active_panel.prev(),
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char('c') => self.show_cmd_events = !self.show_cmd_events,
            KeyCode::Char('p') => self.show_stale_peers = !self.show_stale_peers,
            KeyCode::Char('j') | KeyCode::Down => self.scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_up(),
            KeyCode::Enter => self.toggle_domain_expand(),
            _ => {}
        }
    }

    fn toggle_domain_expand(&mut self) {
        if self.active_panel != Panel::Decisions {
            return;
        }
        // Find which domain is at the current scroll position
        let groups = crate::tui::ui::group_bindings(&self.board.bindings);
        let mut row = 0;
        for (domain, bindings) in &groups {
            if row == self.decision_scroll {
                let domain = (*domain).to_string();
                if self.expanded_domains.contains(&domain) {
                    self.expanded_domains.remove(&domain);
                } else {
                    self.expanded_domains.insert(domain);
                }
                return;
            }
            row += 1; // domain header
            let is_internal = !is_user_facing_domain(domain);
            let expanded = self.expanded_domains.contains(*domain);
            if !is_internal || expanded {
                row += bindings.len();
            }
        }
    }

    /// Count total visible rows in the grouped decisions view.
    fn decisions_row_count(&self) -> usize {
        let groups = crate::tui::ui::group_bindings(&self.board.bindings);
        let mut rows = 0;
        for (domain, bindings) in &groups {
            rows += 1; // domain header
            let is_internal = !is_user_facing_domain(domain);
            let expanded = self.expanded_domains.contains(*domain);
            if !is_internal || expanded {
                rows += bindings.len();
            }
        }
        rows
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
            Panel::Peers => (self.peer_scroll, self.active_peers().len()),
            Panel::Events => (self.event_scroll, self.visible_events().len()),
            Panel::Decisions => (self.decision_scroll, self.decisions_row_count()),
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

    fn make_event(event_type: &str) -> edda_core::types::Event {
        edda_core::types::Event {
            event_id: "evt_test".into(),
            ts: "2026-02-23T05:00:00Z".into(),
            event_type: event_type.into(),
            branch: "main".into(),
            parent_hash: None,
            hash: "abc".into(),
            payload: serde_json::json!({}),
            refs: Default::default(),
            schema_version: 1,
            digests: vec![],
            event_family: None,
            event_level: None,
        }
    }

    fn make_peer(label: &str, age_secs: u64) -> PeerSummary {
        PeerSummary {
            session_id: "test123".into(),
            label: label.into(),
            age_secs,
            focus_files: vec![],
            task_subjects: vec![],
            files_modified_count: 0,
            recent_commits: vec![],
            claimed_paths: vec![],
            branch: None,
        }
    }

    #[test]
    fn new_app_has_empty_data() {
        let app = App::new("test-project".into(), PathBuf::from("/tmp"));
        assert!(app.peers.is_empty());
        assert!(app.board.claims.is_empty());
        assert!(app.events.is_empty());
        assert!(app.error.is_none());
        assert!(!app.should_quit);
        assert!(!app.paused);
        assert_eq!(app.active_panel, Panel::Peers);
        assert!(!app.show_cmd_events);
        assert!(!app.show_stale_peers);
    }

    #[test]
    fn visible_events_hides_cmd_by_default() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        app.events = vec![
            make_event("note"),
            make_event("cmd"),
            make_event("commit"),
            make_event("cmd"),
        ];
        let visible = app.visible_events();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].event_type, "note");
        assert_eq!(visible[1].event_type, "commit");
    }

    #[test]
    fn visible_events_shows_cmd_when_toggled() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        app.events = vec![make_event("note"), make_event("cmd")];
        app.show_cmd_events = true;
        assert_eq!(app.visible_events().len(), 2);
    }

    #[test]
    fn active_peers_hides_stale_unlabeled() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        app.peers = vec![
            make_peer("worker-1", 30),   // active, labeled → show
            make_peer("", 200),          // stale, no label → hide
            make_peer("worker-2", 200),  // stale, labeled → show
            make_peer("", 50),           // active, no label → show
        ];
        let active = app.active_peers();
        assert_eq!(active.len(), 3);
    }

    #[test]
    fn key_c_toggles_cmd_filter() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        assert!(!app.show_cmd_events);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::empty(),
        );
        app.handle_key(key);
        assert!(app.show_cmd_events);
        app.handle_key(key);
        assert!(!app.show_cmd_events);
    }

    #[test]
    fn key_p_toggles_peer_filter() {
        let mut app = App::new("test".into(), PathBuf::from("/tmp"));
        assert!(!app.show_stale_peers);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('p'),
            crossterm::event::KeyModifiers::empty(),
        );
        app.handle_key(key);
        assert!(app.show_stale_peers);
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
