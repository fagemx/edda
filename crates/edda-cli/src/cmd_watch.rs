use std::process::Command;

/// Launch the edda-tui binary (co-located with the edda binary).
pub fn execute() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tui_name = if cfg!(windows) {
        "edda-tui.exe"
    } else {
        "edda-tui"
    };
    let tui_path = dir.join(tui_name);

    if !tui_path.exists() {
        anyhow::bail!(
            "edda-tui not found at {}. Install it with: cargo install --path crates/edda-tui",
            tui_path.display()
        );
    }

    let status = Command::new(&tui_path)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch edda-tui: {e}"))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
