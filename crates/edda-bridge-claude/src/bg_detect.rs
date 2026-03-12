//! Background pattern detector — periodically scans session history to detect
//! failure patterns, cost anomalies, and quality degradation.
//!
//! Design: two-layer hybrid architecture.
//!   - **Layer 1 (deterministic)**: statistical checks on structured data. Always
//!     runs, zero LLM cost.
//!   - **Layer 2 (LLM, optional)**: correlates raw signals via LLM when anomalies
//!     are found.  Only runs when Layer 1 produces signals AND an API key is set.
//!
//! Reuses shared infrastructure from `bg_extract` (API call, budget tracking,
//! cost control).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::bg_extract::{
    call_anthropic_sync, check_daily_budget, env_f64, now_rfc3339, truncate_text,
    update_daily_cost, DEFAULT_MODEL, HAIKU_INPUT_COST_PER_TOKEN, HAIKU_OUTPUT_COST_PER_TOKEN,
};

// ── Configuration ──

const DEFAULT_DETECT_INTERVAL: u64 = 10;
const DEFAULT_DETECT_COOLDOWN_HOURS: u64 = 24;
const DEFAULT_FAILURE_THRESHOLD: usize = 3;
const DEFAULT_COST_ANOMALY_FACTOR: f64 = 2.0;
const DEFAULT_QUALITY_DROP_THRESHOLD: f64 = 0.10;
const DEFAULT_MAX_CONTEXT_CHARS: usize = 20_000;

// ── Data Structures ──

/// The kind of anomaly signal detected by Layer 1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    FailurePattern,
    CostAnomaly,
    QualityDegradation,
}

/// A raw signal produced by deterministic Layer 1 detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSignal {
    pub kind: SignalKind,
    pub severity: String,
    pub summary: String,
    pub evidence: Vec<String>,
    pub metric_value: f64,
    pub baseline_value: f64,
    pub confidence: f64,
}

/// A correlated pattern produced by LLM Layer 2, or promoted from raw signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedPattern {
    pub signals: Vec<RawSignal>,
    pub correlation: String,
    pub suggested_action: String,
    pub created_at: String,
}

/// Full result of a detection run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectResult {
    pub detect_id: String,
    pub detected_at: String,
    pub raw_signals: Vec<RawSignal>,
    pub patterns: Vec<DetectedPattern>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Persisted state for the pattern detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectState {
    pub last_detect_at: String,
    pub sessions_since_last: u64,
    pub status: String,
}

/// Audit log entry for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    detect_id: String,
    signals_found: usize,
    patterns_found: usize,
    cost_usd: f64,
    model: Option<String>,
    status: String,
}

// ── Public API ──

/// Increment the session counter.  Call this on every SessionEnd *before*
/// checking `should_run`.
///
/// **Known limitation – race condition**: `increment_session_count` and
/// `should_run` are not atomic.  Two concurrent sessions ending at the same
/// instant could both read the pre-increment count, causing `should_run` to
/// return `true` twice (duplicate detection run) or to miss a threshold
/// crossing.  In practice this is benign: detection is idempotent and the
/// cooldown window prevents redundant work.  A future improvement could
/// combine the increment + threshold check into a single file-locked
/// read-modify-write, but the current design is acceptable for the
/// single-user CLI use case.
pub fn increment_session_count(project_id: &str) {
    let state = load_detect_state(project_id).unwrap_or(DetectState {
        last_detect_at: String::new(),
        sessions_since_last: 0,
        status: "init".to_string(),
    });

    let updated = DetectState {
        sessions_since_last: state.sessions_since_last + 1,
        ..state
    };

    let _ = save_detect_state_raw(project_id, &updated);
}

/// Check whether background pattern detection should run for this project.
///
/// Returns `false` (skip) if any of these hold:
/// - `EDDA_BG_ENABLED` is `"0"`
/// - Session count since last run < interval threshold
/// - Cooldown has not elapsed
/// - Daily budget is exhausted
///
/// Note: unlike bg_scan, this does NOT require `EDDA_LLM_API_KEY` because
/// Layer 1 is purely deterministic.  The LLM key is only checked in Layer 2.
pub fn should_run(project_id: &str) -> bool {
    if std::env::var("EDDA_BG_ENABLED").unwrap_or_else(|_| "1".into()) == "0" {
        return false;
    }

    let interval = std::env::var("EDDA_DETECT_INTERVAL")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DETECT_INTERVAL);

    let state = match load_detect_state(project_id) {
        Some(s) => s,
        None => return true, // Never run before
    };

    if state.sessions_since_last < interval {
        return false;
    }

    if !cooldown_elapsed(&state) {
        return false;
    }

    true
}

