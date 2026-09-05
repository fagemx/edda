use serde::Deserialize;
use std::time::Duration;
// ── Config ──

/// Notification channel configuration — stored in `.edda/config.json` under key `notify_channels`.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum Channel {
    #[serde(rename = "ntfy")]
    Ntfy { url: String, events: Vec<String> },
    #[serde(rename = "webhook")]
    Webhook { url: String, events: Vec<String> },
    #[serde(rename = "telegram")]
    Telegram {
        bot_token: String,
        chat_id: String,
        events: Vec<String>,
    },
}
impl Channel {
    fn events(&self) -> &[String] {
        match self {
            Channel::Ntfy { events, .. } => events,
            Channel::Webhook { events, .. } => events,
            Channel::Telegram { events, .. } => events,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Channel::Ntfy { url, .. } => format!("ntfy({})", url),
            Channel::Webhook { url, .. } => format!("webhook({})", url),
            Channel::Telegram { chat_id, .. } => format!("telegram(chat:{})", chat_id),
        }
    }

    fn matches(&self, event: &NotifyEvent) -> bool {
        let name = event.event_name();
        self.events().iter().any(|e| e == name || e == "*")
    }
}
/// Top-level notify configuration.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct NotifyConfig {
    pub channels: Vec<Channel>,
}
impl NotifyConfig {
    /// Load from `.edda/config.json` key `notify_channels`.
    /// Returns empty config if key is missing or unparseable.
    pub fn load(paths: &edda_ledger::EddaPaths) -> Self {
        let path = &paths.config_json;
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        let val: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let channels_val = match val.get("notify_channels") {
            Some(v) => v.clone(),
            None => return Self::default(),
        };
        let channels: Vec<Channel> = match serde_json::from_value(channels_val) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        Self { channels }
    }
}

// ── Notification Events ──

/// Notification event types mapped from edda domain events.
#[derive(Clone)]
pub enum NotifyEvent {
    ApprovalPending {
        draft_id: String,
        title: String,
        stage_id: String,
        role: String,
    },
    PhaseChange {
        session_id: String,
        from: String,
        to: String,
        issue: Option<u64>,
    },
    SessionEnd {
        session_id: String,
        outcome: String,
        duration_minutes: u64,
        summary: String,
    },
    Anomaly {
        signal_type: String,
        count: usize,
        detail: String,
    },
    RequestPending {
        from_label: String,
        to_label: String,
        message: String,
    },
    TaskAssigned {
        task_id: u64,
        title: String,
        assignee: String,
    },
    /// GH-564: one notification per phase terminal transition. `state` is
    /// the terminal status name ("Passed" | "Failed" | "Stale" | "Skipped" |
    /// "Aborted"); "Aborted" is plan-level and names the phase that forced
    /// the abort. `final_output` carries the agent's last output line when
    /// the transition site has one (by convention it contains the PR URL).
    PhaseTerminal {
        plan: String,
        phase: String,
        state: String,
        attempt: u32,
        final_output: Option<String>,
    },
    /// GH-551/GH-751: progress notification for a gated phase awaiting verdict.
    GateProgress {
        plan: String,
        phase: String,
        subject: String,
        gate_sha: String,
        wait_label: String,
    },
    /// GH-765: free-text delivery for the daily fleet digest (and any other
    /// operator-facing push). `title` is the first line, `body` the rest.
    Digest { title: String, body: String },
}

