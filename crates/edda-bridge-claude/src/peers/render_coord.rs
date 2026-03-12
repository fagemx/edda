use crate::signals::FileEditCount;

use super::autoclaim::derive_scope_from_files;
use super::board::compute_board_state;
use super::discovery::discover_active_peers;
use super::heartbeat::read_heartbeat;
use super::helpers::{self, format_age, format_peer_suffix, truncate_to_budget};
use super::{
    protocol_budget, BoardState, PeerSummary, RequestEntry, SessionHeartbeat, PEER_UPDATES_BUDGET,
};

// ── Directive Renderer ──

/// Render the full coordination protocol section for SessionStart injection.
///
/// - Multi-session: full protocol (peers, claims, bindings, commits, requests).
/// - Solo with bindings: "## Binding Decisions" section only.
/// - Solo without bindings: returns None.
pub fn render_coordination_protocol(
    project_id: &str,
    session_id: &str,
    _cwd: &str,
) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);

    // Resolve my label to identify which requests are "to me"
    let my_label: String = board
        .claims
        .iter()
        .find(|c| c.session_id == session_id)
        .map(|c| c.label.clone())
        .or_else(|| read_heartbeat(project_id, session_id).map(|hb| hb.label))
        .unwrap_or_default();

    // Collect unacked requests addressed to me (before rendering)
    let unacked_from_labels: Vec<String> = if !my_label.is_empty() {
        board
            .requests
            .iter()
            .filter(|r| r.to_label == my_label)
            .filter(|r| {
                !board
                    .request_acks
                    .iter()
                    .any(|a| a.from_label == r.from_label && a.acker_session == session_id)
            })
            .map(|r| r.from_label.clone())
            .collect()
    } else {
        Vec::new()
    };

    let result = render_coordination_protocol_with(&peers, &board, project_id, session_id);

    // Auto-ack: the agent has now "seen" these requests at SessionStart.
    // Write ack events so they won't appear in subsequent renders.
    for from_label in &unacked_from_labels {
        super::heartbeat::write_request_ack(project_id, session_id, from_label);
    }

    result
}

/// Generate a suggested `edda claim` command based on available session context.
///
/// Priority: focus_files → branch name → heartbeat label → generic template.
pub(super) fn suggest_claim_command(label: &str, heartbeat: &Option<SessionHeartbeat>) -> String {
    if let Some(hb) = heartbeat {
        // Try to derive paths from focus files
        if !hb.focus_files.is_empty() {
            let files: Vec<FileEditCount> = hb
                .focus_files
                .iter()
                .map(|p| FileEditCount {
                    path: p.clone(),
                    count: 1,
                })
                .collect();
            if let Some((derived_label, paths)) = derive_scope_from_files(&files) {
                let claim_label = if !label.is_empty() {
                    label
                } else {
                    &derived_label
                };
                return format!(
                    "`edda claim \"{}\" --paths \"{}\"`",
                    claim_label,
                    paths.join("\" --paths \"")
                );
            }
        }
        // Try branch-based suggestion (label wins over branch if available)
        if let Some(ref branch) = hb.branch {
            let branch_label = branch.split('/').next_back().unwrap_or(branch);
            if !branch_label.is_empty() && branch_label != "main" && branch_label != "master" {
                let claim_label = if !label.is_empty() {
                    label
                } else {
                    branch_label
                };
                return format!("`edda claim \"{claim_label}\" --paths \"<your-scope>/*\"`");
            }
        }
    }
    // Fallback with label or generic
    if !label.is_empty() {
        format!("`edda claim \"{label}\" --paths \"<your-scope>/*\"`")
    } else {
        "`edda claim \"<your-task>\" --paths \"<your-scope>/*\"`".to_string()
    }
}

