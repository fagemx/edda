/// Extract a grouping key from a CmdFail signal text.
///
/// Input format: "cargo check -p edda-mcp (exit=1)" -> "cargo check"
/// Keeps first 2 tokens of the command (before the exit= suffix).
pub(super) fn cmd_base_key(signal_text: &str) -> String {
    // Strip trailing "(exit=N)" suffix if present
    let cmd = signal_text
        .rfind(" (exit=")
        .map(|pos| &signal_text[..pos])
        .unwrap_or(signal_text);
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    match tokens.len() {
        0 => signal_text.to_string(),
        1 => tokens[0].to_string(),
        _ => format!("{} {}", tokens[0], tokens[1]),
    }
}

/// Format a task list line with count and optional truncation.
///
/// - Always shows count: `Done (5): ...`
/// - If <= 5 tasks: show all
/// - If > 5 tasks: show first 3, then `(+N more)`
pub(super) fn format_task_line(label: &str, tasks: &[&str]) -> String {
    let count = tasks.len();
    const DISPLAY_LIMIT: usize = 5;
    const SHOWN_WHEN_OVER: usize = 3;

    if count <= DISPLAY_LIMIT {
        format!("- {label} ({count}): {}\n", tasks.join(", "))
    } else {
        let shown = &tasks[..SHOWN_WHEN_OVER];
        format!(
            "- {label} ({count}): {} (+{} more)\n",
            shown.join(", "),
            count - SHOWN_WHEN_OVER,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_base_key_extracts_first_two_tokens() {
        assert_eq!(
            cmd_base_key("cargo check -p edda-mcp (exit=1)"),
            "cargo check"
        );
        assert_eq!(cmd_base_key("cargo test --all (exit=101)"), "cargo test");
        assert_eq!(cmd_base_key("npm install (exit=1)"), "npm install");
        assert_eq!(cmd_base_key("make (exit=2)"), "make");
        assert_eq!(cmd_base_key(""), "");
    }
}
