use std::sync::LazyLock;

use regex::Regex;

/// Compiled secret patterns, initialized once.
static SECRET_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // OpenAI / Anthropic API keys: sk-..., sk-ant-...
        (
            Regex::new(r"\b(sk-[a-zA-Z0-9_-]{20,})").unwrap(),
            "[REDACTED_API_KEY]",
        ),
        // GitHub tokens: ghp_, gho_, ghs_, ghu_, github_pat_
        (
            Regex::new(r"\b(ghp_[a-zA-Z0-9]{36,}|gho_[a-zA-Z0-9]{36,}|ghs_[a-zA-Z0-9]{36,}|ghu_[a-zA-Z0-9]{36,}|github_pat_[a-zA-Z0-9_]{22,})").unwrap(),
            "[REDACTED_GITHUB_TOKEN]",
        ),
        // GitLab tokens: glpat-
        (
            Regex::new(r"\b(glpat-[a-zA-Z0-9\-]{20,})").unwrap(),
            "[REDACTED_GITLAB_TOKEN]",
        ),
        // AWS access key IDs: AKIA followed by 16 uppercase alphanumeric
        (
            Regex::new(r"\b(AKIA[A-Z0-9]{16})\b").unwrap(),
            "[REDACTED_AWS_KEY]",
        ),
        // Bearer tokens (common in Authorization headers)
        (
            Regex::new(r"(?i)(Bearer\s+)[a-zA-Z0-9._\-]{20,}").unwrap(),
            "${1}[REDACTED_BEARER]",
        ),
        // Shell export of sensitive env vars
        (
            Regex::new(r#"(?mi)^(export\s+\w*(?:KEY|SECRET|TOKEN|PASSWORD|CREDENTIAL)\w*\s*=\s*)\S+"#).unwrap(),
            "${1}[REDACTED]",
        ),
    ]
});

/// Redact known secret patterns from a string.
///
/// Replaces API keys, tokens, passwords, and other secrets with `[REDACTED_*]`
/// placeholders. This is applied before writing to the append-only ledger to
/// prevent secrets from being permanently stored.
pub fn redact_secrets(input: &str) -> String {
    let mut output = input.to_string();
    for (pat, replacement) in SECRET_PATTERNS.iter() {
        output = pat.replace_all(&output, *replacement).to_string();
    }
    output
}

/// Redact secrets from the `raw` JSON value's tool_input and tool_response fields.
///
/// Returns a new JSON value with secrets removed. Non-string fields are unchanged.
pub fn redact_hook_payload(raw: &serde_json::Value) -> serde_json::Value {
    let mut sanitized = raw.clone();

    // Redact tool_input (string or nested JSON)
    if let Some(ti) = sanitized.get("tool_input") {
        sanitized["tool_input"] = redact_json_value(ti);
    }
    // Redact tool_response (if present)
    if let Some(tr) = sanitized.get("tool_response") {
        sanitized["tool_response"] = redact_json_value(tr);
    }

    sanitized
}

/// Recursively redact secrets in a JSON value.
fn redact_json_value(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::String(s) => serde_json::Value::String(redact_secrets(s)),
        serde_json::Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map {
                new_map.insert(k.clone(), redact_json_value(v));
            }
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_json_value).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_openai_api_key() {
        let input = "Using key sk-abc123456789012345678901";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED_API_KEY]"));
        assert!(!output.contains("sk-abc"));
    }

    #[test]
    fn redact_anthropic_api_key() {
        let input = "export ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED"));
        assert!(!output.contains("sk-ant-api03"));
    }

    #[test]
    fn redact_github_token() {
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED_GITHUB_TOKEN]"));
        assert!(!output.contains("ghp_"));
    }

    #[test]
    fn redact_gitlab_token() {
        let input = "GITLAB_TOKEN=glpat-abcdefghijklmnopqrstuvwx";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED"));
        assert!(!output.contains("glpat-"));
    }

    #[test]
    fn redact_aws_key() {
        let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED_AWS_KEY]"));
        assert!(!output.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload.signature";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED_BEARER]"));
        assert!(!output.contains("eyJhbGci"));
    }

    #[test]
    fn redact_env_export() {
        let input = "export API_SECRET=mysupersecretvalue123";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("mysupersecretvalue123"));
    }

    #[test]
    fn redact_preserves_normal_code() {
        let input = r#"fn main() {
    let x = 42;
    println!("Hello, world!");
    let path = "/home/user/project/src/main.rs";
}"#;
        let output = redact_secrets(input);
        assert_eq!(input, output, "Normal code should not be modified");
    }

    #[test]
    fn redact_in_nested_json() {
        let raw = serde_json::json!({
            "tool_input": {
                "command": "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.x.y' https://api.example.com",
                "nested": {
                    "key": "sk-abc123456789012345678901"
                }
            },
            "tool_response": "Response with ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij in it"
        });

        let sanitized = redact_hook_payload(&raw);

        let ti = &sanitized["tool_input"];
        assert!(
            !ti["command"]
                .as_str()
                .unwrap()
                .contains("eyJhbGci"),
            "Bearer token should be redacted"
        );
        assert!(
            !ti["nested"]["key"]
                .as_str()
                .unwrap()
                .contains("sk-abc"),
            "API key should be redacted"
        );
        assert!(
            !sanitized["tool_response"]
                .as_str()
                .unwrap()
                .contains("ghp_"),
            "GitHub token should be redacted"
        );
    }

    #[test]
    fn redact_multiple_secrets_in_one_string() {
        let input = "keys: sk-aaaa1111222233334444bbbb ghp_CCCCddddeeeeffffgggg1111222233334444aaaa";
        let output = redact_secrets(input);
        assert!(!output.contains("sk-aaaa"));
        assert!(!output.contains("ghp_CCCC"));
    }
}
