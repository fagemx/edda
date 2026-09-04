mod build_identity;

use std::path::PathBuf;
use std::process::Command;

fn main() {
    watch_git_metadata();

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    let long_version = build_long_version(&version);
    println!("cargo:rustc-env=EDDA_LONG_VERSION={long_version}");
}

fn watch_git_metadata() {
    let (Some(head_path), Some(common_dir)) = (git_path("HEAD"), git_common_dir()) else {
        return;
    };

    for path in build_identity::git_metadata_paths(&head_path, &common_dir) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn build_long_version(version: &str) -> String {
    let git_ref = git_ref();
    build_identity::format_long_version(version, git_ref.as_deref(), is_git_dirty(), &build_date())
}

fn git_ref() -> Option<String> {
    let sha = run_git(&["rev-parse", "HEAD"])?;
    if sha.len() < 12 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    Some(sha[..12].to_string())
}

fn git_path(path: &str) -> Option<PathBuf> {
    run_git(&["rev-parse", "--path-format=absolute", "--git-path", path]).map(PathBuf::from)
}

fn git_common_dir() -> Option<PathBuf> {
    run_git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).map(PathBuf::from)
}

fn is_git_dirty() -> bool {
    !git_is_clean(&["diff", "--quiet"]) || !git_is_clean(&["diff", "--cached", "--quiet"])
}

fn git_is_clean(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn build_date() -> String {
    if let Some(date) = run_command("date", &["-u", "+%Y-%m-%d"]) {
        if is_utc_date(&date) {
            return date;
        }
    }

    if let Some(date) = run_command(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')",
        ],
    ) {
        if is_utc_date(&date) {
            return date;
        }
    }

    "unknown".to_string()
}

fn is_utc_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn run_git(args: &[&str]) -> Option<String> {
    run_command("git", args)
}

fn run_command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if stdout.is_empty() || stdout.contains('\n') || stdout.contains('\r') {
        return None;
    }

    Some(stdout.to_string())
}
