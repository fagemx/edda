use std::collections::BTreeMap;

use edda_bridge_claude::peers::BindingEntry;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::app::{is_internal_domain, App, Panel};

/// Render the full TUI frame.
pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // main area
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    let active_peers = app.active_peers();
    let has_peers = !active_peers.is_empty();
    let has_claims_or_requests = !app.board.claims.is_empty() || !app.board.requests.is_empty();

    if has_peers || has_claims_or_requests {
        // 3-column layout
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25), // peers
                Constraint::Percentage(50), // events
                Constraint::Percentage(25), // decisions
            ])
            .split(chunks[0]);

        render_peers(f, app, main_chunks[0]);
        render_events(f, app, main_chunks[1]);
        render_decisions(f, app, main_chunks[2]);
    } else {
        // 2-column layout (no active peers)
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60), // events
                Constraint::Percentage(40), // decisions
            ])
            .split(chunks[0]);

        render_events(f, app, main_chunks[0]);
        render_decisions(f, app, main_chunks[1]);
    }

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
    let active = app.active_peers();
    let total = app.peers.len();
    let title = if active.len() == total {
        format!(" Peers ({}) ", active.len())
    } else {
        format!(" Peers ({} active) ", active.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Peers));

    if active.is_empty() {
        let msg = Paragraph::new("No active peers")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = active
        .iter()
        .enumerate()
        .skip(app.peer_scroll)
        .flat_map(|(i, peer)| {
            let indicator = if peer.age_secs < 120 { "●" } else { "○" };
            let label = if peer.label.is_empty() {
                "unknown"
            } else {
                &peer.label
            };
            let header = format!(" {indicator} {label}");
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
                    .take(5)
                    .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
                    .collect();
                let branch_str = peer
                    .branch
                    .as_deref()
                    .map(|b| format!("  ({b})"))
                    .unwrap_or_default();
                let detail = format!("   {}{branch_str}", files.join(", "));
                lines.push(ListItem::new(Line::from(Span::styled(
                    detail,
                    Style::default().fg(Color::DarkGray),
                ))));
            }
            if !peer.task_subjects.is_empty() {
                let task = truncate_str(&peer.task_subjects[0], 30);
                let detail = format!("   >> {task}");
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
    let visible = app.visible_events();
    let total = app.events.len();
    let title = if visible.len() == total {
        format!(" Events ({}) ", visible.len())
    } else {
        format!(" Events ({}/{}) ", visible.len(), total)
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Events));

    let max_preview = area.width.saturating_sub(22) as usize; // borders + time(5) + type(10) + spaces
    let items: Vec<ListItem> = visible
        .iter()
        .skip(app.event_scroll)
        .map(|evt| {
            let ts = if evt.ts.len() >= 16 {
                &evt.ts[11..16] // HH:MM only
            } else {
                &evt.ts
            };
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
    let has_claims_or_requests = !app.board.claims.is_empty() || !app.board.requests.is_empty();

    let title = format!(" Decisions ({}) ", app.board.bindings.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(panel_style(app, Panel::Decisions));

    if has_claims_or_requests {
        // Split: bindings top, claims+requests bottom
        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(60), // bindings
                Constraint::Percentage(20), // claims
                Constraint::Percentage(20), // requests
            ])
            .split(block.inner(area));

        f.render_widget(block, area);
        render_bindings_grouped(f, app, inner_chunks[0]);
        render_claims(f, app, inner_chunks[1]);
        render_requests(f, app, inner_chunks[2]);
    } else {
        // Full space for bindings
        let inner = block.inner(area);
        f.render_widget(block, area);
        render_bindings_grouped(f, app, inner);
    }
}

fn render_bindings_grouped(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let groups = group_bindings(&app.board.bindings);

    let mut items: Vec<ListItem> = Vec::new();

    for (domain, bindings) in &groups {
        let is_internal = is_internal_domain(domain);
        let expanded = app.expanded_domains.contains(*domain) || !is_internal;

        // Domain header
        let arrow = if expanded { "▾" } else { "▸" };
        let header = format!(" {arrow} {domain} ({})", bindings.len());
        let header_style = if is_internal {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        items.push(ListItem::new(Line::from(Span::styled(
            header,
            header_style,
        ))));

        if expanded {
            for b in bindings {
                let short_key = b.key.strip_prefix(&format!("{domain}.")).unwrap_or(&b.key);
                let line = format!("   {short_key} = {}", b.value);
                items.push(ListItem::new(Line::from(Span::styled(
                    line,
                    Style::default(),
                ))));
            }
        }
    }

    // Apply scroll offset
    let items: Vec<ListItem> = items.into_iter().skip(app.decision_scroll).collect();

    let binding_block = Block::default().title(" Bindings ").borders(Borders::TOP);
    let list = List::new(items).block(binding_block);
    f.render_widget(list, area);
}

fn render_claims(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .board
        .claims
        .iter()
        .map(|c| {
            let paths: Vec<&str> = c
                .paths
                .iter()
                .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
                .collect();
            let line = format!(" {} [{}]", c.label, paths.join(", "));
            ListItem::new(Line::from(line))
        })
        .collect();
    let block = Block::default().title(" Claims ").borders(Borders::TOP);
    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_requests(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .board
        .requests
        .iter()
        .map(|r| {
            let msg = truncate_str(&r.message, 40);
            let line = format!(" {} → {}: {msg}", r.from_label, r.to_label);
            ListItem::new(Line::from(line))
        })
        .collect();
    let block = Block::default().title(" Requests ").borders(Borders::TOP);
    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let pause_indicator = if app.paused { " [PAUSED]" } else { "" };
    let cmd_indicator = if app.show_cmd_events {
        ""
    } else {
        " [cmd:hidden]"
    };
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
                " edda watch | {panel_name}{pause_indicator}{cmd_indicator} | Tab:switch  c:cmd  j/k:scroll  Space:pause  q:quit"
            ),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        )
    };
    let bar = Paragraph::new(Line::from(Span::styled(text, style)));
    f.render_widget(bar, area);
}

// ── Public helpers ──

/// Group bindings by domain prefix (part before first `.`).
/// Returns sorted groups: user-facing domains first, then internal.
pub fn group_bindings(bindings: &[BindingEntry]) -> Vec<(&str, Vec<&BindingEntry>)> {
    let mut map: BTreeMap<&str, Vec<&BindingEntry>> = BTreeMap::new();
    for b in bindings {
        let domain = b.key.split('.').next().unwrap_or(&b.key);
        map.entry(domain).or_default().push(b);
    }

    let mut user_facing: Vec<(&str, Vec<&BindingEntry>)> = Vec::new();
    let mut internal: Vec<(&str, Vec<&BindingEntry>)> = Vec::new();

    for (domain, entries) in map {
        if !is_internal_domain(domain) {
            user_facing.push((domain, entries));
        } else {
            internal.push((domain, entries));
        }
    }

    // User-facing first, then internal
    user_facing.extend(internal);
    user_facing
}

// ── Event formatting ──

/// Extract display type, preview text, and style from an event.
fn event_display(payload: &serde_json::Value, event_type: &str) -> (String, String, Style) {
    let default = Style::default();
    match event_type {
        "note" => {
            let tags = payload["tags"].as_array();
            let has_tag = |t: &str| tags.is_some_and(|a| a.iter().any(|v| v.as_str() == Some(t)));

            if has_tag("session_digest") {
                let stats = &payload["session_stats"];
                let dur = stats["duration_minutes"].as_u64().unwrap_or(0);
                let outcome = stats["outcome"].as_str().unwrap_or("?");
                let icon = if outcome == "completed" { "✓" } else { "✗" };
                let dur_str = format_duration(dur);
                let decides = stats["decide_count"].as_u64().unwrap_or(0);
                let files_mod = stats["files_modified"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                let commits = stats["commits_made"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);

                // Build narrative: what happened in this session
                let mut parts: Vec<String> = Vec::new();
                if commits > 0 {
                    parts.push(format!(
                        "{commits} commit{}",
                        if commits > 1 { "s" } else { "" }
                    ));
                }
                if decides > 0 {
                    parts.push(format!(
                        "{decides} decision{}",
                        if decides > 1 { "s" } else { "" }
                    ));
                }
                if files_mod > 0 {
                    parts.push(format!(
                        "{files_mod} file{}",
                        if files_mod > 1 { "s" } else { "" }
                    ));
                }
                let summary = if parts.is_empty() {
                    dur_str.clone()
                } else {
                    format!("{}, {dur_str}", parts.join(", "))
                };

                // Show first task subject or first commit as headline
                let headline = stats["tasks_snapshot"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|t| t["subject"].as_str())
                    .or_else(|| {
                        stats["commits_made"]
                            .as_array()
                            .and_then(|a| a.last())
                            .and_then(|c| c.as_str())
                    });

                let preview = if let Some(h) = headline {
                    let h = first_line(h);
                    let h = truncate_str(h, 40);
                    format!("{icon} {h} ({summary})")
                } else {
                    format!("{icon} session ({summary})")
                };

                let style = if outcome == "completed" {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Red)
                };
                ("digest".into(), preview, style)
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
            (
                "commit".into(),
                format!("● {title}"),
                Style::default().fg(Color::Green),
            )
        }
        "merge" => {
            let src = payload["src"].as_str().unwrap_or("?");
            let dst = payload["dst"].as_str().unwrap_or("?");
            (
                "merge".into(),
                format!("◆ {src} → {dst}"),
                Style::default().fg(Color::Cyan),
            )
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

// ── Utility functions ──

/// Format duration in minutes to a human-readable string.
fn format_duration(minutes: u64) -> String {
    if minutes >= 60 {
        let h = minutes / 60;
        let m = minutes % 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{m}m")
        }
    } else {
        format!("{minutes}m")
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
    fn digest_with_task_shows_narrative() {
        let payload = json!({
            "text": "Session abc: 15 tools...",
            "tags": ["session_digest"],
            "session_id": "abc12345-long-id",
            "session_stats": {
                "tool_calls": 15,
                "duration_minutes": 42,
                "outcome": "completed",
                "commits_made": ["feat: add auth"],
                "decide_count": 2,
                "files_modified": ["src/auth.rs", "src/main.rs"],
                "tasks_snapshot": [{"subject": "Add user authentication", "status": "completed"}]
            }
        });
        let (dtype, preview, _) = event_display(&payload, "note");
        assert_eq!(dtype, "digest");
        assert!(preview.starts_with("✓"), "got: {preview}");
        assert!(
            preview.contains("Add user authentication"),
            "got: {preview}"
        );
        assert!(preview.contains("1 commit"), "got: {preview}");
        assert!(preview.contains("2 decisions"), "got: {preview}");
        assert!(preview.contains("42m"), "got: {preview}");
    }

    #[test]
    fn digest_no_task_shows_commit() {
        let payload = json!({
            "text": "Session abc: ...",
            "tags": ["session_digest"],
            "session_id": "abc12345-long-id",
            "session_stats": {
                "tool_calls": 10,
                "duration_minutes": 5,
                "outcome": "completed",
                "commits_made": ["fix: resolve login bug"],
                "files_modified": ["src/login.rs"]
            }
        });
        let (_, preview, _) = event_display(&payload, "note");
        assert!(preview.starts_with("✓"), "got: {preview}");
        assert!(preview.contains("fix: resolve login bug"), "got: {preview}");
    }

    #[test]
    fn digest_empty_session_shows_fallback() {
        let payload = json!({
            "text": "Session xyz: ...",
            "tags": ["session_digest"],
            "session_id": "xyz99999",
            "session_stats": {
                "tool_calls": 0,
                "duration_minutes": 1,
                "outcome": "completed"
            }
        });
        let (_, preview, _) = event_display(&payload, "note");
        assert!(preview.starts_with("✓"), "got: {preview}");
        assert!(preview.contains("session"), "got: {preview}");
    }

    #[test]
    fn digest_interrupted_shows_cross_red() {
        let payload = json!({
            "text": "Session xyz: interrupted",
            "tags": ["session_digest"],
            "session_id": "xyz99999-long-id",
            "session_stats": {
                "tool_calls": 100,
                "duration_minutes": 120,
                "outcome": "interrupted"
            }
        });
        let (_, preview, style) = event_display(&payload, "note");
        assert!(preview.starts_with("✗"), "got: {preview}");
        assert!(preview.contains("2h"), "got: {preview}");
        assert_eq!(style.fg, Some(Color::Red));
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(304), "5h4m");
        assert_eq!(format_duration(60), "1h");
        assert_eq!(format_duration(42), "42m");
        assert_eq!(format_duration(0), "0m");
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
    fn commit_shows_green_dot() {
        let payload = json!({ "title": "feat: add user auth" });
        let (dtype, preview, style) = event_display(&payload, "commit");
        assert_eq!(dtype, "commit");
        assert!(preview.starts_with("●"));
        assert_eq!(style.fg, Some(Color::Green));
    }

    #[test]
    fn merge_shows_arrow() {
        let payload = json!({ "src": "feat/auth", "dst": "main" });
        let (_, preview, style) = event_display(&payload, "merge");
        assert!(preview.contains("→"));
        assert_eq!(style.fg, Some(Color::Cyan));
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

    #[test]
    fn group_bindings_by_domain() {
        let bindings = vec![
            BindingEntry {
                key: "api.framework".into(),
                value: "fastapi".into(),
                by_session: "s1".into(),
                by_label: "cli".into(),
                ts: "2026-01-01T00:00:00Z".into(),
            },
            BindingEntry {
                key: "bridge.auto_claim".into(),
                value: "stateful".into(),
                by_session: "s1".into(),
                by_label: "cli".into(),
                ts: "2026-01-01T00:00:00Z".into(),
            },
            BindingEntry {
                key: "api.storage".into(),
                value: "memory".into(),
                by_session: "s1".into(),
                by_label: "cli".into(),
                ts: "2026-01-01T00:00:00Z".into(),
            },
            BindingEntry {
                key: "ci.pipeline".into(),
                value: "github".into(),
                by_session: "s1".into(),
                by_label: "cli".into(),
                ts: "2026-01-01T00:00:00Z".into(),
            },
        ];
        let groups = group_bindings(&bindings);
        // User-facing first: api, ci; then internal: bridge
        assert_eq!(groups[0].0, "api");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "ci");
        assert_eq!(groups[1].1.len(), 1);
        assert_eq!(groups[2].0, "bridge");
        assert_eq!(groups[2].1.len(), 1);
    }

    #[test]
    fn internal_domains_collapsed_by_default() {
        assert!(is_internal_domain("bridge"));
        assert!(is_internal_domain("search"));
        assert!(!is_internal_domain("api"));
        assert!(!is_internal_domain("coordination"));
        assert!(!is_internal_domain("runtime"));
    }
}
