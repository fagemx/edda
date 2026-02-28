use std::fs;
use std::path::{Path, PathBuf};

use crate::parse::now_rfc3339;

// ── Install / Uninstall ──

const EDDA_HOOK_COMMAND: &str = "edda hook claude";

/// Hook event names that edda manages.
const HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SessionStart",
    "UserPromptSubmit",
    "PreCompact",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
];

/// Check if a matcher group (Claude Code hook format) contains a edda hook.
fn matcher_group_contains_edda(group: &serde_json::Value) -> bool {
    // New format: { "matcher": "", "hooks": [{ "type": "command", "command": "edda hook claude" }] }
    if let Some(hooks_arr) = group.get("hooks").and_then(|h| h.as_array()) {
        for hook in hooks_arr {
            if let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) {
                if cmd.contains("edda hook") {
                    return true;
                }
            }
        }
    }
    // Legacy format: plain string "edda hook claude"
    if let Some(s) = group.as_str() {
        return s.contains("edda hook");
    }
    false
}

fn settings_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.local.json")
}

/// Install edda hooks into `.claude/settings.local.json`.
pub fn install(repo_root: &Path, no_claude_md: bool) -> anyhow::Result<()> {
    let path = settings_path(repo_root);

    // Ensure .claude/ dir exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Read existing settings or start fresh
    let mut settings: serde_json::Value = if path.exists() {
        let content = fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Backup existing file
    if path.exists() {
        let ts = now_rfc3339().replace(':', "-");
        let backup = path.with_extension(format!("json.edda.bak.{ts}"));
        fs::copy(&path, &backup)?;
    }

    // Merge hooks
    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings is not an object"))?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks is not an object"))?;

    for event_name in HOOK_EVENTS {
        let key = event_name.to_string();

        // Build the edda matcher group in Claude Code's hook format
        let edda_group = serde_json::json!({
            "matcher": "",
            "hooks": [
                {
                    "type": "command",
                    "command": EDDA_HOOK_COMMAND
                }
            ]
        });

        // Preserve existing non-edda matcher groups
        let existing = hooks_obj.get(&key).and_then(|v| v.as_array()).cloned();
        let mut groups: Vec<serde_json::Value> = existing
            .unwrap_or_default()
            .into_iter()
            .filter(|group| !matcher_group_contains_edda(group))
            .collect();
        groups.push(edda_group);

        hooks_obj.insert(key, serde_json::Value::Array(groups));
    }

    // Add MCP server config (mcpServers.edda)
    let mcp_servers = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings is not an object"))?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(mcp_obj) = mcp_servers.as_object_mut() {
        if !mcp_obj.contains_key("edda") {
            mcp_obj.insert(
                "edda".to_string(),
                serde_json::json!({
                    "command": "edda",
                    "args": ["mcp", "serve"]
                }),
            );
        }
    }

    let output = serde_json::to_string_pretty(&settings)?;
    fs::write(&path, output.as_bytes())?;

    println!("Installed edda hooks into {}", path.display());
    println!("Configured MCP server (edda mcp serve)");

    // Onboard CLAUDE.md with edda decision-tracking instructions.
    // B1.5 testing showed CLAUDE.md is the decisive factor for agent compliance:
    // 0% recall without it, ~1.33 decisions/session with it.
    if !no_claude_md {
        ensure_claude_md_edda_section(repo_root)?;
        ensure_claude_md_coordination_section(repo_root)?;
    }

    Ok(())
}

