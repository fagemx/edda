//! Edda (edda) integration helpers for the Conductor.
//!
//! All operations are best-effort: if `edda` is not in PATH or the command
//! fails, the Conductor continues without context injection. This keeps
//! Edda optional — the Conductor works as a plain task runner without it.

use std::path::Path;
use std::process::Command;

/// Ensure `.edda/` ledger exists in the working directory.
/// No-op if already initialized or if `edda` is not available.
pub fn ensure_init(cwd: &Path) {
    if cwd.join(".edda").exists() {
        return;
    }
    let _ = Command::new("edda")
        .arg("init")
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run `edda context` and return the output as a string.
/// Returns empty string if edda is not available or fails.
pub fn get_context(cwd: &Path) -> String {
    Command::new("edda")
        .arg("context")
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Record a note to the edda ledger.
/// Best-effort: silently ignores failures.
pub fn record_note(cwd: &Path, text: &str, tags: &[&str]) {
    let mut cmd = Command::new("edda");
    cmd.arg("note").arg(text).current_dir(cwd);
    for tag in tags {
        cmd.arg("--tag").arg(tag);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status();
}

/// Truncate a string to at most `max` bytes on a valid UTF-8 char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Walk backwards from max to find a char boundary
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Record a phase completion event.
pub fn record_phase_done(cwd: &Path, phase_id: &str, summary: Option<&str>, cost_usd: Option<f64>) {
    let cost_str = cost_usd.map(|c| format!(" [${c:.3}]")).unwrap_or_default();
    let summary_str = summary
        .map(|s| {
            let s = s.trim();
            if s.len() > 300 {
                format!(": {}...", truncate_str(s, 297))
            } else if s.is_empty() {
                String::new()
            } else {
                format!(": {s}")
            }
        })
        .unwrap_or_default();
    let text = format!("Phase \"{phase_id}\" passed{cost_str}{summary_str}");
    record_note(cwd, &text, &["conductor", &format!("phase:{phase_id}")]);
}

/// Record a phase failure event.
pub fn record_phase_failed(cwd: &Path, phase_id: &str, error: &str) {
    let error_str = if error.len() > 200 {
        format!("{}...", truncate_str(error, 200))
    } else {
        error.to_string()
    };
    let text = format!("Phase \"{phase_id}\" failed: {error_str}");
    record_note(
        cwd,
        &text,
        &["conductor", &format!("phase:{phase_id}"), "failure"],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_context_returns_empty_on_missing_edda_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = get_context(dir.path());
        // Either empty (no .edda/) or actual context if edda is in PATH
        // This test just verifies it doesn't panic
        assert!(result.is_empty() || result.contains("CONTEXT"));
    }

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn truncate_str_multibyte() {
        // "café" = [99, 97, 102, 195, 169] — 'é' is 2 bytes
        let s = "café";
        assert_eq!(s.len(), 5);
        // Truncate at 4 would land inside 'é', should back up to 3
        assert_eq!(truncate_str(s, 4), "caf");
        assert_eq!(truncate_str(s, 5), "café");
    }

    #[test]
    fn truncate_str_cjk() {
        // Each CJK char is 3 bytes
        let s = "你好世界";
        assert_eq!(s.len(), 12);
        // 7 bytes = 2 full chars (6) + 1 byte into 3rd char → back to 6
        assert_eq!(truncate_str(s, 7), "你好");
        assert_eq!(truncate_str(s, 6), "你好");
    }
}
