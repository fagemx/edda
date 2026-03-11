//! Background session digester — generates an LLM-powered summary of the
//! session and writes it as an `edda note` event to the workspace ledger.
//!
//! Design: non-blocking, idempotent, cost-controlled.  Triggered at SessionEnd
//! via `std::thread::spawn` so the hook returns immediately.
//!
//! Reuses shared infrastructure from `bg_extract` (API call, budget tracking,
//! transcript reading).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::bg_extract::{
    call_anthropic_sync, check_daily_budget, compute_file_hash, now_rfc3339, read_transcript_turns,
    truncate_text, update_daily_cost, DEFAULT_MAX_TRANSCRIPT_CHARS, DEFAULT_MODEL,
    HAIKU_INPUT_COST_PER_TOKEN, HAIKU_OUTPUT_COST_PER_TOKEN,
};

// ── Data Structures ──

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DigestState {
    status: String, // "completed" | "failed"
    digested_at: String,
    transcript_hash: String,
    summary_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    session_id: String,
    summary_len: usize,
    cost_usd: f64,
    model: String,
    status: String,
}

// ── Public API ──

/// Check whether background session digest should run for this session.
///
/// Returns `false` (skip) if any of these hold:
/// - `EDDA_BG_ENABLED` is `"0"`
/// - `EDDA_LLM_API_KEY` is missing or empty
/// - Transcript doesn't exist for this session
/// - Daily budget is exhausted
/// - Session was already digested (idempotent guard)
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

    let transcript_path = transcript_path(project_id, session_id);
    if !transcript_path.exists() {
        return false;
    }

    if already_digested(project_id, session_id) {
        return false;
    }

    check_daily_budget(project_id).unwrap_or(true)
}

/// Main digest entry point — called from a background thread.
///
/// Reads the stored transcript, calls the LLM for a summary, writes an
/// `edda note` event to the workspace ledger, and updates state tracking.
pub fn run_digest(project_id: &str, session_id: &str, cwd: &str) -> Result<()> {
    let api_key = std::env::var("EDDA_LLM_API_KEY").with_context(|| "EDDA_LLM_API_KEY not set")?;
    if api_key.is_empty() {
        anyhow::bail!("EDDA_LLM_API_KEY is empty");
    }

    let tp = transcript_path(project_id, session_id);
    if !tp.exists() {
        anyhow::bail!("Transcript not found: {}", tp.display());
    }

    // Idempotency check by hash
    let transcript_hash = compute_file_hash(&tp)?;
    if let Some(state) = load_digest_state(project_id, session_id) {
        if state.transcript_hash == transcript_hash && state.status == "completed" {
            return Ok(());
        }
    }

    // Read and truncate transcript
    let transcript_text = read_transcript_turns(&tp)?;
    let max_chars = crate::bg_extract::env_f64(
        "EDDA_BG_MAX_TRANSCRIPT_CHARS",
        DEFAULT_MAX_TRANSCRIPT_CHARS as f64,
    ) as usize;
    let truncated = truncate_text(&transcript_text, max_chars);

    // Read prev_digest for structured stats context
    let stats_context = build_stats_context(project_id);

    // Build prompt and call LLM
    let prompt = build_digest_prompt(&truncated, &stats_context);
    let model = std::env::var("EDDA_BG_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let (summary, input_tokens, output_tokens) = call_anthropic_sync(&api_key, &model, &prompt)?;

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        anyhow::bail!("LLM returned empty summary");
    }

    let cost_usd = (input_tokens as f64 * HAIKU_INPUT_COST_PER_TOKEN)
        + (output_tokens as f64 * HAIKU_OUTPUT_COST_PER_TOKEN);

    // Write note event to workspace ledger
    write_session_note(cwd, &summary)?;

    // Save digest state (idempotency marker)
    save_digest_state(project_id, session_id, &transcript_hash, summary.len())?;

    // Update daily cost (shared with bg_extract)
    update_daily_cost(project_id, cost_usd)?;

    // Append audit log
    append_audit_log(
        project_id,
        &AuditEntry {
            ts: now_rfc3339(),
            session_id: session_id.to_string(),
            summary_len: summary.len(),
            cost_usd,
            model,
            status: "completed".to_string(),
        },
    )?;

    Ok(())
}

// ── Internal Helpers ──

fn transcript_path(project_id: &str, session_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("transcripts")
        .join(format!("{session_id}.jsonl"))
}

fn state_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id).join("state")
}

