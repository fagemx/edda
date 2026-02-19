use crate::agent::launcher::PhaseResult;
use anyhow::Result;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;

/// Relevant fields from Claude Code's stream-json output.
/// Protocol is undocumented — derived from testing Claude Code.
/// Uses `#[serde(other)]` to gracefully ignore unknown message types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamMessage {
    #[serde(rename = "system")]
    System {
        subtype: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        tools: Option<Vec<String>>,
        #[serde(default)]
        model: Option<String>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: serde_json::Value,
    },
    #[serde(rename = "user")]
    User {
        #[serde(default)]
        message: serde_json::Value,
        #[serde(default)]
        tool_use_result: Option<serde_json::Value>,
    },
    #[serde(rename = "result")]
    Result {
        subtype: String,
        #[serde(default)]
        total_cost_usd: Option<f64>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default, rename = "result")]
        result_text: Option<String>,
    },
    /// Catch-all for unknown types — prevents deserialization failures.
    #[serde(other)]
    Unknown,
}

/// Extracted result info from the last Result message.
#[derive(Debug, Clone)]
pub struct ResultInfo {
    pub subtype: String,
    pub total_cost_usd: Option<f64>,
    pub error: Option<String>,
    pub result_text: Option<String>,
}

/// Aggregated output from monitoring a Claude Code stream.
#[derive(Debug)]
pub struct MonitorResult {
    pub total_cost_usd: f64,
    pub result: Option<ResultInfo>,
    pub result_text: Option<String>,
}

/// Reads Claude Code's `--output-format stream-json` stdout line by line,
/// extracting cost and result info.
pub struct StreamMonitor {
    reader: BufReader<ChildStdout>,
    total_cost_usd: f64,
    messages: Vec<StreamMessage>,
    verbose: bool,
    tee_writer: Option<std::io::BufWriter<std::fs::File>>,
}

impl StreamMonitor {
    pub fn new(stdout: ChildStdout) -> Self {
        Self {
            reader: BufReader::new(stdout),
            total_cost_usd: 0.0,
            messages: Vec::new(),
            verbose: false,
            tee_writer: None,
        }
    }

    /// Enable verbose mode: print live activity as the agent works.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Tee raw stdout lines to a file (transcript capture).
    /// Best-effort: if the file can't be opened, tee is silently skipped.
    pub fn with_tee(mut self, path: Option<std::path::PathBuf>) -> Self {
        if let Some(p) = path {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)
            {
                self.tee_writer = Some(std::io::BufWriter::new(file));
            }
        }
        self
    }

    /// Read all lines until EOF. Returns aggregated result.
    pub async fn run(&mut self) -> Result<MonitorResult> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            } // EOF

            // Tee raw line to transcript file
            if let Some(ref mut w) = self.tee_writer {
                use std::io::Write;
                let _ = w.write_all(line.as_bytes());
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(msg) = serde_json::from_str::<StreamMessage>(trimmed) {
                if self.verbose {
                    print_live(&msg);
                }
                if let StreamMessage::Result {
                    total_cost_usd: Some(cost),
                    ..
                } = &msg
                {
                    self.total_cost_usd = *cost;
                }
                self.messages.push(msg);
            }
            // Non-JSON lines silently ignored (stderr leakage, debug output, etc.)
        }

        let result_info = self.messages.iter().rev().find_map(|m| match m {
            StreamMessage::Result {
                subtype,
                total_cost_usd,
                error,
                result_text,
            } => Some(ResultInfo {
                subtype: subtype.clone(),
                total_cost_usd: *total_cost_usd,
                error: error.clone(),
                result_text: result_text.clone(),
            }),
            _ => None,
        });
        let result_text = result_info.as_ref().and_then(|r| r.result_text.clone());

        Ok(MonitorResult {
            total_cost_usd: self.total_cost_usd,
            result: result_info,
            result_text,
        })
    }
}

