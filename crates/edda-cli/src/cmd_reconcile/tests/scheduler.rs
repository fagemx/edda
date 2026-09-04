use super::*;

#[test]
pub(super) fn scheduler_renderer_emits_exact_project_scoped_argv() -> anyhow::Result<()> {
    let manifest = Path::new(
        r"C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json",
    );
    let spec = windows_scheduler_spec(
        Path::new(r"C:\Program Files\edda\edda.exe"),
        manifest,
        "0123456789abcdef0123456789abcdef",
    )?;

    assert_eq!(
        spec.task_name,
        "Edda-Reconcile-0123456789abcdef0123456789abcdef"
    );
    assert_eq!(
        spec.create_args[8],
        r#""C:\Program Files\edda\edda.exe" reconcile --scheduler-manifest "C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json""#
    );
    assert_eq!(
        spec.create_args,
        [
            "/Create",
            "/SC",
            "MINUTE",
            "/MO",
            "1",
            "/TN",
            "Edda-Reconcile-0123456789abcdef0123456789abcdef",
            "/TR",
            r#""C:\Program Files\edda\edda.exe" reconcile --scheduler-manifest "C:\Users\alice\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json""#,
            "/RL",
            "LIMITED",
            "/F",
            "/HRESULT",
        ]
    );
    assert_eq!(
        spec.query_args,
        [
            "/Query",
            "/TN",
            "Edda-Reconcile-0123456789abcdef0123456789abcdef",
            "/XML",
            "/HRESULT",
        ]
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_renderer_is_stable_and_quotes_terminal_backslashes() -> anyhow::Result<()> {
    let id = "0123456789abcdef0123456789abcdef";
    let first = windows_scheduler_spec(
        Path::new(r"C:\edda\edda.exe"),
        Path::new(r"C:\manifest\"),
        id,
    )?;
    let second = windows_scheduler_spec(
        Path::new(r"C:\edda\edda.exe"),
        Path::new(r"C:\manifest\"),
        id,
    )?;

    assert_eq!(first.create_args, second.create_args);
    assert_eq!(
        first.create_args[8],
        r#""C:\edda\edda.exe" reconcile --scheduler-manifest "C:\manifest\\""#
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_renderer_fits_the_preserved_356_unit_fixture() -> anyhow::Result<()>
{
    let executable =
        Path::new(r"\\?\C:\ai_agent\edda-target-gh466-drill-20260816T163456Z\debug\edda.exe");
    let repository = r"\\?\C:\ai_agent\edda-drills\20260816T163456Z\repo";
    let codex = r"\\?\C:\Users\synvoke\AppData\Roaming\npm\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe";
    let old = format!(
            "{} reconcile --repo \"{repository}\" --max-workers 1 --max-attempts 3 --lease-ttl-s 120 --codex-bin \"{codex}\"",
            quote_windows_argument(executable)?,
        );
    assert_eq!(old.encode_utf16().count(), 356);

    let manifest = Path::new(
        r"\\?\C:\Users\synvoke\AppData\Roaming\edda\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json",
    );
    let rendered = render_scheduler_task_run(
        executable,
        manifest,
        "Edda-Reconcile-75ab49a9590f5e1105b928c63a3c0be5",
    )?;
    assert_eq!(rendered.encode_utf16().count(), 238);
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_renderer_enforces_utf16_limit() -> anyhow::Result<()> {
    let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
    let accepted_path = manifest_path_for_task_run_utf16_len(261);
    let accepted = render_scheduler_task_run(Path::new(r"C:\e.exe"), &accepted_path, task_name)?;
    assert_eq!(accepted.encode_utf16().count(), 261);

    let rejected_path = manifest_path_for_task_run_utf16_len(262);
    let error = render_scheduler_task_run(Path::new(r"C:\e.exe"), &rejected_path, task_name)
        .expect_err("262 UTF-16 units must fail")
        .to_string();
    assert!(error.contains("262"));
    assert!(error.contains("261"));
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_renderer_counts_surrogate_pairs_as_two_utf16_units() {
    let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
    let ascii = manifest_path_for_task_run_utf16_len(261);
    let with_pair = PathBuf::from(ascii.to_string_lossy().replacen('x', "😀", 1));
    let unbounded = format!(
        r#""C:\e.exe" reconcile --scheduler-manifest "{}""#,
        with_pair.display()
    );
    assert_eq!(unbounded.chars().count(), 261);
    assert_eq!(unbounded.encode_utf16().count(), 262);
    assert!(render_scheduler_task_run(Path::new(r"C:\e.exe"), &with_pair, task_name).is_err());
}

#[test]
pub(super) fn scheduler_codex_resolver_requires_a_canonical_direct_exe() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let native = dir.path().join("codex.exe");
    std::fs::write(&native, b"MZ")?;
    let shim = dir.path().join("codex.cmd");
    std::fs::write(&shim, b"@echo off")?;
    let search_path = std::env::join_paths([dir.path()])?;

    assert_eq!(
        canonical_direct_codex_executable(Path::new("codex"), Some(&search_path))?,
        native.canonicalize()?
    );
    assert_eq!(
        canonical_direct_codex_executable(&native, None)?,
        native.canonicalize()?
    );
    assert!(canonical_direct_codex_executable(&shim, None).is_err());
    assert!(
        canonical_direct_codex_executable(Path::new("missing-codex"), Some(&search_path)).is_err()
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_codex_resolver_revalidates_the_canonical_target_extension(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let native = dir.path().join("codex.exe");
    std::fs::write(&native, b"MZ")?;
    let shim = dir.path().join("codex.cmd");
    std::fs::write(&shim, b"@echo off")?;

    validate_canonical_direct_codex_target(&native.canonicalize()?)?;
    let error = validate_canonical_direct_codex_target(&shim.canonicalize()?)
        .expect_err("a canonical .cmd target must not be schedulable");
    assert!(error
        .to_string()
        .contains("must be an absolute native .exe file"));
    Ok(())
}

#[cfg(unix)]
#[test]
pub(super) fn scheduler_codex_resolver_rejects_exe_alias_to_cmd_target() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let shim = dir.path().join("codex.cmd");
    std::fs::write(&shim, b"#!/bin/sh")?;
    let alias = dir.path().join("codex.exe");
    std::os::unix::fs::symlink(&shim, &alias)?;

    let error = canonical_direct_codex_executable(&alias, None)
        .expect_err("an .exe alias to a .cmd target must not be schedulable");
    assert!(error.to_string().contains("absolute native .exe file"));
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_target_does_not_require_a_codex_executable() -> anyhow::Result<()>
{
    let (task_name, query_args, delete_args) =
        windows_scheduler_management_args("0123456789abcdef0123456789abcdef")?;
    assert_eq!(task_name, "Edda-Reconcile-0123456789abcdef0123456789abcdef");
    assert_eq!(
        query_args,
        [
            "/Query",
            "/TN",
            "Edda-Reconcile-0123456789abcdef0123456789abcdef",
            "/XML",
            "/HRESULT",
        ]
    );
    assert_eq!(
        delete_args,
        [
            "/Delete",
            "/TN",
            "Edda-Reconcile-0123456789abcdef0123456789abcdef",
            "/F",
            "/HRESULT",
        ]
    );
    Ok(())
}

pub(super) fn scheduler_manifest_xml(executable: &Path, manifest: &Path) -> anyhow::Result<String> {
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    Ok(format!(
            "<Task><Actions><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions></Task>",
            escape(executable.to_str().context("test executable path")?),
            escape(&format!(
                "reconcile --scheduler-manifest {}",
                quote_windows_argument(manifest)?
            )),
        ))
}

#[test]
pub(super) fn scheduler_uninstall_removes_only_a_trusted_exact_manifest_after_absence(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let (_, query_args, delete_args) = windows_scheduler_management_args(&project_id)?;
    let xml = format!(
        "<?xml version=\"1.0\"?>{}",
        scheduler_manifest_xml(&fixture.codex, &prepared.path)?
            .replace("<Actions>", "<!-- harmless scheduler comment --><Actions>")
    );
    let outputs = [
        SchedulerOutput::for_test(0, &xml, ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
    ];
    let mut calls = Vec::new();
    let mut outputs = outputs.into_iter();

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |args| {
        assert!(
            prepared.path.exists(),
            "artifact removed before absence proof"
        );
        calls.push(args.to_vec());
        outputs.next().context("unexpected scheduler call")
    })?;

    assert_eq!(calls, [query_args.clone(), delete_args, query_args]);
    assert!(!prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_structural_xml_ignores_commented_matching_action(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let xml = format!(
        "<?xml version=\"1.0\"?>{}",
        scheduler_manifest_xml(&fixture.codex, &prepared.path)?
            .replace("<Actions>", "<!-- <Actions>")
            .replace("</Actions>", "</Actions> -->")
    );
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let mut outputs = [
        SchedulerOutput::for_test(0, &xml, ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
    ]
    .into_iter();
    let mut calls = 0;

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
        calls += 1;
        outputs.next().context("unexpected scheduler call")
    })?;

    assert_eq!(calls, 3);
    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_structural_xml_rejects_malformed_unrelated_nesting(
) -> anyhow::Result<()> {
    for wrap in [
        "<Settings><Unclosed></Settings>{actions}",
        "<Unclosed>{actions}",
    ] {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let complete = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
        let actions = complete
            .strip_prefix("<Task>")
            .and_then(|xml| xml.strip_suffix("</Task>"))
            .context("test scheduler XML Task wrapper")?;
        let xml = format!("<Task>{}</Task>", wrap.replace("{actions}", actions));
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let mut calls = 0;

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            calls += 1;
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, 3);
        assert!(
            prepared.path.exists(),
            "wrapper {wrap:?} authorized cleanup"
        );
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_quarantine_revalidates_the_claimed_entry_before_removal(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let loaded = load_scheduler_manifest(&prepared.path)?;
    let expected_bytes = serde_json::to_vec(&loaded.manifest)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let quarantine = prepared.path.with_file_name("swap-test.quarantine");
    std::fs::write(&prepared.path, b"replacement")?;

    let error = claim_and_remove_scheduler_manifest_under_lock(
        &prepared.path,
        &quarantine,
        &expected_bytes,
        &fixture.repo,
        &project_id,
    )
    .expect_err("a replacement must be retained after the atomic claim")
    .to_string();

    assert!(!prepared.path.exists());
    assert!(quarantine.exists());
    assert_eq!(std::fs::read(&quarantine)?, b"replacement");
    assert!(error.contains("retain quarantine"));
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_malformed_query_retains_artifacts_but_removes_task(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let mut outputs = [
        SchedulerOutput::for_test(0, "<Task><Exec", ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
    ]
    .into_iter();
    let mut calls = 0;

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
        calls += 1;
        outputs.next().context("unexpected scheduler call")
    })?;

    assert_eq!(calls, 3);
    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_truncated_outer_xml_retains_manifest_but_removes_task(
) -> anyhow::Result<()> {
    for ending in ["", "</Actions>", "</Task>"] {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
        let complete = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
        let body = complete
            .strip_suffix("</Actions></Task>")
            .context("test scheduler XML suffix")?;
        let xml = format!("{body}{ending}");
        let project_id = edda_store::project_id_for_root(&fixture.repo);
        let mut outputs = [
            SchedulerOutput::for_test(0, &xml, ""),
            SchedulerOutput::for_test(0, "", ""),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let mut calls = 0;

        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
            calls += 1;
            outputs.next().context("unexpected scheduler call")
        })?;

        assert_eq!(calls, 3);
        assert!(
            prepared.path.exists(),
            "ending {ending:?} authorized cleanup"
        );
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_untrusted_manifest_does_not_block_task_removal(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let mut wrong_project = prepared.manifest.clone();
    wrong_project.project_id = "0".repeat(32);
    let untrusted =
        write_scheduler_manifest_candidate(&fixture.store, &serde_json::to_vec(&wrong_project)?)?;
    let xml = scheduler_manifest_xml(&fixture.codex, &untrusted)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let mut outputs = [
        SchedulerOutput::for_test(0, &xml, ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
    ]
    .into_iter();
    let mut calls = 0;

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
        calls += 1;
        outputs.next().context("unexpected scheduler call")
    })?;

    assert_eq!(calls, 3);
    assert!(untrusted.exists());
    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_never_sweeps_unproven_artifacts() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let (_, query_args, _) = windows_scheduler_management_args(&project_id)?;
    let mut calls = Vec::new();

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |args| {
        calls.push(args.to_vec());
        Ok(SchedulerOutput::for_test(
            MISSING_TASK_HRESULT,
            "",
            "missing",
        ))
    })?;

    assert_eq!(calls, [query_args]);
    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_missing_codex_retains_manifest_without_blocking_task_removal(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let xml = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
    std::fs::remove_file(&fixture.codex)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let mut outputs = [
        SchedulerOutput::for_test(0, &xml, ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
    ]
    .into_iter();

    uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
        outputs.next().context("unexpected scheduler call")
    })?;

    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_delete_race_accepts_only_missing_hresult() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let xml = "<Task><Actions /></Task>";
    for (delete_code, succeeds) in [(MISSING_TASK_HRESULT, true), (0x8007_0005, false)] {
        let mut outputs = [
            SchedulerOutput::for_test(0, xml, ""),
            SchedulerOutput::for_test(delete_code, "", "delete detail"),
            SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing"),
        ]
        .into_iter();
        let result =
            uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| {
                outputs.next().context("unexpected scheduler call")
            });
        assert_eq!(result.is_ok(), succeeds, "delete code 0x{delete_code:08x}");
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_uninstall_post_delete_uncertainty_retains_manifest() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    let project_id = edda_store::project_id_for_root(&fixture.repo);
    let xml = scheduler_manifest_xml(&fixture.codex, &prepared.path)?;
    let mut outputs = [
        SchedulerOutput::for_test(0, &xml, ""),
        SchedulerOutput::for_test(0, "", ""),
        SchedulerOutput::for_test(0, &xml, "still present"),
    ]
    .into_iter();

    assert!(
        uninstall_scheduler_task_with(&fixture.repo, &fixture.codex, &project_id, |_| outputs
            .next()
            .context("unexpected scheduler call"),)
        .is_err()
    );
    assert!(prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_windows_absolute_paths_are_host_neutral() -> anyhow::Result<()> {
    for path in [
        r"C:\edda\edda.exe",
        "C:/edda/edda.exe",
        r"\\server\share\edda.exe",
        r"\\?\C:\edda\edda.exe",
        r"\\?\UNC\server\share\edda.exe",
    ] {
        assert!(windows_path_is_absolute(Path::new(path))?, "{path}");
    }
    for path in [
        "edda.exe",
        r"C:edda.exe",
        r"\edda.exe",
        r"\\server",
        r"\\?\C:edda.exe",
        r"\\?\UNC\server",
    ] {
        assert!(!windows_path_is_absolute(Path::new(path))?, "{path}");
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_renderer_rejects_ambiguous_inputs() {
    let executable = Path::new(r"C:\edda\edda.exe");
    let manifest = Path::new(r"C:\manifest.json");
    for id in [
        "0123456789abcdef0123456789abcde",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcde*",
    ] {
        assert!(windows_scheduler_spec(executable, manifest, id).is_err());
    }
    assert!(windows_scheduler_spec(
        executable,
        Path::new("C:\\manifest\"quoted.json"),
        "0123456789abcdef0123456789abcdef",
    )
    .is_err());
    assert!(windows_scheduler_spec(
        Path::new("edda.exe"),
        manifest,
        "0123456789abcdef0123456789abcdef",
    )
    .is_err());
    assert!(windows_scheduler_spec(
        executable,
        Path::new("manifest.json"),
        "0123456789abcdef0123456789abcdef",
    )
    .is_err());
}

#[cfg(windows)]
#[test]
pub(super) fn scheduler_renderer_rejects_non_unicode_paths() {
    use std::os::windows::ffi::OsStringExt;

    let invalid = std::ffi::OsString::from_wide(&[b'C' as u16, b':' as u16, 0xd800]);
    assert!(windows_scheduler_spec(
        Path::new(r"C:\edda\edda.exe"),
        Path::new(&invalid),
        "0123456789abcdef0123456789abcdef",
    )
    .is_err());
}

#[test]
pub(super) fn scheduler_query_classifier_accepts_only_success_and_verified_missing_hresult() {
    let present = SchedulerOutput::for_test(0, "xml", "");
    assert_eq!(
        classify_scheduler_query(&present).expect("present"),
        SchedulerTaskState::Present
    );
    let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing");
    assert_eq!(
        classify_scheduler_query(&missing).expect("missing"),
        SchedulerTaskState::Missing
    );
    for code in [5, 0x8007_0005, 0xdead_beef] {
        let error = classify_scheduler_query(&SchedulerOutput::for_test(code, "", "failure"))
            .expect_err("non-missing failures remain errors")
            .to_string();
        assert!(error.contains(&format!("0x{code:08x}")));
        assert!(error.contains(&(code as i32).to_string()));
    }
}

#[test]
pub(super) fn scheduler_query_rejects_truncated_xml_output() {
    let output =
        SchedulerOutput::for_test_with_lengths(0, "<Task />", "", SCHEDULER_OUTPUT_LIMIT + 1, 0);
    let error = output
        .xml()
        .expect_err("truncated Query XML must be rejected")
        .to_string();
    assert!(error.contains(&(SCHEDULER_OUTPUT_LIMIT + 1).to_string()));
    assert!(error.contains(&SCHEDULER_OUTPUT_LIMIT.to_string()));
    assert!(manifest_cleanup_decision(
            &output,
            Path::new(r"C:\edda\edda.exe"),
            Path::new(
                r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json"
            ),
        )
        .is_err());
}

#[test]
pub(super) fn scheduler_query_decodes_raw_utf16_xml_with_non_ascii_paths() -> anyhow::Result<()> {
    let executable = Path::new(r"C:\工具\Edda\edda.exe");
    let manifest = Path::new(
        r"C:\儲存\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?><Task><Actions><Exec><Command>C:\工具\Edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\儲存\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec></Actions></Task>"#;

    let utf8_xml = xml.replace("UTF-16", "UTF-8");
    for bytes in [
        utf8_xml.as_bytes().to_vec(),
        [b"\xef\xbb\xbf", utf8_xml.as_bytes()].concat(),
    ] {
        let output = SchedulerOutput::for_test_bytes(0, &bytes, b"");
        let decoded = output.xml()?;
        assert!(scheduler_query_references_manifest(
            decoded.as_ref(),
            executable,
            manifest,
        )?);
    }
    for (little_endian, bom) in [(true, true), (false, true), (true, false), (false, false)] {
        let bytes = scheduler_xml_utf16_bytes(xml, little_endian, bom);
        let output = SchedulerOutput::for_test_bytes(0, &bytes, b"");
        let decoded = output.xml()?;
        assert!(scheduler_query_references_manifest(
            decoded.as_ref(),
            executable,
            manifest,
        )?);
        assert_eq!(
            manifest_cleanup_decision(&output, executable, manifest)?,
            ManifestCleanupDecision::Retain
        );
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_query_rejects_malformed_raw_xml_but_keeps_diagnostics_lossy() {
    for bytes in [
        vec![0xff, 0xfe, b'<'],
        vec![0xff, 0xfe, 0x00, 0xd8],
        vec![0x00, 0x00, 0xfe, 0xff],
        vec![0x80, 0x81],
    ] {
        assert!(SchedulerOutput::for_test_bytes(0, &bytes, b"")
            .xml()
            .is_err());
    }

    let overflow = SchedulerOutput::for_test_bytes_with_stdout_len(
        0,
        b"<Task />",
        b"",
        SCHEDULER_OUTPUT_LIMIT + 1,
    );
    assert!(overflow.xml().is_err());

    let localized = SchedulerOutput::for_test_bytes(MISSING_TASK_HRESULT, b"", &[0xff]);
    assert_eq!(
        classify_scheduler_query(&localized).expect("non-XML diagnostics stay lossy"),
        SchedulerTaskState::Missing
    );
    assert!(localized.description().contains('\u{fffd}'));
}

#[test]
pub(super) fn scheduler_expected_state_mismatch_preserves_bounded_process_output() {
    let task_name = "Edda-Reconcile-0123456789abcdef0123456789abcdef";
    let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing detail");
    let install_error = require_scheduler_state(
        &missing,
        SchedulerTaskState::Present,
        "post-Create Query",
        task_name,
    )
    .expect_err("Create verification must reject missing")
    .to_string();
    assert!(install_error.contains("post-Create Query"));
    assert!(install_error.contains(task_name));
    assert!(install_error.contains("0x80070002"));
    assert!(install_error.contains("missing detail"));

    let present = SchedulerOutput::for_test(0, "present xml", "");
    let uninstall_error = require_scheduler_state(
        &present,
        SchedulerTaskState::Missing,
        "post-Delete Query",
        task_name,
    )
    .expect_err("Delete verification must reject present")
    .to_string();
    assert!(uninstall_error.contains("post-Delete Query"));
    assert!(uninstall_error.contains(task_name));
    assert!(uninstall_error.contains("0x00000000"));
    assert!(uninstall_error.contains("present xml"));
}

#[cfg(not(windows))]
#[test]
pub(super) fn scheduler_lifecycle_is_explicitly_unsupported_off_windows() {
    let config = scheduler_config("/tmp/codex.exe");
    let error = scheduler_lifecycle(Path::new("/tmp/repo"), Some(&config))
        .expect_err("non-Windows scheduler must fail")
        .to_string();
    assert!(error.contains("supported only on Windows"));
}

#[test]
pub(super) fn scheduler_repo_reentry_requires_absolute_existing_path_and_resolves_main_worktree(
) -> anyhow::Result<()> {
    assert!(canonical_main_repo(Path::new("relative/repo")).is_err());
    let dir = tempfile::tempdir()?;
    assert!(canonical_main_repo(&dir.path().join("missing")).is_err());

    let parent = dir.path().join("parent git");
    std::fs::create_dir(&parent)?;
    init_git(&parent)?;
    // GH-646: bounded walk anchored at the fixture root. The unbounded
    // production walk would climb out of the tempdir into $HOME's
    // coordination workspace and the `is_err` premise below would be
    // environment-dependent, not fixture-established.
    assert!(canonical_main_repo_bounded(&parent, dir.path()).is_err());
    let nested = parent.join("nested edda");
    std::fs::create_dir(&nested)?;
    Ledger::ensure_initialized(&nested)?;
    assert_eq!(
        canonical_main_repo_bounded(&nested, dir.path())?,
        nested.canonicalize()?
    );

    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git").join("worktrees").join("scheduler"))?;
    Ledger::ensure_initialized(&repo)?;
    let worktree = dir.path().join("linked worktree");
    std::fs::create_dir_all(&worktree)?;
    let gitdir = repo.join(".git").join("worktrees").join("scheduler");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}", gitdir.canonicalize()?.display()),
    )?;

    assert_eq!(
        canonical_main_repo_bounded(&worktree, dir.path())?,
        repo.canonicalize()?
    );
    assert_eq!(
        edda_store::project_id(&worktree),
        edda_store::project_id(&repo)
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_reentry_runs_against_its_repo_from_an_unrelated_root(
) -> anyhow::Result<()> {
    let mut fixture = scheduler_manifest_fixture()?;
    fixture.config.max_workers = 0;
    fixture.config.max_attempts = 1;
    let unrelated = fixture._root.path().join("unrelated cwd");
    std::fs::create_dir(&unrelated)?;
    let ledger = Ledger::open(&fixture.repo)?;
    create_task(&ledger, 91, &["src/scheduled.rs".into()])?;
    append_started(&ledger, 91, 1, 1)?;
    ledger.upsert_task_lease(&lease(91, 1, "2026-08-16T00:00:00Z"))?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;

    run(
        &unrelated,
        ReconcileArgs {
            max_workers: 3,
            max_attempts: 3,
            lease_ttl_s: 300,
            codex_bin: None,
            install_scheduler: false,
            uninstall_scheduler: false,
            repo: None,
            run_task: None,
            attempt: None,
            scheduler_manifest: Some(prepared.path),
        },
    )?;

    let view = ledger.task_views()?.remove(0);
    assert_eq!(view.status, TaskStatus::Failed);
    assert_eq!(view.failure_reason.as_deref(), Some("retry-cap-exhausted"));
    assert!(!unrelated.join(".edda").exists());
    Ok(())
}
