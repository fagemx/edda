//! Fixed argv process runner with a shared deadline and bounded output memory.
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) struct Output {
    pub exit: i32,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub truncated: bool,
}

/// Resolve before changing cwd. In particular Windows must never search the
/// reviewed worktree for sh.exe, git.exe or another host executable.
pub(super) fn executable(name: &str) -> Result<PathBuf> {
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if !dir.is_absolute() {
            continue;
        }
        #[cfg(windows)]
        let names = [format!("{name}.exe"), name.to_owned()];
        #[cfg(not(windows))]
        let names = [name.to_owned()];
        for candidate in names {
            let path = dir.join(candidate);
            if path.is_file() {
                return path.canonicalize().context("resolve host executable");
            }
        }
    }
    anyhow::bail!("host executable `{name}` not found in absolute PATH entries")
}

pub(super) fn shell(script: &str, cwd: &Path, deadline: Instant) -> Result<Output> {
    let mut command = Command::new(executable("sh")?);
    command.args(["-c", script]);
    run(&mut command, cwd, deadline, 4000)
}

pub(super) fn run(
    command: &mut Command,
    cwd: &Path,
    deadline: Instant,
    limit: usize,
) -> Result<Output> {
    if Instant::now() >= deadline {
        anyhow::bail!("execution deadline exhausted");
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().context("spawn evidence command")?;
    let mut stdout = child.stdout.take().context("capture evidence stdout")?;
    let capture = Arc::new(Mutex::new((VecDeque::with_capacity(limit), false, false)));
    let reader_capture = Arc::clone(&capture);
    std::thread::spawn(move || {
        let mut block = [0; 8192];
        loop {
            match stdout.read(&mut block) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    if let Ok(mut state) = reader_capture.lock() {
                        for byte in &block[..size] {
                            if state.0.len() == limit {
                                state.0.pop_front();
                                state.1 = true;
                            }
                            state.0.push_back(*byte);
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        if let Ok(mut state) = reader_capture.lock() {
            state.2 = true;
        }
    });
    let mut status = None;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(exit)) => status = Some(exit),
            Ok(None) => {}
            Err(error) => {
                kill_tree(&mut child);
                return Err(error).context("wait for evidence command");
            }
        }
        let drained = capture.lock().map(|state| state.2).unwrap_or(false);
        if status.is_some() && drained {
            break false;
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            break true;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (stdout, truncated) = {
        let state = capture
            .lock()
            .map_err(|_| anyhow::anyhow!("stdout capture poisoned"))?;
        (state.0.iter().copied().collect(), state.1)
    };
    Ok(Output {
        exit: if timed_out {
            -1
        } else {
            status.and_then(|s| s.code()).unwrap_or(-1)
        },
        timed_out,
        stdout,
        truncated,
    })
}

fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(unix)]
    {
        if let Ok(kill) = executable("kill") {
            bounded_terminate(Command::new(kill).args(["-9", "--", &format!("-{pid}")]));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(windows) = std::env::var_os("SystemRoot") {
            bounded_terminate(
                Command::new(PathBuf::from(windows).join("System32/taskkill.exe"))
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x08000000),
            );
        }
    }
    let _ = child.kill();
    // Never turn teardown into an unbounded wait after the execution deadline.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn bounded_terminate(command: &mut Command) {
    let Ok(mut terminator) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match terminator.try_wait() {
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(Some(_)) => break,
            _ => {
                let _ = terminator.kill();
                let _ = terminator.try_wait();
                break;
            }
        }
    }
}
