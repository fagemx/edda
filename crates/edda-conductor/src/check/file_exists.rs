use crate::check::CheckOutput;
use std::path::Path;
use std::time::Duration;

pub fn check_file_exists(path: &str, cwd: &Path) -> CheckOutput {
    let full = cwd.join(path);
    if full.exists() {
        CheckOutput::passed(Duration::ZERO)
    } else {
        CheckOutput::failed(format!("file not found: {path}"), Duration::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let out = check_file_exists("a.txt", dir.path());
        assert!(out.passed);
    }

    #[test]
    fn missing() {
        let dir = tempfile::tempdir().unwrap();
        let out = check_file_exists("nope.txt", dir.path());
        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("nope.txt"));
    }

    #[test]
    fn nested_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file.rs"), "").unwrap();
        let out = check_file_exists("sub/file.rs", dir.path());
        assert!(out.passed);
    }
}
