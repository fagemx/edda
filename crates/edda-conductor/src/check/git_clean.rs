use crate::check::CheckOutput;
use std::path::Path;
use std::time::Instant;
use tokio::process::Command;

pub async fn check_git_clean(allow_untracked: bool, cwd: &Path) -> CheckOutput {
    let start = Instant::now();

    let result = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let dirty_lines: Vec<&str> = stdout
                .lines()
                .filter(|line| {
                    if allow_untracked {
                        // Skip untracked files (lines starting with "??")
                        !line.starts_with("??")
                    } else {
                        true
                    }
                })
                .filter(|line| !line.trim().is_empty())
                .collect();

            if dirty_lines.is_empty() {
                CheckOutput::passed(start.elapsed())
            } else {
                let preview: String = dirty_lines
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                let extra = if dirty_lines.len() > 5 {
                    format!("\n... and {} more", dirty_lines.len() - 5)
                } else {
                    String::new()
                };
                CheckOutput::failed(
                    format!(
                        "working tree not clean ({} files):\n{preview}{extra}",
                        dirty_lines.len()
                    ),
                    start.elapsed(),
                )
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            CheckOutput::failed(
                format!("git status failed: {}", stderr.trim()),
                start.elapsed(),
            )
        }
        Err(e) => CheckOutput::failed(format!("git not available: {e}"), start.elapsed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        // Init a git repo with at least one commit
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await;
        std::fs::write(dir.path().join("README"), "hi").unwrap();
        let _ = Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await;

        let out = check_git_clean(false, dir.path()).await;
        assert!(out.passed);
    }

    #[tokio::test]
    async fn dirty_repo() {
        let dir = tempfile::tempdir().unwrap();
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await;
        std::fs::write(dir.path().join("dirty.txt"), "x").unwrap();

        let out = check_git_clean(false, dir.path()).await;
        assert!(!out.passed);
        assert!(out.detail.unwrap().contains("dirty.txt"));
    }

    #[tokio::test]
    async fn allow_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await;
        std::fs::write(dir.path().join("README"), "hi").unwrap();
        let _ = Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await;

        // Add an untracked file
        std::fs::write(dir.path().join("untracked.txt"), "x").unwrap();

        // With allow_untracked=true, should pass
        let out = check_git_clean(true, dir.path()).await;
        assert!(out.passed);

        // Without, should fail
        let out = check_git_clean(false, dir.path()).await;
        assert!(!out.passed);
    }
}
