//! Background capability scanner -- periodically scans the codebase and decision
//! history via LLM to identify capability gaps and produce issue drafts.
//!
//! Design: non-blocking, idempotent, cost-controlled.  Can be triggered at
//! SessionEnd (with cooldown) or on-demand via `edda scan run`.
//!
//! Reuses shared infrastructure from `bg_extract` (API call, budget tracking,
//! cost control).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::bg_extract::{
    call_anthropic_sync, check_daily_budget, env_f64, now_rfc3339, truncate_text,
    update_daily_cost, DEFAULT_MODEL, HAIKU_INPUT_COST_PER_TOKEN, HAIKU_OUTPUT_COST_PER_TOKEN,
};

// ── Configuration ──

const DEFAULT_SCAN_COOLDOWN_DAYS: u64 = 7;
const DEFAULT_MAX_SNAPSHOT_CHARS: usize = 40_000;
const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.6;

// ── Data Structures ──

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GapStatus {
    #[default]
    Pending,
    Accepted,
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityGap {
    pub title: String,
    pub category: String,
    pub severity: String,
    pub description: String,
    pub evidence: Vec<String>,
    pub suggested_labels: Vec<String>,
    pub confidence: f64,
    #[serde(default)]
    pub status: GapStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub scan_id: String,
    pub scanned_at: String,
    pub gaps: Vec<CapabilityGap>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub codebase_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScanState {
    last_scan_at: String,
    codebase_hash: String,
    gaps_found: usize,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    scan_id: String,
    gaps_found: usize,
    cost_usd: f64,
    model: String,
    status: String,
}

// ── Public API ──

/// Check whether background capability scan should run for this project.
///
/// Returns `false` (skip) if any of these hold:
/// - `EDDA_BG_ENABLED` is `"0"`
/// - `EDDA_LLM_API_KEY` is missing or empty
/// - Daily budget is exhausted
/// - Cooldown has not elapsed (default 7 days)
pub fn should_run(project_id: &str) -> bool {
    if crate::env_var("EDDA_BG_ENABLED").unwrap_or_else(|| "1".into()) == "0" {
        return false;
    }
    if crate::env_var("EDDA_LLM_API_KEY")
        .unwrap_or_default()
        .is_empty()
    {
        return false;
    }

    if !cooldown_elapsed(project_id) {
        return false;
    }

    check_daily_budget(project_id).unwrap_or(true)
}

/// Main scan entry point -- can be called from a background thread or directly.
///
/// Assembles a project snapshot, calls the LLM for gap analysis, saves results
/// as draft issues, and updates state/cost tracking.
pub fn run_scan(project_id: &str, cwd: &str) -> Result<ScanResult> {
    let api_key = crate::env_var("EDDA_LLM_API_KEY").with_context(|| "EDDA_LLM_API_KEY not set")?;
    if api_key.is_empty() {
        anyhow::bail!("EDDA_LLM_API_KEY is empty");
    }

    // Assemble project snapshot
    let snapshot = assemble_project_snapshot(cwd, project_id)?;

    // Compute hash for idempotency
    let codebase_hash = format!("blake3:{}", blake3::hash(snapshot.as_bytes()).to_hex());

    // Check idempotency
    if let Some(state) = load_scan_state(project_id) {
        if state.codebase_hash == codebase_hash && state.status == "completed" {
            return Ok(ScanResult {
                scan_id: String::new(),
                scanned_at: state.last_scan_at,
                gaps: Vec::new(),
                model: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                codebase_hash,
            });
        }
    }

    // Truncate snapshot
    let max_chars = std::env::var("EDDA_SCAN_MAX_SNAPSHOT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_SNAPSHOT_CHARS);
    let truncated = truncate_text(&snapshot, max_chars);

    // Build prompt and call LLM
    let prompt = build_scan_prompt(&truncated);
    let model = std::env::var("EDDA_BG_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let (response_text, input_tokens, output_tokens) =
        call_anthropic_sync(&api_key, &model, &prompt)?;

    // Parse response
    let mut gaps = parse_scan_response(&response_text);

    // Filter by confidence threshold
    let threshold = env_f64(
        "EDDA_SCAN_CONFIDENCE_THRESHOLD",
        DEFAULT_CONFIDENCE_THRESHOLD,
    );
    gaps.retain(|g| g.confidence >= threshold);

    let cost_usd = (input_tokens as f64 * HAIKU_INPUT_COST_PER_TOKEN)
        + (output_tokens as f64 * HAIKU_OUTPUT_COST_PER_TOKEN);

    let scan_id = format!(
        "scan_{}",
        ulid::Ulid::new().to_string()[..12].to_lowercase()
    );
    let result = ScanResult {
        scan_id: scan_id.clone(),
        scanned_at: now_rfc3339(),
        gaps,
        model: model.clone(),
        input_tokens,
        output_tokens,
        cost_usd,
        codebase_hash,
    };

    // Save results
    if !result.gaps.is_empty() {
        save_scan_drafts(project_id, &result)?;
    }
    save_scan_state(project_id, &result)?;
    update_daily_cost(project_id, cost_usd)?;
    append_audit_log(
        project_id,
        &AuditEntry {
            ts: now_rfc3339(),
            scan_id,
            gaps_found: result.gaps.len(),
            cost_usd,
            model,
            status: "completed".to_string(),
        },
    )?;

    tracing::info!(
        gaps = result.gaps.len(),
        cost_usd = format_args!("{:.4}", cost_usd),
        "capability scan complete",
    );

    Ok(result)
}

// ── Review API ──

/// Load a single scan result by ID (regardless of gap status).
pub fn load_scan(project_id: &str, scan_id: &str) -> Result<ScanResult> {
    let path = scan_draft_path(project_id, scan_id);
    let content =
        fs::read_to_string(&path).with_context(|| format!("Scan not found: {scan_id}"))?;
    let scan: ScanResult = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse scan: {}", path.display()))?;
    Ok(scan)
}

/// List all scan results that have pending gaps.
pub fn list_pending_scans(project_id: &str) -> Result<Vec<ScanResult>> {
    let dir = scan_drafts_dir(project_id);
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
        let scan: ScanResult = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse scan: {}", path.display()))?;

        if scan.gaps.iter().any(|g| g.status == GapStatus::Pending) {
            results.push(scan);
        }
    }

    results.sort_by(|a, b| b.scanned_at.cmp(&a.scanned_at));
    Ok(results)
}

/// Dismiss a gap from a scan result.
pub fn dismiss_gap(project_id: &str, scan_id: &str, index: usize) -> Result<()> {
    let path = scan_draft_path(project_id, scan_id);
    let content =
        fs::read_to_string(&path).with_context(|| format!("Scan not found: {scan_id}"))?;
    let mut scan: ScanResult = serde_json::from_str(&content)?;

    if index >= scan.gaps.len() {
        anyhow::bail!(
            "Gap index {index} out of range (scan has {} gaps)",
            scan.gaps.len()
        );
    }

    scan.gaps[index].status = GapStatus::Dismissed;

    let json = serde_json::to_string_pretty(&scan)?;
    fs::write(&path, json)?;
    Ok(())
}

/// Accept a gap (mark it as accepted for issue creation).
pub fn accept_gap(project_id: &str, scan_id: &str, index: usize) -> Result<CapabilityGap> {
    let path = scan_draft_path(project_id, scan_id);
    let content =
        fs::read_to_string(&path).with_context(|| format!("Scan not found: {scan_id}"))?;
    let mut scan: ScanResult = serde_json::from_str(&content)?;

    if index >= scan.gaps.len() {
        anyhow::bail!(
            "Gap index {index} out of range (scan has {} gaps)",
            scan.gaps.len()
        );
    }

    scan.gaps[index].status = GapStatus::Accepted;
    let gap = scan.gaps[index].clone();

    let json = serde_json::to_string_pretty(&scan)?;
    fs::write(&path, json)?;
    Ok(gap)
}

// ── Project Snapshot Assembly ──

/// Assemble a structured text snapshot of the project for LLM analysis.
pub fn assemble_project_snapshot(cwd: &str, _project_id: &str) -> Result<String> {
    let mut sections = Vec::new();

    // 1. Crate inventory from workspace Cargo.toml
    if let Some(inventory) = collect_crate_inventory(cwd) {
        sections.push(format!("## Crate Inventory\n\n{inventory}"));
    }

    // 2. Module structure (src/*.rs per crate, depth 1)
    if let Some(modules) = collect_module_structure(cwd) {
        sections.push(format!("## Module Structure\n\n{modules}"));
    }

    // 3. Active decisions from ledger
    if let Some(decisions) = collect_active_decisions(cwd) {
        sections.push(format!("## Active Decisions\n\n{decisions}"));
    }

    // 4. Recent commits from git log
    if let Some(commits) = collect_recent_commits(cwd) {
        sections.push(format!("## Recent Commits\n\n{commits}"));
    }

    // 5. Recent session notes from ledger
    if let Some(notes) = collect_recent_notes(cwd) {
        sections.push(format!("## Recent Session Notes\n\n{notes}"));
    }

    // 6. Open issues from GitHub (graceful failure)
    if let Some(issues) = collect_open_issues(cwd) {
        sections.push(format!("## Open Issues\n\n{issues}"));
    }

    if sections.is_empty() {
        anyhow::bail!("Could not assemble any project data for scanning");
    }

    Ok(sections.join("\n\n---\n\n"))
}

fn collect_crate_inventory(cwd: &str) -> Option<String> {
    let cargo_path = Path::new(cwd).join("Cargo.toml");
    let content = fs::read_to_string(cargo_path).ok()?;

    let mut members = Vec::new();
    let mut in_members = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
            // Handle inline array
            if let Some(start) = trimmed.find('[') {
                let rest = &trimmed[start + 1..];
                if let Some(end) = rest.find(']') {
                    for item in rest[..end].split(',') {
                        let item = item.trim().trim_matches('"');
                        if !item.is_empty() {
                            members.push(item.to_string());
                        }
                    }
                    in_members = false;
                }
            }
            continue;
        }
        if in_members {
            if trimmed.starts_with(']') {
                in_members = false;
                continue;
            }
            let item = trimmed.trim_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
            if !item.is_empty() {
                members.push(item.to_string());
            }
        }
    }

    if members.is_empty() {
        return None;
    }

    let lines: Vec<String> = members.iter().map(|m| format!("- {m}")).collect();
    Some(lines.join("\n"))
}

fn collect_module_structure(cwd: &str) -> Option<String> {
    let cwd_path = Path::new(cwd);
    let mut lines = Vec::new();

    // Look for crates/ directory first, then src/
    let crates_dir = cwd_path.join("crates");
    if crates_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&crates_dir) {
            let mut crate_names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            crate_names.sort();

            for crate_name in &crate_names {
                let src_dir = crates_dir.join(crate_name).join("src");
                if !src_dir.is_dir() {
                    continue;
                }
                let modules = list_rs_files(&src_dir);
                if !modules.is_empty() {
                    lines.push(format!("### {crate_name}"));
                    for m in &modules {
                        lines.push(format!("  - {m}"));
                    }
                }
            }
        }
    } else {
        let src_dir = cwd_path.join("src");
        if src_dir.is_dir() {
            let modules = list_rs_files(&src_dir);
            for m in &modules {
                lines.push(format!("- {m}"));
            }
        }
    }

    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

