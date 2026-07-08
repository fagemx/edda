//! Deterministic secret scrubbing for text about to enter the ledger
//! (`edda decide`, `edda note`). Pattern-based, zero LLM, hot-path safe.
//!
//! Contract (Foundry q331 EDDA-SECRET-GUARD1):
//! - Redact well-known secret shapes (API keys, private-key blocks, bearer
//!   tokens, AWS access keys, basic-auth URLs) to `[REDACTED:<type>]`.
//! - Report every hit so the caller can audit / warn.
//! - Idempotent: running `redact()` twice yields the same output.
//! - Micro-second hot path: regex set is precompiled once via `OnceLock`.
//!
//! Non-goals for v1:
//! - Entropy scan (harder to tune, false-positive prone) — v2.
//! - LLM assist — the whole point is the zero-LLM path stays honest.
//! - Verbatim transcript ingest (BRIDGE-03 contract preserves the raw file).
//!   This module is for decide/note text only.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// One redacted region: what was hit and where in the input text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretHit {
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
}

struct Pattern {
    kind: &'static str,
    re: Regex,
}

fn patterns() -> &'static [Pattern] {
    static SET: OnceLock<Vec<Pattern>> = OnceLock::new();
    SET.get_or_init(|| {
        vec![
            // -----BEGIN … PRIVATE KEY----- … -----END … PRIVATE KEY-----
            Pattern {
                kind: "private_key",
                re: Regex::new(r"-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]+?-----END [A-Z ]+PRIVATE KEY-----").unwrap(),
            },
            // Anthropic API key (must come before generic sk-)
            Pattern {
                kind: "anthropic_api_key",
                re: Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").unwrap(),
            },
            // OpenAI-style key
            Pattern {
                kind: "openai_api_key",
                re: Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
            },
            // Stripe live/test
            Pattern {
                kind: "stripe_key",
                re: Regex::new(r"sk_(?:live|test)_[A-Za-z0-9]{20,}").unwrap(),
            },
            // GitHub token families
            Pattern {
                kind: "github_token",
                re: Regex::new(r"gh[pousr]_[A-Za-z0-9]{20,}").unwrap(),
            },
            Pattern {
                kind: "github_pat",
                re: Regex::new(r"github_pat_[A-Za-z0-9_]{20,}").unwrap(),
            },
            // AWS access key id
            Pattern {
                kind: "aws_access_key_id",
                re: Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            },
            // Bearer / Authorization tokens (min 20 chars)
            Pattern {
                kind: "bearer_token",
                re: Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/=\-]{20,}").unwrap(),
            },
            // basic-auth in URL (protocol://user:pass@host)
            Pattern {
                kind: "basic_auth_url",
                re: Regex::new(r"(?i)(?:https?|ftp|ssh|redis|postgres|mysql|mongodb)://[A-Za-z0-9._%+-]+:[^\s@:/]+@").unwrap(),
            },
        ]
    })
}

/// Scrub known secret patterns in `text`. Returns `(redacted_text, hits)`.
///
/// - `hits` reports positions in the ORIGINAL text (pre-replacement) so the
///   caller can log what happened without re-scanning.
/// - Idempotent: `redact(redact(x).0).1` is always empty.
pub fn redact(text: &str) -> (String, Vec<SecretHit>) {
    let mut hits: Vec<(usize, usize, &'static str)> = Vec::new();
    for pattern in patterns() {
        for m in pattern.re.find_iter(text) {
            hits.push((m.start(), m.end(), pattern.kind));
        }
    }
    if hits.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // Sort by start; on overlap, longer/earlier wins to avoid nested replacement.
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    let mut chosen: Vec<(usize, usize, &'static str)> = Vec::with_capacity(hits.len());
    let mut cursor_end: usize = 0;
    for h in &hits {
        if h.0 >= cursor_end {
            chosen.push(*h);
            cursor_end = h.1;
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut prev = 0;
    let mut reported = Vec::with_capacity(chosen.len());
    for (start, end, kind) in chosen {
        out.push_str(&text[prev..start]);
        out.push_str(&format!("[REDACTED:{kind}]"));
        reported.push(SecretHit { kind, start, end });
        prev = end;
    }
    out.push_str(&text[prev..]);
    (out, reported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_key_redacted() {
        let text = "auth: sk-abcdefghijklmnopqrstuvwxyz012345 do stuff";
        let (out, hits) = redact(text);
        assert!(out.contains("[REDACTED:openai_api_key]"), "openai key redacted; got: {out}");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn anthropic_key_preferred_over_generic_sk() {
        let text = "sk-ant-api03-abcdefghij0123456789012345 more";
        let (out, hits) = redact(text);
        assert!(out.contains("[REDACTED:anthropic_api_key]"));
        // must not also emit generic openai for the same span
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn private_key_block_redacted() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\nafter";
        let (out, _) = redact(text);
        assert!(out.contains("[REDACTED:private_key]"));
        assert!(out.starts_with("before\n"));
        assert!(out.ends_with("\nafter"));
    }

    #[test]
    fn bearer_token_redacted() {
        let (out, hits) = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc");
        assert!(out.contains("[REDACTED:bearer_token]"), "got: {out}");
        assert_eq!(hits[0].kind, "bearer_token");
    }

    #[test]
    fn basic_auth_url_redacted() {
        let (out, _) = redact("clone https://alice:s3cret@example.com/repo.git");
        assert!(out.contains("[REDACTED:basic_auth_url]"));
        assert!(out.contains("example.com/repo.git"));
    }

    #[test]
    fn aws_access_key_redacted() {
        let (out, hits) = redact("aws AKIAIOSFODNN7EXAMPLE etc");
        assert!(out.contains("[REDACTED:aws_access_key_id]"));
        assert_eq!(hits[0].kind, "aws_access_key_id");
    }

    #[test]
    fn github_token_redacted() {
        let (out, _) = redact("push ghp_abcdefghijklmnop0123456789 ok");
        assert!(out.contains("[REDACTED:github_token]"));
    }

    #[test]
    fn clean_text_unchanged_and_no_hits() {
        let text = "this is a decision with no secrets in it at all.";
        let (out, hits) = redact(text);
        assert_eq!(out, text);
        assert!(hits.is_empty());
    }

    #[test]
    fn idempotent_second_pass_finds_nothing() {
        let text = "sk-abcdefghijklmnopqrstuvwxyz012345 ghp_abcdefghijklmnop0123456789";
        let (first, hits1) = redact(text);
        assert!(!hits1.is_empty(), "first pass should hit");
        let (second, hits2) = redact(&first);
        assert_eq!(second, first, "second pass produces identical output");
        assert!(hits2.is_empty(), "second pass has no hits (idempotent)");
    }

    #[test]
    fn hot_path_under_ten_ms_for_realistic_length() {
        // Realistic decide value + reason ~1-2KB — must be micro-second class.
        let clean = "this is a normal decision reason about architecture choices ".repeat(60);
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = redact(&clean);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "1000 scans of ~4KB clean text should stay under 500ms; got {elapsed:?}"
        );
    }
}