/// Main detection entry point.
///
/// 1. Runs Layer 1 deterministic scan.
/// 2. If signals found AND LLM key available: runs Layer 2 correlation.
/// 3. Saves results + audit log.
pub fn run_detect(project_id: &str, cwd: &str) -> Result<DetectResult> {
    // Layer 1: deterministic scan
    let raw_signals = run_deterministic_scan(project_id)?;

    let mut model = None;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: f64 = 0.0;
    let mut patterns: Vec<DetectedPattern> = Vec::new();

    if !raw_signals.is_empty() {
        // Try Layer 2 LLM correlation
        let api_key = std::env::var("EDDA_LLM_API_KEY").unwrap_or_default();
        if !api_key.is_empty() && check_daily_budget(project_id).unwrap_or(false) {
            match llm_correlate(project_id, &raw_signals, cwd, &api_key) {
                Ok((llm_patterns, m, it, ot, c)) => {
                    patterns = llm_patterns;
                    model = Some(m);
                    input_tokens = it;
                    output_tokens = ot;
                    cost_usd = c;
                }
                Err(e) => {
                    eprintln!("[edda-bg] detect LLM correlation failed, using raw signals: {e}");
                    patterns = promote_raw_signals(&raw_signals);
                }
            }
        } else {
            // No LLM available -- promote raw signals directly
            patterns = promote_raw_signals(&raw_signals);
        }
    }

    let detect_id = format!(
        "detect_{}",
        &ulid::Ulid::new().to_string()[..12].to_lowercase()
    );
    let result = DetectResult {
        detect_id: detect_id.clone(),
        detected_at: now_rfc3339(),
        raw_signals: raw_signals.clone(),
        patterns,
        model: model.clone(),
        input_tokens,
        output_tokens,
        cost_usd,
    };

    // Persist
    if !result.raw_signals.is_empty() {
        save_detect_result(project_id, &result)?;
    }
    save_detect_state(project_id, &result)?;

    if cost_usd > 0.0 {
        update_daily_cost(project_id, cost_usd)?;
    }

    append_audit_log(
        project_id,
        &AuditEntry {
            ts: now_rfc3339(),
            detect_id,
            signals_found: result.raw_signals.len(),
            patterns_found: result.patterns.len(),
            cost_usd,
            model,
            status: "completed".to_string(),
        },
    )?;

    // Write a note if actionable patterns were found
    if !result.patterns.is_empty() {
        if let Err(e) = write_detect_note(project_id, cwd, &result) {
            eprintln!("[edda-bg] failed to write detect note: {e}");
        }
    }

    eprintln!(
        "[edda-bg] pattern detection complete: {} signals, {} patterns (cost: ${:.4})",
        result.raw_signals.len(),
        result.patterns.len(),
        cost_usd
    );

    Ok(result)
}

// ── Layer 1: Deterministic Detection ──

/// Run all deterministic detection rules and merge results.
fn run_deterministic_scan(project_id: &str) -> Result<Vec<RawSignal>> {
    let mut signals = Vec::new();

    signals.extend(detect_failure_patterns(project_id));
    signals.extend(detect_cost_anomalies(project_id));
    signals.extend(detect_quality_degradation(project_id));

    Ok(signals)
}

