use super::types::*;
use crate::secret_guard::redact;
use std::path::{Component, Path};

const MAX_SHORT_CHARS: usize = 160;
const MAX_TEXT_CHARS: usize = 4_000;
const MAX_LIST_ITEMS: usize = 32;
const MAX_LIST_ITEM_CHARS: usize = 1_000;
const MAX_DIRTY_PATHS: usize = 64;
const MAX_PATH_CHARS: usize = 512;

pub fn validate_raw_secrets(bytes: &[u8], label: &str) -> anyhow::Result<()> {
    let text = String::from_utf8_lossy(bytes);
    let (_, hits) = redact(&text);
    if let Some(hit) = hits.first() {
        anyhow::bail!("secret content refused in {label} (kind: {})", hit.kind);
    }
    Ok(())
}

pub fn validate_input_secrets(input: &ContextCapsuleInputV1) -> anyhow::Result<()> {
    let value = serde_json::to_value(input)?;
    scan_value(&value, "input")
}

pub(crate) fn validate_source_secrets(source: &ContextSourceV1) -> anyhow::Result<()> {
    scan_value(&serde_json::to_value(source)?, "local_source")
}

pub fn validate_capsule(capsule: &ContextCapsuleV1) -> anyhow::Result<()> {
    if capsule.capsule_version != CONTINUITY_CAPSULE_VERSION {
        anyhow::bail!("unsupported capsule_version (expected 1)");
    }
    validate_id(&capsule.capsule_id, "cap_", "capsule_id")?;
    time::OffsetDateTime::parse(
        &capsule.created_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| anyhow::anyhow!("created_at must be RFC 3339"))?;
    validate_source(&capsule.source)?;
    validate_repository(&capsule.repository)?;
    validate_git(&capsule.git)?;
    validate_state_bounds(&capsule.state)?;
    validate_references(&capsule.references)?;
    validate_truncation(&capsule.truncation)?;
    scan_value(&serde_json::to_value(capsule)?, "capsule")?;
    Ok(())
}

pub fn validate_event_id(value: &str) -> anyhow::Result<()> {
    validate_id(value, "evt_", "origin_event_id")
}

pub fn validate_portable_repo_id(value: &str) -> anyhow::Result<()> {
    if value.len() != 69
        || !value.starts_with("repo_")
        || !value[5..].chars().all(|c| c.is_ascii_hexdigit())
    {
        anyhow::bail!("portable_repo_id must be repo_ followed by 64 hexadecimal characters");
    }
    Ok(())
}

pub(crate) fn bounded_string(
    value: Option<String>,
    default: &str,
    max_chars: usize,
    field: &str,
    notices: &mut Vec<TruncationNoticeV1>,
) -> String {
    let value = value.unwrap_or_else(|| default.to_string());
    truncate_string(value, max_chars, field, notices)
}

pub(crate) fn bounded_required(
    value: String,
    max_chars: usize,
    field: &str,
    notices: &mut Vec<TruncationNoticeV1>,
) -> anyhow::Result<String> {
    if value.trim().is_empty() {
        anyhow::bail!("{field} must not be blank");
    }
    Ok(truncate_string(value, max_chars, field, notices))
}

pub(crate) fn bounded_list(
    values: Vec<String>,
    field: &str,
    notices: &mut Vec<TruncationNoticeV1>,
) -> Vec<String> {
    let omitted_items = values.len().saturating_sub(MAX_LIST_ITEMS);
    if omitted_items > 0 {
        notices.push(TruncationNoticeV1 {
            field: field.to_string(),
            omitted_chars: 0,
            omitted_items,
        });
    }
    values
        .into_iter()
        .take(MAX_LIST_ITEMS)
        .enumerate()
        .map(|(index, value)| {
            truncate_string(
                value,
                MAX_LIST_ITEM_CHARS,
                &format!("{field}[{index}]"),
                notices,
            )
        })
        .collect()
}

