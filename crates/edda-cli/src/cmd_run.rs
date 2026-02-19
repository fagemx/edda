use edda_core::event::{new_cmd_event, CmdEventParams};
use edda_ledger::blob_store::blob_put;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::io::Write;
use std::path::Path;

pub fn execute(repo_root: &Path, argv: &[String]) -> anyhow::Result<()> {
    if argv.is_empty() {
        anyhow::bail!("usage: edda run -- <command> [args...]");
    }

    let ledger = Ledger::open(repo_root)?;
    let start = std::time::Instant::now();

    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute '{}': {e}", argv[0]))?;

    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_code = output.status.code().unwrap_or(-1);

    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let stdout_blob = blob_put(&ledger.paths, &output.stdout)?;
    let stderr_blob = blob_put(&ledger.paths, &output.stderr)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let cwd = repo_root.to_string_lossy().to_string();

    let event = new_cmd_event(&CmdEventParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        argv,
        cwd: &cwd,
        exit_code,
        duration_ms,
        stdout_blob: &stdout_blob,
        stderr_blob: &stderr_blob,
    })?;
    ledger.append_event(&event, false)?;

    // Replay output to terminal
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;

    println!("Recorded CMD {} exit={exit_code}", event.event_id);
    Ok(())
}
