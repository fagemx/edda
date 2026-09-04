use std::io::Read;
use std::path::Path;

use super::claude::run_hook_resilient;

// ── OpenClaw Bridge ──

/// `edda bridge openclaw install`
pub fn install_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::install(target)
}

/// `edda bridge openclaw uninstall`
pub fn uninstall_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::uninstall(target)
}

/// `edda hook openclaw` — read stdin, dispatch hook
///
/// Resilience: catch_unwind + configurable timeout (EDDA_HOOK_TIMEOUT_MS).
/// On panic or timeout, exits 0 — never blocks the host agent.
pub fn hook_openclaw() -> anyhow::Result<()> {
    run_hook_resilient("OPENCLAW ", |stdin| {
        let r = edda_bridge_openclaw::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// `edda doctor openclaw`
pub fn doctor_openclaw() -> anyhow::Result<()> {
    edda_bridge_openclaw::doctor()
}

/// `edda bridge codex install`
pub fn install_codex(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_codex::install(target).map(|_| ())
}

/// `edda bridge codex uninstall`
pub fn uninstall_codex(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_codex::uninstall(target)
}

/// `edda hook codex` — read stdin, dispatch hook
pub fn hook_codex() -> anyhow::Result<()> {
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin)?;
    let r = edda_bridge_codex::hook_entrypoint_from_stdin(&stdin)?;
    if let Some(out) = r.stdout {
        println!("{out}");
    }
    if let Some(err) = r.stderr {
        eprintln!("{err}");
    }
    Ok(())
}

/// `edda doctor codex`
pub fn doctor_codex() -> anyhow::Result<()> {
    edda_bridge_codex::doctor()
}

/// `edda bridge hermes install`
pub fn install_hermes(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_hermes::install(target).map(|_| ())
}

/// `edda bridge hermes uninstall`
pub fn uninstall_hermes(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_hermes::uninstall(target)
}

/// `edda hook hermes` — read stdin, dispatch hook
pub fn hook_hermes() -> anyhow::Result<()> {
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin)?;
    let r = edda_bridge_hermes::hook_entrypoint_from_stdin(&stdin)?;
    if let Some(out) = r.stdout {
        println!("{out}");
    }
    if let Some(err) = r.stderr {
        eprintln!("{err}");
    }
    Ok(())
}

/// `edda doctor hermes`
pub fn doctor_hermes() -> anyhow::Result<()> {
    edda_bridge_hermes::doctor()
}

/// `edda bridge cursor install`
pub fn install_cursor(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_cursor::install(target).map(|_| ())
}

/// `edda bridge cursor uninstall`
pub fn uninstall_cursor(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_cursor::uninstall(target)
}

/// `edda hook cursor` — read stdin, dispatch hook
pub fn hook_cursor() -> anyhow::Result<()> {
    run_hook_resilient("CURSOR ", |stdin| {
        let r = edda_bridge_cursor::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// `edda doctor cursor`
pub fn doctor_cursor() -> anyhow::Result<()> {
    edda_bridge_cursor::doctor()
}
