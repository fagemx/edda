use edda_core::event::{new_checkpoint_event, CheckpointPayload};
use edda_ledger::Ledger;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

fn run(repo: &Path, store: &Path, args: &[&str]) -> Output {
    Command::new(edda_bin())
        .current_dir(repo)
        .env("EDDA_STORE_ROOT", store)
        .args(args)
        .output()
        .expect("spawn Cargo-built edda test binary")
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

fn write_input(dir: &Path, name: &str, title: &str, next_action: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "capsule_version": 1,
            "state": {"title": title, "next_action": next_action},
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn save(repo: &Path, store: &Path, input: &Path) -> Value {
    successful_json(&run(
        repo,
        store,
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    ))
}

fn successful_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    if !root.exists() {
        return files;
    }
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            files.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                std::fs::read(path).unwrap(),
            );
        }
    }
    files
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn resign_bundle(bundle: &mut Value) {
    let mut content = bundle.clone();
    content.as_object_mut().unwrap().remove("bundle_sha256");
    let bytes = edda_core::canon::canonical_json_bytes(&content).unwrap();
    bundle["bundle_sha256"] = edda_core::hash::sha256_hex(&bytes).into();
}

#[test]
fn harness_and_all_commands_use_the_cargo_built_binary() {
    let path = edda_bin();
    assert!(path.is_file(), "CARGO_BIN_EXE_edda must name a built file");
    assert_ne!(path, PathBuf::from("edda"));
}

#[test]
fn continuity_event_is_excluded_from_checkpoint_hot_pack() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "round1/distinct-event");
    let fixture = "UNTRUSTED_NEXT_ACTION_MUST_NOT_ENTER_HOT_PACK";
    let input = write_input(files.path(), "input.json", "distinct", fixture);
    save(repo.path(), store.path(), &input);

    let ledger = Ledger::open(repo.path()).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "continuity_capsule");
    assert!(ledger.iter_events_by_type("checkpoint").unwrap().is_empty());
    let packed = edda_pack::checkpoint_items(&events);
    assert!(packed.is_empty());
    assert!(!format!("{packed:?}").contains(fixture));
}

#[test]
fn legacy_checkpoint_branch_is_unknown_and_default_restore_selects_it() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    let ledger = Ledger::open_or_init(repo.path()).unwrap();
    let event = new_checkpoint_event(
        "ledger-branch-not-git-branch",
        None,
        "agent",
        &CheckpointPayload {
            hypotheses: Vec::new(),
            rejected: Vec::new(),
            open: Vec::new(),
            next: "legacy selectable next".into(),
        },
    )
    .unwrap();
    ledger.append_event(&event).unwrap();
    drop(ledger);

    let listed = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "list", "--branch", "any-git-branch", "--json"],
    ));
    let entry = &listed["capsules"][0];
    assert_eq!(entry["legacy_partial"], true);
    assert_eq!(entry["capsule"]["git"]["branch"], Value::Null);
    let capsule_id = entry["capsule"]["capsule_id"].as_str().unwrap();

    let shown = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "show", capsule_id, "--json"],
    ));
    assert_eq!(
        shown["capsule"]["state"]["next_action"],
        "legacy selectable next"
    );
    let restored = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "restore", "--json"],
    ));
    assert_eq!(restored["capsule"]["capsule_id"], capsule_id);
}

#[test]
fn caller_controlled_parser_fixture_never_reaches_stdout_or_stderr() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "round1/parser-safety");
    let fixture = "CALLER_CONTROLLED_PARSER_FIXTURE_93c18f";
    let input = files.path().join("unknown-field.json");
    std::fs::write(
        &input,
        format!(r#"{{"capsule_version":1,"state":{{"next_action":"continue"}},"{fixture}":true}}"#),
    )
    .unwrap();

    let refused = run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            input.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!refused.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(combined.contains("invalid ContextCapsuleV1 input schema"));
    assert!(!combined.contains(fixture));
    assert_eq!(
        Ledger::open(repo.path()).unwrap().count_events().unwrap(),
        0
    );
}