fn list_rs_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "rs")
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

fn collect_active_decisions(cwd: &str) -> Option<String> {
    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)?;
    let ledger = edda_ledger::Ledger::open(&root).ok()?;
    let branch = ledger.head_branch().ok()?;
    // GH-401: active_decisions is not branch-filtered; keep only this branch's
    // decisions so another branch's ratified value is not shown as binding here.
    let mut decisions = ledger.active_decisions(None, None, None, None).ok()?;
    decisions.retain(|d| d.branch == branch);
    let ratified = ledger.ratified_decision_events().ok()?;
    render_decisions_two_tier(&decisions, &ratified)
}

/// Split active decisions into an operator-ratified (binding) tier and an
/// unratified tier (GH-401). Binding status is decided solely by ratify
/// events (via [`edda_ledger::view::is_decision_ratified`]) — never by the
/// authority string — so the projection can never launder machine inference
/// into operator authority. The unratified tier annotates each line with its
/// authorship so agent-authored and legacy decisions read as non-binding.
fn render_decisions_two_tier(
    decisions: &[edda_ledger::view::DecisionView],
    ratified: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if decisions.is_empty() {
        return None;
    }

    let mut ratified_lines = Vec::new();
    let mut unratified_lines = Vec::new();
    for d in decisions {
        let reason = if d.reason.is_empty() {
            String::new()
        } else {
            format!(" -- {}", d.reason)
        };
        if edda_ledger::view::is_decision_ratified(d, ratified) {
            ratified_lines.push(format!("- **{}** = {}{}", d.key, d.value, reason));
        } else {
            unratified_lines.push(format!(
                "- [{}] **{}** = {}{}",
                edda_core::types::authorship_tag(&d.authority),
                d.key,
                d.value,
                reason
            ));
        }
    }

    let mut out = String::new();
    if !ratified_lines.is_empty() {
        out.push_str("### Operator-ratified (binding)\n\n");
        out.push_str(&ratified_lines.join("\n"));
    }
    if !unratified_lines.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(
            "### Unratified — recorded, not binding until `edda ratify` (agent working decisions)\n\n",
        );
        out.push_str(&unratified_lines.join("\n"));
    }
    Some(out)
}

