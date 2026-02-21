//! Reactive decision nudge — detect decision-like actions in PostToolUse
//! and generate targeted reminders to record them via `edda decide`.

use serde_json::Value;

/// A detected decision signal from a PostToolUse event.
#[derive(Debug, PartialEq)]
pub(crate) enum NudgeSignal {
    /// Agent committed code. Contains short commit message.
    Commit(String),
    /// Agent added a dependency. Contains package name.
    DependencyAdd(String),
    /// Agent already called `edda decide` — suppress future nudges.
    SelfRecord,
    /// Agent modified a config file. Contains file path.
    ConfigChange(String),
    /// Agent modified a schema/migration file. Contains file path.
    SchemaChange(String),
    /// Agent created a new module entry point. Contains file path.
    NewModule(String),
}

/// Scan PostToolUse `raw` payload for decision-like signals.
pub(crate) fn detect_signal(raw: &Value) -> Option<NudgeSignal> {
    let tool_name = raw.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

    if tool_name == "Bash" {
        let command = raw
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        // Self-record detection (highest priority)
        if command.contains("edda decide") {
            return Some(NudgeSignal::SelfRecord);
        }

        // Git commit detection
        if command.contains("git commit") && !command.contains("--amend") {
            let msg = extract_commit_message(command);
            if !msg.is_empty() {
                return Some(NudgeSignal::Commit(msg));
            }
        }

        // Dependency add detection
        if let Some(pkg) = extract_dependency_add(command) {
            return Some(NudgeSignal::DependencyAdd(pkg));
        }
    } else if tool_name == "Edit" || tool_name == "Write" {
        let file_path = raw
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(|f| f.as_str())
            .unwrap_or("");
        return detect_file_signal(tool_name, file_path);
    }

    None
}

/// Detect decision-like signals from file-based tool use (Edit/Write).
fn detect_file_signal(tool_name: &str, file_path: &str) -> Option<NudgeSignal> {
    if is_config_file(file_path) {
        return Some(NudgeSignal::ConfigChange(file_path.into()));
    }
    if is_schema_file(file_path) {
        return Some(NudgeSignal::SchemaChange(file_path.into()));
    }
    // NewModule only on Write (new file creation), not Edit
    if tool_name == "Write" && is_module_entry(file_path) {
        return Some(NudgeSignal::NewModule(file_path.into()));
    }
    None
}

fn is_config_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let filename = normalized.rsplit('/').next().unwrap_or(&normalized);

    filename.starts_with(".env")
        || filename.starts_with("config.")
        || filename.starts_with("docker-compose")
        || (filename.ends_with(".toml") && filename != "Cargo.toml")
        || filename.ends_with(".yaml")
        || filename.ends_with(".yml")
}

fn is_schema_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let filename = normalized.rsplit('/').next().unwrap_or(&normalized);

    normalized.contains("/migrations/")
        || filename.ends_with(".sql")
        || filename.starts_with("schema.")
}

fn is_module_entry(path: &str) -> bool {
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    matches!(
        filename,
        "mod.rs" | "lib.rs" | "index.ts" | "index.js" | "__init__.py"
    )
}

/// Format a nudge message for the agent.
///
/// When `decide_count` is 0 (no decisions recorded yet this session), the
/// message is stronger — an explicit warning instead of a gentle suggestion.
pub(crate) fn format_nudge(signal: &NudgeSignal, decide_count: u64) -> String {
    if decide_count == 0 {
        format_nudge_strong(signal)
    } else {
        format_nudge_gentle(signal)
    }
}

/// Strong nudge — used when the agent hasn't recorded any decisions this session.
fn format_nudge_strong(signal: &NudgeSignal) -> String {
    let action = match signal {
        NudgeSignal::Commit(msg) => format!("committed \"{msg}\""),
        NudgeSignal::DependencyAdd(pkg) => format!("added dependency '{pkg}'"),
        NudgeSignal::SelfRecord => return String::new(),
        NudgeSignal::ConfigChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!("modified config file '{name}'")
        }
        NudgeSignal::SchemaChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!("modified schema '{name}'")
        }
        NudgeSignal::NewModule(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!("created module entry '{name}'")
        }
    };
    format!(
        "\u{26a0}\u{fe0f} You haven't recorded any decisions this session yet. You just {action}.\n\
         Please record the architectural decisions you've made:\n  \
         `edda decide \"domain.aspect=value\" --reason \"why\"`"
    )
}

