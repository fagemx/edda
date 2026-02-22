//! Public render API for external integrations (CLI consumers, other bridges).
//!
//! Contains shared context rendering utilities used by both Claude and OpenClaw
//! bridges, plus thin wrappers for CLI commands.

use std::fs;
use std::path::Path;

// ── Context Boundary ──

/// Edda context boundary start marker.
pub const BOUNDARY_START: &str = "<!-- edda:start -->";

/// Edda context boundary end marker.
pub const BOUNDARY_END: &str = "<!-- edda:end -->";

/// Default max context chars (~2000 tokens). Overridable via
/// `EDDA_MAX_CONTEXT_CHARS` env var or `bridge.max_context_chars` in config.
pub const DEFAULT_MAX_CONTEXT_CHARS: usize = 8000;

/// Wrap context content with edda boundary markers for multi-plugin coexistence.
pub fn wrap_boundary(content: &str) -> String {
    format!("{BOUNDARY_START}\n{content}\n{BOUNDARY_END}")
}

/// Resolve the context char budget from env or config.
pub fn context_budget(cwd: &str) -> usize {
    std::env::var("EDDA_MAX_CONTEXT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| config_usize(cwd, "bridge.max_context_chars"))
        .unwrap_or(DEFAULT_MAX_CONTEXT_CHARS)
}

/// Truncate content to fit within the char budget, preserving UTF-8 boundaries.
pub fn apply_budget(content: &str, budget: usize) -> String {
    if content.len() <= budget {
        return content.to_string();
    }
    let cut = content.len().min(budget.saturating_sub(50));
    let safe = content.floor_char_boundary(cut);
    format!(
        "{}\n\n... (truncated to {} char budget)",
        &content[..safe],
        budget
    )
}

// ── Write-Back Protocol ──

/// Static write-back protocol text that teaches agents to use `edda decide` and `edda note`.
pub fn writeback() -> String {
    "## Write-Back Protocol\n\
     Record architectural decisions with: `edda decide \"domain.aspect=value\" --reason \"justification\"`\n\
     \n\
     Examples:\n  \
     `edda decide \"db.engine=postgres\" --reason \"need JSONB for flexible metadata\"`\n  \
     `edda decide \"auth.method=JWT\" --reason \"stateless, scales horizontally\"`\n  \
     `edda decide \"api.style=REST\" --reason \"client SDK compatibility\"`\n\
     \n\
     Do NOT record: formatting changes, test fixes, minor refactors, dependency bumps.\n\
     \n\
     Before ending a session, summarize open context:\n  \
     `edda note \"completed X; decided Y; next: Z\" --tag session`"
        .to_string()
}

// ── Workspace Context ──

/// Workspace context rendered from the `.edda/` ledger in `cwd`.
///
/// Returns `None` if no workspace exists at `cwd`.
pub fn workspace(cwd: &str, budget: usize) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)?;
    let ledger = edda_ledger::Ledger::open(&root).ok()?;
    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());

    let max_depth: usize = std::env::var("EDDA_WORKSPACE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    // Try with requested depth, reduce if over budget
    for d in (1..=max_depth).rev() {
        let opt = edda_derive::DeriveOptions { depth: d };
        if let Ok(raw) = edda_derive::render_context(&ledger, &branch, opt) {
            let mut section = transform_context_to_section(&raw);
            // If edda ledger has no commit events, fall back to `git log`
            supplement_git_commits(&mut section, cwd_path, d);
            // Hint for LLM agents to discover the ask tool
            section.push_str(
                "\n> Use edda_ask MCP tool or `edda ask <keyword>` for detailed decision history\n",
            );
            if section.len() <= budget {
                return Some(section);
            }
        }
    }
    None
}

/// Transform `render_context` output into a pack-embeddable section.
/// Replaces `# CONTEXT SNAPSHOT` header with `## Workspace Context`
/// and removes the `## How to cite evidence` footer.
fn transform_context_to_section(raw: &str) -> String {
    let mut out = String::new();
    out.push_str("## Workspace Context\n\n");
    let mut skip_header = true;
    let mut skip_cite = false;
    for line in raw.lines() {
        if skip_header && line.starts_with("# CONTEXT SNAPSHOT") {
            skip_header = false;
            continue;
        }
        if line.starts_with("## How to cite evidence") {
            skip_cite = true;
            continue;
        }
        if skip_cite {
            continue;
        }
        skip_header = false;
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// If the workspace section has empty "Recent Commits", supplement with `git log --oneline`.
fn supplement_git_commits(section: &mut String, cwd: &Path, depth: usize) {
    let empty_marker = format!("## Recent Commits (last {depth})\n- (none)\n");
    if !section.contains(&empty_marker) {
        return;
    }
    let Ok(output) = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("-{depth}")])
        .current_dir(cwd)
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let formatted: String = text.lines().map(|l| format!("- {l}\n")).collect();
    let replacement = format!("## Recent Commits (last {depth})\n{formatted}");
    *section = section.replace(&empty_marker, &replacement);
}

