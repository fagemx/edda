use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::app::{App, Panel};

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

    let max_preview = area.width.saturating_sub(24) as usize; // 24 = borders + time(8) + type(10) + spaces
    let items: Vec<ListItem> = app
        .events
        .iter()
        .skip(app.event_scroll)
        .map(|evt| {
            let ts = &evt.ts[11..19.min(evt.ts.len())]; // HH:MM:SS only
            let (dtype, preview, style) = event_display(&evt.payload, &evt.event_type);
            let preview = truncate_str(&preview, max_preview);
            let line = format!(" {ts}  {dtype:<10} {preview}");
            ListItem::new(Line::from(Span::styled(line, style)))
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

/// Extract display type, preview text, and style from an event.
fn event_display(payload: &serde_json::Value, event_type: &str) -> (String, String, Style) {
    let default = Style::default();
    match event_type {
        "note" => {
            let tags = payload["tags"].as_array();
            let has_tag = |t: &str| tags.is_some_and(|a| a.iter().any(|v| v.as_str() == Some(t)));

            if has_tag("session_digest") {
                let stats = &payload["session_stats"];
                let sid = payload["session_id"].as_str().unwrap_or("?");
                let tools = stats["tool_calls"].as_u64().unwrap_or(0);
                let dur = stats["duration_minutes"].as_u64().unwrap_or(0);
                let outcome = stats["outcome"].as_str().unwrap_or("?");
                (
                    "digest".into(),
                    format!("{:.8} {outcome} ({tools} tools, {dur}m)", sid),
                    Style::default().fg(Color::DarkGray),
                )
            } else if has_tag("decision") {
                let d = &payload["decision"];
                let key = d["key"].as_str().unwrap_or("?");
                let val = d["value"].as_str().unwrap_or("?");
                (
                    "decide".into(),
                    format!("{key} = {val}"),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                let text = first_line(payload["text"].as_str().unwrap_or(""));
                ("note".into(), text.to_string(), default)
            }
        }
        "decision" => {
            let key = payload["key"].as_str().unwrap_or("?");
            let val = payload["value"].as_str().unwrap_or("?");
            (
                "decision".into(),
                format!("{key} = {val}"),
                Style::default().fg(Color::Yellow),
            )
        }
        "cmd" => {
            let argv = &payload["argv"];
            let cmd_str = argv
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let short = shorten_cmd(cmd_str);
            let exit = payload["exit_code"].as_i64().unwrap_or(-1);
            if exit == 0 {
                ("cmd".into(), format!("$ {short}"), default)
            } else {
                (
                    "cmd:fail".into(),
                    format!("$ {short} [exit:{exit}]"),
                    Style::default().fg(Color::Red),
                )
            }
        }
        "commit" => {
            let title = first_line(payload["title"].as_str().unwrap_or(""));
            ("commit".into(), title.to_string(), default)
        }
        "merge" => {
            let src = payload["src"].as_str().unwrap_or("?");
            let dst = payload["dst"].as_str().unwrap_or("?");
            ("merge".into(), format!("{src} -> {dst}"), default)
        }
        other => {
            // Fallback: try text or message fields before raw JSON
            let preview = if let Some(text) = payload["text"].as_str() {
                first_line(text).to_string()
            } else if let Some(msg) = payload["message"].as_str() {
                first_line(msg).to_string()
            } else {
                serde_json::to_string(payload).unwrap_or_default()
            };
            (other.to_string(), preview, default)
        }
    }
}

/// Extract the meaningful command from a shell invocation.
fn shorten_cmd(cmd: &str) -> String {
    let cmd = cmd.trim();
    let cmd = cmd.lines().next().unwrap_or(cmd);
    let cmd = if let Some(pos) = cmd.rfind("&&") {
        cmd[pos + 2..].trim()
    } else {
        cmd
    };
    let cmd = cmd.trim_end_matches("2>&1").trim();
    cmd.to_string()
}

/// Return only the first line of a string.
fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cmd_success_shows_short_command() {
        let payload = json!({
            "argv": ["cd /c/project && cargo test --workspace 2>&1"],
            "exit_code": 0,
            "duration_ms": 1200
        });
        let (dtype, preview, _) = event_display(&payload, "cmd");
        assert_eq!(dtype, "cmd");
        assert_eq!(preview, "$ cargo test --workspace");
    }

    #[test]
    fn cmd_failure_shows_exit_code() {
        let payload = json!({
            "argv": ["cd /tmp && python3 script.py 2>&1"],
            "exit_code": 1
        });
        let (dtype, preview, style) = event_display(&payload, "cmd");
        assert_eq!(dtype, "cmd:fail");
        assert!(preview.contains("[exit:1]"));
        assert_eq!(style.fg, Some(Color::Red));
    }

    #[test]
    fn note_session_digest_shows_compact() {
        let payload = json!({
            "text": "Session abc: 15 tools...\nFailed: ...",
            "tags": ["session_digest"],
            "session_id": "abc12345-long-id",
            "session_stats": {
                "tool_calls": 15,
                "duration_minutes": 42,
                "outcome": "completed"
            }
        });
        let (dtype, preview, _) = event_display(&payload, "note");
        assert_eq!(dtype, "digest");
        assert!(preview.contains("abc12345"));
        assert!(preview.contains("completed"));
        assert!(preview.contains("15 tools"));
        assert!(preview.contains("42m"));
    }

    #[test]
    fn note_decision_shows_key_value() {
        let payload = json!({
            "text": "db.engine: sqlite — reason",
            "tags": ["decision"],
            "decision": { "key": "db.engine", "value": "sqlite" }
        });
        let (dtype, preview, style) = event_display(&payload, "note");
        assert_eq!(dtype, "decide");
        assert_eq!(preview, "db.engine = sqlite");
        assert_eq!(style.fg, Some(Color::Yellow));
    }

    #[test]
    fn note_plain_strips_newlines() {
        let payload = json!({
            "text": "first line\nsecond line\nthird",
            "tags": []
        });
        let (dtype, preview, _) = event_display(&payload, "note");
        assert_eq!(dtype, "note");
        assert_eq!(preview, "first line");
    }

    #[test]
    fn shorten_cmd_extracts_after_cd() {
        assert_eq!(
            shorten_cmd("cd /c/project && cargo build 2>&1"),
            "cargo build"
        );
    }

    #[test]
    fn shorten_cmd_no_cd_prefix() {
        assert_eq!(shorten_cmd("cargo test"), "cargo test");
    }

    #[test]
    fn shorten_cmd_multiline_takes_first() {
        assert_eq!(shorten_cmd("echo hello\necho world"), "echo hello");
    }

    #[test]
    fn first_line_basic() {
        assert_eq!(first_line("a\nb\nc"), "a");
        assert_eq!(first_line("single"), "single");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn unknown_type_tries_text_field() {
        let payload = json!({ "text": "some info\nmore" });
        let (dtype, preview, _) = event_display(&payload, "custom_evt");
        assert_eq!(dtype, "custom_evt");
        assert_eq!(preview, "some info");
    }
}