/// Print a human-readable live line for a stream message.
fn print_live(msg: &StreamMessage) {
    match msg {
        StreamMessage::System { model, .. } => {
            if let Some(m) = model {
                println!("  🔌 Model: {m}");
            }
        }
        StreamMessage::Assistant { message } => {
            // Extract tool_use calls from content array
            if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let input = item.get("input");
                        let detail = match name {
                            "Write" => input
                                .and_then(|i| i.get("file_path"))
                                .and_then(|p| p.as_str())
                                .map(|p| shorten_path(p))
                                .unwrap_or_default(),
                            "Edit" => input
                                .and_then(|i| i.get("file_path"))
                                .and_then(|p| p.as_str())
                                .map(|p| shorten_path(p))
                                .unwrap_or_default(),
                            "Read" => input
                                .and_then(|i| i.get("file_path"))
                                .and_then(|p| p.as_str())
                                .map(|p| shorten_path(p))
                                .unwrap_or_default(),
                            "Bash" => input
                                .and_then(|i| i.get("command"))
                                .and_then(|c| c.as_str())
                                .map(|c| truncate(c, 60))
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        let icon = match name {
                            "Write" => "📝",
                            "Edit" => "✏️",
                            "Read" => "📖",
                            "Bash" => "🔧",
                            "Grep" | "Glob" => "🔍",
                            "WebSearch" | "WebFetch" => "🌐",
                            _ => "🔨",
                        };
                        if detail.is_empty() {
                            println!("  {icon} {name}");
                        } else {
                            println!("  {icon} {name}: {detail}");
                        }
                    } else if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            // Only print short assistant text (final summary, not long explanations)
                            let trimmed = text.trim();
                            if !trimmed.is_empty() && trimmed.len() < 200 {
                                println!("  💬 {}", truncate(trimmed, 80));
                            }
                        }
                    }
                }
            }
        }
        StreamMessage::Result {
            subtype,
            total_cost_usd,
            ..
        } => {
            let cost_str = total_cost_usd
                .map(|c| format!(" (${c:.3})"))
                .unwrap_or_default();
            println!("  📊 Result: {subtype}{cost_str}");
        }
        _ => {}
    }
}

