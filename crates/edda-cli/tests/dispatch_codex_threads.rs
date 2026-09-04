//! Cross-process codex thread resume (GH-535, round 1 review P1-3).
//!
//! The in-crate launcher tests build two launchers inside one test process,
//! which cannot prove the separate-process doneWhen. This test spawns two
//! real `edda dispatch` OS processes — the actual binary, through
//! `run_inner`/`build_launcher` and the default `edda_store::store_root()`
//! resolution (isolated to a temp store via `EDDA_STORE_ROOT`) — against a
//! scripted fake app-server. The first process records
//! sess→thread via `thread/start`; the second must send `thread/resume`
//! (the fake answers "resumed answer" only on that path), proving two
//! separate processes share one conversation.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Write a fake `codex` executable: a wrapper tolerating the launcher's
/// `app-server` argument, feeding a scripted JSON-RPC app-server. First
/// thread request decides the path: `thread/resume` answers t-2 with
/// "resumed answer"; anything else (thread/start) answers t-1 with
/// "turn complete". Request ids follow the client's protocol: initialize=1,
/// open=2, turn/start=3.
fn write_fake_codex_bin(dir: &Path) -> Result<PathBuf, std::io::Error> {
    let body = if cfg!(windows) {
        r#"# The client sends the `initialized` notification after the initialize
# response; consume it so $req below is the first thread request.
Read-Line
$req = [Console]::In.ReadLine()
if ($null -eq $req) { exit 0 }
if ($req -match 'thread/resume') {
  Write-Line '{"id":2,"result":{"thread":{"id":"t-2"}}}'
  Read-Line
  Write-Line '{"id":3,"result":{"turn":{"id":"turn-2"}}}'
  Write-Line '{"method":"item/completed","params":{"threadId":"t-2","turnId":"turn-2","item":{"type":"agentMessage","text":"resumed answer"}}}'
  Write-Line '{"method":"turn/completed","params":{"threadId":"t-2","turn":{"id":"turn-2","status":"completed"}}}'
} else {
  Write-Line '{"id":2,"result":{"thread":{"id":"t-1"}}}'
  Read-Line
  Write-Line '{"id":3,"result":{"turn":{"id":"turn-1"}}}'
  Write-Line '{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"type":"agentMessage","text":"turn complete"}}}'
  Write-Line '{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed"}}}'
}
# A short tail sleep only: the responses are already buffered in the pipe
# when the fake exits, and a long-lived grandchild would hold the inherited
# stdout open and stall `Command::output()` past edda's own exit.
Start-Sleep -Seconds 5"#
    } else {
        r#"# The client sends the `initialized` notification after the initialize
# response; consume it so $req below is the first thread request.
read_line
IFS= read -r req || exit 0
case "$req" in *'"thread/resume"'*)
  write_line '{"id":2,"result":{"thread":{"id":"t-2"}}}'
  read_line
  write_line '{"id":3,"result":{"turn":{"id":"turn-2"}}}'
  write_line '{"method":"item/completed","params":{"threadId":"t-2","turnId":"turn-2","item":{"type":"agentMessage","text":"resumed answer"}}}'
  write_line '{"method":"turn/completed","params":{"threadId":"t-2","turn":{"id":"turn-2","status":"completed"}}}'
  ;;
*)
  write_line '{"id":2,"result":{"thread":{"id":"t-1"}}}'
  read_line
  write_line '{"id":3,"result":{"turn":{"id":"turn-1"}}}'
  write_line '{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"type":"agentMessage","text":"turn complete"}}}'
  write_line '{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed"}}}'
  ;;