/// Render full coordination protocol using pre-computed peers and board state.
///
/// "Pre-computed" refers to `peers` and `board` only — heartbeat writes and
/// other per-session I/O still happen at the call site in `dispatch.rs`.
pub fn render_coordination_protocol_with(
    peers: &[PeerSummary],
    board: &BoardState,
    project_id: &str,
    session_id: &str,
) -> Option<String> {
    let budget = protocol_budget();

    if peers.is_empty() {
        // Solo mode: only render bindings (if any exist)
        if board.bindings.is_empty() {
            return None;
        }
        let mut lines = vec!["## Binding Decisions".to_string()];
        for d in board.bindings.iter().rev().take(5) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
        let result = lines.join("\n");
        return Some(if result.len() > budget {
            truncate_to_budget(&result, budget)
        } else {
            result
        });
    }

    let my_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let my_heartbeat = read_heartbeat(project_id, session_id);

    // Resolve identity: explicit claim wins, heartbeat label is fallback
    let my_label: &str = if let Some(claim) = my_claim {
        claim.label.as_str()
    } else if let Some(ref hb) = my_heartbeat {
        hb.label.as_str()
    } else {
        ""
    };

    let mut lines = Vec::new();

    lines.push(format!(
        "## Coordination Protocol\nYou are one of {} agents working simultaneously.",
        peers.len() + 1
    ));

    // My scope + L2 command instructions
    if let Some(claim) = my_claim {
        lines.push(format!(
            "Your scope: **{}** ({})",
            claim.label,
            claim.paths.join(", ")
        ));
    } else {
        // No claim yet — provide actionable nudge with specific suggestion
        let suggested = suggest_claim_command(my_label, &my_heartbeat);
        lines.push(format!(
            "**Claim your scope** so peers know what you're working on:\n{suggested}",
        ));
    }
    lines.push("Message a peer: `edda request \"peer-label\" \"your message\"`".to_string());

    // Peer activity (tasks + focus files)
    let active_peers: Vec<&PeerSummary> = peers
        .iter()
        .filter(|p| !p.task_subjects.is_empty() || !p.focus_files.is_empty())
        .collect();
    if !active_peers.is_empty() {
        lines.push("### Peers Working On".to_string());
        for p in active_peers.iter().take(5) {
            let age = format_age(p.age_secs);
            let branch_suffix = format_peer_suffix(p.branch.as_deref(), p.current_phase.as_deref());
            if !p.task_subjects.is_empty() {
                for t in p.task_subjects.iter().take(2) {
                    lines.push(format!("- {} ({age}){branch_suffix}: {t}", p.label));
                }
            } else if !p.focus_files.is_empty() {
                let files: Vec<&str> = p
                    .focus_files
                    .iter()
                    .take(2)
                    .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f.as_str()))
                    .collect();
                lines.push(format!(
                    "- {} ({age}){branch_suffix}: editing {}",
                    p.label,
                    files.join(", ")
                ));
            }
        }
    }

    // Off-limits
    let peer_claims: Vec<&PeerSummary> = peers
        .iter()
        .filter(|p| !p.claimed_paths.is_empty())
        .collect();
    if !peer_claims.is_empty() {
        lines.push("### Off-limits (other agents active)".to_string());
        for p in peer_claims.iter().take(5) {
            let age = format_age(p.age_secs);
            lines.push(format!(
                "- {} → Agent {} ({age})",
                p.claimed_paths.join(", "),
                p.label
            ));
        }
    }

    // Binding decisions
    if !board.bindings.is_empty() {
        lines.push("### Binding Decisions".to_string());
        for d in board.bindings.iter().rev().take(5) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
    }

    // Recent commits from peers (sourced from heartbeat, not coordination log)
    let peer_commits: Vec<(&str, &str)> = peers
        .iter()
        .flat_map(|p| {
            p.recent_commits
                .iter()
                .map(move |c| (p.label.as_str(), c.as_str()))
        })
        .take(5)
        .collect();
    if !peer_commits.is_empty() {
        lines.push("### Recent Peer Commits".to_string());
        for (label, commit) in &peer_commits {
            lines.push(format!("- {commit} ({label})"));
        }
    }

    // Requests to me (using resolved my_label from claim or heartbeat fallback)
    // Filter out already-acked requests so stale entries don't accumulate.
    let my_requests: Vec<&RequestEntry> = board
        .requests
        .iter()
        .filter(|r| r.to_label == my_label && !my_label.is_empty())
        .filter(|r| {
            !board
                .request_acks
                .iter()
                .any(|a| a.from_label == r.from_label && a.acker_session == session_id)
        })
        .collect();
    if !my_requests.is_empty() {
        lines.push("### Requests to you".to_string());
        for r in my_requests.iter().take(3) {
            lines.push(format!("- Agent {}: \"{}\"", r.from_label, r.message));
        }
    }

    let result = lines.join("\n");

    // Apply budget
    if result.len() > budget {
        Some(truncate_to_budget(&result, budget))
    } else {
        Some(result)
    }
}

