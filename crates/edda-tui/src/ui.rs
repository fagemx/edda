use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Panel};

/// Render the full TUI frame.
pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // main area
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // peers
            Constraint::Percentage(40), // events
            Constraint::Percentage(30), // decisions
        ])
        .split(chunks[0]);

    render_peers(f, app, main_chunks[0]);
    render_events(f, app, main_chunks[1]);
    render_decisions(f, app, main_chunks[2]);
    render_status_bar(f, app, chunks[1]);
}

fn panel_style(app: &App, panel: Panel) -> Style {
    if app.active_panel == panel {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn render_peers(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = format!(" Peers ({}) ", app.peers.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Peers));

    let items: Vec<ListItem> = app
        .peers
        .iter()
        .enumerate()
        .skip(app.peer_scroll)
        .flat_map(|(i, peer)| {
            let status = if peer.age_secs < 120 { "+" } else { "-" };
            let label = if peer.label.is_empty() {
                "?"
            } else {
                &peer.label
            };
            let header = format!(" {status} {label}  ({:.8})", peer.session_id);
            let style = if app.active_panel == Panel::Peers && i == app.peer_scroll {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut lines = vec![ListItem::new(Line::from(Span::styled(header, style)))];
            if !peer.focus_files.is_empty() {
                let files: Vec<&str> = peer
                    .focus_files
                    .iter()
                    .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
                    .collect();
                let detail = format!("     {}", files.join(", "));
                lines.push(ListItem::new(Line::from(Span::styled(
                    detail,
                    Style::default().fg(Color::DarkGray),
                ))));
            }
            if !peer.task_subjects.is_empty() {
                let task = truncate_str(&peer.task_subjects[0], 30);
                let detail = format!("     >> {task}");
                lines.push(ListItem::new(Line::from(Span::styled(
                    detail,
                    Style::default().fg(Color::Yellow),
                ))));
            }
            lines
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_events(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = format!(" Events ({}) ", app.events.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Events));

    let max_preview = area.width.saturating_sub(24) as usize; // 24 = borders + time(5) + type(10) + spaces
    let items: Vec<ListItem> = app
        .events
        .iter()
        .skip(app.event_scroll)
        .map(|evt| {
            let ts = &evt.ts[11..19.min(evt.ts.len())]; // HH:MM:SS only
            let etype = &evt.event_type;
            let preview = event_preview(&evt.payload, etype);
            let preview = truncate_str(&preview, max_preview);
            let line = format!(" {ts}  {etype:<10} {preview}");
            ListItem::new(Line::from(line))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_decisions(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = format!(" Decisions ({}) ", app.board.bindings.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Decisions));

    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50), // bindings
            Constraint::Percentage(25), // claims
            Constraint::Percentage(25), // requests
        ])
        .split(block.inner(area));

    f.render_widget(block, area);

    // Bindings
    let binding_items: Vec<ListItem> = app
        .board
        .bindings
        .iter()
        .skip(app.decision_scroll)
        .map(|b| {
            let line = format!(" {} = {} ({})", b.key, b.value, b.by_label);
            ListItem::new(Line::from(line))
        })
        .collect();
    let binding_block = Block::default().title(" Bindings ").borders(Borders::TOP);
    let binding_list = List::new(binding_items).block(binding_block);
    f.render_widget(binding_list, inner_chunks[0]);

    // Claims
    let claim_items: Vec<ListItem> = app
        .board
        .claims
        .iter()
        .map(|c| {
            let paths = c.paths.join(", ");
            let line = format!(" {} [{}]", c.label, paths);
            ListItem::new(Line::from(line))
        })
        .collect();
    let claim_block = Block::default().title(" Claims ").borders(Borders::TOP);
    let claim_list = List::new(claim_items).block(claim_block);
    f.render_widget(claim_list, inner_chunks[1]);

    // Requests
    let request_items: Vec<ListItem> = app
        .board
        .requests
        .iter()
        .map(|r| {
            let line = format!(" {} -> {}: {}", r.from_label, r.to_label, r.message);
            ListItem::new(Line::from(line))
        })
        .collect();
    let request_block = Block::default().title(" Requests ").borders(Borders::TOP);
    let request_list = List::new(request_items).block(request_block);
    f.render_widget(request_list, inner_chunks[2]);
}

fn render_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let pause_indicator = if app.paused { " [PAUSED]" } else { "" };
    let panel_name = match app.active_panel {
        Panel::Peers => "Peers",
        Panel::Events => "Events",
        Panel::Decisions => "Decisions",
    };
    let (text, style) = if let Some(err) = &app.error {
        (
            format!(" ERROR: {err}"),
            Style::default().fg(Color::White).bg(Color::Red),
        )
    } else {
        (
            format!(
                " edda watch | {panel_name}{pause_indicator} | Tab:switch  j/k:scroll  Space:pause  q:quit"
            ),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        )
    };
    let bar = Paragraph::new(Line::from(Span::styled(text, style)));
    f.render_widget(bar, area);
}

/// Extract a human-readable preview from an event payload.
fn event_preview(payload: &serde_json::Value, event_type: &str) -> String {
    match event_type {
        "note" => payload["text"].as_str().unwrap_or("").to_string(),
        "decision" => {
            let key = payload["key"].as_str().unwrap_or("?");
            let val = payload["value"].as_str().unwrap_or("?");
            format!("{key} = {val}")
        }
        "commit" => payload["title"].as_str().unwrap_or("").to_string(),
        "merge" => {
            let src = payload["src"].as_str().unwrap_or("?");
            let dst = payload["dst"].as_str().unwrap_or("?");
            format!("{src} -> {dst}")
        }
        _ => {
            // Fallback: compact JSON
            serde_json::to_string(payload).unwrap_or_default()
        }
    }
}

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
