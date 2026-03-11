//! Background decision extractor — scans session transcripts via LLM to find
//! architectural/technical decisions that the agent forgot to record.
//!
//! Design: non-blocking, idempotent, cost-controlled.  Triggered at SessionEnd
//! via `std::thread::spawn` so the hook returns immediately.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ── Configuration ──

pub(crate) const DEFAULT_MODEL: &str = "claude-3-5-haiku-20241022";
pub(crate) const DEFAULT_MAX_TRANSCRIPT_CHARS: usize = 30_000;
const DEFAULT_DAILY_BUDGET_USD: f64 = 0.50;
const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.7;
pub(crate) const API_TIMEOUT_SECS: u64 = 30;

// Haiku pricing (per token)
pub(crate) const HAIKU_INPUT_COST_PER_TOKEN: f64 = 0.000_001; // $1 / 1M input tokens
pub(crate) const HAIKU_OUTPUT_COST_PER_TOKEN: f64 = 0.000_005; // $5 / 1M output tokens

// ── Data Structures ──

/// Distinguishes background-extracted decisions from reason enhancements.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    /// A new decision found by the background extractor.
    #[default]
    Extraction,
    /// An enhanced reason for a decision already recorded via `edda decide`.
    Enhancement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDecision {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub confidence: f64,
    pub evidence: String,
    #[serde(default)]
    pub source_turn: usize,
    #[serde(default)]
    pub status: DraftStatus,
    #[serde(default)]
    pub kind: DecisionKind,
    /// For `Enhancement` kind: the original (vague) reason that was enhanced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    #[default]
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub session_id: String,
    pub decisions: Vec<ExtractedDecision>,
    pub transcript_hash: String,
    pub extracted_at: String,
    pub model_used: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftDecisionFile {
    pub session_id: String,
    pub extracted_at: String,
    pub model: String,
    pub decisions: Vec<ExtractedDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractionState {
    status: String, // "completed" | "pending" | "failed"
    extracted_at: String,
    transcript_hash: String,
    decisions_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DailyCost {
    date: String,
    total_usd: f64,
    calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    session_id: String,
    decisions_found: usize,
    cost_usd: f64,
    model: String,
    status: String,
}

// Anthropic API types (sync, ureq-based)
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<ApiMessage>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ApiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlock {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

// ── Public API ──

/// Check whether background extraction should run for this session.
///
/// Returns `false` (skip) if any of these hold:
/// - `EDDA_BG_ENABLED` is `"0"`
/// - `EDDA_LLM_API_KEY` is missing or empty
/// - Session has zero nudge signals (`signal_count == 0`)
/// - Daily budget is exhausted
/// - Session was already extracted (idempotent guard)
pub fn should_run(project_id: &str, session_id: &str) -> bool {
    if std::env::var("EDDA_BG_ENABLED").unwrap_or_else(|_| "1".into()) == "0" {
        return false;
    }
    if std::env::var("EDDA_LLM_API_KEY")
        .unwrap_or_default()
        .is_empty()
    {
        return false;
    }

    let signal_count = crate::state::read_counter(project_id, session_id, "signal_count");
    if signal_count == 0 {
        return false;
    }

    if already_extracted(project_id, session_id) {
        return false;
    }

    check_daily_budget(project_id).unwrap_or(true)
}

/// Main extraction entry point — called from a background thread.
///
/// Reads the stored transcript, calls the LLM, saves draft decisions,
/// updates extraction state and daily cost tracking.
pub fn run_extraction(project_id: &str, session_id: &str) -> Result<()> {
    let api_key = std::env::var("EDDA_LLM_API_KEY").with_context(|| "EDDA_LLM_API_KEY not set")?;

    if api_key.is_empty() {
        anyhow::bail!("EDDA_LLM_API_KEY is empty");
    }

    let result = extract_decisions(project_id, session_id, &api_key)?;

    // Save extraction state (idempotent marker)
    save_extraction_state(project_id, &result)?;

    // Update daily cost
    update_daily_cost(project_id, result.cost_usd)?;

    // Save draft decisions (only those above confidence threshold)
    let threshold = env_f64("EDDA_BG_CONFIDENCE_THRESHOLD", DEFAULT_CONFIDENCE_THRESHOLD);
    let filtered: Vec<ExtractedDecision> = result
        .decisions
        .into_iter()
        .filter(|d| d.confidence >= threshold)
        .collect();

    if !filtered.is_empty() {
        save_draft_decisions(project_id, session_id, &result.model_used, &filtered)?;
    }

    // Append audit log
    append_audit_log(
        project_id,
        &AuditEntry {
            ts: now_rfc3339(),
            session_id: session_id.to_string(),
            decisions_found: filtered.len(),
            cost_usd: result.cost_usd,
            model: result.model_used,
            status: "completed".to_string(),
        },
    )?;

    Ok(())
}

/// Extract decisions from a session transcript via LLM.
pub fn extract_decisions(
    project_id: &str,
    session_id: &str,
    api_key: &str,
) -> Result<ExtractionResult> {
    let transcript_path = edda_store::project_dir(project_id)
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    if !transcript_path.exists() {
        anyhow::bail!("Transcript not found: {}", transcript_path.display());
    }

    // Read and assemble transcript turns
    let transcript_text = read_transcript_turns(&transcript_path)?;

    // Compute transcript hash for idempotency
    let transcript_hash = compute_file_hash(&transcript_path)?;

    // Check idempotency by hash
    if let Some(state) = load_extraction_state(project_id, session_id) {
        if state.transcript_hash == transcript_hash && state.status == "completed" {
            return Ok(ExtractionResult {
                session_id: session_id.to_string(),
                decisions: Vec::new(),
                transcript_hash,
                extracted_at: state.extracted_at,
                model_used: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            });
        }
    }

    // Truncate transcript text
    let max_chars = env_usize("EDDA_BG_MAX_TRANSCRIPT_CHARS", DEFAULT_MAX_TRANSCRIPT_CHARS);
    let truncated = truncate_text(&transcript_text, max_chars);

    // Find recorded decisions with vague reasons for enhancement
    let recorded = extract_recorded_decisions_from_transcript(&transcript_path);
    let vague: Vec<RecordedDecision> = recorded
        .into_iter()
        .filter(|d| is_vague_reason(d.reason.as_deref()))
        .collect();

    // Build prompt (includes enhancement section if vague decisions found)
    let prompt = build_extraction_prompt(&truncated, &vague);

    // Call LLM
    let model = std::env::var("EDDA_BG_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let (response_text, input_tokens, output_tokens) =
        call_anthropic_sync(api_key, &model, &prompt)?;

    // Parse LLM output
    let decisions = parse_llm_decisions(&response_text);

    let cost_usd = (input_tokens as f64 * HAIKU_INPUT_COST_PER_TOKEN)
        + (output_tokens as f64 * HAIKU_OUTPUT_COST_PER_TOKEN);

    Ok(ExtractionResult {
        session_id: session_id.to_string(),
        decisions,
        transcript_hash,
        extracted_at: now_rfc3339(),
        model_used: model,
        input_tokens,
        output_tokens,
        cost_usd,
    })
}

// ── Review API (used by CLI) ──

/// List all sessions that have pending draft decisions.
pub fn list_pending_drafts(project_id: &str) -> Result<Vec<DraftDecisionFile>> {
    let dir = draft_decisions_dir(project_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        let draft: DraftDecisionFile = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse draft: {}", path.display()))?;

        // Only include if there are pending decisions
        if draft
            .decisions
            .iter()
            .any(|d| d.status == DraftStatus::Pending)
        {
            results.push(draft);
        }
    }

    Ok(results)
}

/// Accept specific decisions by index for a session.
pub fn accept_decisions(
    project_id: &str,
    session_id: &str,
    indices: &[usize],
) -> Result<Vec<ExtractedDecision>> {
    let path = draft_decision_path(project_id, session_id);
    if !path.exists() {
        anyhow::bail!("No draft decisions for session {session_id}");
    }

    let content = fs::read_to_string(&path)?;
    let mut draft: DraftDecisionFile = serde_json::from_str(&content)?;

    let mut accepted = Vec::new();
    for &idx in indices {
        if idx < draft.decisions.len() && draft.decisions[idx].status == DraftStatus::Pending {
            draft.decisions[idx].status = DraftStatus::Accepted;
            accepted.push(draft.decisions[idx].clone());
        }
    }

    // Save updated draft
    let json = serde_json::to_string_pretty(&draft)?;
    fs::write(&path, json)?;

    Ok(accepted)
}

/// Accept all pending decisions for a session.
pub fn accept_all_decisions(project_id: &str, session_id: &str) -> Result<Vec<ExtractedDecision>> {
    let path = draft_decision_path(project_id, session_id);
    if !path.exists() {
        anyhow::bail!("No draft decisions for session {session_id}");
    }

    let content = fs::read_to_string(&path)?;
    let draft: DraftDecisionFile = serde_json::from_str(&content)?;

    let indices: Vec<usize> = draft
        .decisions
        .iter()
        .enumerate()
        .filter(|(_, d)| d.status == DraftStatus::Pending)
        .map(|(i, _)| i)
        .collect();

    accept_decisions(project_id, session_id, &indices)
}

/// Reject specific decisions by index for a session.
pub fn reject_decisions(project_id: &str, session_id: &str, indices: &[usize]) -> Result<()> {
    let path = draft_decision_path(project_id, session_id);
    if !path.exists() {
        anyhow::bail!("No draft decisions for session {session_id}");
    }

    let content = fs::read_to_string(&path)?;
    let mut draft: DraftDecisionFile = serde_json::from_str(&content)?;

    for &idx in indices {
        if idx < draft.decisions.len() && draft.decisions[idx].status == DraftStatus::Pending {
            draft.decisions[idx].status = DraftStatus::Rejected;
        }
    }

    let json = serde_json::to_string_pretty(&draft)?;
    fs::write(&path, json)?;

    Ok(())
}

// ── Internal Helpers ──

fn state_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id).join("state")
}

fn draft_decisions_dir(project_id: &str) -> PathBuf {
    state_dir(project_id).join("draft_decisions")
}

fn draft_decision_path(project_id: &str, session_id: &str) -> PathBuf {
    draft_decisions_dir(project_id).join(format!("{session_id}.json"))
}

fn extraction_state_path(project_id: &str, session_id: &str) -> PathBuf {
    state_dir(project_id).join(format!("bg_extract.{session_id}.json"))
}

fn daily_cost_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_daily_cost.json")
}

fn audit_log_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_audit.jsonl")
}

fn already_extracted(project_id: &str, session_id: &str) -> bool {
    load_extraction_state(project_id, session_id).is_some_and(|s| s.status == "completed")
}

fn load_extraction_state(project_id: &str, session_id: &str) -> Option<ExtractionState> {
    let path = extraction_state_path(project_id, session_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_extraction_state(project_id: &str, result: &ExtractionResult) -> Result<()> {
    let state = ExtractionState {
        status: "completed".to_string(),
        extracted_at: result.extracted_at.clone(),
        transcript_hash: result.transcript_hash.clone(),
        decisions_count: result.decisions.len(),
    };

    let path = extraction_state_path(project_id, &result.session_id);
    fs::create_dir_all(path.parent().context("extraction state path has no parent")?)?;
    let json = serde_json::to_string_pretty(&state)?;
    fs::write(&path, json)?;
    Ok(())
}

pub(crate) fn check_daily_budget(project_id: &str) -> Result<bool> {
    let budget = env_f64("EDDA_BG_DAILY_BUDGET_USD", DEFAULT_DAILY_BUDGET_USD);
    let path = daily_cost_path(project_id);

    if !path.exists() {
        return Ok(true);
    }

    let content = fs::read_to_string(&path)?;
    let cost: DailyCost = serde_json::from_str(&content)?;

    let today = today_date();
    if cost.date != today {
        // New day, budget resets
        return Ok(true);
    }

    Ok(cost.total_usd < budget)
}

pub(crate) fn update_daily_cost(project_id: &str, cost_usd: f64) -> Result<()> {
    let path = daily_cost_path(project_id);
    let today = today_date();

    let mut cost_data = if path.exists() {
        let content = fs::read_to_string(&path)?;
        let existing: DailyCost = serde_json::from_str(&content).unwrap_or(DailyCost {
            date: today.clone(),
            total_usd: 0.0,
            calls: 0,
        });
        if existing.date == today {
            existing
        } else {
            DailyCost {
                date: today,
                total_usd: 0.0,
                calls: 0,
            }
        }
    } else {
        DailyCost {
            date: today,
            total_usd: 0.0,
            calls: 0,
        }
    };

    cost_data.total_usd += cost_usd;
    cost_data.calls += 1;

    fs::create_dir_all(path.parent().context("daily cost path has no parent")?)?;
    let json = serde_json::to_string_pretty(&cost_data)?;
    fs::write(&path, json)?;
    Ok(())
}

fn save_draft_decisions(
    project_id: &str,
    session_id: &str,
    model: &str,
    decisions: &[ExtractedDecision],
) -> Result<()> {
    let draft = DraftDecisionFile {
        session_id: session_id.to_string(),
        extracted_at: now_rfc3339(),
        model: model.to_string(),
        decisions: decisions.to_vec(),
    };

    let path = draft_decision_path(project_id, session_id);
    fs::create_dir_all(path.parent().context("draft decision path has no parent")?)?;
    let json = serde_json::to_string_pretty(&draft)?;
    fs::write(&path, json)?;
    Ok(())
}

fn append_audit_log(project_id: &str, entry: &AuditEntry) -> Result<()> {
    use std::io::Write;
    let path = audit_log_path(project_id);
    fs::create_dir_all(path.parent().context("audit log path has no parent")?)?;
    let line = serde_json::to_string(entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// Read a stored transcript JSONL and assemble user/assistant turns into text.
pub(crate) fn read_transcript_turns(transcript_path: &Path) -> Result<String> {
    let content = fs::read_to_string(transcript_path)
        .with_context(|| format!("Failed to read transcript: {}", transcript_path.display()))?;

    let mut parts = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let role = record.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Only include user/assistant messages
        if role != "human" && role != "assistant" {
            continue;
        }

        let display_role = if role == "human" { "User" } else { "Assistant" };

        // Extract text content from message
        if let Some(msg) = record.get("message") {
            if let Some(content) = msg.get("content") {
                let text = extract_text_from_content(content);
                if !text.is_empty() {
                    parts.push(format!("[{display_role}]: {text}"));
                }
            }
        }
    }

    Ok(parts.join("\n\n"))
}

/// Extract text from Anthropic message content (string or array of content blocks).
fn extract_text_from_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let texts: Vec<String> = arr
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        return texts.join("\n");
    }
    String::new()
}

pub(crate) fn compute_file_hash(path: &Path) -> Result<String> {
    let content = fs::read(path)?;
    let hash = blake3::hash(&content);
    Ok(format!("blake3:{}", hash.to_hex()))
}

pub(crate) fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Keep the last max_chars (more recent turns are more valuable)
    let start = text.len() - max_chars;
    // Find next newline to avoid breaking mid-line
    let start = text[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(start);
    format!("[... transcript truncated ...]\n\n{}", &text[start..])
}

/// A decision recorded by the agent via `edda decide`, parsed from transcript.
#[derive(Debug, Clone)]
struct RecordedDecision {
    key: String,
    value: String,
    reason: Option<String>,
}

/// Returns `true` if the reason is missing, too short, or a generic/vague phrase.
fn is_vague_reason(reason: Option<&str>) -> bool {
    let Some(r) = reason else {
        return true;
    };
    let r = r.trim();
    if r.len() < 15 {
        return true;
    }
    let vague_patterns = [
        "for now",
        "just",
        "simple",
        "easier",
        "because",
        "暫時",
        "先這樣",
        "好了",
        "方便",
    ];
    let lower = r.to_lowercase();
    // Only match if the entire reason is a vague phrase (possibly with trailing period)
    vague_patterns
        .iter()
        .any(|p| lower == *p || lower == format!("{p}."))
}

/// Parse `edda decide "key=value" --reason "reason"` commands from a transcript JSONL file.
fn extract_recorded_decisions_from_transcript(transcript_path: &Path) -> Vec<RecordedDecision> {
    let content = match fs::read_to_string(transcript_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut decisions = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        // Look for tool_input.command containing "edda decide"
        let command = record
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        if !command.contains("edda decide") {
            // Also check in message.content for assistant messages
            let msg_text = record
                .get("message")
                .and_then(|m| m.get("content"))
                .map(extract_text_from_content)
                .unwrap_or_default();
            if !msg_text.contains("edda decide") {
                continue;
            }
            // Try to parse from message text
            if let Some(d) = parse_edda_decide_command(&msg_text) {
                decisions.push(d);
            }
            continue;
        }

        if let Some(d) = parse_edda_decide_command(command) {
            decisions.push(d);
        }
    }

    decisions
}

/// Parse an `edda decide "key=value" --reason "reason"` command string.
fn parse_edda_decide_command(text: &str) -> Option<RecordedDecision> {
    // Find the "edda decide" part and extract arguments after it
    let decide_idx = text.find("edda decide")?;
    let after = &text[decide_idx + "edda decide".len()..];

    // Extract the key=value argument (may be quoted or unquoted)
    let after = after.trim();
    let kv = if let Some(stripped) = after.strip_prefix('"') {
        let end = stripped.find('"')?;
        &stripped[..end]
    } else if let Some(stripped) = after.strip_prefix('\'') {
        let end = stripped.find('\'')?;
        &stripped[..end]
    } else {
        // Unquoted: take until whitespace
        after.split_whitespace().next()?
    };

    let (key, value) = kv.split_once('=')?;

    // Extract --reason
    let reason = if let Some(reason_idx) = after.find("--reason") {
        let after_flag = after[reason_idx + "--reason".len()..].trim();
        if let Some(stripped) = after_flag.strip_prefix('"') {
            let end = stripped.find('"');
            end.map(|e| stripped[..e].to_string())
        } else if let Some(stripped) = after_flag.strip_prefix('\'') {
            let end = stripped.find('\'');
            end.map(|e| stripped[..e].to_string())
        } else {
            // Unquoted: take rest of line
            let val = after_flag.split('\n').next().unwrap_or("").trim();
            if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            }
        }
    } else {
        None
    };

    Some(RecordedDecision {
        key: key.trim().to_string(),
        value: value.trim().to_string(),
        reason,
    })
}

fn build_extraction_prompt(transcript: &str, vague_decisions: &[RecordedDecision]) -> String {
    let enhancement_section = if vague_decisions.is_empty() {
        String::new()
    } else {
        let mut section = String::from(
            r#"

---

## 已記錄的決策（增強模糊 reason）

以下是 agent 在本次對話中記錄的決策，但 reason 缺失或模糊。
請根據 transcript 上下文為每一條提供具體的、有意義的 reason。

輸出時加入 `"kind": "enhancement"` 欄位，以及 `"original_reason"` 保留原始 reason。

"#,
        );
        for d in vague_decisions {
            let reason_display = d.reason.as_deref().unwrap_or("(none)");
            section.push_str(&format!(
                "- `{}={}` — current reason: \"{}\"\n",
                d.key, d.value, reason_display
            ));
        }
        section.push_str(
            r#"
Enhancement 項目格式：
{{
  "kind": "enhancement",
  "key": "domain.aspect",
  "value": "選擇的值",
  "original_reason": "原始的模糊 reason 或 null",
  "reason": "根據對話上下文產生的具體 reason",
  "confidence": 0.0-1.0,
  "evidence": "transcript 中的原文依據（簡短引用）"
}}
"#,
        );
        section
    };

    format!(
        r#"你是決策提取器。分析以下開發對話 transcript，識別架構/技術決策。

決策的特徵：
- 選擇了某個技術、library、模式或策略
- 否決了替代方案
- 定義了規範或約定

每個決策輸出 JSON 格式（以 JSON array 回覆，不要包含其他文字）：
[
  {{
    "key": "domain.aspect",
    "value": "選擇的值",
    "reason": "為什麼這樣選",
    "confidence": 0.0-1.0,
    "evidence": "transcript 中的原文依據（簡短引用）"
  }}
]

不要記錄：
- 格式化改動、typo 修復、重構
- 版本升級（除非換了不同的 library）
- 測試新增（除非是測試策略的改變）
- 暫時性的除錯步驟

如果沒有發現任何決策，回覆空陣列 `[]`。
{enhancement_section}
---

## Transcript

{transcript}"#
    )
}

/// Synchronous Anthropic API call via ureq.
pub(crate) fn call_anthropic_sync(
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<(String, u64, u64)> {
    let request = AnthropicRequest {
        model: model.to_string(),
        max_tokens: 2048,
        messages: vec![ApiMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
    };

    let body = serde_json::to_string(&request)?;

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(API_TIMEOUT_SECS)))
        .build()
        .new_agent();

    let mut response = agent
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send(body)
        .with_context(|| "Anthropic API request failed")?;

    let resp_text = response
        .body_mut()
        .read_to_string()
        .with_context(|| "Failed to read Anthropic response body")?;

    let resp_body: AnthropicResponse = serde_json::from_str(&resp_text)
        .with_context(|| "Failed to parse Anthropic response JSON")?;

    let text = resp_body
        .content
        .first()
        .map(|b| b.text.as_str())
        .unwrap_or("");

    let (input_tokens, output_tokens) = match resp_body.usage {
        Some(u) => (u.input_tokens, u.output_tokens),
        None => (0, 0),
    };

    Ok((text.to_string(), input_tokens, output_tokens))
}

/// Parse LLM output as JSON array of decisions.
pub fn parse_llm_decisions(text: &str) -> Vec<ExtractedDecision> {
    // Try to find JSON array in the response (LLM might wrap it in markdown)
    let json_text = extract_json_array(text);

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parsed
        .into_iter()
        .filter_map(|v| {
            let key = v.get("key")?.as_str()?.to_string();
            let value = v.get("value")?.as_str()?.to_string();
            let reason = v
                .get("reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let confidence = v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.5);
            let evidence = v
                .get("evidence")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            let source_turn = v.get("source_turn").and_then(|t| t.as_u64()).unwrap_or(0) as usize;

            let kind = v
                .get("kind")
                .and_then(|k| k.as_str())
                .map(|k| match k {
                    "enhancement" => DecisionKind::Enhancement,
                    _ => DecisionKind::Extraction,
                })
                .unwrap_or(DecisionKind::Extraction);

            let original_reason = v
                .get("original_reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());

            Some(ExtractedDecision {
                key,
                value,
                reason,
                confidence,
                evidence,
                source_turn,
                status: DraftStatus::Pending,
                kind,
                original_reason,
            })
        })
        .collect()
}

/// Extract JSON array from possibly markdown-wrapped LLM output.
fn extract_json_array(text: &str) -> String {
    let trimmed = text.trim();

    // Direct JSON array
    if trimmed.starts_with('[') {
        return trimmed.to_string();
    }

    // Try to find JSON in code block
    if let Some(start) = trimmed.find("```json") {
        let rest = &trimmed[start + 7..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        if let Some(end) = rest.find("```") {
            let inner = rest[..end].trim();
            if inner.starts_with('[') {
                return inner.to_string();
            }
        }
    }

    // Try to find a bare [ ... ] in the text
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            if end > start {
                return trimmed[start..=end].to_string();
            }
        }
    }

    trimmed.to_string()
}

pub(crate) fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn today_date() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day()
    )
}

