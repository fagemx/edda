pub(crate) enum SpecOrigin {
    None,
    Path,
    ExplicitIssue,
    PrDerived { author_perm: Option<String> },
}

pub(crate) fn spec_trust(origin: &SpecOrigin, trust_flag: bool) -> &'static str {
    match origin {
        SpecOrigin::None => "none",
        SpecOrigin::Path => "operator",
        _ if trust_flag => "operator",
        SpecOrigin::ExplicitIssue => "untrusted",
        SpecOrigin::PrDerived { author_perm } => match author_perm.as_deref() {
            Some("admin" | "maintain" | "write") => "maintainer",
            _ => "untrusted",
        },
    }
}

/// Read commands only inside the named section. Preserve shell quoting and
/// internal whitespace; a sibling YAML key must not become an executable line.
pub(crate) fn extract_verify(spec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut section = false;
    let mut yaml_indent = None;
    let mut fenced = false;
    for line in spec.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if !fenced && trimmed.starts_with('#') {
            section = trimmed
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case("verify");
            yaml_indent = None;
            continue;
        }
        if !fenced && trimmed.eq_ignore_ascii_case("verify:") {
            section = true;
            yaml_indent = Some(indent);
            continue;
        }
        if !section || trimmed.is_empty() {
            continue;
        }
        if yaml_indent.is_some_and(|base| indent <= base && !trimmed.starts_with("- ")) {
            section = false;
            yaml_indent = None;
            continue;
        }
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let command = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let command = command.strip_prefix("$ ").unwrap_or(command);
        if !command.is_empty() {
            out.push(command.to_owned());
        }
    }
    out
}