fn collect_recent_commits(cwd: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "--oneline", "-20"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(text)
}

fn collect_recent_notes(cwd: &str) -> Option<String> {
    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)?;
    let ledger = edda_ledger::Ledger::open(&root).ok()?;
    let events = ledger.iter_events().ok()?;

    // Filter for note events with auto-digest tag, take last 5
    let mut notes: Vec<String> = events
        .iter()
        .filter(|e| e.event_type == "note")
        .filter(|e| {
            e.payload
                .get("tags")
                .and_then(|t| t.as_array())
                .is_some_and(|tags| {
                    tags.iter()
                        .any(|t| t.as_str().is_some_and(|s| s == "auto-digest"))
                })
        })
        .filter_map(|e| {
            let text = e.payload.get("text")?.as_str()?;
            let ts = &e.ts;
            Some(format!("- [{ts}] {text}"))
        })
        .collect();

    // Take only the last 5
    if notes.len() > 5 {
        notes = notes.split_off(notes.len() - 5);
    }

    if notes.is_empty() {
        return None;
    }
    Some(notes.join("\n"))
}

fn collect_open_issues(cwd: &str) -> Option<String> {
    let output = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "title,labels,number",
        ])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let issues: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;

    if issues.is_empty() {
        return None;
    }

    let lines: Vec<String> = issues
        .iter()
        .filter_map(|issue| {
            let number = issue.get("number")?.as_u64()?;
            let title = issue.get("title")?.as_str()?;
            let labels: Vec<String> = issue
                .get("labels")
                .and_then(|l| l.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let label_str = if labels.is_empty() {
                String::new()
            } else {
                format!(" [{}]", labels.join(", "))
            };
            Some(format!("- #{number}: {title}{label_str}"))
        })
        .collect();

    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

// ── LLM Prompt and Response Parsing ──

fn build_scan_prompt(snapshot: &str) -> String {
    format!(
        r#"You are a software project capability scanner. Analyze the following project snapshot and identify capability gaps — areas where the project is missing features, has insufficient testing, lacks error handling, missing documentation, or has other deficiencies.

## Rules
- Focus on actionable, specific gaps (not vague suggestions)
- Each gap should be something that could become a GitHub issue
- Rate confidence 0.0-1.0 based on how certain you are this is a real gap
- Categories: "feature", "testing", "reliability", "docs", "security", "performance", "observability"
- Severity: "low", "medium", "high"
- Do NOT suggest gaps for things already tracked in open issues
- Output valid JSON array only, no markdown fences, no explanation text

## Output Format
Return a JSON array of objects with these fields:
- "title": string (concise issue title)
- "category": string
- "severity": string
- "description": string (2-3 sentences explaining the gap)
- "evidence": array of strings (specific files, modules, or patterns that show the gap)
- "suggested_labels": array of strings (GitHub labels)
- "confidence": number (0.0 to 1.0)

## Project Snapshot

{snapshot}"#
    )
}

pub fn parse_scan_response(response: &str) -> Vec<CapabilityGap> {
    let text = response.trim();

    // Try direct JSON parse first
    if let Ok(gaps) = serde_json::from_str::<Vec<CapabilityGap>>(text) {
        return gaps;
    }

    // Try stripping markdown fences
    let stripped = strip_markdown_fences(text);
    if let Ok(gaps) = serde_json::from_str::<Vec<CapabilityGap>>(&stripped) {
        return gaps;
    }

    // Try finding JSON array in the text
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            let slice = &text[start..=end];
            if let Ok(gaps) = serde_json::from_str::<Vec<CapabilityGap>>(slice) {
                return gaps;
            }
        }
    }

    Vec::new()
}