esac
# A short tail sleep only: the responses are already buffered in the pipe
# when the fake exits, and a long-lived grandchild would hold the inherited
# stdout open and stall `Command::output()` past edda's own exit.
sleep 5"#
    };

    if cfg!(windows) {
        let script = dir.join("fake-app-server.ps1");
        std::fs::write(
            &script,
            format!(
                "$ErrorActionPreference = 'Stop'\nfunction Read-Line {{ if ($null -eq [Console]::In.ReadLine()) {{ exit 0 }} }}\nfunction Write-Line([string]$line) {{ [Console]::Out.WriteLine($line); [Console]::Out.Flush() }}\nRead-Line\nWrite-Line '{{\"id\":1,\"result\":{{}}}}'\n{body}\n"
            ),
        )?;
        let wrapper = dir.join("fake-codex.cmd");
        std::fs::write(
            &wrapper,
            format!(
                "@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" %*\r\n",
                script.display()
            ),
        )?;
        Ok(wrapper)
    } else {
        #[cfg(unix)]
        {
            let script = dir.join("fake-app-server.sh");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nread_line() {{ IFS= read -r _ || exit 0; }}\nwrite_line() {{ printf '%s\\n' \"$1\"; }}\nread_line\nwrite_line '{{\"id\":1,\"result\":{{}}}}'\n{body}\n"
                ),
            )?;
            let wrapper = dir.join("fake-codex.sh");
            std::fs::write(
                &wrapper,
                format!("#!/bin/sh\nexec /bin/sh '{}' \"$@\"\n", script.display()),
            )?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
            Ok(wrapper)
        }
        #[cfg(not(unix))]
        {
            unreachable!("non-Windows, non-unix platform")
        }
    }
}

fn run_dispatch(
    edda_bin: &Path,
    codex_bin: &Path,
    store_root: &Path,
    cwd: &Path,
    prompt_file: &Path,
) -> Result<String, String> {
    let output = Command::new(edda_bin)
        .args([
            "dispatch",
            "--agent",
            "codex",
            "--json",
            "--session-id",
            "sess-cross-proc",
            "--prompt-file",
        ])
        .arg(prompt_file)
        .arg("--cwd")
        .arg(cwd)
        .env("EDDA_STORE_ROOT", store_root)
        .env("EDDA_CODEX_BIN", codex_bin)
        // The fake codex must also survive `verify_available` (`codex
        // --version`), whose child inherits this stdin; keep it detached so
        // the fake's readline sees EOF instead of the test console.
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "edda dispatch failed ({}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        ));
    }
    let receipt: serde_json::Value = serde_json::from_str(&stdout).expect("dispatch JSON");
    assert_eq!(receipt["elapsed_measured"], true);
    assert!(receipt["elapsed_ms"].as_u64().unwrap() > 0);
    Ok(stdout)
}

#[test]
fn two_dispatch_processes_share_one_codex_conversation() {
    let root = tempfile::tempdir().expect("test root");
    let store_root = root.path().join("store");
    let cwd = root.path().join("repo");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let prompt_file = root.path().join("prompt.txt");
    std::fs::write(&prompt_file, "do the thing").expect("prompt");

    let fake_dir = tempfile::tempdir().expect("fake dir");
    let codex_bin = write_fake_codex_bin(fake_dir.path()).expect("fake codex written");

    let edda_bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));

    // Process 1: no persisted map, so this is a plain thread/start.
    let first = run_dispatch(&edda_bin, &codex_bin, &store_root, &cwd, &prompt_file)
        .expect("first dispatch process runs");
    assert!(
        first.contains("turn complete"),
        "first dispatch should complete via thread/start, got:\n{first}"
    );

    // The first process recorded the binding in the shared per-user store.
    let map = store_root
        .join("projects")
        .join(edda_store::project_id(&cwd))
        .join("state")
        .join("codex-threads.json");
    let persisted = std::fs::read_to_string(&map).expect("map persisted by first process");
    assert!(
        persisted.contains(r#""sess-cross-proc":"t-1""#),
        "map should record sess→t-1, got: {persisted}"
    );

    // Process 2: a genuinely separate OS process must load the map and
    // resume — the fake only produces "resumed answer" on thread/resume.
    let second = run_dispatch(&edda_bin, &codex_bin, &store_root, &cwd, &prompt_file)
        .expect("second dispatch process runs");
    assert!(
        second.contains("resumed answer"),
        "second dispatch process should resume the thread the first one recorded, got:\n{second}"
    );
}