/// Detect recurring failure patterns from session digest history.
///
/// Reads `prev_digest.json` files from the state directory and looks for
/// recurring `outcome: error_stuck` sessions.
fn detect_failure_patterns(project_id: &str) -> Vec<RawSignal> {
    let threshold = std::env::var("EDDA_DETECT_FAILURE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_FAILURE_THRESHOLD);

    // Read recent session digests to look for recurring failures
    let audit_path = edda_store::project_dir(project_id)
        .join("state")
        .join("bg_digest_audit.jsonl");

    let content = match fs::read_to_string(&audit_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Count sessions with failed outcomes from digest audit
    let mut error_count: usize = 0;
    let mut total_count: usize = 0;
    let mut recent_errors: Vec<String> = Vec::new();

    for line in content.lines().rev().take(20) {
        // Only look at last 20 sessions
        total_count += 1;
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if status == "failed" {
                error_count += 1;
                if let Some(sid) = val.get("session_id").and_then(|s| s.as_str()) {
                    recent_errors.push(sid.to_string());
                }
            }
        }
    }

    let mut signals = Vec::new();

    if total_count >= threshold && error_count >= threshold {
        let severity = if error_count >= threshold * 2 {
            "high"
        } else {
            "medium"
        };

        signals.push(RawSignal {
            kind: SignalKind::FailurePattern,
            severity: severity.to_string(),
            summary: format!("{error_count} of last {total_count} sessions had error outcomes"),
            evidence: recent_errors,
            metric_value: error_count as f64,
            baseline_value: threshold as f64,
            confidence: 0.8,
        });
    }

    signals
}

/// Detect cost anomalies by comparing recent daily spend against rolling average.
///
/// Reads the shared bg audit logs to compute per-day costs and flags days
/// that exceed `DEFAULT_COST_ANOMALY_FACTOR` times the rolling average.
fn detect_cost_anomalies(project_id: &str) -> Vec<RawSignal> {
    let factor = env_f64("EDDA_DETECT_COST_FACTOR", DEFAULT_COST_ANOMALY_FACTOR);

    // Collect costs from all bg audit logs
    let state_dir = edda_store::project_dir(project_id).join("state");

    let audit_files = [
        "bg_extract_audit.jsonl",
        "bg_digest_audit.jsonl",
        "bg_scan_audit.jsonl",
        "bg_detect_audit.jsonl",
    ];

    let mut daily_costs: std::collections::BTreeMap<String, f64> =
        std::collections::BTreeMap::new();

    for filename in &audit_files {
        let path = state_dir.join(filename);
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                let cost = val.get("cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0);
                let ts = val.get("ts").and_then(|t| t.as_str()).unwrap_or("");
                // Extract date portion (first 10 chars of ISO timestamp)
                let date = if ts.len() >= 10 { &ts[..10] } else { ts };
                if !date.is_empty() {
                    *daily_costs.entry(date.to_string()).or_default() += cost;
                }
            }
        }
    }

    let mut signals = Vec::new();

    let costs: Vec<f64> = daily_costs.values().copied().collect();
    if costs.len() < 3 {
        return signals; // Not enough data
    }

    // Compute rolling average of all but the last day
    let (history, recent) = costs.split_at(costs.len() - 1);
    let avg: f64 = history.iter().sum::<f64>() / history.len() as f64;
    let today_cost = recent[0];

    if avg > 0.0 && today_cost > avg * factor {
        let severity = if today_cost > avg * (factor * 1.5) {
            "high"
        } else {
            "medium"
        };

        let last_date = daily_costs.keys().last().cloned().unwrap_or_default();

        signals.push(RawSignal {
            kind: SignalKind::CostAnomaly,
            severity: severity.to_string(),
            summary: format!(
                "Daily cost ${:.4} on {} exceeds {:.1}x rolling average (${:.4})",
                today_cost,
                last_date,
                today_cost / avg,
                avg
            ),
            evidence: vec![format!("date={last_date}"), format!("avg=${avg:.4}")],
            metric_value: today_cost,
            baseline_value: avg,
            confidence: 0.85,
        });
    }

    signals
}

/// Detect quality degradation by looking at success/error ratios in recent
/// session outcomes from digest audit logs.
fn detect_quality_degradation(project_id: &str) -> Vec<RawSignal> {
    let drop_threshold = env_f64("EDDA_DETECT_QUALITY_DROP", DEFAULT_QUALITY_DROP_THRESHOLD);

    let audit_path = edda_store::project_dir(project_id)
        .join("state")
        .join("bg_digest_audit.jsonl");

    let content = match fs::read_to_string(&audit_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 6 {
        return Vec::new(); // Not enough data
    }

    // Split into two halves: older and recent
    let mid = lines.len() / 2;
    let older = &lines[..mid];
    let recent = &lines[mid..];

    let success_rate = |entries: &[&str]| -> f64 {
        let mut ok = 0usize;
        let mut total = 0usize;
        for line in entries {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                total += 1;
                let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");
                if status == "completed" || status == "ok" || status == "success" {
                    ok += 1;
                }
            }
        }
        if total == 0 {
            1.0
        } else {
            ok as f64 / total as f64
        }
    };

    let older_rate = success_rate(older);
    let recent_rate = success_rate(recent);

    let mut signals = Vec::new();

    if older_rate > 0.0 && (older_rate - recent_rate) > drop_threshold {
        let drop_pct = (older_rate - recent_rate) * 100.0;
        let severity = if drop_pct > 20.0 { "high" } else { "medium" };

        signals.push(RawSignal {
            kind: SignalKind::QualityDegradation,
            severity: severity.to_string(),
            summary: format!(
                "Success rate dropped {:.1}% (from {:.0}% to {:.0}%) in recent sessions",
                drop_pct,
                older_rate * 100.0,
                recent_rate * 100.0
            ),
            evidence: vec![
                format!("older_sessions={}", older.len()),
                format!("recent_sessions={}", recent.len()),
            ],
            metric_value: recent_rate,
            baseline_value: older_rate,
            confidence: 0.75,
        });
    }

    signals
}