fn strip_markdown_fences(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut result = Vec::new();
    let mut in_fence = false;
    for line in &lines {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !line.starts_with("```") {
            result.push(*line);
        }
    }
    result.join("\n")
}

// ── Guard and Cooldown Logic ──

fn cooldown_elapsed(project_id: &str) -> bool {
    let cooldown_days = crate::env_var("EDDA_SCAN_COOLDOWN_DAYS")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SCAN_COOLDOWN_DAYS);

    let Some(state) = load_scan_state(project_id) else {
        return true; // Never scanned
    };

    // Parse last scan date
    let Ok(last) = time::OffsetDateTime::parse(
        &state.last_scan_at,
        &time::format_description::well_known::Rfc3339,
    ) else {
        return true; // Can't parse, assume expired
    };

    let now = time::OffsetDateTime::now_utc();
    let elapsed = now - last;
    let cooldown = time::Duration::days(cooldown_days as i64);

    elapsed >= cooldown
}

/// Check if a milestone (new git tag) has occurred since last scan, overriding cooldown.
pub fn has_recent_milestone(project_id: &str, cwd: &str) -> bool {
    let Some(state) = load_scan_state(project_id) else {
        return false; // No previous scan to compare against
    };

    let output = std::process::Command::new("git")
        .args([
            "tag",
            "--sort=-creatordate",
            "--format=%(creatordate:iso-strict)",
            "-n1",
        ])
        .current_dir(cwd)
        .output();

    let Ok(output) = output else {
        return false;
    };

    if !output.status.success() {
        return false;
    }

    let tag_date = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tag_date.is_empty() {
        return false;
    }

    // Compare tag date with last scan date
    tag_date > state.last_scan_at
}

