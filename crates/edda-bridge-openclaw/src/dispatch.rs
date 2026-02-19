use std::fs;
use std::path::Path;

use crate::parse::*;

// ── Hook Result ──

/// Result from an OpenClaw hook dispatch.
///
/// - `stdout`: JSON string to print to stdout (consumed by TS plugin)
/// - `stderr`: warning message to print to stderr
#[derive(Debug, Default, Clone)]
pub struct HookResult {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl HookResult {
    pub fn output(stdout: String) -> Self {
        Self {
            stdout: Some(stdout),
            stderr: None,
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }
}

// ── Context Boundary ──

const EDDA_BOUNDARY_START: &str = "<!-- edda:start -->";
const EDDA_BOUNDARY_END: &str = "<!-- edda:end -->";

const DEFAULT_MAX_CONTEXT_CHARS: usize = 8000;

fn wrap_context_boundary(content: &str) -> String {
    format!("{EDDA_BOUNDARY_START}\n{content}\n{EDDA_BOUNDARY_END}")
}

fn context_budget(workspace_dir: &str) -> usize {
    std::env::var("EDDA_MAX_CONTEXT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| read_workspace_config_usize(workspace_dir, "bridge.max_context_chars"))
        .unwrap_or(DEFAULT_MAX_CONTEXT_CHARS)
}

fn apply_context_budget(content: &str, budget: usize) -> String {
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

fn render_write_back_protocol() -> String {
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

// ── Hook dispatch ──

/// Main hook entrypoint: parse stdin JSON, dispatch by hook_event_name.
///
/// Returns `HookResult` with optional stdout JSON.
/// Fail-open: errors never crash — always returns Ok.
pub fn hook_entrypoint_from_stdin(stdin: &str) -> anyhow::Result<HookResult> {
    if stdin.trim().is_empty() {
        return Ok(HookResult::empty());
    }

    let envelope = match parse_hook_stdin(stdin) {
        Ok(e) => e,
        Err(_) => {
            // Fail-open: malformed JSON → return ok
            return Ok(ok_json());
        }
    };

    let project_id = resolve_project_id(&envelope.workspace_dir);
    let _ = edda_store::ensure_dirs(&project_id);

    // Append to session ledger
    let _ = append_to_session_ledger(&project_id, &envelope.session_id, &envelope);

    match envelope.hook_event_name.as_str() {
        "before_agent_start" => {
            dispatch_before_agent_start(&project_id, &envelope)
        }
        "agent_end" => {
            dispatch_agent_end(&project_id, &envelope)
        }
        _ => {
            // Unknown event — pass through silently
            Ok(ok_json())
        }
    }
}

/// Return a minimal `{ "ok": true }` JSON response.
fn ok_json() -> HookResult {
    HookResult::output(r#"{"ok":true}"#.to_string())
}

// ── before_agent_start ──

/// Generate context for injection at session start.
///
/// 1. Auto-digest previous sessions
/// 2. Render workspace section (decisions, notes, commits)
/// 3. Render write-back protocol
/// 4. Render coordination protocol (peers)
/// 5. Apply budget and wrap with boundary markers
/// 6. Return as `{ "prependContext": "..." }`
fn dispatch_before_agent_start(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    // Auto-digest previous sessions first
    let _digest_warning = run_auto_digest(project_id, session_id, cwd);

    let mut parts: Vec<String> = Vec::new();

    // Workspace context (decisions, notes, recent commits)
    if let Some(ws) = render_workspace_section(cwd) {
        parts.push(ws);
    }

    // Write-back protocol (always)
    parts.push(render_write_back_protocol());

    // Coordination protocol (multi-session awareness)
    if let Some(coord) =
        edda_bridge_claude::peers::render_coordination_protocol(project_id, session_id, cwd)
    {
        parts.push(coord);
    }

    let content = parts.join("\n\n");
    let budget = context_budget(cwd);
    let budgeted = apply_context_budget(&content, budget);
    let wrapped = wrap_context_boundary(&budgeted);

    let output = serde_json::json!({ "prependContext": wrapped });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

// ── agent_end ──

/// Handle session end: trigger auto-digest.
fn dispatch_agent_end(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    // Auto-digest this session
    if !session_id.is_empty() {
        let _ = edda_bridge_claude::digest::digest_session_manual(
            project_id, session_id, cwd, true,
        );
    }

    Ok(ok_json())
}

// ── Auto-digest ──

fn run_auto_digest(project_id: &str, current_session_id: &str, cwd: &str) -> Option<String> {
    let enabled = match std::env::var("EDDA_BRIDGE_AUTO_DIGEST") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.auto_digest").unwrap_or(true),
    };
    if !enabled {
        return None;
    }

    let lock_timeout_ms: u64 = std::env::var("EDDA_BRIDGE_LOCK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

    let digest_failed_cmds = match std::env::var("EDDA_BRIDGE_DIGEST_FAILED_CMDS") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.digest_failed_cmds").unwrap_or(true),
    };

    match edda_bridge_claude::digest::digest_previous_sessions_with_opts(
        project_id,
        current_session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
    ) {
        edda_bridge_claude::digest::DigestResult::Written { event_id } => {
            eprintln!("[edda] digested previous session -> {event_id}");
            None
        }
        edda_bridge_claude::digest::DigestResult::PermanentFailure(warning) => Some(warning),
        _ => None,
    }
}

// ── Workspace Context ──

/// Render workspace context section from the `.edda/` ledger.
fn render_workspace_section(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)?;
    let ledger = edda_ledger::Ledger::open(&root).ok()?;
    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());

    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);

    let max_depth: usize = std::env::var("EDDA_WORKSPACE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    for d in (1..=max_depth).rev() {
        let opt = edda_derive::DeriveOptions { depth: d };
        if let Ok(raw) = edda_derive::render_context(&ledger, &branch, opt) {
            let mut section = transform_context_to_section(&raw);
            supplement_git_commits(&mut section, cwd_path, d);
            if section.len() <= workspace_budget {
                return Some(section);
            }
        }
    }
    None
}

/// Transform `render_context` output into a pack-embeddable section.
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

// ── Config helpers ──

fn read_workspace_config_bool(cwd: &str, key: &str) -> Option<bool> {
    read_workspace_config_value(cwd, key)?.as_bool()
}

fn read_workspace_config_usize(cwd: &str, key: &str) -> Option<usize> {
    read_workspace_config_value(cwd, key)?
        .as_u64()
        .map(|v| v as usize)
}

fn read_workspace_config_value(cwd: &str, key: &str) -> Option<serde_json::Value> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_unknown_event_returns_ok() {
        let stdin = r#"{"hook_event_name":"some_future_event","session_id":"s1","workspace_dir":"."}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
    }

    #[test]
    fn dispatch_malformed_json_returns_ok() {
        let stdin = "this is not json";
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
    }

    #[test]
    fn dispatch_empty_stdin_returns_empty() {
        let result = hook_entrypoint_from_stdin("").unwrap();
        assert!(result.stdout.is_none());
        assert!(result.stderr.is_none());
    }

    #[test]
    fn dispatch_before_agent_start_returns_context() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let stdin = serde_json::json!({
            "hook_event_name": "before_agent_start",
            "session_id": "oc-test-1",
            "session_key": "agent:main:oc-test-1",
            "agent_id": "main",
            "workspace_dir": ".",
            "event_data": { "prompt": "hello" }
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some(), "should return context");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["prependContext"].as_str().unwrap();
        assert!(ctx.contains("Write-Back Protocol"), "should contain write-back protocol");
        assert!(ctx.contains("edda decide"), "should contain decide instruction");
        assert!(ctx.contains(EDDA_BOUNDARY_START), "should have boundary start");
        assert!(ctx.contains(EDDA_BOUNDARY_END), "should have boundary end");

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn dispatch_before_agent_start_empty_workspace() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "before_agent_start",
            "session_id": "oc-test-2",
            "agent_id": "main",
            "workspace_dir": tmp.path().to_str().unwrap(),
            "event_data": {}
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some(), "should return context even without .edda/");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["prependContext"].as_str().unwrap();
        assert!(ctx.contains("Write-Back Protocol"), "write-back protocol always fires");

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn dispatch_agent_end_returns_ok() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let stdin = serde_json::json!({
            "hook_event_name": "agent_end",
            "session_id": "oc-test-3",
            "agent_id": "main",
            "workspace_dir": ".",
            "event_data": { "success": true }
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn context_budget_truncates() {
        let content = "x".repeat(10000);
        let result = apply_context_budget(&content, 500);
        assert!(result.len() <= 550);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn context_boundary_wraps() {
        let content = "hello";
        let wrapped = wrap_context_boundary(content);
        assert!(wrapped.starts_with(EDDA_BOUNDARY_START));
        assert!(wrapped.ends_with(EDDA_BOUNDARY_END));
        assert!(wrapped.contains("hello"));
    }
}
