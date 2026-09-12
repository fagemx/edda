//! Durable process launch for `dispatch --detach` (GH-605).
use crate::cmd_dispatch::DispatchArgs;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachedOutput {
    pub handle: String,
    /// Machine JSON stdout only.
    pub log: PathBuf,
    /// Diagnostics only; never parsed as the dispatch receipt.
    pub error_log: PathBuf,
    pub manifest: PathBuf,
    pub task: Option<String>,
}

pub fn launch(args: &DispatchArgs, cwd: &Path, session: &str) -> Result<DetachedOutput> {
    let handle = format!("dispatch-{}", ulid::Ulid::new());
    launch_new(args, cwd, session, &handle, None)
}

#[derive(Debug, Clone)]
pub enum IdempotentLaunch {
    Launched(DetachedOutput),
    Adopted(DetachedOutput),
    Failed(DetachedOutput),
    NeedsDecision(DetachedOutput),
}

/// Inspect an existing default-root control manifest without requiring live
/// task, lease, or worktree state and without ever launching a process.
pub fn inspect_control_action(
    cwd: &Path,
    session: &str,
    action_id: &str,
) -> Result<Option<IdempotentLaunch>> {
    anyhow::ensure!(
        action_id.starts_with("action_") && action_id.len() == 71,
        "controlled dispatch action ID is malformed"
    );
    let handle = format!("dispatch-{}", &action_id[7..33]);
    let root = std::env::temp_dir().join("edda-dispatch");
    fs::create_dir_all(&root)?;
    let manifest = fs::canonicalize(root)?.join(format!("{handle}.json"));
    if !manifest.exists() {
        return Ok(None);
    }
    adopt_manifest(&manifest, &handle, action_id, cwd, session).map(Some)
}

/// Launch once for a deterministic control action. Matching running or
/// completed state is adopted; `launching` is surfaced as ambiguous.
pub fn launch_idempotent(
    args: &DispatchArgs,
    cwd: &Path,
    session: &str,
    action_id: &str,
) -> Result<IdempotentLaunch> {
    anyhow::ensure!(
        action_id.starts_with("action_") && action_id.len() == 71,
        "controlled dispatch action ID is malformed"
    );
    let handle = format!("dispatch-{}", &action_id[7..33]);
    let root = dispatch_root(args)?;
    let manifest = root.join(format!("{handle}.json"));
    if manifest.exists() {
        return adopt_manifest(&manifest, &handle, action_id, cwd, session);
    }
    match launch_new(args, cwd, session, &handle, Some(action_id)) {
        Ok(output) => await_control_start(output),
        Err(_error) if manifest.exists() => {
            adopt_manifest(&manifest, &handle, action_id, cwd, session)
        }
        Err(error) => Err(error),
    }
}

fn dispatch_root(args: &DispatchArgs) -> Result<PathBuf> {
    let root = args
        .detach_log_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("edda-dispatch"));
    fs::create_dir_all(&root)?;
    Ok(fs::canonicalize(root)?)
}

fn record_launch_failure(manifest: &Path, error: &anyhow::Error) -> Result<()> {
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(manifest)?)?;
    let mut message = format!("{error:#}");
    if message.len() > 1_000 {
        let mut end = 1_000;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    value["state"] = "failed".into();
    value["error"] = message.into();
    let temporary = manifest.with_extension("failed.new");
    fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
    fs::rename(temporary, manifest)?;
    Ok(())
}

fn adopt_manifest(
    manifest: &Path,
    handle: &str,
    action_id: &str,
    cwd: &Path,
    session: &str,
) -> Result<IdempotentLaunch> {
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest)?)
        .context("controlled dispatch manifest is malformed")?;
    let expected_cwd = match dispatch_cwd(cwd) {
        Ok(cwd) => cwd,
        Err(_) if cwd.is_absolute() => cwd.to_path_buf(),
        Err(error) => return Err(error),
    };
    anyhow::ensure!(
        value["version"].as_u64() == Some(1)
            && value["handle"].as_str() == Some(handle)
            && value["action_id"].as_str() == Some(action_id)
            && value["session_id"].as_str() == Some(session)
            && value["cwd"].as_str() == expected_cwd.to_str(),
        "deterministic dispatch handle belongs to a different request"
    );
    let output = DetachedOutput {
        handle: handle.to_string(),
        log: value["log"]
            .as_str()
            .map(PathBuf::from)
            .context("dispatch manifest omits log")?,
        error_log: value["error_log"]
            .as_str()
            .map(PathBuf::from)
            .context("dispatch manifest omits error_log")?,
        manifest: manifest.to_path_buf(),
        task: value["task"].as_str().map(str::to_owned),
    };
    Ok(match value["state"].as_str() {
        Some("running") => IdempotentLaunch::Adopted(output),
        Some("completed") if value["exit_code"].as_i64() == Some(0) => {
            IdempotentLaunch::Adopted(output)
        }
        Some("completed" | "failed" | "timeout") => IdempotentLaunch::Failed(output),
        Some("launching") => IdempotentLaunch::NeedsDecision(output),
        _ => IdempotentLaunch::NeedsDecision(output),
    })
}