#[test]
fn decoded_malformed_capsule_is_secret_scanned_before_deserialization() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "round1/decoded-secret");
    let input = write_input(files.path(), "input.json", "safe", "continue");
    let saved = save(repo.path(), store.path(), &input);
    let bundle_path = files.path().join("bundle.json");
    assert!(run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "export",
            saved["capsule_id"].as_str().unwrap(),
            "--out",
            bundle_path.to_str().unwrap(),
        ],
    )
    .status
    .success());

    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let malformed =
        format!(r#"{{"capsule_version":1,"state":{{"next_action":"ok"}},"x":"{secret}""#);
    let mut bundle: Value = serde_json::from_slice(&std::fs::read(&bundle_path).unwrap()).unwrap();
    bundle["capsule_bytes_hex"] = hex_encode(malformed.as_bytes()).into();
    bundle["capsule_sha256"] = edda_core::hash::sha256_hex(malformed.as_bytes()).into();
    resign_bundle(&mut bundle);
    std::fs::write(&bundle_path, serde_json::to_vec(&bundle).unwrap()).unwrap();

    let refused = run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "import",
            bundle_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!refused.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(combined.contains("secret content refused in capsule bytes"));
    assert!(!combined.contains(secret));
    assert_eq!(
        Ledger::open(repo.path()).unwrap().count_events().unwrap(),
        1
    );
}

#[test]
fn durable_reverse_aliases_drive_list_and_make_ambiguity_visible_without_writes() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());

    configure_key(repo.path(), "round1/alias-a");
    let first = save(
        repo.path(),
        store.path(),
        &write_input(files.path(), "a.json", "alias a", "continue a"),
    );
    std::fs::remove_file(repo.path().join(".edda/config.json")).unwrap();
    let via_alias = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "list", "--json"],
    ));
    assert_eq!(
        via_alias["capsules"][0]["capsule"]["capsule_id"],
        first["capsule_id"]
    );

    configure_key(repo.path(), "round1/alias-b");
    let second = save(
        repo.path(),
        store.path(),
        &write_input(files.path(), "b.json", "alias b", "continue b"),
    );
    std::fs::remove_file(repo.path().join(".edda/config.json")).unwrap();
    let before = snapshot(store.path());
    let ambiguous = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "list", "--json"],
    ));
    assert_eq!(ambiguous["capsules"].as_array().unwrap().len(), 2);
    assert!(ambiguous["warnings"][0]
        .as_str()
        .unwrap()
        .contains("ambiguous portable repository aliases"));

    let default_restore = run(
        repo.path(),
        store.path(),
        &["continuity", "restore", "--json"],
    );
    assert!(!default_restore.status.success());
    assert!(String::from_utf8_lossy(&default_restore.stderr)
        .contains("ambiguous portable repository aliases"));
    assert!(run(
        repo.path(),
        store.path(),
        &[
            "continuity",
            "restore",
            second["capsule_id"].as_str().unwrap(),
            "--json",
        ],
    )
    .status
    .success());
    assert_eq!(snapshot(store.path()), before);
}

#[test]
fn exact_restore_bypasses_corrupt_and_secret_bearing_alias_sidecars() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "round2/exact-restore");
    let saved = save(
        repo.path(),
        store.path(),
        &write_input(files.path(), "input.json", "exact", "keep reading"),
    );
    let capsule_id = saved["capsule_id"].as_str().unwrap();
    let alias_path = store.path().join("portable_repositories.json");

    std::fs::write(&alias_path, b"{corrupt alias JSON").unwrap();
    let corrupt_before = std::fs::read(&alias_path).unwrap();
    let restored = successful_json(&run(
        repo.path(),
        store.path(),
        &["continuity", "restore", capsule_id, "--json"],
    ));
    assert_eq!(restored["capsule"]["capsule_id"], capsule_id);
    assert_eq!(std::fs::read(&alias_path).unwrap(), corrupt_before);

    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    std::fs::write(
        &alias_path,
        format!(
            r#"{{"version":1,"repositories":{{"repo_{}":{{"local":{{"{secret}":true}}}}}}}}"#,
            "a".repeat(64)
        ),
    )
    .unwrap();
    let secret_before = std::fs::read(&alias_path).unwrap();
    let restored = run(
        repo.path(),
        store.path(),
        &["continuity", "restore", capsule_id, "--json"],
    );
    assert!(restored.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    assert!(!combined.contains(secret));
    assert_eq!(
        serde_json::from_slice::<Value>(&restored.stdout).unwrap()["capsule"]["capsule_id"],
        capsule_id
    );
    assert_eq!(std::fs::read(&alias_path).unwrap(), secret_before);

    let poisoned_read = run(repo.path(), store.path(), &["continuity", "list", "--json"]);
    assert!(!poisoned_read.status.success());
    let error = String::from_utf8_lossy(&poisoned_read.stderr);
    assert!(error.contains("secret content refused in portable repository alias registry"));
    assert!(!error.contains(secret));
}

