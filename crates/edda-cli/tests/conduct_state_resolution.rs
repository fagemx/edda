//! GH-557: the recovery verbs must resolve the plan state that `conduct run`
//! uses, even when it lives in a git worktree. This exercises the real
//! binary so the resolution authority is tested end to end.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git should be runnable");
    assert!(
        out.status.success(),
        "git {:?} in {} failed: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Fabricate a blocked plan whose stale phase lives in `store`, as a plan
/// launched from that cwd would have left it.
fn write_state(store: &Path, plan: &str) {
    let dir = store.join(".edda").join("conductor").join(plan);
    std::fs::create_dir_all(&dir).unwrap();
    let state = format!(
        r#"{{
  "plan_name": "{plan}",
  "plan_file": "plan.yaml",
  "plan_status": "blocked",
  "total_cost_usd": 0.0,
  "cost_measured": false,
  "phases": [
    {{"id": "p1", "status": "stale", "attempts": 1, "checks": [], "gate_redispatches": 0}}
  ],
  "version": 1
}}"#
    );
    std::fs::write(dir.join("state.json"), state).unwrap();
}

fn run_edda(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_edda"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("edda should be runnable")
}

#[test]
fn recovery_verbs_see_a_worktree_launched_plan_from_the_main_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["config", "user.email", "t@example.invalid"]);
    git(&main, &["config", "user.name", "Test"]);
    std::fs::write(main.join("README"), "seed\n").unwrap();
    git(&main, &["add", "README"]);
    git(&main, &["commit", "-qm", "seed"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wtbranch",
        ],
    );

    write_state(&wt, "wave-b");

    // status (no --plan) from the main repo lists the worktree-launched plan.
    let out = run_edda(&main, &["conduct", "status"]);
    assert!(
        out.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("wave-b"),
        "status must list the worktree plan, got: {stdout}"
    );

    // retry from the main repo acts on that same state.
    let out = run_edda(&main, &["conduct", "retry", "p1", "--plan", "wave-b"]);
    assert!(
        out.status.success(),
        "retry failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("reset to Pending"),
        "retry must report the reset, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(wt.join(".edda/conductor/wave-b/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["phases"][0]["status"], "pending");

    // skip and abort resolve the same store too.
    let out = run_edda(
        &main,
        &[
            "conduct", "skip", "p1", "--plan", "wave-b", "--reason", "test",
        ],
    );
    assert!(
        out.status.success(),
        "skip failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run_edda(&main, &["conduct", "abort", "wave-b"]);
    assert!(
        out.status.success(),
        "abort failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(wt.join(".edda/conductor/wave-b/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["plan_status"], "aborted");
}
