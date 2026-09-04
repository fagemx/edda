//! GH-757: the test-support store override must be inert for ordinary `edda`
//! subprocesses (feature-unification regression).
//!
//! Under `cargo test -p edda`, feature unification compiles the `edda` binary
//! with `edda-store`'s `test-support` feature enabled. The override is a
//! thread-local and can never cross a process boundary: a child must keep
//! resolving its store from its own environment (explicit `EDDA_STORE_ROOT`)
//! or the ordinary default, exactly as a production binary would.

use std::path::PathBuf;
use std::process::Command;

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

fn e2e_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("repo tempdir");
    std::fs::create_dir_all(repo.path().join(".edda")).expect("anchor .edda workspace");
    std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
    repo
}

#[test]
fn child_subprocess_keeps_ordinary_resolution_while_the_parent_holds_an_override() {
    // Parent installs an in-process test root A on this thread.
    let root_a = edda_store::test_support::isolated_store_root().expect("isolated store");
    let a_registry = root_a.path().join("registry.json");

    // 1. A child passed an explicit root B writes only to B: the env var
    //    remains the subprocess propagation channel (Command::env), and the
    //    parent's thread-local override must not leak into the child.
    let repo1 = e2e_repo();
    let root_b = tempfile::tempdir().unwrap();
    let out = Command::new(edda_bin())
        .args(["init", "--no-hooks"])
        .current_dir(repo1.path())
        .env("EDDA_STORE_ROOT", root_b.path())
        .output()
        .expect("spawn edda init with explicit root B");
    assert!(
        out.status.success(),
        "init with explicit root B failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        root_b.path().join("registry.json").exists(),
        "child must register the project into the explicit root B"
    );
    assert!(
        !a_registry.exists(),
        "child wrote into the parent thread's test root A"
    );

    // 2. A child with no store env at all keeps ordinary default resolution
    //    (`edda doctor` is read-only and prints the root it resolved), so a
    //    regression that smuggles the override across the process boundary —
    //    e.g. by mutating the process environment instead of a thread-local —
    //    would surface here as the child resolving A (or B).
    let repo2 = e2e_repo();
    let out = Command::new(edda_bin())
        .args(["doctor", "claude"])
        .current_dir(repo2.path())
        .env_remove("EDDA_STORE_ROOT")
        .output()
        .expect("spawn edda doctor");
    assert!(
        out.status.success(),
        "doctor failed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let store_line = stdout
        .lines()
        .find(|l| l.contains("store root:"))
        .expect("doctor prints the resolved store root")
        .to_string();
    assert!(
        !store_line.contains(root_a.path().to_string_lossy().as_ref()),
        "child resolved the parent thread's test root A: {store_line}"
    );
    assert!(
        !store_line.contains(root_b.path().to_string_lossy().as_ref()),
        "child resolved the previous child's root B without being given it: {store_line}"
    );
    assert!(
        !a_registry.exists(),
        "the override-free child wrote into the parent thread's test root A"
    );
}