pub(crate) fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_llm_output_valid_json() {
        let input = r#"[
            {
                "key": "db.engine",
                "value": "sqlite",
                "reason": "embedded, zero-config",
                "confidence": 0.92,
                "evidence": "用 SQLite 就好"
            },
            {
                "key": "auth.method",
                "value": "JWT",
                "reason": "stateless",
                "confidence": 0.85,
                "evidence": "用 JWT RS256"
            }
        ]"#;

        let decisions = parse_llm_decisions(input);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].key, "db.engine");
        assert_eq!(decisions[0].value, "sqlite");
        assert_eq!(
            decisions[0].reason.as_deref(),
            Some("embedded, zero-config")
        );
        assert!((decisions[0].confidence - 0.92).abs() < 0.001);
        assert_eq!(decisions[1].key, "auth.method");
    }

    #[test]
    fn test_parse_llm_output_markdown_wrapped() {
        let input = r#"Here are the decisions I found:

```json
[{"key": "api.framework", "value": "axum", "reason": "async Rust", "confidence": 0.9, "evidence": "chose axum"}]
```

That's it."#;

        let decisions = parse_llm_decisions(input);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].key, "api.framework");
    }

    #[test]
    fn test_parse_llm_output_empty_array() {
        let decisions = parse_llm_decisions("[]");
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_parse_llm_output_garbage() {
        let decisions = parse_llm_decisions("I couldn't find any decisions.");
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_parse_llm_output_missing_fields() {
        let input = r#"[{"key": "db", "value": "pg"}]"#;
        let decisions = parse_llm_decisions(input);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].key, "db");
        assert!(decisions[0].reason.is_none());
        assert!((decisions[0].confidence - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_truncate_text_within_limit() {
        let text = "short text";
        assert_eq!(truncate_text(text, 100), "short text");
    }

    #[test]
    fn test_truncate_text_over_limit() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_text(text, 15);
        assert!(result.contains("[... transcript truncated ...]"));
        assert!(result.contains("line5"));
    }

    #[test]
    fn test_extract_json_array_bare() {
        assert_eq!(extract_json_array("  [1,2,3]  "), "[1,2,3]");
    }

    #[test]
    fn test_extract_json_array_in_codeblock() {
        let input = "```json\n[1,2]\n```";
        assert_eq!(extract_json_array(input), "[1,2]");
    }

    #[test]
    fn test_daily_cost_tracking() {
        let today = today_date();
        let cost = DailyCost {
            date: today.clone(),
            total_usd: 0.10,
            calls: 5,
        };
        let json = serde_json::to_string_pretty(&cost).unwrap();
        let loaded: DailyCost = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.date, today);
        assert!((loaded.total_usd - 0.10).abs() < 0.001);
        assert_eq!(loaded.calls, 5);
    }

    #[test]
    fn test_draft_status_serde() {
        let draft = ExtractedDecision {
            key: "test.key".to_string(),
            value: "test_value".to_string(),
            reason: Some("because".to_string()),
            confidence: 0.8,
            evidence: "evidence".to_string(),
            source_turn: 5,
            status: DraftStatus::Pending,
            kind: DecisionKind::Extraction,
            original_reason: None,
        };

        let json = serde_json::to_string(&draft).unwrap();
        let parsed: ExtractedDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, DraftStatus::Pending);
        assert_eq!(parsed.key, "test.key");
        assert_eq!(parsed.kind, DecisionKind::Extraction);
        assert!(parsed.original_reason.is_none());
    }

    #[test]
    fn test_extraction_state_serde() {
        let state = ExtractionState {
            status: "completed".to_string(),
            extracted_at: "2026-03-11T10:00:00Z".to_string(),
            transcript_hash: "blake3:abc123".to_string(),
            decisions_count: 3,
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: ExtractionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, "completed");
        assert_eq!(parsed.decisions_count, 3);
    }

    #[test]
    fn test_build_extraction_prompt() {
        let prompt = build_extraction_prompt("test transcript", &[]);
        assert!(prompt.contains("決策提取器"));
        assert!(prompt.contains("test transcript"));
        assert!(prompt.contains("JSON"));
        // No enhancement section when no vague decisions
        assert!(!prompt.contains("增強模糊"));
    }

    #[test]
    fn test_extract_text_from_content_string() {
        let content = serde_json::json!("hello world");
        assert_eq!(extract_text_from_content(&content), "hello world");
    }

    #[test]
    fn test_extract_text_from_content_blocks() {
        let content = serde_json::json!([
            {"type": "text", "text": "part 1"},
            {"type": "tool_use", "name": "grep"},
            {"type": "text", "text": "part 2"}
        ]);
        let result = extract_text_from_content(&content);
        assert!(result.contains("part 1"));
        assert!(result.contains("part 2"));
        assert!(!result.contains("grep"));
    }

    // ── Decision Reason Quality Enhancement Tests (#194) ──

    #[test]
    fn test_is_vague_reason_none() {
        assert!(is_vague_reason(None));
    }

    #[test]
    fn test_is_vague_reason_short() {
        assert!(is_vague_reason(Some("ok")));
        assert!(is_vague_reason(Some("yes")));
        assert!(is_vague_reason(Some("   short   "))); // trimmed < 15
    }

    #[test]
    fn test_is_vague_reason_exact_match() {
        assert!(is_vague_reason(Some("for now")));
        assert!(is_vague_reason(Some("just")));
        assert!(is_vague_reason(Some("simple")));
        assert!(is_vague_reason(Some("easier")));
        assert!(is_vague_reason(Some("because")));
        assert!(is_vague_reason(Some("for now.")));
        assert!(is_vague_reason(Some("暫時")));
        assert!(is_vague_reason(Some("先這樣")));
        assert!(is_vague_reason(Some("好了")));
        assert!(is_vague_reason(Some("方便")));
    }

    #[test]
    fn test_is_vague_reason_good() {
        assert!(!is_vague_reason(Some("embedded, zero-config for MVP")));
        assert!(!is_vague_reason(Some("stateless, scales horizontally")));
    }

    #[test]
    fn test_is_vague_reason_threshold() {
        // Exactly 15 chars → not vague
        assert!(!is_vague_reason(Some("123456789012345")));
        // 14 chars → vague (too short)
        assert!(is_vague_reason(Some("12345678901234")));
    }

    #[test]
    fn test_is_vague_reason_contains_vague_word_but_longer() {
        // "just" appears as substring but the whole reason is long and specific
        assert!(!is_vague_reason(Some(
            "just because it supports async well and is production-ready"
        )));
    }

    #[test]
    fn test_decision_kind_serde() {
        // Round-trip for Extraction
        let json = serde_json::to_string(&DecisionKind::Extraction).unwrap();
        assert_eq!(json, r#""extraction""#);
        let parsed: DecisionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DecisionKind::Extraction);

        // Round-trip for Enhancement
        let json = serde_json::to_string(&DecisionKind::Enhancement).unwrap();
        assert_eq!(json, r#""enhancement""#);
        let parsed: DecisionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DecisionKind::Enhancement);
    }

    #[test]
    fn test_backward_compat_no_kind() {
        // Existing JSON without `kind` or `original_reason` should deserialize
        let json = r#"{
            "key": "db.engine",
            "value": "sqlite",
            "reason": "embedded",
            "confidence": 0.9,
            "evidence": "some quote",
            "source_turn": 3,
            "status": "pending"
        }"#;
        let parsed: ExtractedDecision = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.kind, DecisionKind::Extraction);
        assert!(parsed.original_reason.is_none());
    }

    #[test]
    fn test_parse_enhancement_output() {
        let input = r#"[{
            "kind": "enhancement",
            "key": "db.engine",
            "value": "sqlite",
            "original_reason": "for now",
            "reason": "SQLite chosen for MVP — embedded, zero external deps",
            "confidence": 0.85,
            "evidence": "用戶說先用 SQLite"
        }]"#;

        let decisions = parse_llm_decisions(input);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, DecisionKind::Enhancement);
        assert_eq!(decisions[0].original_reason.as_deref(), Some("for now"));
        assert!(decisions[0].reason.as_ref().unwrap().contains("SQLite"));
    }

    #[test]
    fn test_parse_mixed_output() {
        let input = r#"[
            {
                "kind": "extraction",
                "key": "api.framework",
                "value": "axum",
                "reason": "async Rust",
                "confidence": 0.9,
                "evidence": "chose axum"
            },
            {
                "kind": "enhancement",
                "key": "db.engine",
                "value": "sqlite",
                "original_reason": "for now",
                "reason": "embedded, zero-config for MVP phase",
                "confidence": 0.85,
                "evidence": "discussed sqlite"
            }
        ]"#;

        let decisions = parse_llm_decisions(input);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].kind, DecisionKind::Extraction);
        assert!(decisions[0].original_reason.is_none());
        assert_eq!(decisions[1].kind, DecisionKind::Enhancement);
        assert_eq!(decisions[1].original_reason.as_deref(), Some("for now"));
    }

    #[test]
    fn test_prompt_with_vague_decisions() {
        let vague = vec![
            RecordedDecision {
                key: "db.engine".to_string(),
                value: "sqlite".to_string(),
                reason: Some("for now".to_string()),
            },
            RecordedDecision {
                key: "auth.method".to_string(),
                value: "JWT".to_string(),
                reason: None,
            },
        ];
        let prompt = build_extraction_prompt("test transcript", &vague);
        assert!(prompt.contains("增強模糊"));
        assert!(prompt.contains("db.engine"));
        assert!(prompt.contains("auth.method"));
        assert!(prompt.contains("(none)"));
        assert!(prompt.contains("enhancement"));
    }

    #[test]
    fn test_prompt_without_vague_decisions() {
        let prompt = build_extraction_prompt("test transcript", &[]);
        assert!(!prompt.contains("增強模糊"));
        assert!(!prompt.contains("enhancement"));
    }

    #[test]
    fn test_parse_edda_decide_command_double_quoted() {
        let cmd = r#"edda decide "db.engine=sqlite" --reason "embedded, zero-config""#;
        let d = parse_edda_decide_command(cmd).unwrap();
        assert_eq!(d.key, "db.engine");
        assert_eq!(d.value, "sqlite");
        assert_eq!(d.reason.as_deref(), Some("embedded, zero-config"));
    }

    #[test]
    fn test_parse_edda_decide_command_single_quoted() {
        let cmd = "edda decide 'auth.method=JWT' --reason 'stateless'";
        let d = parse_edda_decide_command(cmd).unwrap();
        assert_eq!(d.key, "auth.method");
        assert_eq!(d.value, "JWT");
        assert_eq!(d.reason.as_deref(), Some("stateless"));
    }

    #[test]
    fn test_parse_edda_decide_command_no_reason() {
        let cmd = r#"edda decide "cache.strategy=redis""#;
        let d = parse_edda_decide_command(cmd).unwrap();
        assert_eq!(d.key, "cache.strategy");
        assert_eq!(d.value, "redis");
        assert!(d.reason.is_none());
    }

    #[test]
    fn test_parse_edda_decide_command_not_found() {
        let cmd = "cargo build --release";
        assert!(parse_edda_decide_command(cmd).is_none());
    }

    #[test]
    fn test_extract_recorded_decisions_from_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");

        // Write a transcript with edda decide commands
        let lines = [
            r#"{"type":"assistant","message":{"content":"Let me record this."},"tool_input":{"command":"edda decide \"db.engine=sqlite\" --reason \"for now\""}}"#,
            r#"{"type":"human","message":{"content":"ok"}}"#,
            r#"{"type":"assistant","message":{"content":"Another decision."},"tool_input":{"command":"edda decide \"auth.method=JWT\""}}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        let decisions = extract_recorded_decisions_from_transcript(&path);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].key, "db.engine");
        assert_eq!(decisions[0].value, "sqlite");
        assert_eq!(decisions[0].reason.as_deref(), Some("for now"));
        assert_eq!(decisions[1].key, "auth.method");
        assert_eq!(decisions[1].value, "JWT");
        assert!(decisions[1].reason.is_none());
    }
}
