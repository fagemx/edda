//! Model identity across git trailers, provider observations and receipts.
//! Unknown model families return `None`; callers must treat them as unverified.

const MODEL_FAMILIES: &[&str] = &[
    "claude-",
    "gpt-",
    "glm-",
    "gemini-",
    "deepseek-",
    "qwen",
    "llama",
    "mistral",
    "codex",
];

pub fn canonical_model_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("://") {
        return None;
    }
    let tail = trimmed.rsplit('/').next()?;
    let id = tail
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return None;
    }
    let family = MODEL_FAMILIES.iter().any(|family| {
        id.strip_prefix(*family).is_some_and(|suffix| {
            !family.ends_with('-') || suffix.chars().any(|c| c.is_ascii_alphanumeric())
        })
    });
    let reasoning_model = ["o1", "o3", "o4"].iter().any(|family| {
        id == *family
            || id
                .strip_prefix(*family)
                .and_then(|tail| tail.strip_prefix('-'))
                .is_some_and(|suffix| !suffix.is_empty())
    });
    (family || reasoning_model).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::canonical_model_id;

    #[test]
    fn model_identity_matches_every_source_pair() {
        // Git trailer, modelUsage, pi session, and dispatch receipt.
        let sources = [
            "Claude Opus 5",
            "claude-opus-5",
            "anthropic/claude-opus-5",
            "openrouter/anthropic/claude-opus-5",
        ];
        for first in sources {
            for second in sources {
                assert_eq!(canonical_model_id(first), canonical_model_id(second));
                assert_eq!(canonical_model_id(first).as_deref(), Some("claude-opus-5"));
            }
        }
        for sources in [
            ["openai-codex/gpt-5.6-sol", "gpt-5.6-sol"],
            ["openrouter/z-ai/glm-5.3-flash", "z-ai/glm-5.3-flash"],
            ["Claude Opus 4.6", "claude-opus-4.6"],
            ["Claude Fable 5.1", "claude-fable-5.1"],
        ] {
            assert_eq!(
                canonical_model_id(sources[0]),
                canonical_model_id(sources[1])
            );
            assert!(canonical_model_id(sources[0]).is_some());
        }
    }

    #[test]
    fn unknown_or_malformed_identity_is_never_verified() {
        for raw in [
            "",
            "  ",
            "model://gpt-5",
            "Tim Chen",
            "synvoke",
            "arbitrary-model",
            "gpt-",
            "claude-",
            "gpt-test!",
            "openai/",
            "o123person",
        ] {
            assert_eq!(canonical_model_id(raw), None, "{raw}");
        }
    }
}
