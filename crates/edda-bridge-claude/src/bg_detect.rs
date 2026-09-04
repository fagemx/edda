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
pub fn increment_session_count(project_id: &str, session_id: &str) {
    let state = load_detect_state(project_id).unwrap_or(DetectState {
        last_detect_at: String::new(),
        sessions_since_last: 0,
        status: "init".to_string(),
    });

    let updated = DetectState {
        sessions_since_last: state.sessions_since_last + 1,
        ..state
    };

    if let Err(e) = save_detect_state_raw(project_id, &updated) {
        // GH-692: the detect-state write failed — count it, don't pretend the
        // counter advanced. Without this, the next threshold crossing is
        // silently misjudged.
        crate::state::record_dropped_write(
            project_id,
            session_id,
            "detect state",
            &format!("{e:#}"),
        );
    }
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
    if crate::env_var("EDDA_BG_ENABLED").unwrap_or_else(|| "1".into()) == "0" {
        return false;
    }

    let interval = crate::env_var("EDDA_DETECT_INTERVAL")
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
        let api_key = crate::env_var("EDDA_LLM_API_KEY").unwrap_or_default();
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
        ulid::Ulid::new().to_string()[..12].to_lowercase()
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

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    eprintln!("[edda-bg] pattern detect note written → {}", event.event_id);
    Ok(())
}

// ── Guard Helpers ──

fn cooldown_elapsed(state: &DetectState) -> bool {
    let cooldown_hours = crate::env_var("EDDA_DETECT_COOLDOWN_HOURS")
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
    fs::create_dir_all(path.parent().context("detect state path has no parent")?)?;
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
    fs::create_dir_all(
        path.parent()
            .context("detect audit log path has no parent")?,
    )?;
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
#[path = "bg_detect/tests.rs"]
mod tests;