fn await_control_start(output: DetachedOutput) -> Result<IdempotentLaunch> {
    for _ in 0..500 {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&output.manifest)?)
            .context("controlled dispatch manifest is malformed during start handshake")?;
        match value["state"].as_str() {
            Some("running") => return Ok(IdempotentLaunch::Launched(output)),
            Some("completed") if value["exit_code"].as_i64() == Some(0) => {
                return Ok(IdempotentLaunch::Launched(output));
            }
            Some("completed" | "failed" | "timeout") => {
                return Ok(IdempotentLaunch::Failed(output));
            }
            Some("launching") => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => return Ok(IdempotentLaunch::NeedsDecision(output)),
        }
    }
    Ok(IdempotentLaunch::NeedsDecision(output))
}

fn launch_new(
    args: &DispatchArgs,
    cwd: &Path,
    session: &str,
    handle: &str,
    action_id: Option<&str>,
) -> Result<DetachedOutput> {
    let lane = args.build_lane.as_deref();
    if let Some(name) = lane {
        if !["worker-1", "worker-2", "verifier", "verifier-2"].contains(&name) {
            bail!("--build-lane must be worker-1, worker-2, verifier, or verifier-2");
        }
    }
    let root = dispatch_root(args)?;
    let log = root.join(format!("{handle}.log"));
    let error_log = root.join(format!("{handle}.log.err"));
    let supervisor_log = root.join(format!("{handle}.supervisor.log"));
    let manifest = root.join(format!("{handle}.json"));
    let cargo = lane.map(|name| lane_root().join(name));
    let prompt = root.join(format!("{handle}.prompt.txt"));
    if let Some(source) = args.prompt_file.as_ref() {
        fs::copy(source, &prompt)?;
    } else if !args.agent.is_acp() {
        anyhow::bail!("missing prompt file");
    }
    let cwd = dispatch_cwd(cwd)?;
    let argv = foreground_argv(args, &cwd, &prompt, session);
    let task = cfg!(windows).then(|| format!("edda-{handle}"));
    let receipt = DetachedOutput {
        handle: handle.to_string(),
        log,
        error_log,
        manifest,
        task,
    };
    let value = serde_json::json!({
        "version": 1, "handle": receipt.handle, "action_id": action_id,
        "session_id": session, "controller_pid": std::process::id(), "cwd": cwd,
        "log": receipt.log, "error_log": receipt.error_log,
        "supervisor_log": supervisor_log, "task": receipt.task, "state": "launching",
        "cargo_target_dir": cargo, "worker_pid": null, "exit_code": null, "error": null,
    });
    let bytes = serde_json::to_vec_pretty(&value)?;
    if action_id.is_some() {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&receipt.manifest)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    } else {
        fs::write(&receipt.manifest, bytes)?;
    }
    #[cfg(windows)]
    let launched = launch_windows(
        &receipt,
        &argv,
        &cwd,
        session,
        &args.owns,
        cargo.as_deref(),
        args.timeout_sec.unwrap_or(1800),
    );
    #[cfg(not(windows))]
    let launched = launch_unix(&receipt, &argv, &cwd, cargo.as_deref());
    if let Err(error) = launched {
        record_launch_failure(&receipt.manifest, &error)?;
        return Err(error);
    }
    Ok(receipt)
}

