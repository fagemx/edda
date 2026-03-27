/// Validate that a string looks like a valid ISO 8601 / RFC 3339 timestamp.
pub(crate) fn validate_iso8601(s: &str) -> Result<(), String> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map(|_| ())
        .map_err(|_| format!("invalid ISO 8601 timestamp: {s}"))
}

pub(crate) fn time_now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}