impl NotifyEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            NotifyEvent::ApprovalPending { .. } => "approval_pending",
            NotifyEvent::PhaseChange { .. } => "phase_change",
            NotifyEvent::SessionEnd { .. } => "session_end",
            NotifyEvent::Anomaly { .. } => "anomaly",
            NotifyEvent::RequestPending { .. } => "request_pending",
            NotifyEvent::TaskAssigned { .. } => "task_assigned",
            NotifyEvent::PhaseTerminal { .. } => "phase_terminal",
            NotifyEvent::GateProgress { .. } => "gate_progress",
            NotifyEvent::Digest { .. } => "digest",
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            NotifyEvent::ApprovalPending {
                draft_id,
                title,
                stage_id,
                role,
            } => serde_json::json!({
                "draft_id": draft_id,
                "title": title,
                "stage_id": stage_id,
                "role": role,
            }),
            NotifyEvent::PhaseChange {
                session_id,
                from,
                to,
                issue,
            } => serde_json::json!({
                "session_id": session_id,
                "from": from,
                "to": to,
                "issue": issue,
            }),
            NotifyEvent::SessionEnd {
                session_id,
                outcome,
                duration_minutes,
                summary,
            } => serde_json::json!({
                "session_id": session_id,
                "outcome": outcome,
                "duration_minutes": duration_minutes,
                "summary": summary,
            }),
            NotifyEvent::Anomaly {
                signal_type,
                count,
                detail,
            } => serde_json::json!({
                "signal_type": signal_type,
                "count": count,
                "detail": detail,
            }),
            NotifyEvent::RequestPending {
                from_label,
                to_label,
                message,
            } => serde_json::json!({
                "from_label": from_label,
                "to_label": to_label,
                "message": message,
            }),
            NotifyEvent::TaskAssigned {
                task_id,
                title,
                assignee,
            } => serde_json::json!({
                "task_id": task_id,
                "title": title,
                "assignee": assignee,
            }),
            NotifyEvent::PhaseTerminal {
                plan,
                phase,
                state,
                attempt,
                final_output,
            } => serde_json::json!({
                "plan": plan,
                "phase": phase,
                "state": state,
                "attempt": attempt,
                "final_output": final_output,
            }),
            NotifyEvent::GateProgress {
                plan,
                phase,
                subject,
                gate_sha,
                wait_label,
            } => serde_json::json!({
                "plan": plan,
                "phase": phase,
                "subject": subject,
                "gate_sha": gate_sha,
                "wait_label": wait_label,
            }),
            NotifyEvent::Digest { title, body } => serde_json::json!({
                "title": title,
                "body": body,
            }),
        }
    }
}

// ── Dispatch ──

const TIMEOUT: Duration = Duration::from_secs(5);

fn make_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent()
}

/// Send notifications to all channels matching this event.
/// Errors are logged to stderr but never propagated.
pub fn dispatch(config: &NotifyConfig, event: &NotifyEvent) {
    let agent = make_agent();
    for channel in &config.channels {
        if !channel.matches(event) {
            continue;
        }
        let name = channel.display_name();
        if let Err(e) = send(&agent, channel, event) {
            tracing::warn!(channel = %name, error = %e, "notification send failed");
        }
    }
}

/// Send a test notification to all configured channels.
/// Returns per-channel results for CLI display.
pub fn test_channels(config: &NotifyConfig) -> Vec<(String, Result<(), String>)> {
    let test_event = NotifyEvent::SessionEnd {
        session_id: "test".to_string(),
        outcome: "test".to_string(),
        duration_minutes: 0,
        summary: "edda notify test — if you see this, notifications are working!".to_string(),
    };
    let agent = make_agent();
    config
        .channels
        .iter()
        .map(|ch| {
            (
                ch.display_name(),
                send(&agent, ch, &test_event).map_err(|e| e.to_string()),
            )
        })
        .collect()
}
/// Send a free-text message (event name "digest") to every channel whose
/// `events` list contains "digest" or "*". Same shape as [`test_channels`]:
/// per-channel results for CLI display; channels that do not subscribe get
/// a skip error so the operator sees why nothing arrived.
pub fn send_text(
    config: &NotifyConfig,
    title: &str,
    body: &str,
) -> Vec<(String, Result<(), String>)> {
    let event = NotifyEvent::Digest {
        title: title.to_string(),
        body: body.to_string(),
    };
    send_to_all(config, &event)
}

fn send_to_all(config: &NotifyConfig, event: &NotifyEvent) -> Vec<(String, Result<(), String>)> {
    let agent = make_agent();
    config
        .channels
        .iter()
        .map(|ch| {
            let name = ch.display_name();
            let result = if ch.matches(event) {
                send(&agent, ch, event).map_err(|e| e.to_string())
            } else {
                Err(format!(
                    "channel does not subscribe to {:?} (events: {:?})",
                    event.event_name(),
                    ch.events()
                ))
            };
            (name, result)
        })
        .collect()
}

