use crate::check::CheckOutput;
use std::path::Path;
use std::time::Instant;
use tokio::process::Command;

/// Check that a edda event of the given type exists.
/// Shells out to `edda log` (or `edda log` after rename).
pub async fn check_edda_event(
    event_type: &str,
    after: Option<&str>,
    cwd: &Path,
) -> CheckOutput {
    let start = Instant::now();

    let mut args = vec![
        "log".to_string(),
        "--json".to_string(),
        "--type".to_string(),
        event_type.to_string(),
        "--limit".to_string(),
        "1".to_string(),
    ];
    if let Some(after_ts) = after {
        if !after_ts.is_empty() {
            args.push("--after".to_string());
            args.push(after_ts.to_string());
        }
    }

    let result = Command::new("edda")
        .args(&args)
        .current_dir(cwd)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim().is_empty() {
                CheckOutput::failed(
                    format!("no event of type \"{event_type}\" found"),
                    start.elapsed(),
                )
            } else {
                CheckOutput::passed(start.elapsed())
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            CheckOutput::failed(
                format!("edda log failed: {}", stderr.trim()),
                start.elapsed(),
            )
        }
        Err(e) => CheckOutput::failed(
            format!("edda not available: {e}"),
            start.elapsed(),
        ),
    }
}
