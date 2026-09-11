use super::*;
use edda_core::event::{new_cmd_event_with_git_context, CmdEventParams};
use std::collections::BTreeMap;

fn ci(required: &[&str], runs: &[(&str, &str)]) -> GitHubChecks {
    GitHubChecks {
        required_names: required.iter().map(|name| (*name).into()).collect(),
        latest_runs: runs
            .iter()
            .map(|(name, bucket)| ((*name).into(), (*bucket).into()))
            .collect(),
    }
}

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
fn empty_gate_set_is_undeclared_even_when_evidence_exists() {
    let read = vec![ReviewGateRead {
        kind: "ci".into(),
        r#ref: "CI Gate".into(),
        cmd: "CI Gate".into(),
        result: "red".into(),
    }];
    assert_eq!(gate_status(&[], &read, &[]), "undeclared");
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
fn required_ci_rows_preserve_status_but_green_never_covers_a_gate() {
    assert_eq!(read_ci(&GitHubChecks::default()).0, None);
    let (green, rows) = read_ci(&ci(&["CI Gate"], &[("CI Gate", "pass")]));
    assert_eq!(green.as_deref(), Some("verified"));
    assert_eq!(rows[0].result, "green");
    assert_eq!(gate_status(&["CI Gate".into()], &rows, &[]), "unverified");
    let (pending, _) = read_ci(&ci(&["CI Gate"], &[("CI Gate", "pending")]));
    assert_eq!(pending.as_deref(), Some("unverified"));
}

#[test]
fn empty_required_names_never_use_a_successful_mapped_job() {
    let gates = gate_set(&FrontMatter::default(), &["cargo test".into()], &[]);
    let checks = ci(&[], &[("Test (ubuntu-latest)", "pass")]);
    let mappings = BTreeMap::from([("cargo test".into(), vec!["Test (ubuntu-latest)".into()])]);
    let (status, rows, mapped) = read_ci_job_map(&checks, &gates, &mappings);
    assert!(status.is_none());
    assert!(rows.is_empty());
    assert!(mapped.is_empty());
}

#[test]
fn mapped_gate_requires_all_exact_jobs_and_ref_lists_each_status() {
    let gate = "cargo clippy --workspace";
    let gates = gate_set(&FrontMatter::default(), &[gate.into()], &[]);
    let mappings = BTreeMap::from([(
        gate.into(),
        vec!["Clippy (ubuntu)".into(), "Clippy (macos)".into()],
    )]);
    let checks = ci(
        &["CI Gate"],
        &[
            ("CI Gate", "pass"),
            ("Clippy (ubuntu)", "pass"),
            ("Clippy (macos)", "pass"),
        ],
    );
    let (status, rows, mapped) = read_ci_job_map(&checks, &gates, &mappings);
    assert_eq!(status.as_deref(), Some("verified"));
    assert_eq!(mapped, [gate]);
    assert_eq!(rows[0].kind, "ci-job-map");
    assert_eq!(rows[0].result, "green");
    assert!(rows[0].r#ref.contains("Clippy (ubuntu)=pass"));
    assert!(rows[0].r#ref.contains("Clippy (macos)=pass"));
}

#[test]
fn mapped_failure_dominates_other_green_evidence() {
    let gate = "cargo test";
    let gates = gate_set(&FrontMatter::default(), &[gate.into()], &[]);
    let mappings = BTreeMap::from([(gate.into(), vec!["Linux".into(), "macOS".into()])]);
    let checks = ci(
        &["CI Gate"],
        &[("CI Gate", "pass"), ("Linux", "pass"), ("macOS", "fail")],
    );
    let (_, mut rows, _) = read_ci_job_map(&checks, &gates, &mappings);
    assert_eq!(rows[0].result, "red");
    rows.push(ReviewGateRead {
        kind: "cmd-event".into(),
        r#ref: "receipt".into(),
        cmd: gate.into(),
        result: "green".into(),
    });
    assert_eq!(gate_status(&gates.cmds, &rows, &[]), "red");
}

#[test]
fn receipt_required_and_mapped_red_each_globally_dominate() {
    let gates: Vec<String> = vec!["fmt".into(), "test".into()];
    let green = vec![
        ReviewGateRead {
            kind: "cmd-event".into(),
            r#ref: "receipt".into(),
            cmd: gates[0].clone(),
            result: "green".into(),
        },
        ReviewGateRead {
            kind: "ci-job-map".into(),
            r#ref: "Test=pass".into(),
            cmd: gates[1].clone(),
            result: "green".into(),
        },
    ];
    for (kind, cmd) in [
        ("cmd-event", gates[0].as_str()),
        ("ci", "CI Gate"),
        ("ci-job-map", gates[1].as_str()),
    ] {
        let mut read = green.clone();
        read.push(ReviewGateRead {
            kind: kind.into(),
            r#ref: "failed evidence".into(),
            cmd: cmd.into(),
            result: "red".into(),
        });
        assert_eq!(gate_status(&gates, &read, &[]), "red", "{kind}");
    }
}

#[test]
fn partial_receipt_and_partial_mapped_green_collectively_verify() {
    let gates = gate_set(&FrontMatter::default(), &["fmt".into(), "test".into()], &[]);
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    receipt(&ledger, &["fmt"], "head", false, 0);
    let (_, mut read, _) = read_gates(&ledger, "head", &gates).unwrap();

    let checks = ci(&["CI Gate"], &[("CI Gate", "pass"), ("Test", "pass")]);
    read.extend(read_ci(&checks).1);
    let mappings = BTreeMap::from([("test".into(), vec!["Test".into()])]);
    read.extend(read_ci_job_map(&checks, &gates, &mappings).1);

    assert_eq!(gate_status(&gates.cmds, &read, &[]), "verified");
}

#[test]
fn missing_pending_and_skipped_mapped_jobs_are_unverified() {
    let gate = "cargo test";
    let gates = gate_set(&FrontMatter::default(), &[gate.into()], &[]);
    for (label, runs) in [
        ("missing", vec![]),
        ("pending", vec![("Linux", "pending")]),
        ("skipped", vec![("Linux", "skipped")]),
    ] {
        let mappings = BTreeMap::from([(gate.into(), vec!["Linux".into()])]);
        let checks = ci(&["CI Gate"], &runs);
        let (status, rows, _) = read_ci_job_map(&checks, &gates, &mappings);
        assert_eq!(status.as_deref(), Some("unverified"), "{label}");
        assert_eq!(rows[0].result, "pending", "{label}");
        assert!(rows[0].r#ref.contains(label), "{label}: {}", rows[0].r#ref);
    }
}

#[test]
fn mapped_gate_is_not_also_uncovered_but_unmapped_gate_stays_visible() {
    let mapped_gate = "cargo fmt";
    let unmapped_gate = "custom lint";
    let gates = gate_set(
        &FrontMatter::default(),
        &[mapped_gate.into(), unmapped_gate.into()],
        &[],
    );
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_or_init(dir.path()).unwrap();
    let (_, mut read, mut uncovered) = read_gates(&ledger, "head", &gates).unwrap();
    let mappings = BTreeMap::from([(mapped_gate.into(), vec!["Format".into()])]);
    let checks = ci(&["CI Gate"], &[("Format", "pass")]);
    let (_, mapped_rows, mapped) = read_ci_job_map(&checks, &gates, &mappings);
    read.extend(mapped_rows);
    remove_mapped_uncovered(&mut uncovered, &mapped);
    assert_eq!(uncovered, [unmapped_gate]);
    let text = evidence_text(&read, &uncovered, &[], &[], None);
    assert!(text.contains("\"cargo fmt\": green (ci-job-map \"Format=pass\")"));
    assert!(!text.contains("\"cargo fmt\": not covered"));
    assert!(text.contains("\"custom lint\": not covered"));
}

#[test]
fn ran_fills_only_a_remaining_gate_with_stored_success_before_timeout() {
    let gates: Vec<String> = vec!["fmt".into(), "test".into()];
    let read = vec![ReviewGateRead {
        kind: "ci-job-map".into(),
        r#ref: "Format=pass".into(),
        cmd: gates[0].clone(),
        result: "green".into(),
    }];
    let mut ran = vec![ReviewGateRan {
        cmd: gates[1].clone(),
        exit: 0,
        duration_ms: 1,
        stdout_blob: Some("blob".into()),
        timed_out: false,
    }];
    assert_eq!(gate_status(&gates, &read, &ran), "verified");
    ran[0].cmd = "other".into();
    assert_eq!(gate_status(&gates, &read, &ran), "unverified");
    ran[0].cmd = gates[1].clone();
    ran[0].stdout_blob = None;
    assert_eq!(gate_status(&gates, &read, &ran), "unverified");
    ran[0].stdout_blob = Some("blob".into());
    ran[0].timed_out = true;
    assert_eq!(gate_status(&gates, &read, &ran), "unverified");
    ran[0].timed_out = false;
    ran[0].exit = 1;
    assert_eq!(gate_status(&gates, &read, &ran), "red");
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
    assert_eq!(gate_status(&gates, &[], &ran), "unverified");
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
