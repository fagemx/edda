use edda_derive::rebuild_branch;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let head = ledger.head_branch()?;
    let snap = rebuild_branch(&ledger, &head)?;

    if json {
        // Stable contract (ledger decision `compat.stable-json-surfaces`;
        // COMPATIBILITY.md § "Stable `--json` contracts"): within 0.x keys may
        // be added, never deleted, renamed, or retyped. The key set is the
        // three facts the text form states and nothing more, because a key
        // that ships here can never be withdrawn. `last_commit` is null until
        // the branch has one, and its own keys are part of the same contract.
        // Pinned by `compat_golden_fixture_status_json_keys_and_types` below.
        let payload = serde_json::json!({
            "branch": head,
            "last_commit": snap.last_commit.as_ref().map(|c| serde_json::json!({
                "event_id": c.event_id,
                "ts": c.ts,
                "title": c.title,
            })),
            "uncommitted_events": snap.uncommitted_events,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("On branch {head}");

    if let Some(c) = &snap.last_commit {
        println!("Last commit: {} {} \"{}\"", c.ts, c.event_id, c.title);
    } else {
        println!("Last commit: (none)");
    }

    println!("Uncommitted events: {}", snap.uncommitted_events);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;
    use serde_json::Value;
    use std::path::PathBuf;

    /// Path to the `edda` binary cargo just built for this test run
    /// (`current_exe` = `target/debug/deps/<test>-<hash>.exe`).
    fn edda_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe
            .parent()
            .and_then(|d| d.parent())
            .expect("deps/.. = target/debug")
            .to_path_buf();
        dir.join(format!("edda{}", std::env::consts::EXE_SUFFIX))
    }

    /// Run `edda` in `repo` and return (exit code, stdout, stderr).
    fn run_edda(args: &[&str], repo: &Path) -> (i32, String, String) {
        assert!(edda_bin().exists(), "edda binary not found");
        let out = std::process::Command::new(edda_bin())
            .args(args)
            .current_dir(repo)
            .output()
            .expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// A real SQLite ledger holding one note event (tempfile, no mocks), which
    /// leaves the branch with an uncommitted event and no commit yet.
    fn seeded_ledger(repo: &Path) {
        let ledger = Ledger::open_or_init(repo).expect("open_or_init");
        let e1 = new_note_event("main", None, "system", "first event", &[]).expect("e1");
        ledger.append_event(&e1).expect("append e1");
        drop(ledger);
    }

    fn sorted_keys(v: &Value) -> Vec<String> {
        let mut keys: Vec<String> = v
            .as_object()
            .expect("one JSON object")
            .keys()
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Golden fixture for the stable `edda status --json` contract (ledger
    /// decision `compat.stable-json-surfaces`; policy page: COMPATIBILITY.md
    /// § "Stable `--json` contracts"). Within 0.x, keys may be added, never
    /// deleted, renamed, or retyped. Pins the exact key set and per-key types
    /// on both sides of the only optional field — `last_commit` null before a
    /// commit exists, and an object with a pinned key set after — through the
    /// real binary.
    #[test]
    fn compat_golden_fixture_status_json_keys_and_types() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        seeded_ledger(repo.path());

        // Before any commit: last_commit is null, the note is uncommitted.
        let (code, stdout, stderr) = run_edda(&["status", "--json"], repo.path());
        assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
        let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
        assert_eq!(
            sorted_keys(&v),
            vec!["branch", "last_commit", "uncommitted_events"],
            "status --json key set changed — this is a stable contract; \
             see COMPATIBILITY.md"
        );
        assert_eq!(v["branch"], Value::from("main"));
        assert_eq!(v["last_commit"], Value::Null);
        assert!(
            v["uncommitted_events"].is_u64(),
            "uncommitted_events must be an integer: {v}"
        );
        assert_eq!(v["uncommitted_events"], Value::from(1));

        // After a commit: same outer key set, last_commit becomes an object
        // whose own keys are equally part of the contract.
        let (code, stdout, stderr) =
            run_edda(&["commit", "--title", "fixture commit"], repo.path());
        assert_eq!(
            code, 0,
            "commit failed: stdout={stdout:?} stderr={stderr:?}"
        );

        let (code, stdout, stderr) = run_edda(&["status", "--json"], repo.path());
        assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
        let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
        assert_eq!(
            sorted_keys(&v),
            vec!["branch", "last_commit", "uncommitted_events"],
            "status --json key set changed — this is a stable contract; \
             see COMPATIBILITY.md"
        );
        let commit = &v["last_commit"];
        assert_eq!(
            sorted_keys(commit),
            vec!["event_id", "title", "ts"],
            "status --json last_commit key set changed — this is a stable \
             contract; see COMPATIBILITY.md"
        );
        assert!(
            commit["event_id"].is_string(),
            "event_id must be a string: {v}"
        );
        // `ts` is the one field a consumer must machine-parse, so pin its
        // shape and not merely its JSON type: `is_string()` alone would stay
        // green if the format moved to, say, epoch seconds in a string.
        // RFC 3339 UTC comes from `now_rfc3339` (edda-core/src/event.rs:19-22);
        // sub-second precision is platform-dependent, so it is not asserted.
        let ts = commit["ts"].as_str().expect("ts must be a string");
        assert!(
            ts.len() >= 20
                && ts.as_bytes()[4] == b'-'
                && ts.as_bytes()[7] == b'-'
                && ts.as_bytes()[10] == b'T'
                && ts.ends_with('Z'),
            "ts must be RFC 3339 UTC — this is a stable contract; \
             see COMPATIBILITY.md: {ts:?}"
        );
        assert_eq!(commit["title"], Value::from("fixture commit"));
        assert_eq!(v["uncommitted_events"], Value::from(0));
    }

    /// COMPATIBILITY.md claims `--json` "adds no failure mode of its own" and
    /// that both forms produce the same exit code and stderr in every state.
    /// That sentence is only worth having if something checks it: the two
    /// forms must agree on the exit code, and the JSON form must never print a
    /// half-object consumers would try to parse.
    ///
    /// Covers the two states reachable without hand-building a ledger: no
    /// workspace at all, and a `.edda/ledger.db` that is not a database. The
    /// third — a ledger whose schema is newer than the binary — is pinned for
    /// the text form by `crates/edda-cli/tests/schema_refusal_contract.rs`.
    #[test]
    fn status_json_adds_no_failure_mode() {
        let no_workspace = tempfile::tempdir().expect("no-workspace tempdir");

        let corrupt = tempfile::tempdir().expect("corrupt tempdir");
        std::fs::create_dir_all(corrupt.path().join(".edda")).expect("mkdir .edda");
        std::fs::write(
            corrupt.path().join(".edda").join("ledger.db"),
            b"this is not a database",
        )
        .expect("write corrupt ledger");

        for repo in [no_workspace.path(), corrupt.path()] {
            let (text_code, text_out, text_err) = run_edda(&["status"], repo);
            let (json_code, json_out, json_err) = run_edda(&["status", "--json"], repo);

            assert_eq!(
                text_code, json_code,
                "--json changed the exit code on a failure path in {repo:?}: \
                 text={text_code} json={json_code}"
            );
            assert_ne!(json_code, 0, "expected a failure in {repo:?}");
            assert_eq!(
                text_err, json_err,
                "--json changed stderr on a failure path in {repo:?}"
            );
            assert!(
                json_out.trim().is_empty(),
                "a failure must not print JSON on stdout: {json_out:?}"
            );
            assert!(
                text_out.trim().is_empty(),
                "a failure must not print status text on stdout: {text_out:?}"
            );
        }
    }

    /// The text form is the default and is not disturbed by the new flag.
    #[test]
    fn status_without_json_stays_text() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        seeded_ledger(repo.path());

        let (code, stdout, stderr) = run_edda(&["status"], repo.path());
        assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
        assert!(stdout.starts_with("On branch main"), "stdout={stdout:?}");
        assert!(stdout.contains("Last commit: (none)"), "stdout={stdout:?}");
        assert!(
            stdout.contains("Uncommitted events: 1"),
            "stdout={stdout:?}"
        );
        assert!(
            serde_json::from_str::<Value>(&stdout).is_err(),
            "the default form must stay text, not JSON: {stdout:?}"
        );
    }
}
