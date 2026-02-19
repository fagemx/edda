use crate::check::CheckOutput;
use std::path::Path;
use std::time::Duration;

pub fn check_file_contains(path: &str, pattern: &str, cwd: &Path) -> CheckOutput {
    let full = cwd.join(path);
    match std::fs::read_to_string(&full) {
        Ok(content) if content.contains(pattern) => CheckOutput::passed(Duration::ZERO),
        Ok(_) => CheckOutput::failed(
            format!("pattern not found in {path}: \"{pattern}\""),
            Duration::ZERO,
        ),
        Err(_) => CheckOutput::failed(format!("file not found: {path}"), Duration::ZERO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "hello world").unwrap();
        let out = check_file_contains("f.txt", "world", dir.path());
        assert!(out.passed);
    }

    #[test]
    fn missing_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "hello").unwrap();
        let out = check_file_contains("f.txt", "world", dir.path());
        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("pattern not found"));
    }

    #[test]
    fn missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = check_file_contains("nope.txt", "x", dir.path());
        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("file not found"));
    }
}
