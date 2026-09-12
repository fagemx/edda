use edda_core::continuity::{is_valid_portable_dirty_path, CapsuleGitV1};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_GIT_TEXT: usize = 4 * 1024;
const MAX_STATUS_BYTES: usize = 256 * 1024;
const MAX_DIRTY_PATHS: usize = 64;

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
        retain_path(record.get(3..), &mut paths, &mut omitted);
        if status_code.iter().any(|byte| matches!(*byte, b'R' | b'C')) {
            retain_path(records.next(), &mut paths, &mut omitted);
        }
    }
    (paths, omitted)
}

fn retain_path(raw: Option<&[u8]>, paths: &mut Vec<String>, omitted: &mut bool) {
    match raw.and_then(parse_path) {
        Some(path) => paths.push(path),
        None => *omitted = true,
    }
}

fn parse_path(raw: &[u8]) -> Option<String> {
    let value = String::from_utf8(raw.to_vec()).ok()?;
    is_valid_portable_dirty_path(&value).then_some(value)
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
    fn preserves_portable_unicode_slash_and_trailing_directory_paths() {
        let (paths, omitted) = parse_status_paths(
            " M src/界.rs\0?? nested/file.rs\0?? untracked-directory/\0".as_bytes(),
        );
        assert_eq!(
            paths,
            ["src/界.rs", "nested/file.rs", "untracked-directory/"]
        );
        assert!(!omitted);
        assert!(paths.iter().all(|path| is_valid_portable_dirty_path(path)));
    }

    #[test]
    fn raw_status_omits_every_nonportable_path_and_preserves_dirty_truth() {
        let oversized = "a".repeat(513);
        let mut status = b"?? name:colon\0?? dir\\file\0?? /absolute\0?? ./dot\0?? ../parent\0?? dir//file\0?? control\nname\0?? ".to_vec();
        status.extend_from_slice(&[0xff, 0]);
        status.extend_from_slice(b"?? ");
        status.extend_from_slice(oversized.as_bytes());
        status.push(0);

        assert_eq!(parse_status_paths(&status), (Vec::new(), true));
        assert_eq!(
            status_metadata(Some((status, false))),
            (Some(true), Vec::new(), true)
        );
    }

    #[test]
    fn rename_and_copy_consume_both_sides_when_either_is_omitted() {
        assert_eq!(
            parse_status_paths(
                b"R  rejected:new.rs\0old/valid.rs\0 M after-rename.rs\0C  copied/valid.rs\0rejected\\source.rs\0?? tail/\0"
            ),
            (
                vec![
                    "old/valid.rs".to_string(),
                    "after-rename.rs".to_string(),
                    "copied/valid.rs".to_string(),
                    "tail/".to_string(),
                ],
                true
            )
        );
    }
}
