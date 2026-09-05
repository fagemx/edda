//! Shared model selection for Edda's own background Anthropic requests.

/// The current low-cost Anthropic model for Edda background work.
pub const DEFAULT_BACKGROUND_MODEL: &str = "claude-haiku-4-5-20251001";

/// Resolve the background model from an optional `EDDA_BG_MODEL` value.
///
/// The value is supplied by the caller so production code can read its normal
/// environment while tests can exercise the same selection without mutating
/// the process environment.
#[must_use]
pub fn resolve_background_model(override_value: Option<&str>) -> String {
    override_value
        .unwrap_or(DEFAULT_BACKGROUND_MODEL)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{resolve_background_model, DEFAULT_BACKGROUND_MODEL};

    #[test]
    fn defaults_when_no_override_is_supplied() {
        assert_eq!(resolve_background_model(None), DEFAULT_BACKGROUND_MODEL);
    }

    #[test]
    fn preserves_the_override_as_the_request_model() {
        assert_eq!(
            resolve_background_model(Some("claude-test-model")),
            "claude-test-model"
        );
    }

    #[test]
    fn preserves_an_empty_override_for_backwards_compatibility() {
        assert_eq!(resolve_background_model(Some("")), "");
    }
}
