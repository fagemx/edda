use super::*;
use edda_core::event::{new_cmd_event_with_git_context, CmdEventParams};

fn receipt(ledger: &Ledger, command: &[&str], sha: &str, dirty: bool, exit: i32) {
    let args = command.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let event = new_cmd_event_with_git_context(
        &CmdEventParams {
            branch: "main",
            parent_hash: ledger.last_event_hash().unwrap().as_deref(),
            argv: &args,
            cwd: "/repo",
            exit_code: exit,
            duration_ms: 1,
            stdout_blob: "",
            stderr_blob: "",
        },
        Some(sha),
        Some(dirty),
    )
    .unwrap();
    ledger.append_event(&event).unwrap();
}

#[test]
fn receipt_matching_requires_exact_clean_head_and_latest_event() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let gates = gate_set(&FrontMatter::default(), &["cargo  test -p x".into()], &[]);
    receipt(&ledger, &["cargo", "test", "-p", "x"], "other", false, 0);
    receipt(&ledger, &["cargo", "test", "-p", "x"], "head", true, 0);
    assert_eq!(read_gates(&ledger, "head", &gates).unwrap().0, "unverified");
    receipt(&ledger, &["cargo", "test", "-p", "x"], "head", false, 0);
    assert_eq!(read_gates(&ledger, "head", &gates).unwrap().0, "verified");
    receipt(&ledger, &["cargo", "test", "-p", "x"], "head", false, 1);
    assert_eq!(read_gates(&ledger, "head", &gates).unwrap().0, "red");
}

#[test]
fn unreadable_ledger_is_an_error_not_uncovered_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let database = rusqlite::Connection::open(&ledger.paths.ledger_db).unwrap();
    database
        .execute("ALTER TABLE events RENAME TO unavailable_events", [])
        .unwrap();
    let gates = gate_set(&FrontMatter::default(), &["echo hi".into()], &[]);
    assert!(read_gates(&ledger, "head", &gates).is_err());
}

#[test]
fn gate_lattice_is_exhaustive_and_silence_is_neutral() {
    let states = ["undeclared", "red", "verified", "unverified"];
    for (i, a) in states.iter().enumerate() {
        for (j, b) in states.iter().enumerate() {
            assert_eq!(combine_gate_status(a, Some(b)), states[i.min(j)]);
        }
        assert_eq!(combine_gate_status(a, None), *a);
    }
}

#[test]
fn gate_commands_preserve_semantic_whitespace() {
    let commands = ["printf 'a  b'".into(), "printf 'a b'".into()];
    let gates = gate_set(&FrontMatter::default(), &commands, &[]);
    assert_eq!(gates.cmds, commands);
    assert_eq!(gates.declared_by, ["--gate"]);
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    assert_eq!(
        read_gates(
            &ledger,
            "head",
            &gate_set(&FrontMatter::default(), &[], &[])
        )
        .unwrap()
        .0,
        "undeclared"
    );
}

#[test]
fn probe_extraction_never_passes_payload_or_invalid_token() {
    let diff = "+ `edda run -- rm -rf /` `edda ask x` `edda Run` `edda review;rm -rf /` `edda ../x`\n- `edda old`\n";
    assert_eq!(
        extract_probe_verbs(diff, None, &["edda".into()]),
        vec![("edda".into(), "run".into()), ("edda".into(), "ask".into())]
    );
}

#[test]
fn explicit_issue_is_not_execution_permission() {
    assert_eq!(spec_trust(&SpecOrigin::None, true), "none");
    assert_eq!(spec_trust(&SpecOrigin::Path, false), "operator");
    assert_eq!(spec_trust(&SpecOrigin::ExplicitIssue, false), "untrusted");
    assert_eq!(spec_trust(&SpecOrigin::ExplicitIssue, true), "operator");
    assert_eq!(
        spec_trust(
            &SpecOrigin::PrDerived {
                author_perm: Some("write".into())
            },
            false
        ),
        "maintainer"
    );
    assert_eq!(
        spec_trust(
            &SpecOrigin::PrDerived {
                author_perm: Some("read".into())
            },
            false
        ),
        "untrusted"
    );
}

#[test]
fn verify_section_preserves_quotes_and_stops_at_sibling_yaml_key() {
    assert_eq!(
        extract_verify("## verify\n```sh\nprintf 'a  b'\n```\n## done\nignored"),
        ["printf 'a  b'"]
    );
    assert_eq!(
        extract_verify("verify:\n  - echo yes\nother:\n  - echo no"),
        ["echo yes"]
    );
    assert!(extract_verify("no verify section").is_empty());
}

