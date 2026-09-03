use std::path::Path;

use super::request::resolve_session_id;

// ── Render Commands ──

/// `edda bridge claude render-writeback`
pub fn render_writeback() -> anyhow::Result<()> {
    println!("{}", edda_bridge_claude::render::writeback());
    Ok(())
}

/// `edda bridge claude render-workspace`
pub fn render_workspace(repo_root: &Path, budget: usize) -> anyhow::Result<()> {
    let cwd = repo_root.to_str().unwrap_or(".");
    match edda_bridge_claude::render::workspace(cwd, budget) {
        Some(s) => println!("{s}"),
        None => println!("(no workspace context)"),
    }
    Ok(())
}

/// `edda bridge claude render-coordination`
pub fn render_coordination(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    match edda_bridge_claude::render::coordination(&project_id, &session_id) {
        Some(s) => println!("{s}"),
        None => println!("(no coordination context)"),
    }
    Ok(())
}

/// `edda bridge claude render-fleet`
///
/// The same section the SessionStart pack embeds. It exists as a verb because a
/// section only reachable through a hook is a section nobody can look at —
/// including whoever has to work out why it said what it said.
pub fn render_fleet(repo_root: &Path, budget: usize) -> anyhow::Result<()> {
    match edda_bridge_claude::render::fleet(&repo_root.to_string_lossy(), budget) {
        Some(s) => println!("{s}"),
        None => println!("(no siblings with rulings or waiting work)"),
    }
    Ok(())
}

/// `edda bridge claude render-pack`
pub fn render_pack(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::pack(&project_id) {
        Some(s) => println!("{s}"),
        None => println!("(no hot pack available)"),
    }
    Ok(())
}

/// `edda bridge claude render-plan`
pub fn render_plan(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::plan(Some(&project_id)) {
        Some(s) => println!("{s}"),
        None => println!("(no active plan)"),
    }
    Ok(())
}

// ── Heartbeat Commands ──

/// `edda bridge claude heartbeat-write`
pub fn heartbeat_write(
    repo_root: &Path,
    label: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label)?;
    let _ = edda_store::ensure_dirs(&project_id);
    edda_bridge_claude::peers::write_heartbeat_minimal(
        &project_id,
        &session_id,
        label,
        repo_root.to_str().unwrap_or("."),
    );
    println!("Heartbeat written: {label} ({session_id})");
    Ok(())
}

/// `edda bridge claude heartbeat-touch`
pub fn heartbeat_touch(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    edda_bridge_claude::peers::touch_heartbeat(&project_id, &session_id);
    println!("Heartbeat touched: {session_id}");
    Ok(())
}

/// `edda bridge claude heartbeat-remove`
pub fn heartbeat_remove(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    edda_bridge_claude::peers::remove_heartbeat(&project_id, &session_id);
    println!("Heartbeat removed: {session_id}");
    Ok(())
}