/// Gentle nudge — used when the agent has already recorded at least one decision.
fn format_nudge_gentle(signal: &NudgeSignal) -> String {
    match signal {
        NudgeSignal::Commit(msg) => {
            format!(
                "You committed \"{msg}\". If this involves an architectural decision, record it:\n\
                 `edda decide \"key=value\" --reason \"why\"`"
            )
        }
        NudgeSignal::DependencyAdd(pkg) => {
            format!(
                "You added dependency '{pkg}'. Record why:\n\
                 `edda decide \"dep={pkg}\" --reason \"why this dependency\"`"
            )
        }
        NudgeSignal::SelfRecord => String::new(), // should not be called
        NudgeSignal::ConfigChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!(
                "You modified config file '{name}'. If this changes deployment/environment behavior, record it:\n\
                 `edda decide \"config=...\" --reason \"why\"`"
            )
        }
        NudgeSignal::SchemaChange(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!(
                "You modified schema '{name}'. Record the schema decision:\n\
                 `edda decide \"schema=...\" --reason \"what changed\"`"
            )
        }
        NudgeSignal::NewModule(file) => {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            format!(
                "You created module entry '{name}'. If this establishes a new boundary, record it:\n\
                 `edda decide \"module=...\" --reason \"purpose\"`"
            )
        }
    }
}

/// Extract the first line of the commit message from a git commit command.
fn extract_commit_message(command: &str) -> String {
    // Pattern 1: git commit -m "message"
    if let Some(idx) = command.find("-m ") {
        let after_flag = &command[idx + 3..];
        let msg = extract_quoted_or_heredoc(after_flag);
        return truncate(&msg, 80);
    }
    // Pattern 2: git commit -m"message" (no space)
    if let Some(idx) = command.find("-m\"") {
        let after_flag = &command[idx + 2..];
        let msg = extract_quoted_or_heredoc(after_flag);
        return truncate(&msg, 80);
    }
    String::new()
}

