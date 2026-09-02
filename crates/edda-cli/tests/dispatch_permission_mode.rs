//! Cross-process honesty checks for `edda dispatch` permission contracts
//! (GH-574, round 2 — P1-2; round 3 adds the --list-models path).
//!
//! The refusal of an unusable `--permission-mode` value must happen before
//! anything is spawned, and its message only exists on the real stderr of a
//! real process, so these behaviors are asserted through the actual binary:
//! the backend bin env var points at a path that cannot exist, so a pre-fix
//! binary fails at the backend call (a different message) instead of
//! spawning it, while a post-fix binary refuses the combination explicitly.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn explicit_permission_mode_on_codex_is_refused_not_silently_dropped() {
    let root = tempfile::tempdir().expect("test root");
    let prompt = root.path().join("prompt.txt");
    std::fs::write(&prompt, "do the thing").expect("prompt written");

    let edda_bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
    let output = Command::new(&edda_bin)
        .args([
            "dispatch",
            "--agent",
            "codex",
            "--permission-mode",
            "bypassPermissions",
            "--prompt-file",
        ])
        .arg(&prompt)
        .env("EDDA_CODEX_BIN", root.path().join("no-such-codex"))
        .env("EDDA_STORE_ROOT", root.path().join("store"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("edda binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "an unusable permission contract must not exit 0"
    );
    assert!(
        stderr.contains("--permission-mode") && stderr.contains("does not support"),
        "stderr must explicitly refuse --permission-mode for codex, got:\n{stderr}"
    );
}

/// The reviewer's round-3 repro, cross-process: the --list-models
/// short-circuit must not bypass the capability gate. `EDDA_PI_BIN` points
/// at a path that cannot exist, so a pre-fix binary fails inside the model
/// query (a different message) instead of spawning pi, while a post-fix
/// binary refuses the combination explicitly before any query.
#[test]
fn list_models_permission_mode_on_pi_is_refused_not_silently_dropped() {
    let root = tempfile::tempdir().expect("test root");

    let edda_bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
    let output = Command::new(&edda_bin)
        .args([
            "dispatch",
            "--agent",
            "pi",
            "--list-models",
            "definitely-no-such-model-8f3a",
            "--permission-mode",
            "bypassPermissions",
        ])
        .env("EDDA_PI_BIN", root.path().join("no-such-pi"))
        .env("EDDA_STORE_ROOT", root.path().join("store"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("edda binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "an unusable permission contract must not exit 0"
    );
    assert!(
        stderr.contains("--permission-mode") && stderr.contains("does not support"),
        "stderr must explicitly refuse --permission-mode for pi in listing mode, got:\n{stderr}"
    );
}