/// Render lightweight peer updates for UserPromptSubmit (only new bindings/requests).
///
/// - Multi-session: peers header + tasks + bindings + requests.
/// - Solo with bindings: binding lines only (no header).
/// - Solo without bindings: returns None.
#[cfg(test)]
pub(crate) fn render_peer_updates(project_id: &str, session_id: &str) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);
    render_peer_updates_with(&peers, &board, project_id, session_id)
}

/// Render lightweight peer updates using pre-computed peers and board state.
///
/// "Pre-computed" refers to `peers` and `board` only — heartbeat writes and
/// other per-session I/O still happen at the call site in `dispatch.rs`.
pub(crate) fn render_peer_updates_with(
    peers: &[PeerSummary],
    board: &BoardState,
    project_id: &str,
    session_id: &str,
) -> Option<String> {
    if peers.is_empty() {
        // Solo mode: only render bindings (if any)
        if board.bindings.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for d in board.bindings.iter().rev().take(3) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
        let result = lines.join("\n");
        return Some(if result.len() > PEER_UPDATES_BUDGET {
            truncate_to_budget(&result, PEER_UPDATES_BUDGET)
        } else {
            result
        });
    }

    let mut lines = vec![format!("## Peers ({} active)", peers.len())];

    // L2 instructions (condensed single line)
    lines.push(
        "Claim: `edda claim \"label\" --paths \"path\"` | Message: `edda request \"peer\" \"msg\"`"
            .to_string(),
    );

    // Peer activity (tasks → focus files → bare label)
    for p in peers.iter().take(3) {
        let age = format_age(p.age_secs);
        let branch_suffix = format_peer_suffix(p.branch.as_deref(), p.current_phase.as_deref());
        if !p.task_subjects.is_empty() {
            for t in p.task_subjects.iter().take(1) {
                lines.push(format!("- {} ({age}){branch_suffix}: {t}", p.label));
            }
        } else if !p.focus_files.is_empty() {
            let file = p.focus_files[0]
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&p.focus_files[0]);
            lines.push(format!(
                "- {} ({age}){branch_suffix}: editing {file}",
                p.label
            ));
        } else {
            lines.push(format!("- {} ({age}){branch_suffix}", p.label));
        }
    }

    // Latest bindings (max 3)
    if !board.bindings.is_empty() {
        for d in board.bindings.iter().rev().take(3) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
    }

    // Requests to current session (claim label → heartbeat label fallback)
    let my_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let my_heartbeat = read_heartbeat(project_id, session_id);
    let my_label: &str = if let Some(claim) = my_claim {
        claim.label.as_str()
    } else if let Some(ref hb) = my_heartbeat {
        hb.label.as_str()
    } else {
        ""
    };
    // Filter out acked requests so stale entries don't appear in peer updates.
    let my_requests: Vec<&RequestEntry> = board
        .requests
        .iter()
        .filter(|r| r.to_label == my_label && !my_label.is_empty())
        .filter(|r| {
            !board
                .request_acks
                .iter()
                .any(|a| a.from_label == r.from_label && a.acker_session == session_id)
        })
        .collect();
    if !my_requests.is_empty() {
        for r in my_requests.iter().take(2) {
            lines.push(format!(
                "- Request from {}: \"{}\"",
                r.from_label, r.message
            ));
        }
    }

    let result = lines.join("\n");
    if result.len() > PEER_UPDATES_BUDGET {
        Some(truncate_to_budget(&result, PEER_UPDATES_BUDGET))
    } else {
        Some(result)
    }
}

