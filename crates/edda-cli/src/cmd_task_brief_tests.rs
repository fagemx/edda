//! GH-793: `edda task start` renders the task brief block on stdout, using
//! the same `render_task_brief_block` the SessionStart hook uses.

use super::*;

fn temp_ws(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("edda_cmdtask_brief_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Ledger::ensure_initialized(&dir).unwrap();
    dir
}

fn brief_args<'a>(title: &'a str, scope_paths: &'a [String]) -> NewTaskArgs<'a> {
    NewTaskArgs {
        title,
        assignee: Some("tester"),
        agent_kind: None,
        after: &[],
        plan: None,
        work_unit: None,
        brief: Some("brief.md"),
        idempotency_key: None,
        scope_paths,
    }
}

/// (e) `task start` on a task with a `brief_ref` prints the block header and
/// the brief text.
#[test]
fn start_prints_task_brief_block_for_task_with_brief_ref() {
    let _store = crate::test_support::isolated_store();
    let ws = temp_ws("brief_start");
    std::fs::write(ws.join("brief.md"), "Fix the flange, then verify the weld.").unwrap();
    do_new(&ws, &brief_args("weld the flange", &[])).unwrap();

    let out = run_start(&ws, 1, 3600).unwrap();

    assert!(out.contains("[edda task #1]"), "stdout was:\n{out}");
    assert!(out.contains("weld the flange"), "stdout was:\n{out}");
    assert!(
        out.contains("Fix the flange, then verify the weld."),
        "stdout was:\n{out}"
    );
    assert!(out.contains("status: running"), "stdout was:\n{out}");
    let _ = std::fs::remove_dir_all(&ws);
}

/// (f) The block `task start` prints is byte-identical to the one the hook
/// path renders — both come from `render_task_brief_block`.
#[test]
fn start_block_matches_shared_renderer() {
    let _store = crate::test_support::isolated_store();
    let ws = temp_ws("brief_shared");
    std::fs::write(ws.join("brief.md"), "shared renderer body").unwrap();
    do_new(
        &ws,
        &brief_args("weld the flange", &["crates/edda-weld/**".to_string()]),
    )
    .unwrap();

    let out = run_start(&ws, 1, 3600).unwrap();
    let view = do_show(&ws, 1).unwrap();
    let expected = edda_ledger::tasks::render_task_brief_block(&view, Some(&ws));

    assert!(
        out.contains(&expected),
        "stdout must contain the shared-renderer block.\nstdout:\n{out}\nexpected block:\n{expected}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
