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
        let ref_exists = ref_path.exists();
        if ref_exists {
            paths.push(ref_path.clone());
        }

        let packed_refs = common_dir.join("packed-refs");
        if packed_refs.exists() {
            paths.push(packed_refs);
        }

        if !ref_exists {
            let mut ancestor = ref_path.parent();
            while let Some(candidate) = ancestor {
                if candidate.is_dir() {
                    paths.push(candidate.to_path_buf());
                    break;
                }
                ancestor = candidate.parent();
            }
        }
    }

    paths
}