// ── State Persistence ──

fn state_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id).join("state")
}

fn scan_state_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_scan_last.json")
}

fn scan_drafts_dir(project_id: &str) -> PathBuf {
    state_dir(project_id).join("scan_drafts")
}

fn scan_draft_path(project_id: &str, scan_id: &str) -> PathBuf {
    scan_drafts_dir(project_id).join(format!("{scan_id}.json"))
}

fn audit_log_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_scan_audit.jsonl")
}

fn load_scan_state(project_id: &str) -> Option<ScanState> {
    let path = scan_state_path(project_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_scan_state(project_id: &str, result: &ScanResult) -> Result<()> {
    let state = ScanState {
        last_scan_at: result.scanned_at.clone(),
        codebase_hash: result.codebase_hash.clone(),
        gaps_found: result.gaps.len(),
        status: "completed".to_string(),
    };
    let path = scan_state_path(project_id);
    fs::create_dir_all(path.parent().context("scan state path has no parent")?)?;
    let json = serde_json::to_string_pretty(&state)?;
    fs::write(&path, json)?;
    Ok(())
}

fn save_scan_drafts(project_id: &str, result: &ScanResult) -> Result<()> {
    let path = scan_draft_path(project_id, &result.scan_id);
    fs::create_dir_all(path.parent().context("scan draft path has no parent")?)?;
    let json = serde_json::to_string_pretty(result)?;
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

// ── Tests ──

#[cfg(test)]
#[path = "bg_scan/tests.rs"]
mod tests;