#[test]
fn suspicious_checkout_alias_warning_is_redacted_and_registry_stays_readable() {
    let safe_repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(safe_repo.path());
    configure_key(safe_repo.path(), "round2/safe-alias");
    save(
        safe_repo.path(),
        store.path(),
        &write_input(files.path(), "safe.json", "safe", "continue safe"),
    );
    let alias_path = store.path().join("portable_repositories.json");
    let before = std::fs::read(&alias_path).unwrap();

    let parent = tempfile::tempdir().unwrap();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let suspicious_repo = parent.path().join(secret);
    std::fs::create_dir(&suspicious_repo).unwrap();
    initialize(&suspicious_repo);
    configure_key(&suspicious_repo, "round2/refused-alias");
    let saved = run(
        &suspicious_repo,
        store.path(),
        &[
            "continuity",
            "save",
            "--file",
            write_input(files.path(), "suspicious.json", "suspicious", "continue")
                .to_str()
                .unwrap(),
            "--json",
        ],
    );
    assert!(saved.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&saved.stdout),
        String::from_utf8_lossy(&saved.stderr)
    );
    assert!(combined.contains("portable repository alias was not recorded"));
    assert!(combined.contains("secret content refused in portable repository clone path"));
    assert!(!combined.contains(secret));
    assert_eq!(std::fs::read(&alias_path).unwrap(), before);

    let later_read = successful_json(&run(
        safe_repo.path(),
        store.path(),
        &["continuity", "list", "--json"],
    ));
    assert_eq!(later_read["capsules"].as_array().unwrap().len(), 1);
}

#[test]
fn read_only_continuity_git_queries_do_not_refresh_the_index() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(repo.path(), &["config", "user.email", "test@example.com"]);
    git(repo.path(), &["config", "user.name", "Test"]);
    std::fs::write(repo.path().join(".gitignore"), ".edda/\n").unwrap();
    std::fs::write(repo.path().join("tracked.txt"), "same\n").unwrap();
    git(repo.path(), &["add", ".gitignore", "tracked.txt"]);
    git(repo.path(), &["commit", "-m", "base"]);
    initialize(repo.path());
    configure_key(repo.path(), "round1/no-index-refresh");
    let saved = save(
        repo.path(),
        store.path(),
        &write_input(files.path(), "input.json", "read only", "continue"),
    );

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(repo.path().join("tracked.txt"), "same\n").unwrap();
    let index = repo.path().join(".git/index");
    let before_bytes = std::fs::read(&index).unwrap();
    let before_modified = std::fs::metadata(&index).unwrap().modified().unwrap();
    for args in [
        vec!["continuity", "list", "--json"],
        vec![
            "continuity",
            "show",
            saved["capsule_id"].as_str().unwrap(),
            "--json",
        ],
        vec![
            "continuity",
            "restore",
            saved["capsule_id"].as_str().unwrap(),
            "--json",
        ],
    ] {
        assert!(run(repo.path(), store.path(), &args).status.success());
    }
    assert_eq!(std::fs::read(&index).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&index).unwrap().modified().unwrap(),
        before_modified
    );
    assert!(!repo.path().join(".git/index.lock").exists());
}
