use super::*;
use edda_conductor::agent::launcher::ClaudeCodeLauncher;

#[test]
fn claude_measures_real_child_and_resets_before_a_failed_spawn() {
    let _store = crate::test_support::isolated_store();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let bin = {
            let bin = dir.path().join("claude.cmd");
            std::fs::write(&bin, "@echo off\r\necho {\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}\r\n").unwrap();
            bin
        };
        #[cfg(unix)]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let bin = dir.path().join("claude.sh");
            std::fs::write(&bin, "#!/bin/sh\nsleep 0.1\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}'\n").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            bin
        };
        let mut launcher = ClaudeCodeLauncher::with_bin(bin);
        let phase = build_phase("test", None, None, "default", CapabilityOptions::default());
        let out = run_with_launcher(&launcher, &phase, "test", dir.path(), CancellationToken::new()).await.unwrap();
        assert_eq!(out.outcome, Outcome::Done);
        assert!(out.elapsed_ms.is_some());
        launcher.claude_bin = dir.path().join("gone");
        let out = run_with_launcher(&launcher, &phase, "test", dir.path(), CancellationToken::new()).await.unwrap();
        assert_eq!(out.outcome, Outcome::Crash);
        assert_eq!(out.elapsed_ms, None);
    });
}

#[test]
fn failed_spawn_is_unmeasured_and_does_not_invent_zero() {
    let _store = crate::test_support::isolated_store();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let launcher = ClaudeCodeLauncher::with_bin(dir.path().join("missing-backend"));
        let phase = build_phase("test", None, None, "default", CapabilityOptions::default());
        let out = run_with_launcher(
            &launcher,
            &phase,
            "test",
            dir.path(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.to_json()).unwrap();
        assert_eq!(value["outcome"], "crash");
        assert_eq!(value["elapsed_measured"], false);
        assert!(value.get("elapsed_ms").unwrap().is_null());
        assert!(out.render_text().contains("Elapsed: —"));
    });
}