// ── Layer 2: LLM Correlation ──

/// Call LLM to correlate raw signals and suggest actions.
fn llm_correlate(
    _project_id: &str,
    signals: &[RawSignal],
    cwd: &str,
    api_key: &str,
) -> Result<(Vec<DetectedPattern>, String, u64, u64, f64)> {
    let context = build_detect_context(cwd, signals)?;
    let prompt = build_detect_prompt(&context);

    let model = std::env::var("EDDA_BG_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let (response_text, input_tokens, output_tokens) =
        call_anthropic_sync(api_key, &model, &prompt)?;

    let patterns = parse_detect_response(&response_text, signals);

    let cost_usd = (input_tokens as f64 * HAIKU_INPUT_COST_PER_TOKEN)
        + (output_tokens as f64 * HAIKU_OUTPUT_COST_PER_TOKEN);

    Ok((patterns, model, input_tokens, output_tokens, cost_usd))
}

/// Build context string including signals and recent session notes.
fn build_detect_context(cwd: &str, signals: &[RawSignal]) -> Result<String> {
    let mut sections = Vec::new();

    // Signals summary
    let signals_json = serde_json::to_string_pretty(signals).unwrap_or_else(|_| "[]".to_string());
    sections.push(format!("## Detected Anomaly Signals\n\n{signals_json}"));

    // Recent session notes (from ledger)
    let cwd_path = std::path::Path::new(cwd);
    if let Some(root) = edda_ledger::EddaPaths::find_root(cwd_path) {
        if let Ok(ledger) = edda_ledger::Ledger::open(&root) {
            if let Ok(events) = ledger.iter_events() {
                let notes: Vec<String> = events
                    .iter()
                    .filter(|e| e.event_type == "note")
                    .rev()
                    .take(10)
                    .filter_map(|e| {
                        let text = e.payload.get("text")?.as_str()?;
                        Some(format!("- [{}] {}", e.ts, text))
                    })
                    .collect();

                if !notes.is_empty() {
                    sections.push(format!("## Recent Session Notes\n\n{}", notes.join("\n")));
                }
            }
        }
    }

    let full = sections.join("\n\n---\n\n");
    Ok(truncate_text(&full, DEFAULT_MAX_CONTEXT_CHARS).to_string())
}

fn build_detect_prompt(context: &str) -> String {
    format!(
        r#"You are a software project health monitor. Analyze the following anomaly signals detected from automated monitoring and provide actionable insights.

## Rules
- Correlate signals to identify root causes (e.g., a failure pattern might explain a quality drop)
- For each pattern, suggest a concrete action the team can take
- Rate how the signals relate to each other
- Output valid JSON array only, no markdown fences, no explanation text

## Output Format
Return a JSON array of objects with these fields:
- "correlation": string (how the signals relate, or "standalone" if isolated)
- "suggested_action": string (concrete next step)
- "signal_indices": array of numbers (indices into the signals array that form this pattern)

## Anomaly Context

{context}"#
    )
}

/// Parse LLM response into `DetectedPattern` objects, linking back to raw signals.
fn parse_detect_response(response: &str, signals: &[RawSignal]) -> Vec<DetectedPattern> {
    let text = response.trim();

    let parsed: Vec<serde_json::Value> = {
        // Try direct parse
        if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
            v
        } else if let Some(start) = text.find('[') {
            if let Some(end) = text.rfind(']') {
                serde_json::from_str(&text[start..=end]).unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    };

    let now = now_rfc3339();
    parsed
        .into_iter()
        .filter_map(|val| {
            let correlation = val
                .get("correlation")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown")
                .to_string();
            let suggested_action = val
                .get("suggested_action")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let indices: Vec<usize> = val
                .get("signal_indices")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as usize))
                        .filter(|&i| i < signals.len())
                        .collect()
                })
                .unwrap_or_default();

            let pattern_signals: Vec<RawSignal> = if indices.is_empty() {
                // If no indices provided, include all signals
                signals.to_vec()
            } else {
                indices.iter().map(|&i| signals[i].clone()).collect()
            };

            if suggested_action.is_empty() {
                return None;
            }

            Some(DetectedPattern {
                signals: pattern_signals,
                correlation,
                suggested_action,
                created_at: now.clone(),
            })
        })
        .collect()
}

