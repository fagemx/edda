use std::fs;

use crate::parse::now_rfc3339;
use crate::signals::{FileEditCount, SessionSignals};

use super::board::compute_board_state;
use super::heartbeat::write_claim;
use super::{autoclaim_state_path, detect_git_branch, AutoClaimState};

// ── Auto-Claim ──

/// Try to derive scope from crates/{name} or packages/{name} pattern.
fn try_crate_pattern(files: &[FileEditCount]) -> Option<(String, Vec<String>)> {
    let mut groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        let normalized = f.path.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        for (i, seg) in segments.iter().enumerate() {
            if (*seg == "crates" || *seg == "packages") && i + 1 < segments.len() {
                *groups.entry(segments[i + 1].to_string()).or_default() += f.count;
                break;
            }
        }
    }

    groups
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(label, _)| (label.clone(), vec![format!("crates/{}/*", label)]))
}

/// Try to derive scope from src/{module} pattern.
fn try_src_pattern(files: &[FileEditCount]) -> Option<(String, Vec<String>)> {
    let mut groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        let normalized = f.path.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(src_pos) = segments.iter().position(|s| *s == "src") {
            if let Some(module) = segments.get(src_pos + 1) {
                if !module.contains('.') {
                    *groups.entry(module.to_string()).or_default() += f.count;
                }
            }
        }
    }

    groups
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(label, _)| (label.clone(), vec![format!("src/{}/*", label)]))
}

/// Try to derive scope from top-level directory pattern.
/// Handles non-Rust projects (JS, Python, Go, etc.).
fn try_top_level_directory(files: &[FileEditCount]) -> Option<(String, Vec<String>)> {
    let mut groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for f in files {
        let normalized = f.path.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

        // Get first directory segment
        if let Some(first_seg) = segments.first() {
            // Skip hidden directories (start with '.')
            // Skip root-level files (no directory)
            if !first_seg.starts_with('.') && segments.len() > 1 {
                *groups.entry(first_seg.to_string()).or_default() += f.count;
            }
        }
    }

    groups
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(label, _)| (label.clone(), vec![format!("{}/*", label)]))
}

/// Derive a scope label and path globs from edited file paths.
///
/// Groups files by crate/package directory, returns the dominant group.
/// Returns `None` if no files modified or no clear grouping.
pub(super) fn derive_scope_from_files(files: &[FileEditCount]) -> Option<(String, Vec<String>)> {
    if files.is_empty() {
        return None;
    }

    // Try 1: crates/{name} or packages/{name}
    if let Some(result) = try_crate_pattern(files) {
        return Some(result);
    }

    // Try 2: src/{module}
    if let Some(result) = try_src_pattern(files) {
        return Some(result);
    }

    // Try 3: top-level directory (for non-Rust projects)
    if let Some(result) = try_top_level_directory(files) {
        return Some(result);
    }

    None
}

/// Auto-claim scope from session signals if no manual claim exists.
///
/// - Skips if session already has an explicit claim in `coordination.jsonl`
/// - Skips if derived scope is identical to last auto-claim (dedup)
/// - Writes claim event + saves state file for dedup
pub(crate) fn maybe_auto_claim(project_id: &str, session_id: &str, signals: &SessionSignals) {
    // 1. Check existing state
    let board = compute_board_state(project_id);
    let existing_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let state_path = autoclaim_state_path(project_id, session_id);
    let prev_auto = fs::read_to_string(&state_path)
        .ok()
        .and_then(|c| serde_json::from_str::<AutoClaimState>(&c).ok());

    // If a claim exists but no auto-claim state file → it was manual → skip
    if existing_claim.is_some() && prev_auto.is_none() {
        return;
    }

    // 2. Derive scope from edited files, fallback to git branch
    let (label, paths) = match derive_scope_from_files(&signals.files_modified) {
        Some(v) => v,
        None => {
            // No file edits yet (fresh session) — use git branch as fallback label
            // so the peer is visible in `edda watch` immediately (#128)
            match detect_git_branch() {
                Some(branch) => (branch, vec!["**/*".to_string()]),
                None => return,
            }
        }
    };

    // 3. Dedup: skip if scope unchanged from last auto-claim
    if let Some(ref prev) = prev_auto {
        if prev.label == label && prev.paths == paths {
            return;
        }
    }

    // 4. Write claim to coordination.jsonl
    write_claim(project_id, session_id, &label, &paths);

    // 5. Save state for dedup
    let state = AutoClaimState {
        label,
        paths,
        ts: now_rfc3339(),
        files: Default::default(),
    };
    if let Ok(data) = serde_json::to_string_pretty(&state) {
        let _ = edda_store::write_atomic(&state_path, data.as_bytes());
    }
}

/// Real-time auto-claim from a single file edit (PostToolUse path).
///
/// Maintains an incremental file set in the auto-claim state file.
/// On each call, adds the file, re-derives scope, and writes a claim
/// only if the scope changed.
pub(crate) fn maybe_auto_claim_file(project_id: &str, session_id: &str, file_path: &str) {
    let state_path = autoclaim_state_path(project_id, session_id);

    // Fast path: if no state file exists, check for manual claim via coordination.jsonl.
    // This only happens once per session (first Edit); subsequent calls find the state file.
    let state_file_content = fs::read_to_string(&state_path).ok();
    if state_file_content.is_none() {
        let board = compute_board_state(project_id);
        if board.claims.iter().any(|c| c.session_id == session_id) {
            // Manual claim exists, no auto-claim state → skip
            return;
        }
    }

    let mut state: AutoClaimState = state_file_content
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    // Add file to tracked set
    let normalized = file_path.replace('\\', "/");
    if !state.files.insert(normalized) {
        // File already tracked, scope won't change
        return;
    }

    // Derive scope from all tracked files
    let file_counts: Vec<FileEditCount> = state
        .files
        .iter()
        .map(|p| FileEditCount {
            path: p.clone(),
            count: 1,
        })
        .collect();
    let Some((label, paths)) = derive_scope_from_files(&file_counts) else {
        // Save files set even if no scope derived yet
        if let Ok(data) = serde_json::to_string_pretty(&state) {
            let _ = edda_store::write_atomic(&state_path, data.as_bytes());
        }
        return;
    };

    // Dedup: skip claim write if scope unchanged
    if state.label == label && state.paths == paths {
        // Save updated files set
        if let Ok(data) = serde_json::to_string_pretty(&state) {
            let _ = edda_store::write_atomic(&state_path, data.as_bytes());
        }
        return;
    }

    // Write claim
    write_claim(project_id, session_id, &label, &paths);
    state.label = label;
    state.paths = paths;
    state.ts = now_rfc3339();
    if let Ok(data) = serde_json::to_string_pretty(&state) {
        let _ = edda_store::write_atomic(&state_path, data.as_bytes());
    }
}

/// Remove auto-claim state file on session end.
pub(crate) fn remove_autoclaim_state(project_id: &str, session_id: &str) {
    let _ = fs::remove_file(autoclaim_state_path(project_id, session_id));
}