fn dispatch_cwd(cwd: &Path) -> Result<PathBuf> {
    let cwd = fs::canonicalize(cwd)?;
    #[cfg(windows)]
    let cwd = match cwd.to_str().and_then(|path| path.strip_prefix("\\\\?\\")) {
        Some(path) => PathBuf::from(path),
        None => cwd,
    };
    Ok(cwd)
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
    ];
    if !args.agent.is_acp() {
        out.extend(["--session-id".into(), session.into()]);
    }
    if args.prompt_file.is_some() {
        out.extend(["--prompt-file".into(), prompt.into()]);
    }
    for (flag, value) in [
        ("--task-id", args.task_id.map(|value| value.to_string())),
        ("--brief-event-id", args.brief_event_id.clone()),
        ("--brief-digest", args.brief_digest.clone()),
    ] {
        if let Some(value) = value {
            out.extend([flag.into(), value.into()]);
        }
    }
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
    let stderr = fs::File::create(&receipt.error_log)?;
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
    fs::write(&helper, include_str!("../resources/dispatch-task.ps1"))?;
    let config_value = serde_json::json!({
        "manifest": receipt.manifest, "log": receipt.log, "error_log": receipt.error_log,
        "supervisor_log": receipt.log.with_extension("supervisor.log"), "task": receipt.task,
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
    fn deterministic_manifest_adopts_only_an_exact_request_and_never_respawns_launching() {
        let dir = tempfile::tempdir().expect("manifest directory");
        let cwd = dispatch_cwd(dir.path()).unwrap();
        let action = format!("action_{}", "a".repeat(64));
        let handle = format!("dispatch-{}", &action[7..33]);
        let manifest = dir.path().join(format!("{handle}.json"));
        let log = dir.path().join(format!("{handle}.log"));
        let value = serde_json::json!({
            "version": 1,
            "handle": handle,
            "action_id": action,
            "session_id": "session-1",
            "cwd": cwd,
            "log": log,
            "error_log": dir.path().join(format!("{handle}.log.err")),
            "task": null,
            "state": "running"
        });
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            adopt_manifest(&manifest, &handle, &action, dir.path(), "session-1").unwrap(),
            IdempotentLaunch::Adopted(_)
        ));

        let mut launching = value;
        launching["state"] = "launching".into();
        fs::write(&manifest, serde_json::to_vec(&launching).unwrap()).unwrap();
        assert!(matches!(
            adopt_manifest(&manifest, &handle, &action, dir.path(), "session-1").unwrap(),
            IdempotentLaunch::NeedsDecision(_)
        ));
        launching["state"] = "failed".into();
        fs::write(&manifest, serde_json::to_vec(&launching).unwrap()).unwrap();
        assert!(matches!(
            adopt_manifest(&manifest, &handle, &action, dir.path(), "session-1").unwrap(),
            IdempotentLaunch::Failed(_)
        ));
        launching["state"] = "completed".into();
        launching["exit_code"] = 1.into();
        fs::write(&manifest, serde_json::to_vec(&launching).unwrap()).unwrap();
        assert!(matches!(
            adopt_manifest(&manifest, &handle, &action, dir.path(), "session-1").unwrap(),
            IdempotentLaunch::Failed(_)
        ));
        assert!(adopt_manifest(&manifest, &handle, &action, dir.path(), "other-session").is_err());
    }

    #[test]
    fn acp_supervisor_filters_parent_only_and_unsupported_child_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let args = DispatchArgs {
            owns: vec!["src/**".into()],
            detach: true,
            build_lane: None,
            detach_log_dir: None,
            agent: crate::agent_kind::AgentKind::AcpPi,
            task_id: Some(7),
            brief_event_id: Some("evt_brief".into()),
            brief_digest: Some("a".repeat(64)),
            prompt_file: None,
            session_id: Some("parent-session".into()),
            resume: false,
            cwd: None,
            budget_usd: None,
            timeout_sec: None,
            permission_mode: None,
            model: None,
            thinking: None,
            tools: None,
            exclude_tools: None,
            session_dir: None,
            list_models: None,
            issue: None,
            machine: None,
            json: true,
        };
        let argv = foreground_argv(
            &args,
            dir.path(),
            &dir.path().join("unused.prompt"),
            "supervisor-session",
        );
        let text = argv
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        for forbidden in [
            "--detach",
            "--session-id",
            "--resume",
            "--prompt-file",
            "--budget-usd",
            "--model",
            "--thinking",
            "--tools",
        ] {
            assert!(
                !text.iter().any(|arg| arg == forbidden),
                "leaked {forbidden}"
            );
        }
        for required in ["--task-id", "--brief-event-id", "--brief-digest"] {
            assert!(text.iter().any(|arg| arg == required), "missing {required}");
        }
    }

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
