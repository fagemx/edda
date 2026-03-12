use std::path::Path;

use edda_core::event::finalize_event;
use edda_core::types::{Event, Provenance, Refs, SCHEMA_VERSION};

use super::extract::{compute_tool_ratios, extract_stats, render_digest_text};
use super::helpers::now_rfc3339;
use super::{FailedCommand, SessionStats};

pub fn build_digest_event(
    session_id: &str,
    stats: &SessionStats,
    branch: &str,
    parent_hash: Option<&str>,
    notes: &[String],
) -> anyhow::Result<Event> {
    let text = render_digest_text(session_id, stats);

    let (edit_ratio, search_ratio) =
        compute_tool_ratios(&stats.tool_call_breakdown, stats.tool_calls);

    let payload = serde_json::json!({
        "role": "system",
        "text": text,
        "tags": ["session_digest"],
        "source": "bridge:session_digest",
        "session_id": session_id,
        "session_stats": {
            "tool_calls": stats.tool_calls,
            "tool_failures": stats.tool_failures,
            "user_prompts": stats.user_prompts,
            "files_modified": stats.files_modified,
            "failed_commands": stats.failed_commands,
            "commits_made": stats.commits_made,
            "tasks_snapshot": stats.tasks_snapshot,
            "outcome": stats.outcome.to_string(),
            "duration_minutes": stats.duration_minutes,
            "nudge_count": stats.nudge_count,
            "decide_count": stats.decide_count,
            "signal_count": stats.signal_count,
            "deps_added": stats.deps_added,
            "tool_call_breakdown": stats.tool_call_breakdown,
            "edit_ratio": edit_ratio,
            "search_ratio": search_ratio,
            "model": stats.model,
            "input_tokens": stats.input_tokens,
            "output_tokens": stats.output_tokens,
            "cache_read_tokens": stats.cache_read_tokens,
            "cache_creation_tokens": stats.cache_creation_tokens,
            "estimated_cost_usd": stats.estimated_cost_usd,
            "activity": stats.activity.to_string(),
            "notes": notes,
        }
    });

    let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
    let ts = now_rfc3339();

    let mut event = Event {
        event_id,
        ts,
        event_type: "note".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            provenance: vec![Provenance {
                target: format!("session:{session_id}"),
                rel: "based_on".to_string(),
                note: Some(format!(
                    "bridge digest of session {}",
                    &session_id[..session_id.len().min(8)]
                )),
            }],
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event);
    Ok(event)
}

/// Convenience: extract stats + build event in one call.
pub fn extract_session_digest(
    session_ledger_path: &Path,
    session_id: &str,
    branch: &str,
    parent_hash: Option<&str>,
) -> anyhow::Result<Event> {
    let stats = extract_stats(session_ledger_path)?;
    build_digest_event(session_id, &stats, branch, parent_hash, &[])
}

/// Build a `cmd` milestone event for a failed Bash command.
///
/// Each failed command gets its own event with `payload.source = "bridge:cmd"`.
pub fn build_cmd_milestone_event(
    session_id: &str,
    failed_cmd: &FailedCommand,
    branch: &str,
    parent_hash: Option<&str>,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "argv": [failed_cmd.command],
        "cwd": failed_cmd.cwd,
        "exit_code": failed_cmd.exit_code,
        "duration_ms": 0,
        "stdout_blob": "",
        "stderr_blob": "",
        "source": "bridge:cmd",
        "session_id": session_id,
    });

    let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
    let ts = now_rfc3339();

    let mut event = Event {
        event_id,
        ts,
        event_type: "cmd".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            provenance: vec![Provenance {
                target: format!("session:{session_id}"),
                rel: "based_on".to_string(),
                note: Some(format!(
                    "bridge failed cmd from session {}",
                    &session_id[..session_id.len().min(8)]
                )),
            }],
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event);
    Ok(event)
}
