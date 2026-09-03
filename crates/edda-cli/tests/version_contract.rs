//! Integration tests for build identity in `edda --version` (GH-746).

#[path = "../build_identity.rs"]
mod build_identity;

use std::path::PathBuf;
use std::process::Command;

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

#[test]
fn format_long_version_has_dirty_and_gitless_forms() {
    assert_eq!(
        build_identity::format_long_version("0.4.0", Some("0123456789ab"), false, "2026-09-04"),
        "0.4.0 (0123456789ab 2026-09-04)"
    );
    assert_eq!(
        build_identity::format_long_version("0.4.0", Some("0123456789ab"), true, "2026-09-04"),
        "0.4.0 (0123456789ab-dirty 2026-09-04)"
    );
    assert_eq!(
        build_identity::format_long_version("0.4.0", None, false, "2026-09-04"),
        "0.4.0 (unknown)"
    );
}

#[test]
fn git_metadata_paths_follow_a_worktree_head_to_its_ref() {
    let temp = tempfile::tempdir().expect("tempdir");
    let common_dir = temp.path().join("common");
    let head_path = temp.path().join("worktrees").join("pr804").join("HEAD");
    let ref_path = common_dir.join("refs").join("heads").join("feature");
    std::fs::create_dir_all(head_path.parent().expect("HEAD parent")).expect("create HEAD parent");
    std::fs::create_dir_all(ref_path.parent().expect("ref parent")).expect("create ref parent");
    std::fs::write(&head_path, "ref: refs/heads/feature\n").expect("write HEAD");
    std::fs::write(&ref_path, "0123456789abcdef\n").expect("write ref");

    assert_eq!(
        build_identity::git_metadata_paths(&head_path, &common_dir),
        vec![head_path, ref_path]
    );
}

#[test]
fn git_metadata_paths_watch_packed_refs_and_a_new_loose_ref() {
    let temp = tempfile::tempdir().expect("tempdir");
    let common_dir = temp.path().join("common");
    let ref_dir = common_dir.join("refs").join("heads");
    let head_path = temp.path().join("worktrees").join("pr804").join("HEAD");
    let packed_refs = common_dir.join("packed-refs");
    std::fs::create_dir_all(head_path.parent().expect("HEAD parent")).expect("create HEAD parent");
    std::fs::create_dir_all(&ref_dir).expect("create ref directory");
    std::fs::write(&head_path, "ref: refs/heads/feature\n").expect("write HEAD");
    std::fs::write(&packed_refs, "0123456789abcdef refs/heads/feature\n")
        .expect("write packed refs");

    assert_eq!(
        build_identity::git_metadata_paths(&head_path, &common_dir),
        vec![head_path, packed_refs, ref_dir]
    );
}

#[test]
fn version_reports_the_built_commit_and_date() {
    let output = Command::new(edda_bin())
        .arg("--version")
        .output()
        .expect("spawn edda --version");
    assert!(output.status.success(), "stderr={:?}", output.stderr);

    let built_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read current git SHA");
    assert!(built_sha.status.success());
    let built_sha = String::from_utf8_lossy(&built_sha.stdout);
    let expected_sha = &built_sha.trim()[..12];

    let stdout = String::from_utf8_lossy(&output.stdout);
    let prefix = format!("edda {} (", env!("CARGO_PKG_VERSION"));
    let identity = stdout
        .trim()
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(')'))
        .expect("version must include a parenthesized build identity");
    let mut parts = identity.split_whitespace();
    let sha = parts.next().expect("build SHA").trim_end_matches("-dirty");
    let date = parts.next().expect("build date");
    assert_eq!(parts.next(), None, "unexpected version fields: {identity}");
    assert_eq!(sha, expected_sha, "identity={identity}");
    assert!(sha.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(
        date.len() == 10
            && date.as_bytes()[4] == b'-'
            && date.as_bytes()[7] == b'-'
            && date
                .bytes()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit()),
        "date must be YYYY-MM-DD: {date}"
    );
}