#[test]
fn required_ci_is_independent_and_pending_is_neutral() {
    assert_eq!(read_ci(&[]).0, None);
    let green = read_ci(&[("Test".into(), "pass".into())]).0;
    assert_eq!(
        combine_gate_status("unverified", green.as_deref()),
        "verified"
    );
    let pending = read_ci(&[("Test".into(), "pending".into())]).0;
    assert_eq!(
        combine_gate_status("verified", pending.as_deref()),
        "verified"
    );
}

#[test]
fn ran_requires_all_commands_and_blobs_and_timeout_is_silent() {
    let gates: Vec<String> = vec!["echo hi".into()];
    let mut ran = vec![ReviewGateRan {
        cmd: gates[0].clone(),
        exit: 0,
        duration_ms: 1,
        stdout_blob: Some("blob".into()),
        timed_out: false,
    }];
    assert_eq!(ran_status(&gates, &ran).as_deref(), Some("verified"));
    ran[0].stdout_blob = None;
    assert_eq!(ran_status(&gates, &ran), None);
    ran[0].exit = -1;
    ran[0].timed_out = true;
    assert_eq!(ran_status(&gates, &ran), None);
    ran[0].timed_out = false;
    assert_eq!(ran_status(&gates, &ran).as_deref(), Some("red"));
}

#[test]
fn cargo_without_lane_and_expired_budget_do_not_execute() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let (ran, notes) = ran_gates(
        dir.path(),
        &[" cargo\ttest".into()],
        30,
        false,
        &ledger.paths,
        dir.path(),
    );
    assert!(ran.is_empty());
    assert!(notes[0].contains("CARGO_TARGET_DIR"));
    let (ran, notes) = ran_gates(
        dir.path(),
        &["echo no".into()],
        0,
        true,
        &ledger.paths,
        dir.path(),
    );
    assert!(ran.is_empty());
    assert!(notes[0].contains("exhausted"));
}

#[test]
fn runner_preserves_shell_quoting_and_bounds_output() {
    let dir = tempfile::tempdir().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let output = process::shell("printf 'a  b'", dir.path(), deadline).unwrap();
    assert_eq!(output.stdout, b"a  b");
    let output = process::shell(
        "i=0; while [ $i -lt 5000 ]; do printf x; i=$((i+1)); done",
        dir.path(),
        deadline,
    )
    .unwrap();
    assert_eq!(output.stdout.len(), 4000);
    assert!(output.truncated);
}

#[test]
fn failed_blob_write_is_loud_and_cannot_verify_ran() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let mut paths = ledger.paths.clone();
    paths.blobs_dir = dir.path().join("missing/blobs");
    let gates = vec!["printf success".into()];
    let (ran, notes) = ran_gates(dir.path(), &gates, 10, true, &paths, dir.path());
    assert_eq!(ran[0].exit, 0);
    assert!(ran[0].stdout_blob.is_none());
    assert!(notes.iter().any(|note| note.contains("not stored")));
    assert_eq!(ran_status(&gates, &ran), None);
}

#[test]
fn deadline_stops_descendant_and_does_not_start_next_gate() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let started = Instant::now();
    let (ran, notes) = ran_gates(
        dir.path(),
        &[
            "sleep 2; printf orphan > orphan.txt".into(),
            "printf ran > next.txt".into(),
        ],
        1,
        true,
        &ledger.paths,
        dir.path(),
    );
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(ran.len(), 1);
    assert!(ran[0].timed_out);
    assert!(notes.iter().any(|n| n.contains("next.txt")));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!dir.path().join("orphan.txt").exists());
    assert!(!dir.path().join("next.txt").exists());
}

#[test]
fn wiring_scan_executes_base_script_even_when_head_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let git = process::executable("git").unwrap();
    let git_cmd = |args: &[&str]| {
        let output = std::process::Command::new(&git)
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git_cmd(&["init"]);
    std::fs::create_dir(dir.path().join("scripts")).unwrap();
    let script = dir.path().join("scripts/wiring-scan.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'base-script'\n").unwrap();
    git_cmd(&["add", "."]);
    git_cmd(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "base",
    ]);
    let base = git_cmd(&["rev-parse", "HEAD"]);
    std::fs::write(script, "#!/bin/sh\nprintf head-script > head-executed\n").unwrap();
    git_cmd(&["add", "."]);
    git_cmd(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "head",
    ]);
    let head = git_cmd(&["rev-parse", "HEAD"]);
    let result = run_wiring_scan(dir.path(), &base, &head).unwrap().unwrap();
    assert!(result.contains("base-script"));
    assert!(!dir.path().join("head-executed").exists());
}
