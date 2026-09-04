//! Durable process launch for `dispatch --detach` (GH-605).
use crate::cmd_dispatch::DispatchArgs;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct DetachedOutput {
    pub handle: String,
    pub log: PathBuf,
    pub manifest: PathBuf,
    pub task: Option<String>,
}

pub fn launch(args: &DispatchArgs, cwd: &Path, session: &str) -> Result<DetachedOutput> {
    let lane = args.build_lane.as_deref();
    if let Some(name) = lane {
        if !["worker-1", "worker-2", "verifier", "verifier-2"].contains(&name) {
            bail!("--build-lane must be worker-1, worker-2, verifier, or verifier-2");
        }
    }
    let root = args
        .detach_log_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("edda-dispatch"));
    fs::create_dir_all(&root)?;
    let root = fs::canonicalize(root)?;
    let handle = format!("dispatch-{}", ulid::Ulid::new());
    let log = root.join(format!("{handle}.log"));
    let manifest = root.join(format!("{handle}.json"));
    let cargo = lane.map(|name| lane_root().join(name));
    // Snapshot the prompt so a restarted controller cannot remove or replace
    // the request before the scheduled worker reads it. All paths are absolute.
    let prompt = root.join(format!("{handle}.prompt.txt"));
    fs::copy(
        args.prompt_file.as_ref().context("missing prompt file")?,
        &prompt,
    )?;
    let cwd = fs::canonicalize(cwd)?;
    // Win32's extended-path prefix is useful to filesystem APIs but is not
    // preserved by a Scheduled Task's child current directory. Keep the
    // durable config and worker argv on the ordinary spelling so cleanup
    // commands resolve the same coordination project.
    #[cfg(windows)]
    let cwd = match cwd.to_str().and_then(|path| path.strip_prefix("\\\\?\\")) {
        Some(path) => PathBuf::from(path),
        None => cwd,
    };
    let argv = foreground_argv(args, &cwd, &prompt, session);
    let task = cfg!(windows).then(|| format!("edda-{handle}"));
    let receipt = DetachedOutput {
        handle,
        log,
        manifest,
        task,
    };
    let value = serde_json::json!({
        "version": 1, "handle": receipt.handle, "controller_pid": std::process::id(),
        "cwd": cwd, "log": receipt.log, "task": receipt.task, "state": "launching",
        "cargo_target_dir": cargo, "worker_pid": null, "exit_code": null, "error": null,
    });
    fs::write(&receipt.manifest, serde_json::to_vec_pretty(&value)?)?;
    #[cfg(windows)]
    launch_windows(
        &receipt,
        &argv,
        &cwd,
        session,
        &args.owns,
        cargo.as_deref(),
        args.timeout_sec.unwrap_or(1800),
    )?;
    #[cfg(not(windows))]
    launch_unix(&receipt, &argv, &cwd, cargo.as_deref())?;
    Ok(receipt)
}

fn lane_root() -> PathBuf {
    if let Some(root) = std::env::var_os("FLEET_LANE_ROOT") {
        return root.into();
    }
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(windows))]
    let root = std::env::temp_dir();
    root.join("fleet-workstation").join("lanes")
}

fn foreground_argv(args: &DispatchArgs, cwd: &Path, prompt: &Path, session: &str) -> Vec<OsString> {
    let mut out = vec![
        "dispatch".into(),
        "--agent".into(),
        args.agent.as_str().into(),
        "--cwd".into(),
        cwd.into(),
        "--prompt-file".into(),
        prompt.into(),
        "--session-id".into(),
        session.into(),
    ];
    for (flag, value) in [
        ("--model", args.model.clone()),
        ("--thinking", args.thinking.clone()),
        ("--permission-mode", args.permission_mode.clone()),
        ("--session-dir", args.session_dir.clone()),
        ("--machine", args.machine.clone()),
        ("--issue", args.issue.map(|v| v.to_string())),
        ("--budget-usd", args.budget_usd.map(|v| v.to_string())),
        ("--timeout-sec", args.timeout_sec.map(|v| v.to_string())),
        ("--tools", args.tools.as_ref().map(|v| v.join(","))),
        (
            "--exclude-tools",
            args.exclude_tools.as_ref().map(|v| v.join(",")),
        ),
    ] {
        if let Some(value) = value {
            out.extend([flag.into(), value.into()]);
        }
    }
    if !args.owns.is_empty() {
        out.extend(["--owns".into(), args.owns.join(",").into()]);
    }
    if args.resume {
        out.push("--resume".into());
    }
    if args.json {
        out.push("--json".into());
    }
    out
}

/// Task Scheduler starts with a service-owned environment.  Preserve only the
/// small, non-secret set a dispatch worker needs; notably `PATH` must be
/// obtained case-insensitively on Windows because PowerShell commonly exports
/// it as `Path`.
#[cfg(windows)]
fn scheduled_worker_environment() -> std::collections::BTreeMap<String, String> {
    const ALLOWED: &[&str] = &[
        "EDDA_STORE_ROOT",
        "EDDA_CLAUDE_BIN",
        "EDDA_PI_BIN",
        "EDDA_CODEX_BIN",
        "EDDA_MACHINE",
        "EDDA_LANE_HEARTBEAT_SECS",
        "PATH",
        "RUSTUP_HOME",
        "CARGO_HOME",
    ];
    std::env::vars()
        .filter(|(key, _)| {
            ALLOWED
                .iter()
                .any(|allowed| key.eq_ignore_ascii_case(allowed))
        })
        .map(|(key, value)| {
            let canonical = if key.eq_ignore_ascii_case("PATH") {
                "PATH".to_owned()
            } else {
                key
            };
            (canonical, value)
        })
        .collect()
}