fn send(agent: &ureq::Agent, channel: &Channel, event: &NotifyEvent) -> anyhow::Result<()> {
    match channel {
        Channel::Ntfy { url, .. } => send_ntfy(agent, url, event),
        Channel::Webhook { url, .. } => send_webhook(agent, url, event),
        Channel::Telegram {
            bot_token, chat_id, ..
        } => send_telegram(agent, bot_token, chat_id, event),
    }
}

// ── ntfy ──

fn send_ntfy(agent: &ureq::Agent, url: &str, event: &NotifyEvent) -> anyhow::Result<()> {
    let (title, body, priority) = format_ntfy(event);
    agent
        .post(url)
        .header("Title", &title)
        .header("Priority", &priority)
        .send(&body)?;
    Ok(())
}

fn format_ntfy(event: &NotifyEvent) -> (String, String, String) {
    match event {
        NotifyEvent::ApprovalPending {
            title,
            role,
            draft_id,
            ..
        } => (
            format!("Approval needed: {title}"),
            format!("Draft {draft_id} requires {role} approval"),
            "high".to_string(),
        ),
        NotifyEvent::PhaseChange {
            from, to, issue, ..
        } => {
            let issue_str = issue.map_or(String::new(), |i| format!(" (#{i})"));
            (
                format!("Phase: {from} -> {to}{issue_str}"),
                format!("Agent transitioned from {from} to {to}"),
                "default".to_string(),
            )
        }
        NotifyEvent::SessionEnd {
            outcome, summary, ..
        } => (
            format!("Session ended: {outcome}"),
            if summary.is_empty() {
                "Agent session completed".to_string()
            } else {
                summary.clone()
            },
            "low".to_string(),
        ),
        NotifyEvent::Anomaly {
            signal_type,
            count,
            detail,
        } => (
            format!("Anomaly: {signal_type} x{count}"),
            detail.clone(),
            "urgent".to_string(),
        ),
        NotifyEvent::RequestPending {
            from_label,
            to_label,
            message,
        } => (
            format!("Request for {to_label} from {from_label}"),
            message.clone(),
            "high".to_string(),
        ),
        NotifyEvent::TaskAssigned {
            task_id,
            title,
            assignee,
        } => (
            format!("Task assigned: {title}"),
            format!("#{task_id} assigned to {assignee}"),
            "default".to_string(),
        ),
        NotifyEvent::PhaseTerminal {
            plan,
            phase,
            state,
            attempt,
            final_output,
        } => {
            let priority = match state.as_str() {
                "Failed" | "Aborted" | "Stale" => "high",
                "Skipped" => "low",
                _ => "default",
            };
            let mut body = format!("plan {plan} · attempt {attempt}");
            if let Some(out) = final_output {
                body.push('\n');
                body.push_str(out);
            }
            (
                format!("Phase {phase}: {state}"),
                body,
                priority.to_string(),
            )
        }
        NotifyEvent::GateProgress {
            subject,
            gate_sha,
            wait_label,
            ..
        } => (
            format!("Verdict needed: {subject}"),
            format!("Waiting on sha {gate_sha} — {wait_label}"),
            "default".to_string(),
        ),
        NotifyEvent::Digest { title, body } => (title.clone(), body.clone(), "default".to_string()),
    }
}

// ── Webhook (generic JSON POST) ──

fn send_webhook(agent: &ureq::Agent, url: &str, event: &NotifyEvent) -> anyhow::Result<()> {
    let payload = format_webhook(event);
    agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(payload.to_string())?;
    Ok(())
}

fn format_webhook(event: &NotifyEvent) -> serde_json::Value {
    // GH-765: the digest posts a flat payload (event name + title + body) so
    // simple receivers do not have to unwrap the generic envelope.
    if let NotifyEvent::Digest { title, body } = event {
        return serde_json::json!({
            "event": "digest",
            "title": title,
            "body": body,
        });
    }
    serde_json::json!({
        "event_type": event.event_name(),
        "data": event.to_json(),
    })
}

// ── Telegram ──

fn send_telegram(
    agent: &ureq::Agent,
    bot_token: &str,
    chat_id: &str,
    event: &NotifyEvent,
) -> anyhow::Result<()> {
    let text = format_telegram(event);
    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
    });
    agent
        .post(&url)
        .header("Content-Type", "application/json")
        .send(body.to_string())?;
    Ok(())
}