fn digest_state_path(project_id: &str, session_id: &str) -> PathBuf {
    state_dir(project_id).join(format!("bg_digest.{session_id}.json"))
}

fn audit_log_path(project_id: &str) -> PathBuf {
    state_dir(project_id).join("bg_digest_audit.jsonl")
}

fn already_digested(project_id: &str, session_id: &str) -> bool {
    load_digest_state(project_id, session_id).is_some_and(|s| s.status == "completed")
}

fn load_digest_state(project_id: &str, session_id: &str) -> Option<DigestState> {
    let path = digest_state_path(project_id, session_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_digest_state(
    project_id: &str,
    session_id: &str,
    transcript_hash: &str,
    summary_len: usize,
) -> Result<()> {
    let state = DigestState {
        status: "completed".to_string(),
        digested_at: now_rfc3339(),
        transcript_hash: transcript_hash.to_string(),
        summary_len,
    };
    let path = digest_state_path(project_id, session_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let json = serde_json::to_string_pretty(&state)?;
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

/// Write the LLM-generated summary as an `edda note` event to the workspace ledger.
fn write_session_note(cwd: &str, summary: &str) -> Result<()> {
    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)
        .ok_or_else(|| anyhow::anyhow!("No edda workspace found from {cwd}"))?;
    let ledger = edda_ledger::Ledger::open(&root)?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let tags = vec!["session".to_string(), "auto-digest".to_string()];
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent_hash.as_deref(),
        "bridge",
        summary,
        &tags,
    )?;

    // Mark source so collect_session_ledger_extras filters it out
    event.payload["source"] = serde_json::json!("bridge:session-digest");

    edda_core::event::finalize_event(&mut event);
    ledger.append_event(&event)?;

    eprintln!("[edda-bg] session digest written → {}", event.event_id);
    Ok(())
}

/// Build structured stats context from prev_digest.json.
fn build_stats_context(project_id: &str) -> String {
    let Some(digest) = crate::digest::read_prev_digest(project_id) else {
        return String::new();
    };

    let mut parts = Vec::new();
    parts.push(format!("Session outcome: {}", digest.outcome));
    parts.push(format!("Duration: {} minutes", digest.duration_minutes));
    if !digest.activity.is_empty() {
        parts.push(format!("Activity type: {}", digest.activity));
    }
    if digest.files_modified_count > 0 {
        parts.push(format!("Files modified: {}", digest.files_modified_count));
    }
    if !digest.commits.is_empty() {
        parts.push(format!("Commits: {}", digest.commits.join("; ")));
    }
    if !digest.completed_tasks.is_empty() {
        parts.push(format!(
            "Completed tasks: {}",
            digest.completed_tasks.join("; ")
        ));
    }
    if !digest.pending_tasks.is_empty() {
        parts.push(format!(
            "Pending tasks: {}",
            digest.pending_tasks.join("; ")
        ));
    }
    if !digest.decisions.is_empty() {
        parts.push(format!("Decisions: {}", digest.decisions.join("; ")));
    }
    if !digest.notes.is_empty() {
        parts.push(format!("Notes: {}", digest.notes.join("; ")));
    }

    parts.join("\n")
}

/// Build the LLM prompt for session digest.
pub(crate) fn build_digest_prompt(transcript: &str, stats_context: &str) -> String {
    let stats_section = if stats_context.is_empty() {
        String::new()
    } else {
        format!(
            r#"
## Session Stats

{stats_context}

"#
        )
    };

    format!(
        r#"你是 session 摘要器。根據以下開發對話 transcript 和統計數據，生成簡潔的 session 摘要。

要求：
- 2-4 句話，概述 session 做了什麼、達成了什麼、遇到什麼問題
- 用自然語言，像是寫給同事的交接備忘
- 如果有未完成的工作，提到 next steps
- 直接輸出摘要文字，不要加標題或格式化標記
- 使用英文撰寫
{stats_section}
## Transcript

{transcript}"#
    )
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_run_returns_false_when_disabled() {
        std::env::set_var("EDDA_BG_ENABLED", "0");
        std::env::set_var("EDDA_LLM_API_KEY", "test-key");
        assert!(!should_run("test_proj", "test_sess"));
        std::env::remove_var("EDDA_BG_ENABLED");
        std::env::remove_var("EDDA_LLM_API_KEY");
    }

    #[test]
    fn should_run_returns_false_without_api_key() {
        std::env::set_var("EDDA_BG_ENABLED", "1");
        std::env::remove_var("EDDA_LLM_API_KEY");
        assert!(!should_run("test_proj", "test_sess"));
    }

    #[test]
    fn should_run_returns_false_without_transcript() {
        std::env::set_var("EDDA_BG_ENABLED", "1");
        std::env::set_var("EDDA_LLM_API_KEY", "test-key");
        // No transcript file exists for a random project/session
        assert!(!should_run("nonexistent_proj_digest", "nonexistent_sess"));
        std::env::remove_var("EDDA_LLM_API_KEY");
    }

    #[test]
    fn should_run_returns_false_when_already_digested() {
        let pid = "test_digest_idempotent";
        let sid = "sess-digest-1";
        let _ = edda_store::ensure_dirs(pid);

        // Create a transcript so that check passes
        let transcript_dir = edda_store::project_dir(pid).join("transcripts");
        let _ = fs::create_dir_all(&transcript_dir);
        let _ = fs::write(transcript_dir.join(format!("{sid}.jsonl")), "{}");

        // Write completed digest state
        let state = DigestState {
            status: "completed".to_string(),
            digested_at: "2026-01-01T00:00:00Z".to_string(),
            transcript_hash: "test-hash".to_string(),
            summary_len: 100,
        };
        let state_path = digest_state_path(pid, sid);
        let _ = fs::create_dir_all(state_path.parent().unwrap());
        let _ = fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap());

        std::env::set_var("EDDA_BG_ENABLED", "1");
        std::env::set_var("EDDA_LLM_API_KEY", "test-key");

        assert!(!should_run(pid, sid));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_LLM_API_KEY");
    }

    #[test]
    fn build_digest_prompt_includes_transcript() {
        let prompt = build_digest_prompt("hello world transcript", "");
        assert!(prompt.contains("hello world transcript"));
        assert!(prompt.contains("session 摘要器"));
    }

    #[test]
    fn build_digest_prompt_includes_stats() {
        let stats = "Duration: 30 minutes\nFiles modified: 5";
        let prompt = build_digest_prompt("transcript text", stats);
        assert!(prompt.contains("Duration: 30 minutes"));
        assert!(prompt.contains("Files modified: 5"));
        assert!(prompt.contains("## Session Stats"));
    }

    #[test]
    fn build_digest_prompt_omits_stats_section_when_empty() {
        let prompt = build_digest_prompt("transcript text", "");
        assert!(!prompt.contains("## Session Stats"));
    }

    #[test]
    fn digest_state_roundtrip() {
        let state = DigestState {
            status: "completed".to_string(),
            digested_at: "2026-03-12T10:00:00Z".to_string(),
            transcript_hash: "blake3:abc123".to_string(),
            summary_len: 250,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: DigestState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, "completed");
        assert_eq!(parsed.transcript_hash, "blake3:abc123");
        assert_eq!(parsed.summary_len, 250);
    }

    #[test]
    fn idempotency_guard_works() {
        let pid = "test_digest_guard";
        let sid = "sess-guard-1";
        let _ = edda_store::ensure_dirs(pid);

        // Initially not digested
        assert!(!already_digested(pid, sid));

        // Save completed state
        let _ = save_digest_state(pid, sid, "hash-1", 100);
        assert!(already_digested(pid, sid));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn build_stats_context_handles_missing_digest() {
        // No prev_digest.json for a nonexistent project
        let ctx = build_stats_context("nonexistent_proj_ctx");
        assert!(ctx.is_empty());
    }

    #[test]
    fn audit_log_appends() {
        let pid = "test_digest_audit";
        let _ = edda_store::ensure_dirs(pid);

        let entry = AuditEntry {
            ts: "2026-03-12T10:00:00Z".to_string(),
            session_id: "sess-1".to_string(),
            summary_len: 200,
            cost_usd: 0.01,
            model: "test-model".to_string(),
            status: "completed".to_string(),
        };
        append_audit_log(pid, &entry).unwrap();

        let path = audit_log_path(pid);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("sess-1"));
        assert!(content.contains("test-model"));

        // Append another
        let entry2 = AuditEntry {
            ts: "2026-03-12T11:00:00Z".to_string(),
            session_id: "sess-2".to_string(),
            summary_len: 300,
            cost_usd: 0.02,
            model: "test-model".to_string(),
            status: "completed".to_string(),
        };
        append_audit_log(pid, &entry2).unwrap();

        let content2 = fs::read_to_string(&path).unwrap();
        assert_eq!(content2.lines().count(), 2);

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
