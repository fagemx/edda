use std::fs;
use std::path::{Path, PathBuf};

pub fn format_long_version(version: &str, sha: Option<&str>, dirty: bool, date: &str) -> String {
    match sha {
        Some(sha) => {
            let dirty = if dirty { "-dirty" } else { "" };
            format!("{version} ({sha}{dirty} {date})")
        }
        None => format!("{version} (unknown)"),
    }
}

pub fn git_metadata_paths(head_path: &Path, common_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![head_path.to_path_buf()];
    let reference = fs::read_to_string(head_path)
        .ok()
        .and_then(|head| head.strip_prefix("ref: ").map(str::trim).map(str::to_owned))
        .filter(|reference| reference.starts_with("refs/"));

    if let Some(reference) = reference {
        let ref_path = common_dir.join(reference);
        paths.push(if ref_path.exists() {
            ref_path
        } else {
            common_dir.join("packed-refs")
        });
    }

    paths
}