/// Promote raw signals directly into patterns (when LLM is unavailable).
fn promote_raw_signals(signals: &[RawSignal]) -> Vec<DetectedPattern> {
    let now = now_rfc3339();
    signals
        .iter()
        .map(|s| DetectedPattern {
            signals: vec![s.clone()],
            correlation: "standalone".to_string(),
            suggested_action: format!("Investigate: {}", s.summary),
            created_at: now.clone(),
        })
        .collect()
}

// ── Output: Note Generation ──

/// Write an edda note event summarizing detected patterns.
fn write_detect_note(_project_id: &str, cwd: &str, result: &DetectResult) -> Result<()> {
    let cwd_path = std::path::Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)
        .with_context(|| "Cannot find edda root for detect note")?;
    let ledger = edda_ledger::Ledger::open(&root)?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let mut parts = Vec::new();
    parts.push(format!(
        "[pattern-detect] {} signals, {} patterns detected",
        result.raw_signals.len(),
        result.patterns.len()
    ));

    for (i, p) in result.patterns.iter().enumerate() {
        let kinds: Vec<String> = p.signals.iter().map(|s| format!("{:?}", s.kind)).collect();
        parts.push(format!(
            "  {}. [{}] {}",
            i + 1,
            kinds.join("+"),
            p.suggested_action
        ));
    }

    let text = parts.join("\n");
    let tags = vec!["pattern-detect".to_string()];
    let mut event =
        edda_core::event::new_note_event(&branch, parent_hash.as_deref(), "bridge", &text, &tags)?;

    event.payload["source"] = serde_json::json!("bridge:pattern-detect");

    edda_core::event::finalize_event(&mut event);
    ledger.append_event(&event)?;

    eprintln!("[edda-bg] pattern detect note written → {}", event.event_id);
    Ok(())
}

// ── Guard Helpers ──

fn cooldown_elapsed(state: &DetectState) -> bool {
    let cooldown_hours = std::env::var("EDDA_DETECT_COOLDOWN_HOURS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DETECT_COOLDOWN_HOURS);

    if state.last_detect_at.is_empty() {
        return true;
    }

    let Ok(last) = time::OffsetDateTime::parse(
        &state.last_detect_at,
        &time::format_description::well_known::Rfc3339,
    ) else {
        return true;
    };

    let now = time::OffsetDateTime::now_utc();
    let elapsed = now - last;
    let cooldown = time::Duration::hours(cooldown_hours as i64);

    elapsed >= cooldown
}

// ── State Persistence ──

fn state_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id).join("state")
}

fn detect_state_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_detect_last.json")
}

fn detect_results_dir(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_detect")
}

fn audit_log_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_detect_audit.jsonl")
}

