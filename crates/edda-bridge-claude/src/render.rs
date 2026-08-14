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

/// Drop complete context sections/items to fit within the char budget.
pub fn apply_budget(content: &str, budget: usize) -> String {
    if content.len() <= budget {
        return content.to_string();
    }

    let items = split_budget_items(content);
    if items.len() == 1 && items[0].title.is_none() {
        return format!(
            "[edda: dropped_items=1; truncated as one whole item; no partial content; {budget} char budget]\n"
        );
    }

    let mut dropped_items = items.len();
    let mut selected = Vec::new();
    for _ in 0..4 {
        selected = select_budget_items(&items, budget, dropped_items);
        let next_dropped = items.len().saturating_sub(selected.len());
        if next_dropped == dropped_items {
            break;
        }
        dropped_items = next_dropped;
    }

    render_budget_items(
        &items,
        &selected,
        items.len().saturating_sub(selected.len()),
    )
}

#[derive(Debug)]
struct BudgetItem {
    title: Option<String>,
    text: String,
    salience: usize,
    index: usize,
}

fn split_budget_items(content: &str) -> Vec<BudgetItem> {
    let mut boundaries = Vec::new();
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(title) = trimmed.strip_prefix("## ") {
            boundaries.push((offset, Some(title.trim().to_string())));
        }
        offset += line.len();
    }

    if boundaries.is_empty() {
        return vec![BudgetItem {
            title: None,
            text: content.to_string(),
            salience: 0,
            index: 0,
        }];
    }

    if boundaries[0].0 > 0 {
        boundaries.insert(0, (0, None));
    }

    boundaries
        .iter()
        .enumerate()
        .map(|(index, (start, title))| {
            let end = boundaries
                .get(index + 1)
                .map(|(offset, _)| *offset)
                .unwrap_or(content.len());
            let title = title.clone();
            BudgetItem {
                salience: title.as_deref().map(section_salience).unwrap_or(95),
                title,
                text: content[*start..end].to_string(),
                index,
            }
        })
        .collect()
}

fn section_salience(title: &str) -> usize {
    let title = title.to_ascii_lowercase();
    if title.contains("goal") || title.contains("binding") {
        100
    } else if title.contains("ratified")
        || title.contains("checkpoint")
        || title.contains("constraint")
    {
        90
    } else if title.contains("workspace") {
        80
    } else if title.contains("recent") {
        60
    } else if title.contains("doctrine") {
        40
    } else if title.contains("coordination") {
        10
    } else {
        20
    }
}

fn dropped_summary(dropped_items: usize) -> String {
    format!("\n[edda: dropped_items={dropped_items} whole sections/items]\n")
}

fn render_budget_items(items: &[BudgetItem], selected: &[usize], dropped_items: usize) -> String {
    let mut selected = selected.to_vec();
    selected.sort_by_key(|index| items[*index].index);

    let mut out = String::new();
    for index in selected {
        out.push_str(&items[index].text);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&dropped_summary(dropped_items));
    out
}

fn select_budget_items(items: &[BudgetItem], budget: usize, dropped_items: usize) -> Vec<usize> {
    let mut ranked: Vec<usize> = (0..items.len()).collect();
    ranked.sort_by(|a, b| {
        items[*b]
            .salience
            .cmp(&items[*a].salience)
            .then_with(|| items[*a].index.cmp(&items[*b].index))
    });

    let mut selected = Vec::new();
    for index in ranked {
        selected.push(index);
        if render_budget_items(items, &selected, dropped_items).len() > budget {
            selected.pop();
        }
    }
    selected
}

// ── Write-Back Protocol ──

/// Static write-back protocol text that teaches agents the read verbs first
/// (ask/search), then the write verbs (decide/note/task). Read before you
/// write: an unqueried ledger is a write-only ledger.
pub fn writeback() -> String {
    "## Write-Back Protocol\n\
     Read before you write — the ledger answers questions:\n  \
     `edda ask \"<domain or keyword>\"` — has this been decided already? (run before any `edda decide`)\n  \
     `edda search query \"<keyword>\"` — has this been done before? (run before building)\n\
     \n\
     Record architectural decisions with: `edda decide \"domain.aspect=value\" --reason \"justification\"`\n\
     \n\
     Examples:\n  \
     `edda decide \"db.engine=postgres\" --reason \"need JSONB for flexible metadata\"`\n  \
     `edda decide \"auth.method=JWT\" --reason \"stateless, scales horizontally\"`\n  \
     `edda decide \"api.style=REST\" --reason \"client SDK compatibility\"`\n\
     \n\
     Do NOT record: formatting changes, test fixes, minor refactors, dependency bumps.\n\
     \n\
     Decisions you record are agent-authored and land in the *unratified* tier — \
     recorded, not binding. Only the operator confers binding authority (via `edda ratify`); \
     do not ratify your own decisions.\n\
     \n\
     Before ending a session, summarize open context:\n  \
     `edda note \"completed X; decided Y; next: Z\" --tag session`\n\
     \n\
     Hand off work on the task rail:\n  \
     `edda task new \"run integration tests\" --assignee tester --after 12`\n  \
     `edda task start 13` → work → `edda task done 13 --receipt \"110/601 green, artifact in dist/\"`\n\
     A done without a receipt does not exist; done unlocks successor tasks (`edda task list`)."
        .to_string()
}

// ── Workspace Context ──

/// The Fleet section: what sibling projects ruled, and what waits there
/// (GH-408).
///
/// Returns `None` for a solo project or a quiet fleet — the pack says nothing
/// rather than heading an empty list.
pub fn fleet(cwd: &str, budget: usize) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    crate::peers::fleet_section(Path::new(cwd), budget)
}

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
            if section.len() <= budget {
                // Hint for LLM agents to discover the ask tool
                section.push_str(
                    "\n> Use edda_ask MCP tool or `edda ask <keyword>` for detailed decision history\n",
                );
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
    fn apply_budget_drops_long_content_as_one_item() {
        let content = "x".repeat(10000);
        let result = apply_budget(&content, 500);
        assert!(result.len() <= 500);
        assert!(result.contains("dropped_items=1"));
        assert!(!result.contains(&"x".repeat(100)));
        assert!(!result.contains("truncated to 500"));
    }

    #[test]
    fn apply_budget_keeps_schema_salient_section_and_drops_complete_items_deterministically() {
        let content = concat!(
            "## Coordination\nCOORDINATION_DROP_012345678901234567890123456789\n\n",
            "## Goals\nGOAL_KEEP\n"
        );

        let first = apply_budget(content, 65);
        let second = apply_budget(content, 65);

        assert_eq!(first, second, "same input must produce the same output");
        assert!(first.contains("## Goals"));
        assert!(first.contains("GOAL_KEEP"));
        assert!(!first.contains("## Coordination"));
        assert!(!first.contains("COORDINATION_DROP"));
        assert!(first.contains("dropped_items=1"));
    }

    #[test]
    fn context_budget_uses_env_var() {
        crate::with_env_guard(&[("EDDA_MAX_CONTEXT_CHARS", Some("1234"))], || {
            let budget = context_budget("");
            assert_eq!(budget, 1234);
        });
    }

    #[test]
    fn context_budget_default_without_config() {
        crate::with_env_guard(&[("EDDA_MAX_CONTEXT_CHARS", None)], || {
            let budget = context_budget("/nonexistent/dir");
            assert_eq!(budget, DEFAULT_MAX_CONTEXT_CHARS);
        });
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
