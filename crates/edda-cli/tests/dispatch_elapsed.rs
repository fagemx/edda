//! Real dispatch process against a deterministic pi RPC fixture (GH-644).
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(dir: &Path) -> PathBuf {
    let body = r#"read_cmd
write_line '{"id":"req-1","type":"response","command":"prompt","success":true}'
pause
write_line '{"type":"agent_settled"}'
read_cmd
write_line '{"id":"req-2","type":"response","command":"get_session_stats","success":true,"data":{"cost":0.42}}'
read_cmd
write_line '{"id":"req-3","type":"response","command":"get_state","success":true,"data":{"model":null}}'
"#;
    #[cfg(windows)]
    {
        let script = dir.join("pi.ps1");
        let body = body
            .replace("read_cmd", "Read-Line")
            .replace("write_line", "Write-Line")
            .replace("pause", "Start-Sleep -Milliseconds 100");
        std::fs::write(&script, format!("if ($args -contains '--version') {{ exit 0 }}\nfunction Read-Line {{ if ($null -eq [Console]::In.ReadLine()) {{ exit 0 }} }}\nfunction Write-Line([string]$line) {{ [Console]::Out.WriteLine($line); [Console]::Out.Flush() }}\n{body}")).unwrap();
        let wrapper = dir.join("pi.cmd");
        std::fs::write(&wrapper, format!("@echo off\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" %*\r\n", script.display())).unwrap();
        wrapper
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let wrapper = dir.join("pi.sh");
        std::fs::write(&wrapper, format!("#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nread_cmd() {{ IFS= read -r _ || exit 0; }}\nwrite_line() {{ printf '%s\\n' \"$1\"; }}\n{}", body.replace("pause", "sleep 0.1"))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        wrapper
    }
}

#[test]
fn real_dispatch_json_measures_spawn_to_exit() {
    let root = tempfile::tempdir().unwrap();
    let backend = fixture(root.path());
    let prompt = root.path().join("prompt.txt");
    std::fs::write(&prompt, "fixture").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_edda"))
        .args(["dispatch", "--agent", "pi", "--json", "--prompt-file"])
        .arg(prompt)
        .arg("--cwd")
        .arg(root.path())
        .env("EDDA_PI_BIN", backend)
        .env("EDDA_STORE_ROOT", root.path().join("store"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "done");
    assert_eq!(value["elapsed_measured"], true);
    assert!(value["elapsed_ms"].as_u64().unwrap() >= 100);
    assert_eq!(value["cost_usd"], 0.42);
}