/// Extract content from a quoted string or heredoc.
fn extract_quoted_or_heredoc(s: &str) -> String {
    let s = s.trim();

    // Heredoc: "$(cat <<'EOF'\n...\nEOF\n)"
    if s.starts_with("\"$(cat <<") {
        // Find the first real line of content after the heredoc marker
        if let Some(nl) = s.find('\n') {
            let body = &s[nl + 1..];
            // Take first non-empty line as the commit subject
            for line in body.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && trimmed != "EOF" && !trimmed.starts_with(")\"") {
                    return trimmed.to_string();
                }
            }
        }
        return String::new();
    }

    // Simple quoted string
    if let Some(inner) = s.strip_prefix('"') {
        if let Some(end) = inner.find('"') {
            return inner[..end].to_string();
        }
    }
    if let Some(inner) = s.strip_prefix('\'') {
        if let Some(end) = inner.find('\'') {
            return inner[..end].to_string();
        }
    }

    // Unquoted: take until whitespace or end
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// Extract package name from cargo add / npm install / pnpm add commands.
pub(crate) fn extract_dependency_add(command: &str) -> Option<String> {
    // cargo add <pkg>
    if let Some(idx) = command.find("cargo add ") {
        let after = &command[idx + "cargo add ".len()..];
        let pkg = after.split_whitespace().next()?;
        if !pkg.starts_with('-') {
            return Some(pkg.to_string());
        }
    }
    // npm install <pkg> (not bare npm install)
    for prefix in &["npm install ", "npm i ", "pnpm add ", "pnpm install "] {
        if let Some(idx) = command.find(prefix) {
            let after = &command[idx + prefix.len()..];
            let pkg = after.split_whitespace().next()?;
            if !pkg.starts_with('-') {
                return Some(pkg.to_string());
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let safe = s.floor_char_boundary(max.saturating_sub(3));
        format!("{}...", &s[..safe])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_signal_git_commit() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: switch to postgres\"" }
        });
        let signal = detect_signal(&raw);
        assert_eq!(
            signal,
            Some(NudgeSignal::Commit("feat: switch to postgres".into()))
        );
    }

    #[test]
    fn detect_signal_git_commit_heredoc() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"$(cat <<'EOF'\nfeat: add postgres support\n\nDetailed body here.\n\nCo-Authored-By: Claude\nEOF\n)\"" }
        });
        let signal = detect_signal(&raw);
        assert_eq!(
            signal,
            Some(NudgeSignal::Commit("feat: add postgres support".into()))
        );
    }

    #[test]
    fn detect_signal_cargo_add() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cargo add tokio --features full" }
        });
        let signal = detect_signal(&raw);
        assert_eq!(signal, Some(NudgeSignal::DependencyAdd("tokio".into())));
    }

    #[test]
    fn detect_signal_npm_install() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "npm install express" }
        });
        let signal = detect_signal(&raw);
        assert_eq!(signal, Some(NudgeSignal::DependencyAdd("express".into())));
    }

    #[test]
    fn detect_signal_edda_decide() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "edda decide \"db=postgres\" --reason \"need JSONB\"" }
        });
        let signal = detect_signal(&raw);
        assert_eq!(signal, Some(NudgeSignal::SelfRecord));
    }

    #[test]
    fn detect_signal_unrelated_bash() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test -p edda-cli" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn detect_signal_edit_tool_ignored() {
        let raw = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "Cargo.toml" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn detect_signal_amend_ignored() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit --amend --no-edit" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn format_nudge_commit_gentle() {
        let nudge = format_nudge(&NudgeSignal::Commit("feat: switch to postgres".into()), 1);
        assert!(nudge.contains("switch to postgres"));
        assert!(nudge.contains("edda decide"));
        assert!(!nudge.contains("\u{26a0}\u{fe0f}"));
    }

    #[test]
    fn format_nudge_dependency_gentle() {
        let nudge = format_nudge(&NudgeSignal::DependencyAdd("tokio".into()), 1);
        assert!(nudge.contains("tokio"));
        assert!(nudge.contains("edda decide"));
        assert!(!nudge.contains("\u{26a0}\u{fe0f}"));
    }

    #[test]
    fn format_nudge_commit_strong_when_zero() {
        let nudge = format_nudge(&NudgeSignal::Commit("feat: add auth".into()), 0);
        assert!(nudge.contains("\u{26a0}\u{fe0f}"));
        assert!(nudge.contains("haven't recorded any decisions"));
        assert!(nudge.contains("committed \"feat: add auth\""));
        assert!(nudge.contains("edda decide"));
    }

    #[test]
    fn format_nudge_dependency_strong_when_zero() {
        let nudge = format_nudge(&NudgeSignal::DependencyAdd("tokio".into()), 0);
        assert!(nudge.contains("\u{26a0}\u{fe0f}"));
        assert!(nudge.contains("haven't recorded any decisions"));
        assert!(nudge.contains("added dependency 'tokio'"));
    }

    #[test]
    fn format_nudge_config_strong_when_zero() {
        let nudge = format_nudge(&NudgeSignal::ConfigChange("/project/.env".into()), 0);
        assert!(nudge.contains("\u{26a0}\u{fe0f}"));
        assert!(nudge.contains("modified config file '.env'"));
    }

    #[test]
    fn npm_bare_install_not_detected() {
        // Bare npm install (no package) should not trigger
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "npm install" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn pnpm_add_detected() {
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "pnpm add zod" }
        });
        assert_eq!(
            detect_signal(&raw),
            Some(NudgeSignal::DependencyAdd("zod".into()))
        );
    }

    // ── File-based signal tests ──

    #[test]
    fn detect_signal_edit_env_file() {
        let raw = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "/project/.env.local" }
        });
        assert_eq!(
            detect_signal(&raw),
            Some(NudgeSignal::ConfigChange("/project/.env.local".into()))
        );
    }

    #[test]
    fn detect_signal_write_migration() {
        let raw = serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": "/project/migrations/001_init.sql" }
        });
        assert_eq!(
            detect_signal(&raw),
            Some(NudgeSignal::SchemaChange(
                "/project/migrations/001_init.sql".into()
            ))
        );
    }

    #[test]
    fn detect_signal_write_mod_rs() {
        let raw = serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": "src/auth/mod.rs" }
        });
        assert_eq!(
            detect_signal(&raw),
            Some(NudgeSignal::NewModule("src/auth/mod.rs".into()))
        );
    }

    #[test]
    fn detect_signal_edit_mod_rs_not_new_module() {
        // Edit (not Write) on mod.rs should NOT trigger NewModule
        let raw = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "src/auth/mod.rs" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn detect_signal_edit_cargo_toml_excluded() {
        // Cargo.toml is explicitly excluded from config detection
        let raw = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "Cargo.toml" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn detect_signal_write_regular_rs() {
        // A regular .rs file should not trigger any file-based signal
        let raw = serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": "src/main.rs" }
        });
        assert_eq!(detect_signal(&raw), None);
    }

    #[test]
    fn commit_message_truncated() {
        let long_msg = "a".repeat(120);
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": format!("git commit -m \"{long_msg}\"") }
        });
        let signal = detect_signal(&raw);
        match signal {
            Some(NudgeSignal::Commit(msg)) => assert!(msg.len() <= 83), // 80 + "..."
            other => panic!("expected Commit, got {other:?}"),
        }
    }
}
