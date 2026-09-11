use edda_core::continuity::CapsuleGitV1;
use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Stdio};

const MAX_GIT_TEXT: usize = 4 * 1024;
const MAX_STATUS_BYTES: usize = 256 * 1024;
const MAX_DIRTY_PATHS: usize = 64;
const MAX_PATH_CHARS: usize = 512;

pub fn gather_git_metadata(checkout: &Path) -> CapsuleGitV1 {
    if git_text(checkout, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return CapsuleGitV1::default();
    }
    let branch = git_text(checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let head_sha = git_text(checkout, &["rev-parse", "HEAD"])
        .filter(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()));
    let (status, output_truncated) = git_bytes(
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        MAX_STATUS_BYTES,
    )
    .unwrap_or_default();
    let mut dirty_paths = Vec::new();
    let mut omitted_path = false;
    for record in status
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        if let Some(path) = parse_status_path(record) {
            dirty_paths.push(path);
        } else {
            omitted_path = true;
        }
    }
    let dirty_paths_truncated =
        output_truncated || omitted_path || dirty_paths.len() > MAX_DIRTY_PATHS;
    dirty_paths.truncate(MAX_DIRTY_PATHS);
    CapsuleGitV1 {
        detached: Some(branch.is_none()),
        tree_dirty: Some(!status.is_empty()),
        branch,
        head_sha,
        dirty_paths,
        dirty_paths_truncated,
    }
}

pub fn commit_exists(checkout: &Path, sha: &str) -> bool {
    git_bytes(
        checkout,
        &["cat-file", "-e", &format!("{sha}^{{commit}}")],
        MAX_GIT_TEXT,
    )
    .is_some_and(|(_, truncated)| !truncated)
}

fn parse_status_path(record: &[u8]) -> Option<String> {
    let raw = record.get(3..)?;
    let value = String::from_utf8(raw.to_vec()).ok()?;
    if value.is_empty()
        || value.chars().count() > MAX_PATH_CHARS
        || value.chars().any(char::is_control)
        || Path::new(&value).is_absolute()
        || !Path::new(&value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(value)
}

fn git_text(checkout: &Path, args: &[&str]) -> Option<String> {
    let (bytes, truncated) = git_bytes(checkout, args, MAX_GIT_TEXT)?;
    if truncated {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(text.trim().to_string())
}

fn git_bytes(checkout: &Path, args: &[&str], limit: usize) -> Option<(Vec<u8>, bool)> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > limit;
    if truncated {
        bytes.truncate(limit);
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    if !status.success() && !truncated {
        return None;
    }
    Some((bytes, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unicode_status_path() {
        assert_eq!(
            parse_status_path(" M src/界.rs".as_bytes()),
            Some("src/界.rs".to_string())
        );
    }

    #[test]
    fn rejects_traversal_status_path() {
        assert_eq!(parse_status_path(b"?? ../secret"), None);
    }
}