#[cfg(not(windows))]
fn launch_unix(
    receipt: &DetachedOutput,
    argv: &[OsString],
    cwd: &Path,
    cargo: Option<&Path>,
) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let stdout = fs::File::create(&receipt.log)?;
    let stderr = stdout.try_clone()?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(argv)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    let home = edda_core::paths::home_dir().context("cannot resolve HOME for detached dispatch")?;
    command.env("HOME", home);
    if let Some(target) = cargo {
        command.env("CARGO_TARGET_DIR", target);
    } else {
        command.env_remove("CARGO_TARGET_DIR");
    }
    // This capability belongs to the one worker process. `run` consumes it
    // before launching an agent, so an agent or nested ordinary dispatch never
    // inherits authority over this parent's manifest.
    command.env("EDDA_DETACHED_MANIFEST", &receipt.manifest);
    command.spawn().context("spawn detached dispatch")?;
    Ok(())
}

#[cfg(windows)]
fn launch_windows(
    receipt: &DetachedOutput,
    argv: &[OsString],
    cwd: &Path,
    session: &str,
    owned_paths: &[String],
    cargo: Option<&Path>,
    timeout: u64,
) -> Result<()> {
    let executable = std::env::current_exe()?;
    let config = receipt.manifest.with_extension("launch.json");
    let helper = receipt.manifest.with_extension("task.ps1");
    fs::write(
        &helper,
        include_str!("../../../scripts/fleet/dispatch-task.ps1"),
    )?;
    let config_value = serde_json::json!({
        "manifest": receipt.manifest, "log": receipt.log, "task": receipt.task,
        "cwd": cwd, "executable": executable,
        "controller_pid": std::process::id(),
        "session": session, "owned_paths": owned_paths,
        "argv": argv.iter().map(|v| v.to_string_lossy()).collect::<Vec<_>>(),
        "cargo": cargo,
        "timeout": timeout.min(2_000_000),
        // Never persist an arbitrary EDDA_* environment: it may contain tokens.
        "environment": scheduled_worker_environment(),
        "home": std::env::var("USERPROFILE").context("USERPROFILE unavailable")?,
    });
    fs::write(&config, serde_json::to_vec_pretty(&config_value)?)?;
    let output = Command::new("where.exe").arg("pwsh.exe").output()?;
    if !output.status.success() {
        bail!("pwsh.exe is required for detached dispatch on Windows");
    }
    let paths = String::from_utf8(output.stdout)?;
    let pwsh = paths.lines().next().context("pwsh.exe not found")?;
    let status = Command::new(pwsh)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&helper)
        .args(["-Mode", "Launch", "-Config"])
        .arg(config)
        .status()?;
    if !status.success() {
        bail!("detached task registration failed ({status})");
    }
    Ok(())
}

/// Take the private manifest capability for this detached worker and remove
/// it from the process environment before any backend can inherit it.
pub fn take_worker_manifest() -> Option<PathBuf> {
    let path = std::env::var_os("EDDA_DETACHED_MANIFEST").map(PathBuf::from);
    std::env::remove_var("EDDA_DETACHED_MANIFEST");
    path
}

/// Unix workers own manifest writes; the launcher never races their completion.
pub fn update_worker_manifest(path: Option<&Path>, code: Option<i32>) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    value["worker_pid"] = std::process::id().into();
    value["state"] = if code.is_some() {
        "completed"
    } else {
        "running"
    }
    .into();
    value["exit_code"] = code.into();
    let temporary = path.with_extension("new");
    fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static MANIFEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn worker_manifest_capability_is_consumed_before_a_nested_dispatch() {
        let _guard = MANIFEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let previous = std::env::var_os("EDDA_DETACHED_MANIFEST");
        let dir = tempfile::tempdir().expect("manifest directory");
        let manifest = dir.path().join("parent.json");
        std::env::set_var("EDDA_DETACHED_MANIFEST", &manifest);

        let taken = take_worker_manifest().expect("worker receives manifest capability");
        assert_eq!(taken, manifest);
        assert!(
            std::env::var_os("EDDA_DETACHED_MANIFEST").is_none(),
            "a nested ordinary dispatch must not inherit the parent manifest"
        );

        match previous {
            Some(value) => std::env::set_var("EDDA_DETACHED_MANIFEST", value),
            None => std::env::remove_var("EDDA_DETACHED_MANIFEST"),
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::scheduled_worker_environment;

    #[test]
    fn scheduled_worker_environment_keeps_path_with_windows_casing() {
        let environment = scheduled_worker_environment();
        assert!(environment.contains_key("PATH"));
        assert!(environment.keys().all(|key| {
            [
                "EDDA_STORE_ROOT",
                "EDDA_CLAUDE_BIN",
                "EDDA_PI_BIN",
                "EDDA_CODEX_BIN",
                "EDDA_MACHINE",
                "EDDA_LANE_HEARTBEAT_SECS",
                "PATH",
                "RUSTUP_HOME",
                "CARGO_HOME",
            ]
            .contains(&key.as_str())
        }));
    }
}