fn format_telegram(event: &NotifyEvent) -> String {
    match event {
        NotifyEvent::ApprovalPending {
            title,
            role,
            draft_id,
            ..
        } => {
            let t = escape_html(title);
            let d = escape_html(draft_id);
            let r = escape_html(role);
            format!(
                "<b>Approval needed</b>\n{t}\nDraft <code>{d}</code> requires <i>{r}</i> approval"
            )
        }
        NotifyEvent::PhaseChange {
            from, to, issue, ..
        } => {
            let issue_str = issue.map_or(String::new(), |i| format!(" (#{})", i));
            let f = escape_html(from);
            let t = escape_html(to);
            format!("<b>Phase change</b>{issue_str}\n{f} \u{2192} {t}")
        }
        NotifyEvent::SessionEnd {
            outcome, summary, ..
        } => {
            let o = escape_html(outcome);
            if summary.is_empty() {
                format!("<b>Session ended</b>: {o}")
            } else {
                let s = escape_html(summary);
                format!("<b>Session ended</b>: {o}\n{s}")
            }
        }
        NotifyEvent::Anomaly {
            signal_type,
            count,
            detail,
        } => {
            let st = escape_html(signal_type);
            let d = escape_html(detail);
            format!("<b>Anomaly detected</b>\n{st} x{count}\n{d}")
        }
        NotifyEvent::RequestPending {
            from_label,
            to_label,
            message,
        } => format!(
            "<b>Request pending</b>\n{} → {}\n{}",
            escape_html(from_label),
            escape_html(to_label),
            escape_html(message)
        ),
        NotifyEvent::TaskAssigned {
            task_id,
            title,
            assignee,
        } => format!(
            "<b>Task assigned</b>\n#{} {}\n{}",
            task_id,
            escape_html(title),
            escape_html(assignee)
        ),
        NotifyEvent::PhaseTerminal {
            plan,
            phase,
            state,
            attempt,
            final_output,
        } => {
            let mut text = format!(
                "<b>Phase {}: {}</b>\nplan {} · attempt {}",
                escape_html(phase),
                escape_html(state),
                escape_html(plan),
                attempt
            );
            if let Some(out) = final_output {
                text.push('\n');
                text.push_str(&escape_html(out));
            }
            text
        }
        NotifyEvent::GateProgress {
            subject,
            gate_sha,
            wait_label,
            ..
        } => {
            let s = escape_html(subject);
            let g = escape_html(gate_sha);
            let w = escape_html(wait_label);
            format!("<b>Verdict needed: {s}</b>\nsha <code>{g}</code> — {w}")
        }
        NotifyEvent::Digest { title, body } => truncate_telegram_digest(title, body),
    }
}
/// Telegram rejects messages over 4096 characters; keep a margin for the
/// HTML entities and the title. A longer text is truncated with a trailing
/// ellipsis rather than failing delivery.
const TELEGRAM_MAX_CHARS: usize = 3900;