pub(crate) fn bounded_rejected(
    values: Vec<RejectedContextHypothesisV1>,
    notices: &mut Vec<TruncationNoticeV1>,
) -> Vec<RejectedContextHypothesisV1> {
    let omitted_items = values.len().saturating_sub(MAX_LIST_ITEMS);
    if omitted_items > 0 {
        notices.push(TruncationNoticeV1 {
            field: "state.rejected".to_string(),
            omitted_chars: 0,
            omitted_items,
        });
    }
    values
        .into_iter()
        .take(MAX_LIST_ITEMS)
        .enumerate()
        .map(|(index, value)| RejectedContextHypothesisV1 {
            hypothesis: truncate_string(
                value.hypothesis,
                MAX_LIST_ITEM_CHARS,
                &format!("state.rejected[{index}].hypothesis"),
                notices,
            ),
            reason: truncate_string(
                value.reason,
                MAX_LIST_ITEM_CHARS,
                &format!("state.rejected[{index}].reason"),
                notices,
            ),
        })
        .collect()
}

fn truncate_string(
    value: String,
    max_chars: usize,
    field: &str,
    notices: &mut Vec<TruncationNoticeV1>,
) -> String {
    let chars = value.chars().count();
    if chars <= max_chars {
        return value;
    }
    let truncated: String = value.chars().take(max_chars).collect();
    notices.push(TruncationNoticeV1 {
        field: field.to_string(),
        omitted_chars: chars - max_chars,
        omitted_items: 0,
    });
    truncated
}

fn validate_id(value: &str, prefix: &str, field: &str) -> anyhow::Result<()> {
    let suffix = value
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("{field} must start with {prefix}"))?;
    if suffix.is_empty()
        || suffix.len() > 64
        || !suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        anyhow::bail!("{field} has an invalid identifier shape");
    }
    Ok(())
}

fn validate_source(source: &ContextSourceV1) -> anyhow::Result<()> {
    for (field, value) in [
        ("source.machine_alias", source.machine_alias.as_deref()),
        ("source.actor", source.actor.as_deref()),
    ] {
        if value
            .is_some_and(|value| value.chars().count() > MAX_SHORT_CHARS || value.contains('\0'))
        {
            anyhow::bail!("{field} exceeds its bound or contains NUL");
        }
    }
    Ok(())
}

fn validate_repository(repository: &CapsuleRepositoryV1) -> anyhow::Result<()> {
    match (&repository.portable_repo_id, &repository.local_only_reason) {
        (Some(id), None) => validate_portable_repo_id(id)?,
        (None, Some(reason)) if !reason.trim().is_empty() && reason.chars().count() <= 256 => {}
        (Some(_), Some(_)) => anyhow::bail!("repository cannot be portable and local-only"),
        _ => anyhow::bail!("repository must declare portable_repo_id or local_only_reason"),
    }
    if let Some(hint) = &repository.display_hint {
        if hint.len() > MAX_PATH_CHARS || hint.contains('@') || !safe_relative_slash_path(hint) {
            anyhow::bail!("repository.display_hint is not a safe credential-free host/path");
        }
    }
    Ok(())
}

fn validate_git(git: &CapsuleGitV1) -> anyhow::Result<()> {
    if let Some(branch) = &git.branch {
        if branch.chars().count() > MAX_PATH_CHARS || !safe_relative_slash_path(branch) {
            anyhow::bail!("git.branch is unsafe or exceeds its bound");
        }
    }
    if let Some(sha) = &git.head_sha {
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("git.head_sha must be a full 40-hex SHA");
        }
    }
    if git.dirty_paths.len() > MAX_DIRTY_PATHS {
        anyhow::bail!("git.dirty_paths exceeds {MAX_DIRTY_PATHS} entries");
    }
    for path in &git.dirty_paths {
        if path.chars().count() > MAX_PATH_CHARS || !safe_portable_dirty_path(path) {
            anyhow::bail!("git.dirty_paths contains an unsafe or oversized path");
        }
    }
    Ok(())
}