// ── Workspace Config ──

/// Read a boolean value from `.edda/config.json` in the workspace.
/// Supports dot-notation keys (e.g. "bridge.auto_digest").
pub fn config_bool(cwd: &str, key: &str) -> Option<bool> {
    config_value(cwd, key)?.as_bool()
}

/// Read a usize value from `.edda/config.json` in the workspace.
pub fn config_usize(cwd: &str, key: &str) -> Option<usize> {
    config_value(cwd, key)?.as_u64().map(|v| v as usize)
}

/// Read a raw JSON value from `.edda/config.json` using dot-notation keys.
pub fn config_value(cwd: &str, key: &str) -> Option<serde_json::Value> {
    if cwd.is_empty() {
        return None;
    }
    let root = edda_ledger::EddaPaths::find_root(Path::new(cwd))?;
    let config_path = root.join(".edda").join("config.json");
    let content = fs::read_to_string(&config_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let mut current = val;
    for part in key.split('.') {
        current = current.get(part)?.clone();
    }
    Some(current)
}

// ── High-Level Wrappers (CLI Commands) ──

/// Full L2 coordination protocol (peers, claims, bindings, requests).
///
/// Returns `None` in solo mode with no bindings.
pub fn coordination(project_id: &str, session_id: &str) -> Option<String> {
    crate::peers::render_coordination_protocol(project_id, session_id, "")
}

/// Read the existing hot pack file (recent turns summary).
///
/// Returns `None` if no pack has been built yet for this project.
/// Note: this reads the last-built pack, not a fresh build.
pub fn pack(project_id: &str) -> Option<String> {
    crate::dispatch::read_hot_pack(project_id)
}

/// Active plan excerpt from `.claude/plans/*.md`.
///
/// Returns `None` if no plan file exists.
pub fn plan(project_id: Option<&str>) -> Option<String> {
    crate::dispatch::render_active_plan(project_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_boundary_adds_markers() {
        let content = "hello world";
        let wrapped = wrap_boundary(content);
        assert!(wrapped.starts_with(BOUNDARY_START));
        assert!(wrapped.ends_with(BOUNDARY_END));
        assert!(wrapped.contains("hello world"));
    }

    #[test]
    fn apply_budget_no_truncation() {
        let content = "short content";
        let result = apply_budget(content, 8000);
        assert_eq!(result, content);
    }

    #[test]
    fn apply_budget_truncates_long_content() {
        let content = "x".repeat(10000);
        let result = apply_budget(&content, 500);
        assert!(result.len() <= 550);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn context_budget_uses_env_var() {
        std::env::set_var("EDDA_MAX_CONTEXT_CHARS", "1234");
        let budget = context_budget("");
        assert_eq!(budget, 1234);
        std::env::remove_var("EDDA_MAX_CONTEXT_CHARS");
    }

    #[test]
    fn context_budget_default_without_config() {
        std::env::remove_var("EDDA_MAX_CONTEXT_CHARS");
        let budget = context_budget("/nonexistent/dir");
        assert_eq!(budget, DEFAULT_MAX_CONTEXT_CHARS);
    }

    #[test]
    fn writeback_contains_decide_command() {
        let text = writeback();
        assert!(text.contains("edda decide"));
        assert!(text.contains("edda note"));
    }

    #[test]
    fn transform_context_strips_header_and_cite() {
        let raw = "# CONTEXT SNAPSHOT\n\n## Project (main)\n- head: main\n\n## How to cite evidence\n- Use event_id\n";
        let section = transform_context_to_section(raw);
        assert!(section.starts_with("## Workspace Context\n"));
        assert!(section.contains("## Project (main)"));
        assert!(!section.contains("# CONTEXT SNAPSHOT"));
        assert!(!section.contains("How to cite evidence"));
        assert!(!section.contains("Use event_id"));
    }
}