/// Uninstall edda hooks from `.claude/settings.local.json`.
pub fn uninstall(repo_root: &Path) -> anyhow::Result<()> {
    let path = settings_path(repo_root);

    if !path.exists() {
        println!("No settings file found at {}", path.display());
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(hooks) = settings
        .as_object_mut()
        .and_then(|obj| obj.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
    {
        for event_name in HOOK_EVENTS {
            let key = event_name.to_string();
            if let Some(arr) = hooks.get(&key).and_then(|v| v.as_array()).cloned() {
                let filtered: Vec<serde_json::Value> = arr
                    .into_iter()
                    .filter(|v| !matcher_group_contains_edda(v))
                    .collect();
                if filtered.is_empty() {
                    hooks.remove(&key);
                } else {
                    hooks.insert(key, serde_json::Value::Array(filtered));
                }
            }
        }
    }

    // Remove edda MCP server config
    if let Some(mcp_servers) = settings
        .as_object_mut()
        .and_then(|obj| obj.get_mut("mcpServers"))
        .and_then(|m| m.as_object_mut())
    {
        mcp_servers.remove("edda");
    }
    // Clean up empty mcpServers object
    if settings
        .get("mcpServers")
        .and_then(|m| m.as_object())
        .is_some_and(|m| m.is_empty())
    {
        settings.as_object_mut().unwrap().remove("mcpServers");
    }

    let output = serde_json::to_string_pretty(&settings)?;
    fs::write(&path, output.as_bytes())?;

    println!("Uninstalled edda hooks from {}", path.display());
    Ok(())
}

// ── CLAUDE.md Onboarding ──

/// Marker used to detect whether CLAUDE.md already has the edda section.
const EDDA_SECTION_MARKER: &str = "<!-- edda:decision-tracking -->";

/// Marker for the coordination section (independent of decision-tracking).
const EDDA_COORDINATION_MARKER: &str = "<!-- edda:coordination -->";

/// Coordination rules appended to existing CLAUDE.md (or embedded in CREATE template).
const EDDA_COORDINATION_SECTION: &str = r#"
<!-- edda:coordination -->
## Multi-Agent Coordination (edda)

When edda detects multiple agents, it injects peer information into your context.

**You MUST follow these rules:**
- **Check Off-limits** before editing any file — if a file is listed under "Off-limits", do NOT edit it
- **Claim your scope** at session start: `edda claim "label" --paths "src/scope/*"`
- **Request before crossing boundaries**: `edda request "peer-label" "your message"`
- **Respect binding decisions** — they apply to all sessions

Ignoring these rules causes merge conflicts and duplicated work.
"#;

/// Full template for creating a NEW `.claude/CLAUDE.md` with edda onboarding.
/// The marker is at the end so the model sees actionable content first.
const EDDA_CLAUDE_MD_CREATE: &str = r#"# Project Guidelines

This project uses **edda** for decision tracking across sessions.

## Decision Recording

When you make an architectural decision (choosing a library, defining a pattern,
changing infrastructure), record it:

```bash
edda decide "domain.aspect=value" --reason "why"
```

### What to record
- Choosing a database, ORM, or storage engine
- Picking an auth strategy or session management approach
- Defining error handling or logging patterns
- Adding or changing deployment configuration
- Creating new modules or establishing code structure

### What NOT to record
- Formatting changes, typo fixes, minor refactors
- Dependency version bumps (unless switching libraries)
- Test additions that don't change architecture

### Expectations
- **Record at least 1-2 decisions per session** — if you chose a library, defined a pattern, or changed config, that's a decision
- Record decisions AS you make them, not at the end
- When in doubt, record it — too many decisions is better than too few

### Examples
```bash
edda decide "db.engine=sqlite" --reason "embedded, zero-config for MVP"
edda decide "auth.strategy=JWT" --reason "stateless, scales horizontally"
edda decide "error.pattern=enum+IntoResponse" --reason "axum idiomatic, typed errors"
```

## Session Notes

Before ending a session, summarize what you did:

```bash
edda note "completed X; decided Y; next: Z" --tag session
```

<!-- edda:decision-tracking -->

<!-- edda:coordination -->
## Multi-Agent Coordination (edda)

When edda detects multiple agents, it injects peer information into your context.

**You MUST follow these rules:**
- **Check Off-limits** before editing any file — if a file is listed under "Off-limits", do NOT edit it
- **Claim your scope** at session start: `edda claim "label" --paths "src/scope/*"`
- **Request before crossing boundaries**: `edda request "peer-label" "your message"`
- **Respect binding decisions** — they apply to all sessions

Ignoring these rules causes merge conflicts and duplicated work.
"#;

/// Shorter section appended to an EXISTING `.claude/CLAUDE.md`.
const EDDA_CLAUDE_MD_APPEND: &str = r#"
<!-- edda:decision-tracking -->
## Decision Tracking (edda)

This project uses **edda** for decision tracking across sessions.

When you make an architectural decision (choosing a library, defining a pattern,
changing infrastructure), record it:

```bash
edda decide "domain.aspect=value" --reason "why"
```

**What to record:** choosing a database/ORM, auth strategy, error handling pattern,
deployment config, new module structure.

**What NOT to record:** formatting, typo fixes, minor refactors, dependency bumps.

Before ending a session, summarize what you did:

```bash
edda note "completed X; decided Y; next: Z" --tag session
```
"#;

/// Ensure `.claude/CLAUDE.md` contains the edda decision-tracking section.
/// Creates the file if missing; appends the section if the marker is absent.
fn ensure_claude_md_edda_section(repo_root: &Path) -> anyhow::Result<()> {
    let claude_dir = repo_root.join(".claude");
    fs::create_dir_all(&claude_dir)?;
    let claude_md = claude_dir.join("CLAUDE.md");

    if claude_md.exists() {
        let content = fs::read_to_string(&claude_md)?;
        if content.contains(EDDA_SECTION_MARKER) {
            // Already has edda section — skip
            return Ok(());
        }
        // Append shorter section to existing file
        let mut appended = content;
        if !appended.ends_with('\n') {
            appended.push('\n');
        }
        appended.push_str(EDDA_CLAUDE_MD_APPEND);
        fs::write(&claude_md, appended)?;
        println!("Appended edda section to {}", claude_md.display());
    } else {
        // Create new file with full onboarding template
        fs::write(&claude_md, EDDA_CLAUDE_MD_CREATE.trim_start())?;
        println!(
            "Created {} with edda decision tracking",
            claude_md.display()
        );
    }
    Ok(())
}

/// Ensure `.claude/CLAUDE.md` contains the edda coordination section.
/// Appends the section if the coordination marker is absent.
/// Skips if the file doesn't exist (it will be created by `ensure_claude_md_edda_section`
/// with both sections included).
fn ensure_claude_md_coordination_section(repo_root: &Path) -> anyhow::Result<()> {
    let claude_md = repo_root.join(".claude").join("CLAUDE.md");
    if !claude_md.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&claude_md)?;
    if content.contains(EDDA_COORDINATION_MARKER) {
        return Ok(());
    }
    let mut appended = content;
    if !appended.ends_with('\n') {
        appended.push('\n');
    }
    appended.push_str(EDDA_COORDINATION_SECTION);
    fs::write(&claude_md, appended)?;
    println!("Appended coordination rules to {}", claude_md.display());
    Ok(())
}

// ── Doctor ──

/// Check edda bridge health.
pub fn doctor(repo_root: &Path) -> anyhow::Result<()> {
    // 1. Check edda in PATH
    let edda_in_path = which_edda();
    println!(
        "[{}] edda in PATH: {}",
        if edda_in_path.is_some() { "OK" } else { "WARN" },
        edda_in_path.unwrap_or_else(|| "not found".into())
    );

    // 2. Check settings.local.json has hooks
    let path = settings_path(repo_root);
    let has_hooks = if path.exists() {
        let content = fs::read_to_string(&path).unwrap_or_default();
        content.contains("edda hook")
    } else {
        false
    };
    println!(
        "[{}] hooks in {}",
        if has_hooks { "OK" } else { "WARN" },
        path.display()
    );

    // 3. Check store root exists
    let root = edda_store::store_root();
    println!(
        "[{}] store root: {}",
        if root.exists() { "OK" } else { "WARN" },
        root.display()
    );

    Ok(())
}

fn which_edda() -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exe_name = if cfg!(windows) { "edda.exe" } else { "edda" };
    for dir in path_var.split(sep) {
        let candidate = Path::new(dir).join(exe_name);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), false).unwrap();
        let path = tmp.path().join(".claude").join("settings.local.json");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("edda hook claude"));
        assert!(content.contains("PreToolUse"));
        assert!(content.contains("SessionStart"));

        // Verify Claude Code hook object format
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
        let pre_tool = &settings["hooks"]["PreToolUse"];
        let group = pre_tool.as_array().unwrap().first().unwrap();
        assert!(group.get("matcher").is_some());
        assert_eq!(group["hooks"][0]["type"].as_str().unwrap(), "command");
        assert_eq!(
            group["hooks"][0]["command"].as_str().unwrap(),
            "edda hook claude"
        );

        // Verify CLAUDE.md was created with edda section
        let claude_md = tmp.path().join(".claude").join("CLAUDE.md");
        assert!(claude_md.exists(), "CLAUDE.md should be created");
        let md_content = fs::read_to_string(&claude_md).unwrap();
        assert!(md_content.contains("edda:decision-tracking"), "marker");
        assert!(md_content.contains("edda decide"), "decide instruction");
        assert!(md_content.contains("edda note"), "note instruction");
        assert!(
            md_content.contains("edda:coordination"),
            "coordination marker"
        );
        assert!(md_content.contains("edda claim"), "claim instruction");
        assert!(md_content.contains("edda request"), "request instruction");

        // Verify MCP server config
        assert_eq!(
            settings["mcpServers"]["edda"]["command"].as_str().unwrap(),
            "edda",
            "MCP command"
        );
        assert_eq!(
            settings["mcpServers"]["edda"]["args"],
            serde_json::json!(["mcp", "serve"]),
            "MCP args"
        );

        uninstall(tmp.path()).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("edda hook"));
        // MCP config should also be removed
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            settings.get("mcpServers").is_none(),
            "mcpServers should be removed after uninstall"
        );
    }

    #[test]
    fn install_appends_to_existing_claude_md() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_md = claude_dir.join("CLAUDE.md");
        fs::write(&claude_md, "# My Project\n\nExisting content.\n").unwrap();

        install(tmp.path(), false).unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        assert!(
            content.starts_with("# My Project"),
            "existing content preserved"
        );
        assert!(
            content.contains("Existing content."),
            "existing content preserved"
        );
        assert!(
            content.contains("edda:decision-tracking"),
            "edda section appended"
        );
        assert!(
            content.contains("edda decide"),
            "decide instruction present"
        );
        assert!(
            content.contains("edda:coordination"),
            "coordination section appended"
        );
        assert!(content.contains("edda claim"), "claim instruction present");
    }

    #[test]
    fn install_no_claude_md_flag() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), true).unwrap();

        // Hooks should still be installed
        let settings = tmp.path().join(".claude").join("settings.local.json");
        assert!(settings.exists());
        let content = fs::read_to_string(&settings).unwrap();
        assert!(content.contains("edda hook claude"));

        // But CLAUDE.md should NOT exist
        let claude_md = tmp.path().join(".claude").join("CLAUDE.md");
        assert!(
            !claude_md.exists(),
            "CLAUDE.md should not be created with --no-claude-md"
        );
    }

    #[test]
    fn install_skips_if_edda_section_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_md = claude_dir.join("CLAUDE.md");
        fs::write(
            &claude_md,
            "# Project\n<!-- edda:decision-tracking -->\nAlready here.\n",
        )
        .unwrap();

        install(tmp.path(), false).unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        // Should NOT have duplicate sections
        assert_eq!(
            content.matches("edda:decision-tracking").count(),
            1,
            "should not duplicate edda section"
        );
        // But coordination should be appended
        assert!(
            content.contains("edda:coordination"),
            "coordination section appended to existing"
        );
    }

    #[test]
    fn install_appends_coordination_to_existing_without_it() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_md = claude_dir.join("CLAUDE.md");
        // Has decision-tracking but no coordination
        fs::write(
            &claude_md,
            "# Project\n<!-- edda:decision-tracking -->\nDecision stuff.\n",
        )
        .unwrap();

        install(tmp.path(), false).unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        assert!(
            content.contains("edda:coordination"),
            "coordination section appended"
        );
        assert!(content.contains("edda claim"), "claim instruction present");
        assert!(content.contains("Off-limits"), "off-limits rule present");
        assert_eq!(
            content.matches("edda:decision-tracking").count(),
            1,
            "decision-tracking not duplicated"
        );
    }

    #[test]
    fn install_skips_if_coordination_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_md = claude_dir.join("CLAUDE.md");
        // Has both markers
        fs::write(
            &claude_md,
            "# Project\n<!-- edda:decision-tracking -->\n<!-- edda:coordination -->\nBoth here.\n",
        )
        .unwrap();

        install(tmp.path(), false).unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        assert_eq!(
            content.matches("edda:coordination").count(),
            1,
            "should not duplicate coordination section"
        );
    }

    #[test]
    fn install_preserves_existing_mcp_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Pre-populate with another MCP server
        let settings = serde_json::json!({
            "mcpServers": {
                "other-tool": {
                    "command": "other-tool",
                    "args": ["serve"]
                }
            }
        });
        let path = claude_dir.join("settings.local.json");
        fs::write(&path, serde_json::to_string_pretty(&settings).unwrap()).unwrap();

        install(tmp.path(), true).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Both MCP servers should exist
        assert!(
            settings["mcpServers"]["other-tool"].is_object(),
            "existing MCP server preserved"
        );
        assert!(
            settings["mcpServers"]["edda"].is_object(),
            "edda MCP server added"
        );

        // Uninstall should only remove edda, keep other-tool
        uninstall(tmp.path()).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            settings["mcpServers"]["other-tool"].is_object(),
            "other MCP server preserved after uninstall"
        );
        assert!(
            settings["mcpServers"].get("edda").is_none(),
            "edda MCP server removed after uninstall"
        );
    }

    #[test]
    fn install_idempotent_mcp_config() {
        let tmp = tempfile::tempdir().unwrap();

        install(tmp.path(), true).unwrap();
        install(tmp.path(), true).unwrap();

        let path = tmp.path().join(".claude").join("settings.local.json");
        let content = fs::read_to_string(&path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Should have exactly one edda entry
        let mcp = settings["mcpServers"].as_object().unwrap();
        assert_eq!(mcp.len(), 1, "only one MCP server entry");
        assert_eq!(
            mcp["edda"]["command"].as_str().unwrap(),
            "edda",
            "correct command"
        );
    }
}
