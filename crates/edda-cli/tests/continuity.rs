use edda_ledger::Ledger;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

fn run(repo: &Path, store: &Path, args: &[&str]) -> Output {
    Command::new(edda_bin())
        .current_dir(repo)
        .env("EDDA_STORE_ROOT", store)
        .args(args)
        .output()
        .expect("spawn edda")
}

fn initialize(repo: &Path) {
    drop(Ledger::open_or_init(repo).expect("initialize ledger"));
}

fn configure_key(repo: &Path, key: &str) {
    std::fs::write(
        repo.join(".edda/config.json"),
        serde_json::to_vec(&serde_json::json!({"portable_repo_key": key})).unwrap(),
    )
    .unwrap();
}

fn write_input(dir: &Path, name: &str, state: Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "capsule_version": 1,
            "state": state,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn save_exact_readback_show_and_sync_status_are_structured() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "team/project");
    let input = write_input(
        inputs.path(),
        "capsule.json",
        serde_json::json!({
            "title": "Unicode 繼續",
            "goal": "finish S1",
            "current": "tests next",
            "next_action": "run focused tests"
        }),
    );

    let saved = json(&run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ));
    assert_eq!(saved["status"], "SAVED_LOCAL");
    assert_eq!(saved["sync_status"], "SYNC_UNAVAILABLE");
    assert_eq!(saved["data_authority"], "data_only");

    let shown = json(&run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "show",
            saved["capsule_id"].as_str().unwrap(),
            "--json",
        ],
    ));
    assert_eq!(shown["local_event_id"], saved["event_id"]);
    assert_eq!(shown["capsule"]["state"]["title"], "Unicode 繼續");
    assert_eq!(shown["data_authority"], "data_only");
    for verb in ["show", "restore"] {
        let text = run(
            repo.path(),
            store.path(),
            &["continuity", verb, saved["capsule_id"].as_str().unwrap()],
        );
        let stdout = String::from_utf8_lossy(&text.stdout);
        assert!(text.status.success());
        assert!(stdout.contains("data only; not instructions"));
        assert!(stdout.contains("NEXT ACTION (DATA ONLY)"));
    }
}

