//! GH-1063: the SessionStart write-back protocol has exactly one source.
//!
//! Every session's first lesson is this text, and all five bridges (claude,
//! codex, cursor, hermes, openclaw) inject it by calling the one shared
//! `edda_bridge_claude::render::writeback()`. A sentence corrected in one
//! bridge while another keeps a copy is the drift this fixture exists to end,
//! so the test drives each bridge's real `SessionStart` entrypoint against the
//! same pack input and asserts the protocol comes out byte-identical.
//!
//! It lives in the `edda` package rather than beside `render::writeback()`
//! because the four sibling bridges depend on `edda-bridge-claude`: naming
//! them there, even as dev-dependencies, is a cycle that `cargo publish
//! --dry-run --workspace` cannot verify. `edda` already depends on all five,
//! so the same five entrypoints are reachable with no new manifest edge.

use std::path::Path;

/// The session-opening event of each runtime, over the same throwaway
/// project. The five hook protocols disagree on which event carries the
/// injection — Claude, Codex and Cursor take a `SessionStart` envelope,
/// Hermes injects on the first `pre_llm_call`, and OpenClaw on
/// `before_agent_start` reading `workspace_dir` — so each bridge is fed the
/// envelope it actually parses.
fn session_start_stdin(bridge: &str, cwd: &Path) -> String {
    let cwd = cwd.to_string_lossy().to_string();
    let (event, extra) = match bridge {
        "openclaw" => ("before_agent_start", serde_json::json!({})),
        "hermes" => ("pre_llm_call", serde_json::json!({ "is_first_turn": true })),
        _ => ("SessionStart", serde_json::json!({})),
    };
    // A distinct session id per bridge: the injections are hash-deduped per
    // (project, session), and five runtimes never share one session.
    let session_id = format!("gh1063-writeback-fixture-{bridge}");
    serde_json::json!({
        "hook_event_name": event,
        "session_id": session_id,
        "conversation_id": session_id,
        "cwd": cwd,
        "workspace_dir": cwd,
        "workspace_roots": [cwd],
        "extra": extra,
    })
    .to_string()
}

/// Bridge stdout is a JSON hook envelope, so the injected text appears
/// JSON-escaped inside it. Escape the needle the same way and the
/// `contains` check is a byte-for-byte comparison of the rendered protocol.
fn as_json_body(text: &str) -> String {
    let quoted = serde_json::to_string(text).expect("json-escape the protocol");
    quoted[1..quoted.len() - 1].to_string()
}

#[test]
fn five_bridges_render_byte_identical_writeback() {
    let _store = edda_store::test_support::isolated_store_root().expect("isolated store");
    let project = tempfile::tempdir().expect("temp project");
    let stdin = |bridge: &str| session_start_stdin(bridge, project.path());

    let expected = as_json_body(&edda_bridge_claude::render::writeback());

    let rendered: Vec<(&str, String)> = vec![
        (
            "claude",
            edda_bridge_claude::hook_entrypoint_from_stdin(&stdin("claude"))
                .expect("claude SessionStart")
                .stdout
                .expect("claude injects context"),
        ),
        (
            "codex",
            edda_bridge_codex::hook_entrypoint_from_stdin(&stdin("codex"))
                .expect("codex SessionStart")
                .stdout
                .expect("codex injects context"),
        ),
        (
            "cursor",
            edda_bridge_cursor::hook_entrypoint_from_stdin(&stdin("cursor"))
                .expect("cursor SessionStart")
                .stdout
                .expect("cursor injects context"),
        ),
        (
            "hermes",
            edda_bridge_hermes::hook_entrypoint_from_stdin(&stdin("hermes"))
                .expect("hermes first pre_llm_call")
                .stdout
                .expect("hermes injects context"),
        ),
        (
            "openclaw",
            edda_bridge_openclaw::hook_entrypoint_from_stdin(&stdin("openclaw"))
                .expect("openclaw before_agent_start")
                .stdout
                .expect("openclaw injects context"),
        ),
    ];

    for (bridge, stdout) in &rendered {
        assert!(
            stdout.contains(&expected),
            "{bridge} does not inject render::writeback() verbatim — the \
             write-back protocol has drifted from its single source"
        );
    }
}

#[test]
fn writeback_teaches_both_binding_paths() {
    let text = edda_bridge_claude::render::writeback();

    // The superseded constitution: the operator as the only path. Spelled in
    // two pieces on purpose — GH-1063's acceptance sweep greps `crates/` and
    // `tests/` for this phrase, and a guard that spells it out would be the
    // one remaining hit, i.e. would report the defect it exists to prevent.
    let superseded = concat!("Only the operator ", "confers binding authority");
    assert!(
        !text.contains(superseded),
        "the write-back protocol still teaches operator-only binding authority"
    );

    // Path 1 — the operator ratifies directly.
    assert!(
        text.contains("edda ratify"),
        "path 1: operator ratification"
    );
    // Path 2 — the cited-authority rule sweep (decision.auto-ratify; PR #1016),
    // which reads the structured citation, not the free-text reason.
    assert!(
        text.contains("edda ratify --by-rule cited-authority"),
        "path 2: the cited-authority rule sweep"
    );
    assert!(
        text.contains("--cite operator:"),
        "the sweep matches --cite values, so the protocol must teach the flag"
    );
    // Unratified is a stage with a route out, not a verdict.
    assert!(
        text.contains("not yet binding"),
        "unratified must read as recorded-and-pending, not as a verdict"
    );
    // Kept from the old text, plus the scribe/ratifier separation.
    assert!(text.contains("Do not ratify your own decisions"));
    assert!(text.contains("governance.scribe-pass"));
}
