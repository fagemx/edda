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
        .expect("spawn edda")
}

fn initialize(repo: &Path) {
    drop(Ledger::open_or_init(repo).expect("initialize ledger"));
}

#[cfg(unix)]
fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
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

fn configure_key(repo: &Path, key: &str) {
    std::fs::write(
        repo.join(".edda/config.json"),
        serde_json::to_vec(&serde_json::json!({"portable_repo_key": key})).unwrap(),
    )
    .unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, current: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !current.exists() {
            return;
        }
        for entry in std::fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
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

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Vec<u8> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn legacy_checkpoint_is_listed_as_partial() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(repo.path()).unwrap();
    let event = new_checkpoint_event(
        "main",
        None,
        "agent",
        &CheckpointPayload {
            hypotheses: vec!["one".into()],
            rejected: Vec::new(),
            open: Vec::new(),
            next: "legacy next".into(),
        },
    )
    .unwrap();
    ledger.append_event(&event).unwrap();
    drop(ledger);

    let listed = json(&run(
        repo.path(),
        store.path(),
        &["continuity", "list", "--json"],
    ));
    assert_eq!(listed["capsules"][0]["legacy_partial"], true);
    assert_eq!(
        listed["capsules"][0]["capsule"]["state"]["next_action"],
        "legacy next"
    );
}

#[test]
fn secret_in_truncated_suffix_is_refused_without_leak_or_write() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "safe/repo");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let input = inputs.path().join("secret.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&serde_json::json!({
            "capsule_version": 1,
            "state": {
                "summary": format!("{}{}", "界".repeat(4_100), secret),
                "next_action": "continue"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run(
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let failure: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(failure["status"], "SAVE_FAILED");
    assert!(stderr.contains("secret content refused in continuity input"));
    assert!(!stderr.contains(secret));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert_eq!(
        Ledger::open(repo.path()).unwrap().count_events().unwrap(),
        0
    );
}

#[test]
fn non_git_without_remote_saves_locally_and_reports_local_only() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    let input = inputs.path().join("local.json");
    std::fs::write(
        &input,
        r#"{"capsule_version":1,"state":{"next_action":"continue locally"}}"#,
    )
    .unwrap();
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
    assert_eq!(saved["portable_repo_id"], Value::Null);
    assert!(saved["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("LOCAL_ONLY")));
}

#[test]
fn unknown_trusted_metadata_field_is_refused_without_leaking_value() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "safe/repo");
    let secret = "https://user:credential@example.com/team/repo.git";
    let input = inputs.path().join("override.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&serde_json::json!({
            "capsule_version": 1,
            "repository": {"remote": secret},
            "state": {"next_action": "continue"}
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run(
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("secret content refused in continuity input"));
    assert!(!stderr.contains(secret));
    assert_eq!(
        Ledger::open(repo.path()).unwrap().count_events().unwrap(),
        0
    );
}

#[test]
fn read_only_verbs_leave_existing_repository_and_store_bytes_unchanged() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "safe/repo");
    let input = inputs.path().join("state.json");
    std::fs::write(
        &input,
        r#"{"capsule_version":1,"state":{"next_action":"continue"}}"#,
    )
    .unwrap();
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
    let before_repo = snapshot(&repo.path().join(".edda"));
    let before_store = snapshot(store.path());
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
    assert_eq!(snapshot(&repo.path().join(".edda")), before_repo);
    assert_eq!(snapshot(store.path()), before_store);
}

#[cfg(unix)]
#[test]
fn legal_unix_nonportable_git_names_are_omitted_without_blocking_save() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(repo.path(), &["config", "user.email", "test@example.com"]);
    git(repo.path(), &["config", "user.name", "Test"]);
    std::fs::write(repo.path().join(".gitignore"), ".edda/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "base"]);
    initialize(repo.path());
    configure_key(repo.path(), "security/legal-unix-names");
    std::fs::write(repo.path().join("legal:colon.txt"), "colon\n").unwrap();
    std::fs::write(repo.path().join(r"legal\backslash.txt"), "backslash\n").unwrap();
    let input = files.path().join("input.json");
    std::fs::write(
        &input,
        br#"{"capsule_version":1,"state":{"next_action":"continue"}}"#,
    )
    .unwrap();

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
    assert!(shown["capsule"]["git"].get("dirty_paths").is_none());
    assert_eq!(shown["capsule"]["git"]["dirty_paths_truncated"], true);
    assert_eq!(
        Ledger::open(repo.path()).unwrap().count_events().unwrap(),
        1
    );
}

#[test]
fn nonportable_dirty_paths_are_refused_before_append() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    initialize(repo.path());
    configure_key(repo.path(), "security/traversal");
    let input = files.path().join("input.json");
    std::fs::write(
        &input,
        br#"{"capsule_version":1,"state":{"next_action":"continue"}}"#,
    )
    .unwrap();
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
    let bundle_path = files.path().join("capsule.bundle.json");
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

    let original: Value = serde_json::from_slice(&std::fs::read(&bundle_path).unwrap()).unwrap();
    let count = Ledger::open(repo.path()).unwrap().count_events().unwrap();
    for dirty_path in [
        "../outside",
        r"..\outside",
        r"dir\..\outside",
        r"dir\file",
        "name:part",
        "C:/outside",
        "dir//file",
    ] {
        let mut bundle = original.clone();
        let capsule_hex = bundle["capsule_bytes_hex"].as_str().unwrap();
        let mut capsule: Value = serde_json::from_slice(&hex_decode(capsule_hex)).unwrap();
        capsule["git"]["dirty_paths"] = serde_json::json!([dirty_path]);
        let capsule_bytes = edda_core::canon::canonical_json_bytes(&capsule).unwrap();
        bundle["capsule_bytes_hex"] = hex_encode(&capsule_bytes).into();
        bundle["capsule_sha256"] = edda_core::hash::sha256_hex(&capsule_bytes).into();
        let mut bundle_content = bundle.clone();
        bundle_content
            .as_object_mut()
            .unwrap()
            .remove("bundle_sha256");
        let bundle_bytes = edda_core::canon::canonical_json_bytes(&bundle_content).unwrap();
        bundle["bundle_sha256"] = edda_core::hash::sha256_hex(&bundle_bytes).into();
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
        assert!(
            !refused.status.success(),
            "dirty path accepted: {dirty_path}"
        );
        let refusal: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(refusal["status"], "refused");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains("unsafe"), "{dirty_path}: {stderr}");
        assert_eq!(
            Ledger::open(repo.path()).unwrap().count_events().unwrap(),
            count
        );
    }
}

#[test]
fn read_only_verbs_do_not_create_a_missing_ledger_or_export() {
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".edda")).unwrap();
    let out = repo.path().join("must-not-exist.bundle");
    for args in [
        vec!["continuity", "list", "--json"],
        vec!["continuity", "show", "cap_missing", "--json"],
        vec![
            "continuity",
            "export",
            "cap_missing",
            "--out",
            out.to_str().unwrap(),
        ],
        vec!["continuity", "restore", "--json"],
    ] {
        assert!(!run(repo.path(), store.path(), &args).status.success());
        assert!(!repo.path().join(".edda/ledger.db").exists());
        assert!(!out.exists());
    }
    assert!(!store.path().join("portable_repositories.json").exists());
}
