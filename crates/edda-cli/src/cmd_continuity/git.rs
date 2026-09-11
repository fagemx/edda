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
    let status = git_bytes(
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        MAX_STATUS_BYTES,
    );
    let (tree_dirty, mut dirty_paths, dirty_paths_truncated) = status_metadata(status);
    dirty_paths.truncate(MAX_DIRTY_PATHS);
    CapsuleGitV1 {
        detached: Some(branch.is_none()),
        tree_dirty,
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

fn status_metadata(status: Option<(Vec<u8>, bool)>) -> (Option<bool>, Vec<String>, bool) {
    match status {
        Some((status, output_truncated)) => {
            let (paths, omitted_path) = parse_status_paths(&status);
            let truncated = output_truncated || omitted_path || paths.len() > MAX_DIRTY_PATHS;
            (Some(!status.is_empty()), paths, truncated)
        }
        None => (None, Vec::new(), true),
    }
}

fn parse_status_paths(status: &[u8]) -> (Vec<String>, bool) {
    let mut records = status
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut paths = Vec::new();
    let mut omitted = false;
    while let Some(record) = records.next() {
        let Some(status_code) = record.get(..2) else {
            omitted = true;
            continue;
        };
        let Some(path) = record.get(3..).and_then(parse_path) else {
            omitted = true;
            continue;
        };
        paths.push(path);
        if status_code.iter().any(|byte| matches!(*byte, b'R' | b'C')) {
            match records.next().and_then(parse_path) {
                Some(source_path) => paths.push(source_path),
                None => omitted = true,
            }
        }
    }
    (paths, omitted)
}

fn parse_path(raw: &[u8]) -> Option<String> {
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
        .env("GIT_OPTIONAL_LOCKS", "0")
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
    fn failed_status_is_unknown_not_clean() {
        assert_eq!(status_metadata(None), (None, Vec::new(), true));
    }

    #[test]
    fn parses_unicode_status_path() {
        assert_eq!(
            parse_status_paths(" M src/界.rs\0".as_bytes()),
            (vec!["src/界.rs".to_string()], false)
        );
    }

    #[test]
    fn parses_rename_pair_without_treating_source_as_status() {
        assert_eq!(
            parse_status_paths(b"R  new/name.rs\0old/name.rs\0"),
            (
                vec!["new/name.rs".to_string(), "old/name.rs".to_string()],
                false
            )
        );
    }

    #[test]
    fn rejects_traversal_status_path() {
        assert_eq!(parse_status_paths(b"?? ../secret\0"), (Vec::new(), true));
    }
}
