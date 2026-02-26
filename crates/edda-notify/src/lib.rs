use std::time::Duration;

use serde::Deserialize;

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

    fn display_name(&self) -> String {
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
}

impl NotifyEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            NotifyEvent::ApprovalPending { .. } => "approval_pending",
            NotifyEvent::PhaseChange { .. } => "phase_change",
            NotifyEvent::SessionEnd { .. } => "session_end",
            NotifyEvent::Anomaly { .. } => "anomaly",
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
        }
    }
}

// ── Dispatch ──

const TIMEOUT: Duration = Duration::from_secs(5);

/// Send notifications to all channels matching this event.
/// Errors are logged to stderr but never propagated.
pub fn dispatch(config: &NotifyConfig, event: &NotifyEvent) {
    for channel in &config.channels {
        if !channel.matches(event) {
            continue;
        }
        let name = channel.display_name();
        if let Err(e) = send(channel, event) {
            eprintln!("[edda-notify] failed to send to {name}: {e}");
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
    config
        .channels
        .iter()
        .map(|ch| {
            let name = ch.display_name();
            let result = send(ch, &test_event).map_err(|e| e.to_string());
            (name, result)
        })
        .collect()
}

fn send(channel: &Channel, event: &NotifyEvent) -> anyhow::Result<()> {
    match channel {
        Channel::Ntfy { url, .. } => send_ntfy(url, event),
        Channel::Webhook { url, .. } => send_webhook(url, event),
        Channel::Telegram {
            bot_token, chat_id, ..
        } => send_telegram(bot_token, chat_id, event),
    }
}

// ── ntfy ──

fn send_ntfy(url: &str, event: &NotifyEvent) -> anyhow::Result<()> {
    let (title, body, priority) = format_ntfy(event);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
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
    }
}

// ── Webhook (generic JSON POST) ──

fn send_webhook(url: &str, event: &NotifyEvent) -> anyhow::Result<()> {
    let payload = format_webhook(event);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(payload.to_string())?;
    Ok(())
}

fn format_webhook(event: &NotifyEvent) -> serde_json::Value {
    serde_json::json!({
        "event_type": event.event_name(),
        "data": event.to_json(),
    })
}

// ── Telegram ──

fn send_telegram(bot_token: &str, chat_id: &str, event: &NotifyEvent) -> anyhow::Result<()> {
    let text = format_telegram(event);
    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown",
    });
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
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
            format!("*Approval needed*\n{title}\nDraft `{draft_id}` requires _{role}_ approval")
        }
        NotifyEvent::PhaseChange {
            from, to, issue, ..
        } => {
            let issue_str = issue.map_or(String::new(), |i| format!(" (#{i})"));
            format!("*Phase change*{issue_str}\n{from} → {to}")
        }
        NotifyEvent::SessionEnd {
            outcome, summary, ..
        } => {
            if summary.is_empty() {
                format!("*Session ended*: {outcome}")
            } else {
                format!("*Session ended*: {outcome}\n{summary}")
            }
        }
        NotifyEvent::Anomaly {
            signal_type,
            count,
            detail,
        } => {
            format!("*Anomaly detected*\n{signal_type} x{count}\n{detail}")
        }
    }
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
        assert!(text.contains("*Approval needed*"));
        assert!(text.contains("Deploy v2"));
        assert!(text.contains("`drf_1`"));
        assert!(text.contains("_ops_"));
    }
}