fn validate_state_bounds(state: &ContextStateV1) -> anyhow::Result<()> {
    for (field, value, max) in [
        ("state.title", &state.title, MAX_SHORT_CHARS),
        ("state.summary", &state.summary, MAX_TEXT_CHARS),
        ("state.goal", &state.goal, MAX_TEXT_CHARS),
        ("state.current", &state.current, MAX_TEXT_CHARS),
        ("state.next_action", &state.next_action, MAX_TEXT_CHARS),
    ] {
        if value.chars().count() > max || (field == "state.next_action" && value.trim().is_empty())
        {
            anyhow::bail!("{field} is blank or exceeds its bound");
        }
    }
    validate_string_list(&state.hypotheses, "state.hypotheses")?;
    validate_string_list(&state.open_questions, "state.open_questions")?;
    if state.rejected.len() > MAX_LIST_ITEMS {
        anyhow::bail!("state.rejected exceeds {MAX_LIST_ITEMS} entries");
    }
    for item in &state.rejected {
        if item.hypothesis.chars().count() > MAX_LIST_ITEM_CHARS
            || item.reason.chars().count() > MAX_LIST_ITEM_CHARS
        {
            anyhow::bail!("state.rejected contains an oversized entry");
        }
    }
    Ok(())
}

fn validate_references(references: &ContextReferencesV1) -> anyhow::Result<()> {
    validate_string_list(&references.task_ids, "references.task_ids")?;
    validate_string_list(&references.event_ids, "references.event_ids")?;
    for event_id in &references.event_ids {
        validate_id(event_id, "evt_", "references.event_ids")?;
    }
    Ok(())
}

fn validate_truncation(notices: &[TruncationNoticeV1]) -> anyhow::Result<()> {
    if notices.len() > 256
        || notices.iter().any(|notice| {
            notice.field.is_empty()
                || notice.field.len() > 128
                || !notice.field.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '[' | ']')
                })
                || (notice.omitted_chars == 0 && notice.omitted_items == 0)
        })
    {
        anyhow::bail!("truncation notices exceed their typed bound");
    }
    Ok(())
}

fn validate_string_list(values: &[String], field: &str) -> anyhow::Result<()> {
    if values.len() > MAX_LIST_ITEMS {
        anyhow::bail!("{field} exceeds {MAX_LIST_ITEMS} entries");
    }
    if values
        .iter()
        .any(|v| v.chars().count() > MAX_LIST_ITEM_CHARS)
    {
        anyhow::bail!("{field} contains an oversized entry");
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(char::is_control)
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn safe_relative_slash_path(value: &str) -> bool {
    !value.contains('\\') && safe_relative_path(value)
}

fn safe_portable_dirty_path(value: &str) -> bool {
    let path = value.strip_suffix('/').unwrap_or(value);
    !path.is_empty()
        && !value.chars().any(char::is_control)
        && !value.contains(['\\', ':'])
        && !value.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

fn scan_value(value: &serde_json::Value, path: &str) -> anyhow::Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                scan_value(child, &format!("{path}.{key}"))?;
            }
        }
        serde_json::Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                scan_value(child, &format!("{path}[{index}]"))?;
            }
        }
        serde_json::Value::String(text) => {
            let (_, hits) = redact(text);
            if let Some(hit) = hits.first() {
                anyhow::bail!(
                    "secret content refused in field {path} (kind: {})",
                    hit.kind
                );
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_paths_use_one_portable_slash_grammar_on_every_host() {
        for path in [
            r"..\outside",
            r"dir\file",
            r"dir\..\outside",
            "C:/outside",
            "/absolute",
            "dir//file",
            "dir/../file",
            "dir/./file",
            "dir//",
        ] {
            let git = CapsuleGitV1 {
                dirty_paths: vec![path.to_string()],
                ..CapsuleGitV1::default()
            };
            let error = validate_git(&git).expect_err(path);
            assert!(error.to_string().contains("unsafe"), "{path}: {error}");
        }

        let git = CapsuleGitV1 {
            dirty_paths: vec!["src/界.rs".to_string(), ".edda/".to_string()],
            ..CapsuleGitV1::default()
        };
        validate_git(&git).unwrap();
    }
}
