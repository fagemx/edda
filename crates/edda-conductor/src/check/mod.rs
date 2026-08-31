pub mod cmd_succeeds;
pub mod edda_event;
pub mod engine;
pub mod file_contains;
pub mod file_exists;
pub mod git_clean;
pub mod wait_until;

use std::time::Duration;

/// Output from a single check execution.
#[derive(Debug, Clone)]
pub struct CheckOutput {
    pub passed: bool,
    pub detail: Option<String>,
    pub duration: Duration,
    /// True when the failure was a harness-side timeout (GH-529). A timeout
    /// is a property of the harness, not of the agent's work, so it must be
    /// distinguishable from a genuine check failure downstream (retry
    /// policy, console output, persisted error type).
    pub timed_out: bool,
}

impl CheckOutput {
    pub fn passed(duration: Duration) -> Self {
        Self {
            passed: true,
            detail: None,
            duration,
            timed_out: false,
        }
    }

    pub fn passed_with_detail(detail: String, duration: Duration) -> Self {
        Self {
            passed: true,
            detail: Some(detail),
            duration,
            timed_out: false,
        }
    }

    pub fn failed(detail: String, duration: Duration) -> Self {
        Self {
            passed: false,
            detail: Some(detail),
            duration,
            timed_out: false,
        }
    }

    /// A harness-side timeout failure (GH-529): the command outlived its
    /// `timeout_sec`. Never classified as a retryable check failure.
    pub fn timed_out(detail: String, duration: Duration) -> Self {
        Self {
            passed: false,
            detail: Some(detail),
            duration,
            timed_out: true,
        }
    }
}

/// Mask secrets in output strings before storing.
pub fn mask_secrets(text: &str) -> String {
    let patterns = [
        // API keys
        (r"sk-[a-zA-Z0-9]{20,}", "[MASKED]"),
        (r"pk-[a-zA-Z0-9]{20,}", "[MASKED]"),
        // Bearer tokens
        (r"Bearer\s+[a-zA-Z0-9._\-]+", "Bearer [MASKED]"),
        // key=value patterns
        (
            r"(?i)(password|secret|token|key|api_key|apikey)=[^\s&]+",
            "$1=[MASKED]",
        ),
    ];

    let mut result = text.to_string();
    for (pattern, replacement) in patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            result = re.replace_all(&result, replacement).into_owned();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_api_keys() {
        let input = "Error: sk-ant1234567890abcdefghij is invalid";
        let masked = mask_secrets(input);
        assert!(masked.contains("[MASKED]"));
        assert!(!masked.contains("sk-ant"));
    }

    #[test]
    fn mask_bearer_tokens() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test";
        let masked = mask_secrets(input);
        assert!(masked.contains("Bearer [MASKED]"));
    }

    #[test]
    fn mask_key_value() {
        let input = "password=hunter2&token=abc123";
        let masked = mask_secrets(input);
        assert!(masked.contains("password=[MASKED]"));
        assert!(masked.contains("token=[MASKED]"));
    }

    #[test]
    fn no_mask_normal_text() {
        let input = "cargo test --workspace passed";
        assert_eq!(mask_secrets(input), input);
    }
}