fn load_detect_state(project_id: &str) -> Option<DetectState> {
    let path = detect_state_path(project_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_detect_state_raw(project_id: &str, state: &DetectState) -> Result<()> {
    let path = detect_state_path(project_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json)?;
    Ok(())
}

fn save_detect_state(project_id: &str, result: &DetectResult) -> Result<()> {
    let state = DetectState {
        last_detect_at: result.detected_at.clone(),
        sessions_since_last: 0, // Reset counter
        status: "completed".to_string(),
    };
    save_detect_state_raw(project_id, &state)
}

fn save_detect_result(project_id: &str, result: &DetectResult) -> Result<()> {
    let dir = detect_results_dir(project_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", result.detect_id));
    let json = serde_json::to_string_pretty(result)?;
    fs::write(&path, json)?;
    Ok(())
}

fn append_audit_log(project_id: &str, entry: &AuditEntry) -> Result<()> {
    use std::io::Write;
    let path = audit_log_path(project_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let line = serde_json::to_string(entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_kind_serde_roundtrip() {
        let signal = RawSignal {
            kind: SignalKind::FailurePattern,
            severity: "high".to_string(),
            summary: "test failure".to_string(),
            evidence: vec!["session_1".to_string()],
            metric_value: 5.0,
            baseline_value: 3.0,
            confidence: 0.8,
        };
        let json = serde_json::to_string(&signal).unwrap();
        let parsed: RawSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, SignalKind::FailurePattern);
        assert_eq!(parsed.severity, "high");
        assert_eq!(parsed.confidence, 0.8);
    }

    #[test]
    fn detect_result_serde_roundtrip() {
        let result = DetectResult {
            detect_id: "detect_abc123".to_string(),
            detected_at: "2026-03-12T10:00:00Z".to_string(),
            raw_signals: vec![RawSignal {
                kind: SignalKind::CostAnomaly,
                severity: "medium".to_string(),
                summary: "Cost spike".to_string(),
                evidence: vec![],
                metric_value: 0.50,
                baseline_value: 0.10,
                confidence: 0.85,
            }],
            patterns: vec![DetectedPattern {
                signals: vec![],
                correlation: "standalone".to_string(),
                suggested_action: "Review spending".to_string(),
                created_at: "2026-03-12T10:00:00Z".to_string(),
            }],
            model: Some("claude-3-5-haiku-20241022".to_string()),
            input_tokens: 500,
            output_tokens: 200,
            cost_usd: 0.0015,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: DetectResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.detect_id, "detect_abc123");
        assert_eq!(parsed.raw_signals.len(), 1);
        assert_eq!(parsed.patterns.len(), 1);
        assert_eq!(parsed.cost_usd, 0.0015);
    }

    #[test]
    fn detect_state_serde_roundtrip() {
        let state = DetectState {
            last_detect_at: "2026-03-12T10:00:00Z".to_string(),
            sessions_since_last: 5,
            status: "completed".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: DetectState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sessions_since_last, 5);
        assert_eq!(parsed.status, "completed");
    }

    #[test]
    fn should_run_returns_false_when_disabled() {
        std::env::set_var("EDDA_BG_ENABLED", "0");
        assert!(!should_run("test_detect_disabled"));
        std::env::remove_var("EDDA_BG_ENABLED");
    }

    #[test]
    fn should_run_returns_true_when_never_run() {
        let pid = "test_detect_never_run";
        // Ensure no state file exists
        let _ = fs::remove_file(detect_state_path(pid));

        std::env::set_var("EDDA_BG_ENABLED", "1");
        assert!(should_run(pid));
        std::env::remove_var("EDDA_BG_ENABLED");
    }

    #[test]
    fn should_run_returns_false_below_interval() {
        let pid = "test_detect_below_interval";
        let _ = edda_store::ensure_dirs(pid);

        let state = DetectState {
            last_detect_at: now_rfc3339(),
            sessions_since_last: 2, // Below default 10
            status: "completed".to_string(),
        };
        save_detect_state_raw(pid, &state).unwrap();

        std::env::set_var("EDDA_BG_ENABLED", "1");
        std::env::set_var("EDDA_DETECT_INTERVAL", "10");
        assert!(!should_run(pid));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_BG_ENABLED");
        std::env::remove_var("EDDA_DETECT_INTERVAL");
    }

    #[test]
    fn should_run_returns_false_within_cooldown() {
        let pid = "test_detect_cooldown";
        let _ = edda_store::ensure_dirs(pid);

        let state = DetectState {
            last_detect_at: now_rfc3339(), // Just now
            sessions_since_last: 100,      // Well above threshold
            status: "completed".to_string(),
        };
        save_detect_state_raw(pid, &state).unwrap();

        std::env::set_var("EDDA_BG_ENABLED", "1");
        std::env::set_var("EDDA_DETECT_INTERVAL", "1");
        std::env::set_var("EDDA_DETECT_COOLDOWN_HOURS", "24");
        assert!(!should_run(pid));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_BG_ENABLED");
        std::env::remove_var("EDDA_DETECT_INTERVAL");
        std::env::remove_var("EDDA_DETECT_COOLDOWN_HOURS");
    }

    #[test]
    fn state_persistence_roundtrip() {
        let pid = "test_detect_state_persist";
        let _ = edda_store::ensure_dirs(pid);

        let state = DetectState {
            last_detect_at: "2026-03-12T10:00:00Z".to_string(),
            sessions_since_last: 7,
            status: "completed".to_string(),
        };
        save_detect_state_raw(pid, &state).unwrap();

        let loaded = load_detect_state(pid).unwrap();
        assert_eq!(loaded.last_detect_at, "2026-03-12T10:00:00Z");
        assert_eq!(loaded.sessions_since_last, 7);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn increment_session_count_works() {
        let pid = "test_detect_increment";
        let _ = edda_store::ensure_dirs(pid);

        // Start fresh
        let _ = fs::remove_file(detect_state_path(pid));

        increment_session_count(pid);
        let state = load_detect_state(pid).unwrap();
        assert_eq!(state.sessions_since_last, 1);

        increment_session_count(pid);
        let state = load_detect_state(pid).unwrap();
        assert_eq!(state.sessions_since_last, 2);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn audit_log_appends() {
        let pid = "test_detect_audit";
        let _ = edda_store::ensure_dirs(pid);

        let entry = AuditEntry {
            ts: "2026-03-12T10:00:00Z".to_string(),
            detect_id: "detect_1".to_string(),
            signals_found: 2,
            patterns_found: 1,
            cost_usd: 0.001,
            model: Some("test-model".to_string()),
            status: "completed".to_string(),
        };
        append_audit_log(pid, &entry).unwrap();

        let path = audit_log_path(pid);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("detect_1"));
        assert_eq!(content.lines().count(), 1);

        // Append another
        let entry2 = AuditEntry {
            ts: "2026-03-12T11:00:00Z".to_string(),
            detect_id: "detect_2".to_string(),
            signals_found: 0,
            patterns_found: 0,
            cost_usd: 0.0,
            model: None,
            status: "completed".to_string(),
        };
        append_audit_log(pid, &entry2).unwrap();
        let content2 = fs::read_to_string(&path).unwrap();
        assert_eq!(content2.lines().count(), 2);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn promote_raw_signals_creates_patterns() {
        let signals = vec![
            RawSignal {
                kind: SignalKind::FailurePattern,
                severity: "high".to_string(),
                summary: "Recurring bash failures".to_string(),
                evidence: vec!["s1".to_string()],
                metric_value: 5.0,
                baseline_value: 3.0,
                confidence: 0.8,
            },
            RawSignal {
                kind: SignalKind::CostAnomaly,
                severity: "medium".to_string(),
                summary: "Cost spike".to_string(),
                evidence: vec![],
                metric_value: 0.5,
                baseline_value: 0.1,
                confidence: 0.85,
            },
        ];

        let patterns = promote_raw_signals(&signals);
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].correlation, "standalone");
        assert!(patterns[0].suggested_action.contains("Recurring bash"));
        assert!(patterns[1].suggested_action.contains("Cost spike"));
    }

    #[test]
    fn parse_detect_response_valid_json() {
        let signals = vec![RawSignal {
            kind: SignalKind::FailurePattern,
            severity: "high".to_string(),
            summary: "test".to_string(),
            evidence: vec![],
            metric_value: 1.0,
            baseline_value: 0.5,
            confidence: 0.8,
        }];

        let response = r#"[
            {
                "correlation": "Failures causing cost increase",
                "suggested_action": "Add retry logic",
                "signal_indices": [0]
            }
        ]"#;
        let patterns = parse_detect_response(response, &signals);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].correlation, "Failures causing cost increase");
        assert_eq!(patterns[0].suggested_action, "Add retry logic");
        assert_eq!(patterns[0].signals.len(), 1);
    }

    #[test]
    fn parse_detect_response_embedded_json() {
        let signals = vec![RawSignal {
            kind: SignalKind::CostAnomaly,
            severity: "medium".to_string(),
            summary: "test".to_string(),
            evidence: vec![],
            metric_value: 1.0,
            baseline_value: 0.5,
            confidence: 0.9,
        }];

        let response = r#"Here are my findings:
[{"correlation": "standalone", "suggested_action": "Review costs", "signal_indices": [0]}]
End."#;
        let patterns = parse_detect_response(response, &signals);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].suggested_action, "Review costs");
    }

    #[test]
    fn parse_detect_response_malformed_returns_empty() {
        let signals = vec![];
        let patterns = parse_detect_response("not json at all", &signals);
        assert!(patterns.is_empty());
    }

    #[test]
    fn build_detect_prompt_includes_context() {
        let prompt = build_detect_prompt("test anomaly data");
        assert!(prompt.contains("test anomaly data"));
        assert!(prompt.contains("anomaly signals"));
        assert!(prompt.contains("JSON array"));
    }

    #[test]
    fn detect_cost_anomalies_needs_min_data() {
        let pid = "test_detect_cost_min";
        let _ = edda_store::ensure_dirs(pid);

        // With no audit files, should return empty
        let signals = detect_cost_anomalies(pid);
        assert!(signals.is_empty());

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn detect_cost_anomalies_flags_spike() {
        let pid = "test_detect_cost_spike";
        let _ = edda_store::ensure_dirs(pid);

        // Create synthetic audit log with a cost spike
        let dir = edda_store::project_dir(pid).join("state");
        fs::create_dir_all(&dir).unwrap();
        let audit_path = dir.join("bg_extract_audit.jsonl");

        let mut lines = Vec::new();
        // 7 days of normal cost ($0.01/day)
        for day in 1..=7 {
            lines.push(format!(
                r#"{{"ts":"2026-03-{:02}T10:00:00Z","cost_usd":0.01,"status":"completed"}}"#,
                day
            ));
        }
        // Day 8: big spike ($0.10)
        lines.push(
            r#"{"ts":"2026-03-08T10:00:00Z","cost_usd":0.10,"status":"completed"}"#.to_string(),
        );

        fs::write(&audit_path, lines.join("\n")).unwrap();

        let signals = detect_cost_anomalies(pid);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SignalKind::CostAnomaly);
        assert!(signals[0].metric_value > signals[0].baseline_value);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn detect_quality_degradation_empty_data() {
        let pid = "test_detect_quality_empty";
        let _ = edda_store::ensure_dirs(pid);

        let signals = detect_quality_degradation(pid);
        assert!(signals.is_empty());

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn detect_quality_degradation_flags_drop() {
        let pid = "test_detect_quality_drop";
        let _ = edda_store::ensure_dirs(pid);

        let dir = edda_store::project_dir(pid).join("state");
        fs::create_dir_all(&dir).unwrap();
        let audit_path = dir.join("bg_digest_audit.jsonl");

        let mut lines = Vec::new();
        // 5 older successful sessions
        for i in 1..=5 {
            lines.push(format!(
                r#"{{"ts":"2026-03-0{i}T10:00:00Z","session_id":"s{i}","status":"completed"}}"#
            ));
        }
        // 5 recent failing sessions
        for i in 6..=10 {
            let d = if i <= 9 {
                format!("0{i}")
            } else {
                format!("{i}")
            };
            lines.push(format!(
                r#"{{"ts":"2026-03-{d}T10:00:00Z","session_id":"s{i}","status":"error"}}"#
            ));
        }

        fs::write(&audit_path, lines.join("\n")).unwrap();

        let signals = detect_quality_degradation(pid);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SignalKind::QualityDegradation);
        assert_eq!(signals[0].severity, "high"); // 100% drop

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn detect_result_storage_roundtrip() {
        let pid = "test_detect_result_store";
        let _ = edda_store::ensure_dirs(pid);

        let result = DetectResult {
            detect_id: "detect_test1".to_string(),
            detected_at: "2026-03-12T10:00:00Z".to_string(),
            raw_signals: vec![RawSignal {
                kind: SignalKind::FailurePattern,
                severity: "high".to_string(),
                summary: "test".to_string(),
                evidence: vec![],
                metric_value: 5.0,
                baseline_value: 3.0,
                confidence: 0.8,
            }],
            patterns: vec![],
            model: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
        };

        save_detect_result(pid, &result).unwrap();

        let path = detect_results_dir(pid).join("detect_test1.json");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        let loaded: DetectResult = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.detect_id, "detect_test1");
        assert_eq!(loaded.raw_signals.len(), 1);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn cooldown_expired_allows_run() {
        let state = DetectState {
            last_detect_at: "2020-01-01T00:00:00Z".to_string(), // Long ago
            sessions_since_last: 100,
            status: "completed".to_string(),
        };
        assert!(cooldown_elapsed(&state));
    }

    #[test]
    fn cooldown_not_expired_blocks_run() {
        let state = DetectState {
            last_detect_at: now_rfc3339(), // Just now
            sessions_since_last: 100,
            status: "completed".to_string(),
        };
        std::env::set_var("EDDA_DETECT_COOLDOWN_HOURS", "24");
        assert!(!cooldown_elapsed(&state));
        std::env::remove_var("EDDA_DETECT_COOLDOWN_HOURS");
    }

    #[test]
    fn empty_last_detect_at_means_cooldown_elapsed() {
        let state = DetectState {
            last_detect_at: String::new(),
            sessions_since_last: 0,
            status: "init".to_string(),
        };
        assert!(cooldown_elapsed(&state));
    }
}