fn truncate_telegram_digest(title: &str, body: &str) -> String {
    let escaped_len = |s: &str| escape_html(s).chars().count();
    let overhead = "<b></b>\n".chars().count();
    if overhead + escaped_len(title) + escaped_len(body) <= TELEGRAM_MAX_CHARS {
        return format!("<b>{}</b>\n{}", escape_html(title), escape_html(body));
    }
    let title_budget = TELEGRAM_MAX_CHARS - overhead - 1;
    if escaped_len(title) > title_budget {
        return format!("<b>{}…</b>", escape_prefix(title, title_budget));
    }
    let body_budget = TELEGRAM_MAX_CHARS - overhead - escaped_len(title) - 1;
    format!(
        "<b>{}</b>\n{}…",
        escape_html(title),
        escape_prefix(body, body_budget)
    )
}
fn escape_prefix(text: &str, budget: usize) -> String {
    let mut prefix = String::new();
    for ch in text.chars() {
        let escaped = escape_html(&ch.to_string());
        if prefix.chars().count() + escaped.chars().count() > budget {
            break;
        }
        prefix.push_str(&escaped);
    }
    prefix
}
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_deserialize_ntfy() {
        let json =
            r#"[{"type":"ntfy","url":"https://ntfy.sh/test","events":["approval_pending"]}]"#;
        let channels: Vec<Channel> = serde_json::from_str(json).unwrap();
        assert_eq!(channels.len(), 1);
        assert!(
            matches!(&channels[0], Channel::Ntfy { url, events } if url == "https://ntfy.sh/test" && events == &["approval_pending"])
        );
    }

    #[test]
    fn config_deserialize_all_types() {
        let json = r#"[
            {"type":"ntfy","url":"https://ntfy.sh/t","events":["approval_pending"]},
            {"type":"webhook","url":"https://hooks.slack.com/xxx","events":["phase_change"]},
            {"type":"telegram","bot_token":"123:ABC","chat_id":"456","events":["session_end"]}
        ]"#;
        let channels: Vec<Channel> = serde_json::from_str(json).unwrap();
        assert_eq!(channels.len(), 3);
        assert!(matches!(&channels[0], Channel::Ntfy { .. }));
        assert!(matches!(&channels[1], Channel::Webhook { .. }));
        assert!(matches!(&channels[2], Channel::Telegram { .. }));
    }

    #[test]
    fn config_load_missing_file() {
        let paths = edda_ledger::EddaPaths::discover(std::path::Path::new("/nonexistent"));
        let config = NotifyConfig::load(&paths);
        assert!(config.channels.is_empty());
    }

    #[test]
    fn event_matches_channel() {
        let ch: Channel = serde_json::from_value(serde_json::json!({
            "type": "ntfy",
            "url": "https://ntfy.sh/test",
            "events": ["approval_pending", "anomaly"]
        }))
        .unwrap();

        let approval = NotifyEvent::ApprovalPending {
            draft_id: "d1".into(),
            title: "t".into(),
            stage_id: "s1".into(),
            role: "reviewer".into(),
        };
        assert!(ch.matches(&approval));

        let phase = NotifyEvent::PhaseChange {
            session_id: "s1".into(),
            from: "Research".into(),
            to: "Plan".into(),
            issue: None,
        };
        assert!(!ch.matches(&phase));
    }

    #[test]
    fn coordination_events_have_stable_names_and_payloads() {
        let request = NotifyEvent::RequestPending {
            from_label: "auth".into(),
            to_label: "billing".into(),
            message: "need invoice type".into(),
        };
        assert_eq!(request.event_name(), "request_pending");
        assert_eq!(request.to_json()["to_label"], "billing");

        let task = NotifyEvent::TaskAssigned {
            task_id: 11,
            title: "Fix coordination".into(),
            assignee: "coord-worker".into(),
        };
        assert_eq!(task.event_name(), "task_assigned");
        assert_eq!(task.to_json()["task_id"], 11);
    }

    #[test]
    fn wildcard_matches_all() {
        let ch: Channel = serde_json::from_value(serde_json::json!({
            "type": "webhook",
            "url": "https://example.com/hook",
            "events": ["*"]
        }))
        .unwrap();

        let event = NotifyEvent::SessionEnd {
            session_id: "s1".into(),
            outcome: "completed".into(),
            duration_minutes: 30,
            summary: String::new(),
        };
        assert!(ch.matches(&event));
    }

    #[test]
    fn phase_terminal_has_stable_name_payload_and_matching() {
        let event = NotifyEvent::PhaseTerminal {
            plan: "gh564".into(),
            phase: "implement".into(),
            state: "Passed".into(),
            attempt: 2,
            final_output: Some("PR: https://github.com/x/y/pull/9".into()),
        };
        assert_eq!(event.event_name(), "phase_terminal");
        assert_eq!(event.to_json()["state"], "Passed");
        assert_eq!(event.to_json()["attempt"], 2);
        assert_eq!(
            event.to_json()["final_output"],
            "PR: https://github.com/x/y/pull/9"
        );

        let ch: Channel = serde_json::from_value(serde_json::json!({
            "type": "ntfy",
            "url": "https://ntfy.sh/test",
            "events": ["phase_terminal"]
        }))
        .unwrap();
        assert!(ch.matches(&event));
    }

    #[test]
    fn phase_terminal_none_final_output_serializes_to_null() {
        let event = NotifyEvent::PhaseTerminal {
            plan: "p".into(),
            phase: "a".into(),
            state: "Stale".into(),
            attempt: 1,
            final_output: None,
        };
        assert!(event.to_json()["final_output"].is_null());
    }

    #[test]
    fn format_ntfy_phase_terminal_priority_by_state() {
        let mk = |state: &str| NotifyEvent::PhaseTerminal {
            plan: "p".into(),
            phase: "a".into(),
            state: state.into(),
            attempt: 1,
            final_output: Some("PR: https://x/1".into()),
        };
        let (title, body, priority) = format_ntfy(&mk("Failed"));
        assert!(title.contains("Phase a: Failed"));
        assert!(body.contains("PR: https://x/1"));
        assert_eq!(priority, "high");
        assert_eq!(format_ntfy(&mk("Passed")).2, "default");
        assert_eq!(format_ntfy(&mk("Skipped")).2, "low");
        assert_eq!(format_ntfy(&mk("Aborted")).2, "high");
    }

    #[test]
    fn format_telegram_phase_terminal_escapes_html() {
        let event = NotifyEvent::PhaseTerminal {
            plan: "p".into(),
            phase: "<a>".into(),
            state: "Failed".into(),
            attempt: 1,
            final_output: Some("err <&>".into()),
        };
        let text = format_telegram(&event);
        assert!(text.contains("Phase &lt;a&gt;: Failed"));
        assert!(text.contains("err &lt;&amp;&gt;"));
    }

    #[test]
    fn format_webhook_phase_terminal_payload() {
        let event = NotifyEvent::PhaseTerminal {
            plan: "p".into(),
            phase: "a".into(),
            state: "Skipped".into(),
            attempt: 3,
            final_output: None,
        };
        let payload = format_webhook(&event);
        assert_eq!(payload["event_type"], "phase_terminal");
        assert_eq!(payload["data"]["plan"], "p");
        assert_eq!(payload["data"]["attempt"], 3);
    }

    #[test]
    fn format_gate_progress_events() {
        let event = NotifyEvent::GateProgress {
            plan: "p".into(),
            phase: "a".into(),
            subject: "p/a".into(),
            gate_sha: "1234567890abcdef".into(),
            wait_label: "9m0s remaining".into(),
        };
        let (title, body, priority) = format_ntfy(&event);
        assert_eq!(title, "Verdict needed: p/a");
        assert_eq!(body, "Waiting on sha 1234567890abcdef — 9m0s remaining");
        assert_eq!(priority, "default");

        let text = format_telegram(&event);
        assert!(text.contains("<b>Verdict needed: p/a</b>"));
        assert!(text.contains("sha <code>1234567890abcdef</code> — 9m0s remaining"));

        let payload = format_webhook(&event);
        assert_eq!(payload["event_type"], "gate_progress");
        assert_eq!(payload["data"]["plan"], "p");
        assert_eq!(payload["data"]["phase"], "a");
        assert_eq!(payload["data"]["subject"], "p/a");
        assert_eq!(payload["data"]["gate_sha"], "1234567890abcdef");
        assert_eq!(payload["data"]["wait_label"], "9m0s remaining");
    }

    #[test]
    fn format_ntfy_approval_pending() {
        let event = NotifyEvent::ApprovalPending {
            draft_id: "drf_123".into(),
            title: "Add auth module".into(),
            stage_id: "stage_1".into(),
            role: "tech-lead".into(),
        };
        let (title, body, priority) = format_ntfy(&event);
        assert!(title.contains("Approval needed"));
        assert!(title.contains("Add auth module"));
        assert!(body.contains("drf_123"));
        assert!(body.contains("tech-lead"));
        assert_eq!(priority, "high");
    }

    #[test]
    fn format_ntfy_phase_change() {
        let event = NotifyEvent::PhaseChange {
            session_id: "s1".into(),
            from: "Research".into(),
            to: "Implement".into(),
            issue: Some(42),
        };
        let (title, body, priority) = format_ntfy(&event);
        assert!(title.contains("Research -> Implement"));
        assert!(title.contains("#42"));
        assert!(body.contains("Research"));
        assert_eq!(priority, "default");
    }

    #[test]
    fn format_webhook_payload() {
        let event = NotifyEvent::ApprovalPending {
            draft_id: "drf_1".into(),
            title: "Fix bug".into(),
            stage_id: "s1".into(),
            role: "reviewer".into(),
        };
        let payload = format_webhook(&event);
        assert_eq!(payload["event_type"], "approval_pending");
        assert_eq!(payload["data"]["draft_id"], "drf_1");
        assert_eq!(payload["data"]["title"], "Fix bug");
    }

    #[test]
    fn format_telegram_approval() {
        let event = NotifyEvent::ApprovalPending {
            draft_id: "drf_1".into(),
            title: "Deploy v2".into(),
            stage_id: "s1".into(),
            role: "ops".into(),
        };
        let text = format_telegram(&event);
        assert!(text.contains("<b>Approval needed</b>"));
        assert!(text.contains("Deploy v2"));
        assert!(text.contains("<code>drf_1</code>"));
        assert!(text.contains("<i>ops</i>"));
    }

    #[test]
    fn format_telegram_escapes_html() {
        let event = NotifyEvent::ApprovalPending {
            draft_id: "d1".into(),
            title: "Fix <script> & stuff".into(),
            stage_id: "s1".into(),
            role: "dev".into(),
        };
        let text = format_telegram(&event);
        assert!(text.contains("Fix &lt;script&gt; &amp; stuff"));
    }

    #[test]
    fn digest_has_stable_name_and_flat_webhook_payload() {
        let event = NotifyEvent::Digest {
            title: "Fleet digest 2026-09-03".into(),
            body: "## 例外\n（無）\n".into(),
        };
        assert_eq!(event.event_name(), "digest");
        let payload = format_webhook(&event);
        assert_eq!(payload["event"], "digest");
        assert_eq!(payload["title"], "Fleet digest 2026-09-03");
        assert_eq!(payload["body"], "## 例外\n（無）\n");

        let ch: Channel = serde_json::from_value(serde_json::json!({
            "type": "webhook",
            "url": "https://example.com/hook",
            "events": ["digest"]
        }))
        .unwrap();
        assert!(ch.matches(&event));
    }

    #[test]
    fn format_telegram_digest_escapes_html() {
        let event = NotifyEvent::Digest {
            title: "<&>".into(),
            body: "a<b>&c".into(),
        };
        let text = format_telegram(&event);
        assert!(text.starts_with("<b>&lt;&amp;&gt;</b>"), "text={text}");
        assert!(text.contains("a&lt;b&gt;&amp;c"), "text={text}");
    }

    #[test]
    fn format_telegram_digest_truncates_to_3900_with_ellipsis() {
        let event = NotifyEvent::Digest {
            title: "t".into(),
            body: "x".repeat(5000),
        };
        let text = format_telegram(&event);
        assert_eq!(text.chars().count(), 3900);
        assert!(text.ends_with('\u{2026}'));

        let short = NotifyEvent::Digest {
            title: "t".into(),
            body: "short".into(),
        };
        let text = format_telegram(&short);
        assert!(!text.ends_with('\u{2026}'));
    }

    #[test]
    fn digest_truncation_keeps_entities_and_tags_complete() {
        let event = NotifyEvent::Digest {
            title: "t".into(),
            body: format!("{}&", "x".repeat(3889)),
        };
        let text = format_telegram(&event);
        assert!(!text.ends_with("&…"), "text={text}");
        assert!(text.starts_with("<b>t</b>\n"), "text={text}");
        assert!(text.chars().count() <= TELEGRAM_MAX_CHARS);
        let text = format_telegram(&NotifyEvent::Digest {
            title: "x".repeat(4000),
            body: String::new(),
        });
        assert!(text.ends_with("…</b>"), "text={text}");
    }
    #[test]
    fn test_channels_ignores_event_subscriptions() {
        let config = NotifyConfig {
            channels: vec![Channel::Webhook {
                url: "http://127.0.0.1:9".into(),
                events: vec!["approval_pending".into()],
            }],
        };
        let results = test_channels(&config);
        assert!(!results[0]
            .1
            .as_ref()
            .unwrap_err()
            .contains("does not subscribe"));
    }
}