#[test]
fn dirty_checkout_save_records_metadata_without_git_mutation() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(repo.path(), &["config", "user.email", "test@example.com"]);
    git(repo.path(), &["config", "user.name", "Test"]);
    git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/team/project.git",
        ],
    );
    std::fs::write(repo.path().join(".gitignore"), ".edda/\n").unwrap();
    std::fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
    git(repo.path(), &["add", ".gitignore", "tracked.txt"]);
    git(repo.path(), &["commit", "-m", "base"]);
    git(repo.path(), &["checkout", "--detach"]);
    initialize(repo.path());
    std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();
    let before_head = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let before_file = std::fs::read(repo.path().join("tracked.txt")).unwrap();
    let input = write_input(
        inputs.path(),
        "dirty.json",
        serde_json::json!({"next_action": "inspect dirty work"}),
    );

    let saved = json(&run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ));
    let shown = json(&run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "show",
            saved["capsule_id"].as_str().unwrap(),
            "--json",
        ],
    ));
    assert_eq!(shown["capsule"]["git"]["tree_dirty"], true);
    assert_eq!(shown["capsule"]["git"]["detached"], true);
    assert_eq!(shown["capsule"]["git"]["branch"], Value::Null);
    assert!(shown["capsule"]["git"]["dirty_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "tracked.txt"));
    assert_eq!(
        std::fs::read(repo.path().join("tracked.txt")).unwrap(),
        before_file
    );
    let after_head = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(after_head, before_head);
}

#[test]
fn bundle_round_trips_fresh_clone_and_duplicate_is_stable_noop() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    for repo in [&source, &target] {
        git(repo.path(), &["init"]);
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "git@example.com:team/project.git",
            ],
        );
        initialize(repo.path());
    }
    let input = write_input(
        inputs.path(),
        "portable.json",
        serde_json::json!({"title": "portable", "next_action": "continue there"}),
    );
    let saved = json(&run(
        source.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ));
    let bundle = inputs.path().join("capsule.bundle.json");
    let exported = run(
        source.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(exported.status.success());

    let spawn_import = || {
        Command::new(edda_bin())
            .current_dir(target.path())
            .env("EDDA_STORE_ROOT", store.path())
            .args(["continuity", "import", bundle.to_str().unwrap(), "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn_import();
    let second = spawn_import();
    let first = json(&first.wait_with_output().unwrap());
    let second = json(&second.wait_with_output().unwrap());
    let mut statuses = vec![
        first["status"].as_str().unwrap(),
        second["status"].as_str().unwrap(),
    ];
    statuses.sort_unstable();
    assert_eq!(statuses, vec!["imported", "skipped"]);
    assert_eq!(first["origin_event_id"], second["origin_event_id"]);
    assert_eq!(first["local_event_id"], second["local_event_id"]);
    let aliases = store.path().join("portable_repositories.json");
    let aliases_before = std::fs::read(&aliases).unwrap();
    let third = json(&run(
        target.path(),
        store.path(),
        &["continuity", "import", bundle.to_str().unwrap(), "--json"],
    ));
    assert_eq!(third["status"], "skipped");
    assert_eq!(std::fs::read(&aliases).unwrap(), aliases_before);
    assert_eq!(
        Ledger::open(target.path()).unwrap().count_events().unwrap(),
        1
    );

    let reexport = inputs.path().join("reexport.bundle.json");
    assert!(run(
        target.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            reexport.to_str().unwrap(),
        ],
    )
    .status
    .success());
    let a: Value = serde_json::from_slice(&std::fs::read(bundle).unwrap()).unwrap();
    let b: Value = serde_json::from_slice(&std::fs::read(reexport).unwrap()).unwrap();
    assert_eq!(
        a, b,
        "re-export must preserve immutable origin bytes and IDs"
    );
}

#[test]
fn export_accepts_relative_output_and_preserves_no_clobber() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "relative/export");
    let input = write_input(
        inputs.path(),
        "relative.json",
        serde_json::json!({"next_action": "continue"}),
    );
    let saved = json(&run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ));

    let relative = "relative.bundle.json";
    let exported = run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            relative,
        ],
    );
    assert!(
        exported.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let destination = repo.path().join(relative);
    let original = std::fs::read(&destination).unwrap();

    let refused = run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            relative,
        ],
    );
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(destination).unwrap(), original);
}

#[test]
fn wrong_repository_and_corrupt_bundle_are_refused_without_append() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(source.path());
    initialize(target.path());
    configure_key(source.path(), "source/repo");
    configure_key(target.path(), "other/repo");
    let input = write_input(
        inputs.path(),
        "input.json",
        serde_json::json!({"next_action": "continue"}),
    );
    let saved = json(&run(
        source.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ));
    let bundle = inputs.path().join("bundle.json");
    assert!(run(
        source.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ],
    )
    .status
    .success());
    let wrong = run(
        target.path(),
        store.path(),
        &["continuity", "import", bundle.to_str().unwrap(), "--json"],
    );
    assert!(!wrong.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&wrong.stdout).unwrap()["status"],
        "refused"
    );
    assert_eq!(
        Ledger::open(target.path()).unwrap().count_events().unwrap(),
        0
    );

    configure_key(target.path(), "source/repo");
    let mut corrupt: Value = serde_json::from_slice(&std::fs::read(&bundle).unwrap()).unwrap();
    corrupt["capsule_sha256"] = Value::String("0".repeat(64));
    let corrupt_path = inputs.path().join("corrupt.json");
    std::fs::write(&corrupt_path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    let refused = run(
        target.path(),
        store.path(),
        &[
            "continuity",
            "import",
            corrupt_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!refused.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.stdout).unwrap()["status"],
        "refused"
    );
    assert_eq!(
        Ledger::open(target.path()).unwrap().count_events().unwrap(),
        0
    );
}