// ── Coordination Diff (Real-time Delta Injection) ──

/// Maximum chars for the coordination diff section.
const COORD_DIFF_BUDGET: usize = 200;

/// Maximum number of events to include in a single diff injection.
const COORD_DIFF_MAX_EVENTS: usize = 5;

/// Render new coordination events since the last injection for this session.
///
/// Reads `coordination.jsonl` from the stored byte offset, parses new lines,
/// filters out own events and low-priority types, and returns a compact diff.
/// Updates the offset after reading.
///
/// Returns `None` if no new relevant events exist.
pub(crate) fn render_coord_diff(project_id: &str, session_id: &str) -> Option<String> {
    use super::{coordination_path, CoordEvent, CoordEventType};
    use crate::state::{read_coord_offset, write_coord_offset};
    use std::io::Read;

    let coord_path = coordination_path(project_id);
    let file_len = std::fs::metadata(&coord_path).ok()?.len();

    // Check if offset was ever seeded (by SessionStart). If not, seed it now
    // and skip this cycle to avoid injecting all historical events.
    let offset_path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("coord_offset.{session_id}"));
    if !offset_path.exists() {
        write_coord_offset(project_id, session_id, file_len);
        return None;
    }

    let offset = read_coord_offset(project_id, session_id);

    // Compaction guard: if file shrank, reset offset
    let effective_offset = if file_len < offset { 0 } else { offset };

    // No new data
    if file_len == effective_offset {
        return None;
    }

    // Read bytes from offset to EOF
    let mut file = std::fs::File::open(&coord_path).ok()?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(effective_offset)).ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;

    // Update offset to current file end
    write_coord_offset(project_id, session_id, file_len);

    let now_epoch = {
        let now = time::OffsetDateTime::now_utc();
        now.unix_timestamp() as u64
    };

    let mut diff_lines: Vec<String> = Vec::new();

    for line in tail.lines() {
        if diff_lines.len() >= COORD_DIFF_MAX_EVENTS {
            break;
        }
        let event: CoordEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip own events
        if event.session_id == session_id {
            continue;
        }

        // Skip low-priority event types
        match event.event_type {
            CoordEventType::Unclaim
            | CoordEventType::TaskCompleted
            | CoordEventType::SubagentCompleted
            | CoordEventType::RequestAck => continue,
            _ => {}
        }

        let age = helpers::parse_rfc3339_to_epoch(&event.ts)
            .map(|ts| helpers::format_age(now_epoch.saturating_sub(ts)))
            .unwrap_or_else(|| "just now".to_string());

        let label = event.payload["label"]
            .as_str()
            .or_else(|| event.payload["from_label"].as_str())
            .or_else(|| event.payload["by_label"].as_str())
            .unwrap_or("peer");

        let rendered = match event.event_type {
            CoordEventType::Claim => {
                let paths: Vec<&str> = event.payload["paths"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                format!("- {label} claimed {} ({age})", paths.join(", "))
            }
            CoordEventType::Binding => {
                let key = event.payload["key"].as_str().unwrap_or("?");
                let value = event.payload["value"].as_str().unwrap_or("?");
                format!("- {label} decided \"{key}={value}\" ({age})")
            }
            CoordEventType::Request => {
                let msg = event.payload["message"].as_str().unwrap_or("");
                let to = event.payload["to_label"].as_str().unwrap_or("?");
                format!("- {label} -> {to}: \"{msg}\" ({age})")
            }
            // Already filtered above
            _ => continue,
        };

        diff_lines.push(rendered);
    }

    if diff_lines.is_empty() {
        return None;
    }

    let mut result = format!("[coordination update]\n{}", diff_lines.join("\n"));
    if result.len() > COORD_DIFF_BUDGET {
        result = helpers::truncate_to_budget(&result, COORD_DIFF_BUDGET);
    }
    Some(result)
}