/// Shorten a file path to just the last 2 components.
fn shorten_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() <= 2 {
        parts.join("/")
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

/// Truncate a string to max_len, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    let s = s.replace('\n', " ").replace('\r', "");
    if s.len() <= max_len {
        s
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Classify a monitor result + exit code into a PhaseResult.
pub fn classify_result(monitor: &MonitorResult, exit_code: Option<i32>) -> PhaseResult {
    match &monitor.result {
        Some(info) => match info.subtype.as_str() {
            "success" => PhaseResult::AgentDone {
                cost_usd: info.total_cost_usd,
                result_text: monitor.result_text.clone(),
            },
            "error_max_turns" => PhaseResult::MaxTurns {
                cost_usd: info.total_cost_usd,
            },
            "error_max_budget_usd" => PhaseResult::BudgetExceeded {
                cost_usd: info.total_cost_usd,
            },
            "error_during_execution" => PhaseResult::AgentCrash {
                error: info.error.clone().unwrap_or_else(|| "unknown".into()),
            },
            other => PhaseResult::AgentCrash {
                error: format!("unknown result subtype: {other}"),
            },
        },
        None => PhaseResult::AgentCrash {
            error: format!(
                "agent exited with code {} without result",
                exit_code.unwrap_or(-1)
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_system_init() {
        let json = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"claude-sonnet-4-5-20250929"}"#;
        let msg: StreamMessage = serde_json::from_str(json).unwrap();
        match msg {
            StreamMessage::System {
                subtype,
                session_id,
                model,
                ..
            } => {
                assert_eq!(subtype, "init");
                assert_eq!(session_id.unwrap(), "abc-123");
                assert_eq!(model.unwrap(), "claude-sonnet-4-5-20250929");
            }
            other => panic!("expected System, got {:?}", other),
        }
    }

    #[test]
    fn parse_assistant() {
        let json = r#"{"type":"assistant","message":{"content":"hello"}}"#;
        let msg: StreamMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, StreamMessage::Assistant { .. }));
    }

    #[test]
    fn parse_result_success() {
        let json =
            r#"{"type":"result","subtype":"success","total_cost_usd":0.42,"error":null}"#;
        let msg: StreamMessage = serde_json::from_str(json).unwrap();
        match msg {
            StreamMessage::Result {
                subtype,
                total_cost_usd,
                error,
                ..
            } => {
                assert_eq!(subtype, "success");
                assert!((total_cost_usd.unwrap() - 0.42).abs() < 0.001);
                assert!(error.is_none());
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn parse_result_error() {
        let json = r#"{"type":"result","subtype":"error_during_execution","total_cost_usd":0.10,"error":"tool failed"}"#;
        let msg: StreamMessage = serde_json::from_str(json).unwrap();
        match msg {
            StreamMessage::Result { subtype, error, .. } => {
                assert_eq!(subtype, "error_during_execution");
                assert_eq!(error.unwrap(), "tool failed");
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_type_gracefully() {
        let json = r#"{"type":"future_new_type","data":"whatever"}"#;
        let msg: StreamMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, StreamMessage::Unknown));
    }

    #[test]
    fn classify_success() {
        let monitor = MonitorResult {
            total_cost_usd: 0.5,
            result: Some(ResultInfo {
                subtype: "success".into(),
                total_cost_usd: Some(0.5),
                error: None,
                result_text: None,
            }),
            result_text: None,
        };
        let r = classify_result(&monitor, Some(0));
        assert!(matches!(r, PhaseResult::AgentDone { cost_usd: Some(c), .. } if (c - 0.5).abs() < 0.001));
    }

    #[test]
    fn classify_max_turns() {
        let monitor = MonitorResult {
            total_cost_usd: 1.0,
            result: Some(ResultInfo {
                subtype: "error_max_turns".into(),
                total_cost_usd: Some(1.0),
                error: None,
                result_text: None,
            }),
            result_text: None,
        };
        let r = classify_result(&monitor, Some(1));
        assert!(matches!(r, PhaseResult::MaxTurns { .. }));
    }

    #[test]
    fn classify_budget_exceeded() {
        let monitor = MonitorResult {
            total_cost_usd: 2.0,
            result: Some(ResultInfo {
                subtype: "error_max_budget_usd".into(),
                total_cost_usd: Some(2.0),
                error: None,
                result_text: None,
            }),
            result_text: None,
        };
        let r = classify_result(&monitor, Some(1));
        assert!(matches!(r, PhaseResult::BudgetExceeded { .. }));
    }

    #[test]
    fn classify_execution_error() {
        let monitor = MonitorResult {
            total_cost_usd: 0.1,
            result: Some(ResultInfo {
                subtype: "error_during_execution".into(),
                total_cost_usd: Some(0.1),
                error: Some("tool crashed".into()),
                result_text: None,
            }),
            result_text: None,
        };
        let r = classify_result(&monitor, Some(1));
        match r {
            PhaseResult::AgentCrash { error } => assert_eq!(error, "tool crashed"),
            other => panic!("expected AgentCrash, got {:?}", other),
        }
    }

    #[test]
    fn classify_unknown_subtype() {
        let monitor = MonitorResult {
            total_cost_usd: 0.0,
            result: Some(ResultInfo {
                subtype: "new_error_type".into(),
                total_cost_usd: None,
                error: None,
                result_text: None,
            }),
            result_text: None,
        };
        let r = classify_result(&monitor, Some(2));
        assert!(matches!(r, PhaseResult::AgentCrash { .. }));
    }

    #[test]
    fn classify_no_result_message() {
        let monitor = MonitorResult {
            total_cost_usd: 0.0,
            result: None,
            result_text: None,
        };
        let r = classify_result(&monitor, Some(137));
        match r {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("137"));
            }
            other => panic!("expected AgentCrash, got {:?}", other),
        }
    }

    #[test]
    fn classify_no_result_no_exit_code() {
        let monitor = MonitorResult {
            total_cost_usd: 0.0,
            result: None,
            result_text: None,
        };
        let r = classify_result(&monitor, None);
        match r {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("-1"));
            }
            other => panic!("expected AgentCrash, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tee_captures_raw_lines() {
        use tokio::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let tee_path = dir.path().join("transcript.jsonl");

        // Spawn a process that writes lines to stdout
        let mut child = Command::new("cmd")
            .args(["/C", "echo line_one & echo line_two"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let mut monitor = StreamMonitor::new(stdout).with_tee(Some(tee_path.clone()));
        // Lines won't parse as JSON — that's fine, tee captures raw output regardless
        let _result = monitor.run().await.unwrap();

        // Tee file captured raw lines (drop monitor to flush BufWriter)
        drop(monitor);
        let content = std::fs::read_to_string(&tee_path).unwrap();
        assert!(content.contains("line_one"), "tee should capture raw lines: {content}");
        assert!(content.contains("line_two"), "tee should capture both lines: {content}");
    }
}
