use super::*;

#[test]
pub(super) fn scheduler_manifest_is_canonical_content_addressed_and_strict() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let first = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let second = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.path, second.path);
    assert!(first.path.ends_with(format!("{}.json", first.digest)));
    assert!(!first.bytes.ends_with(b"\n"));
    assert_eq!(first.manifest.schema_version, 1);
    assert_eq!(
        first.manifest.project_id,
        edda_store::project_id_for_root(&fixture.repo)
    );
    edda_store::write_atomic(&first.path, &first.bytes)?;
    let loaded = load_scheduler_manifest(&first.path)?;
    assert_eq!(loaded.manifest, first.manifest);
    assert_eq!(loaded.repo, fixture.repo);
    assert_eq!(loaded.config.codex_bin, fixture.codex);
    assert_eq!(loaded.config.max_workers, fixture.config.max_workers);
    assert_eq!(loaded.config.max_attempts, fixture.config.max_attempts);
    assert_eq!(loaded.config.lease_ttl_s, fixture.config.lease_ttl_s);
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_changed_config_changes_digest() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let first = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let mut changed = fixture.config.clone();
    changed.max_workers += 1;
    let second = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &changed)?;

    assert_ne!(first.bytes, second.bytes);
    assert_ne!(first.digest, second.digest);
    assert_ne!(first.path, second.path);
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_publish_reuses_identical_bytes_without_replacing(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

    assert!(publish_scheduler_manifest(&prepared)?);
    assert!(!publish_scheduler_manifest(&prepared)?);
    assert_eq!(std::fs::read(&prepared.path)?, prepared.bytes);

    std::fs::write(&prepared.path, b"different")?;
    assert!(publish_scheduler_manifest(&prepared).is_err());
    assert_eq!(std::fs::read(&prepared.path)?, b"different");
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_first_publish_from_absent_store_root_is_loadable(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    std::fs::remove_dir_all(&fixture.store)?;
    assert!(!fixture.store.exists());

    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    assert!(publish_scheduler_manifest(&prepared)?);
    assert_eq!(
        prepared.path.parent(),
        Some(scheduler_manifest_directory(&fixture.store, true)?.as_path())
    );
    assert_eq!(
        load_scheduler_manifest(&prepared.path)?.manifest,
        prepared.manifest
    );
    assert!(!publish_scheduler_manifest(&prepared)?);
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_atomic_link_never_replaces_a_racer() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let directory = prepared.path.parent().context("manifest directory")?;
    std::fs::create_dir_all(directory)?;
    let temp = directory.join("race.tmp");
    edda_store::write_atomic(&temp, &prepared.bytes)?;
    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;

    assert!(!link_scheduler_manifest_noclobber(&temp, &prepared)?);
    assert!(!temp.exists());
    assert_eq!(std::fs::read(&prepared.path)?, prepared.bytes);

    std::fs::write(&prepared.path, b"racer bytes")?;
    edda_store::write_atomic(&temp, &prepared.bytes)?;
    assert!(link_scheduler_manifest_noclobber(&temp, &prepared).is_err());
    assert!(!temp.exists());
    assert_eq!(std::fs::read(&prepared.path)?, b"racer bytes");
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_changed_install_retains_prior_artifact() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let old = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    assert!(publish_scheduler_manifest(&old)?);

    let mut changed = fixture.config.clone();
    changed.max_attempts += 1;
    let new = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &changed)?;
    assert_ne!(old.path, new.path);
    assert!(publish_scheduler_manifest(&new)?);
    assert_eq!(std::fs::read(&old.path)?, old.bytes);
    assert_eq!(std::fs::read(&new.path)?, new.bytes);
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_write_failure_precedes_scheduler_cleanup() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    std::fs::create_dir_all(fixture.store.join("scheduler-launch"))?;
    std::fs::write(
        fixture.store.join("scheduler-launch").join("v1"),
        b"blocked",
    )?;

    assert!(publish_scheduler_manifest(&prepared).is_err());
    assert!(!prepared.path.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_publish_validates_containment_before_mutation(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let mut prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let outside = fixture._root.path().join("outside-store");
    prepared.path = outside
        .join("scheduler-launch")
        .join("v1")
        .join(format!("{}.json", prepared.digest));

    assert!(publish_scheduler_manifest(&prepared).is_err());
    assert!(!outside.exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_query_xml_requires_exact_escaped_command_and_arguments(
) -> anyhow::Result<()> {
    let executable = Path::new(r"C:\Program Files\Edda & Co\edda.exe");
    let manifest = Path::new(r"C:\Store & State\scheduler-launch\v1\expected.json");
    let xml = r#"<Task><Actions><Exec><Command>C:\Program Files\Edda &amp; Co\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\Store &amp; State\scheduler-launch\v1\expected.json&quot;</Arguments></Exec></Actions></Task>"#;
    assert!(scheduler_query_references_manifest(
        xml, executable, manifest
    )?);

    let wrong_arguments = xml.replace("expected.json&quot;", "expected.json&quot; --extra");
    assert!(!scheduler_query_references_manifest(
        &wrong_arguments,
        executable,
        manifest
    )?);
    let wrong_command = xml.replace("edda.exe</Command>", "other.exe</Command>");
    assert!(!scheduler_query_references_manifest(
        &wrong_command,
        executable,
        manifest
    )?);
    Ok(())
}

#[test]
pub(super) fn scheduler_query_accepts_exact_windows_quoted_executable_only() -> anyhow::Result<()> {
    let executable = Path::new(r"\\?\C:\Program Files\Edda\edda.exe");
    let manifest = Path::new(r"C:\Store\scheduler-launch\v1\expected.json");
    let arguments =
        r#"reconcile --scheduler-manifest &quot;C:\Store\scheduler-launch\v1\expected.json&quot;"#;
    let xml = |command: &str| {
        format!(
                "<Task><Actions><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments></Exec></Actions></Task>"
            )
    };

    let quoted = xml(r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#);
    assert!(scheduler_query_references_manifest(
        &quoted, executable, manifest
    )?);
    assert_eq!(
        recover_scheduler_manifest_candidate(&quoted, executable)?,
        Some(manifest.to_path_buf())
    );
    assert_eq!(
        manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &quoted, ""),
            executable,
            manifest,
        )?,
        ManifestCleanupDecision::Retain
    );

    let literal_quoted = xml(r#""\\?\C:\Program Files\Edda\edda.exe""#);
    assert!(scheduler_query_references_manifest(
        &literal_quoted,
        executable,
        manifest,
    )?);

    for rejected in [
        r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; --extra"#,
        r#"&quot;\\?\C:\Program Files\Edda\edda.exe"#,
        r#"&quot;&quot;\\?\C:\Program Files\Edda\edda.exe&quot;&quot;"#,
        r#" &quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#,
        r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; "#,
        r#"&quot;C:\other\edda.exe&quot;"#,
        r#"cmd.exe /c &quot;\\?\C:\Program Files\Edda\edda.exe&quot;"#,
        r#"&quot;\\?\C:\Program Files\Edda\edda.exe&quot; &quot;C:\other.exe&quot;"#,
    ] {
        let rejected = xml(rejected);
        assert!(!scheduler_query_references_manifest(
            &rejected, executable, manifest
        )?);
        assert_eq!(
            recover_scheduler_manifest_candidate(&rejected, executable)?,
            None
        );
        assert!(manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &rejected, ""),
            executable,
            manifest,
        )
        .is_err());
    }

    let entity_trick = xml(r#"&#34;\\?\C:\Program Files\Edda\edda.exe&#34;"#);
    assert!(scheduler_query_references_manifest(&entity_trick, executable, manifest).is_err());
    assert!(recover_scheduler_manifest_candidate(&entity_trick, executable).is_err());
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &entity_trick, ""),
        executable,
        manifest,
    )
    .is_err());

    let extra_exec = quoted.replace(
        "</Actions>",
        "<Exec><Command>cmd.exe</Command><Arguments>/c exit</Arguments></Exec></Actions>",
    );
    assert!(!scheduler_query_references_manifest(
        &extra_exec,
        executable,
        manifest,
    )?);
    assert_eq!(
        recover_scheduler_manifest_candidate(&extra_exec, executable)?,
        None
    );
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &extra_exec, ""),
        executable,
        manifest,
    )
    .is_err());

    let com_handler = quoted.replace(
            "</Actions>",
            "<ComHandler><ClassId>00000000-0000-0000-0000-000000000000</ClassId><Data>ignored</Data></ComHandler></Actions>",
        );
    assert!(scheduler_query_references_manifest(&com_handler, executable, manifest).is_err());
    assert!(recover_scheduler_manifest_candidate(&com_handler, executable).is_err());
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &com_handler, ""),
        executable,
        manifest,
    )
    .is_err());

    let harmless_comment = quoted.replace("<Exec>", "<!-- harmless --><Exec>");
    assert!(scheduler_query_references_manifest(
        &harmless_comment,
        executable,
        manifest,
    )?);
    assert_eq!(
        recover_scheduler_manifest_candidate(&harmless_comment, executable)?,
        Some(manifest.to_path_buf())
    );

    let commented_match = quoted.replace("<Exec>", "<!-- <Exec>").replace(
        "</Exec>",
        "</Exec> --><Exec><Command>cmd.exe</Command><Arguments>/c exit</Arguments></Exec>",
    );
    assert!(scheduler_query_references_manifest(&commented_match, executable, manifest).is_err());
    assert!(recover_scheduler_manifest_candidate(&commented_match, executable).is_err());
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &commented_match, ""),
        executable,
        manifest,
    )
    .is_err());
    Ok(())
}

#[test]
pub(super) fn windows_manifest_path_components_are_host_neutral() -> anyhow::Result<()> {
    assert_eq!(
        windows_manifest_path_components(Path::new(r"C:\store\scheduler-launch\v1\manifest.json"))?,
        (r"C:\store\scheduler-launch\v1", "manifest.json")
    );
    assert_eq!(
        windows_manifest_path_components(Path::new(
            r"\\?\C:\store\scheduler-launch\v1\manifest.json"
        ))?,
        (r"\\?\C:\store\scheduler-launch\v1", "manifest.json")
    );
    assert_eq!(
        windows_manifest_path_components(Path::new(r"\\?\UNC\server\share\v1\manifest.json"))?,
        (r"\\?\UNC\server\share\v1", "manifest.json")
    );
    assert_eq!(
        windows_manifest_path_components(Path::new("C:/store/scheduler-launch/v1/manifest.json"))?,
        ("C:/store/scheduler-launch/v1", "manifest.json")
    );
    assert!(windows_manifest_path_components(Path::new(r"C:\")).is_err());
    Ok(())
}

#[test]
pub(super) fn scheduler_query_ignores_execution_time_limit_setting() -> anyhow::Result<()> {
    let executable = Path::new(r"C:\Program Files\Edda\edda.exe");
    let manifest = Path::new(
        r"C:\Store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Settings>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>C:\Program Files\Edda\edda.exe</Command>
      <Arguments>reconcile --scheduler-manifest &quot;C:\Store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments>
    </Exec>
  </Actions>
</Task>"#;

    assert!(scheduler_query_references_manifest(
        xml, executable, manifest
    )?);
    assert_eq!(
        recover_scheduler_manifest_candidate(xml, executable)?,
        Some(manifest.to_path_buf())
    );
    assert_eq!(
        manifest_cleanup_decision(&SchedulerOutput::for_test(0, xml, ""), executable, manifest,)?,
        ManifestCleanupDecision::Retain
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_query_scans_every_exec_before_deciding() {
    let executable = Path::new(r"C:\edda\edda.exe");
    let expected = Path::new(
        r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let expected_xml = r#"<Task><Actions><Exec><Command>C:\edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec><Exec><Command>truncated"#;
    assert!(scheduler_query_references_manifest(expected_xml, executable, expected).is_err());
    let cut_open = format!(
        "{}<Exec",
        expected_xml.trim_end_matches("<Exec><Command>truncated")
    );
    assert!(scheduler_query_references_manifest(&cut_open, executable, expected).is_err());
    let self_closing = format!(
        "{}<Exec/>",
        expected_xml.trim_end_matches("<Exec><Command>truncated")
    );
    assert!(scheduler_query_references_manifest(&self_closing, executable, expected).is_err());

    let different_xml = expected_xml.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
    );
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &different_xml, ""),
        executable,
        expected,
    )
    .is_err());
    let different_cut_open = cut_open.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
    );
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &different_cut_open, ""),
        executable,
        expected,
    )
    .is_err());
}

#[test]
pub(super) fn scheduler_query_compares_decoded_xml_element_values() -> anyhow::Result<()> {
    let executable = Path::new(r"C:\O'Brien & Sons\edda.exe");
    let expected = Path::new(
        r"C:\Store & State\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let literal_xml = r#"<Task><Actions><Exec><Command>C:\O'Brien &amp; Sons\edda.exe</Command><Arguments>reconcile --scheduler-manifest "C:\Store &amp; State\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json"</Arguments></Exec></Actions></Task>"#;
    assert!(scheduler_query_references_manifest(
        literal_xml,
        executable,
        expected,
    )?);

    let named_xml = literal_xml
        .replace("O'Brien", "O&apos;Brien")
        .replace('"', "&quot;");
    assert!(scheduler_query_references_manifest(
        &named_xml, executable, expected,
    )?);

    let unknown_entity = literal_xml.replace("&amp;", "&unknown;");
    assert!(scheduler_query_references_manifest(&unknown_entity, executable, expected).is_err());

    let different = literal_xml.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
    );
    assert_eq!(
        manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &different, ""),
            executable,
            expected,
        )?,
        ManifestCleanupDecision::RemoveNewArtifact
    );
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_cleanup_requires_proved_non_reference() -> anyhow::Result<()> {
    let executable = Path::new(r"C:\edda\edda.exe");
    let expected = Path::new(
        r"C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let missing = SchedulerOutput::for_test(MISSING_TASK_HRESULT, "", "missing");
    assert_eq!(
        manifest_cleanup_decision(&missing, executable, expected)?,
        ManifestCleanupDecision::RemoveNewArtifact
    );

    let expected_xml = r#"<Task><Actions><Exec><Command>C:\edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json&quot;</Arguments></Exec></Actions></Task>"#;
    assert_eq!(
        manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, expected_xml, ""),
            executable,
            expected,
        )?,
        ManifestCleanupDecision::Retain
    );

    let previous_xml = expected_xml.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json",
    );
    assert_eq!(
        manifest_cleanup_decision(
            &SchedulerOutput::for_test(0, &previous_xml, ""),
            executable,
            expected,
        )?,
        ManifestCleanupDecision::RemoveNewArtifact
    );

    let two_previous_actions = previous_xml.replace(
            "</Actions>",
            r#"<Exec><Command>C:\edda\edda.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc.json&quot;</Arguments></Exec></Actions>"#,
        );
    assert!(manifest_cleanup_decision(
        &SchedulerOutput::for_test(0, &two_previous_actions, ""),
        executable,
        expected,
    )
    .is_err());

    let aliased_expected_xml = expected_xml.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "&#97;aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
    );
    let non_content_addressed_xml = expected_xml.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json",
        "previous.json",
    );
    let different_directory_xml = previous_xml.replace(
        r"C:\store\scheduler-launch\v1",
        r"C:\other\scheduler-launch\v1",
    );
    for uncertain in [
        SchedulerOutput::for_test(5, "", "access denied"),
        SchedulerOutput::for_test(0, "<Task />", ""),
        SchedulerOutput::for_test(0, &aliased_expected_xml, ""),
        SchedulerOutput::for_test(0, &non_content_addressed_xml, ""),
        SchedulerOutput::for_test(0, &different_directory_xml, ""),
        SchedulerOutput::for_test(
            0,
            r#"<Task><Actions><Exec><Command>C:\other.exe</Command><Arguments>reconcile --scheduler-manifest &quot;C:\store\scheduler-launch\v1\bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json&quot;</Arguments></Exec></Actions></Task>"#,
            "",
        ),
    ] {
        assert!(manifest_cleanup_decision(&uncertain, executable, expected).is_err());
    }
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_cleanup_failure_retains_original_error_first() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let manifest = root
        .path()
        .join("scheduler-launch")
        .join("v1")
        .join(format!("{}.json", "a".repeat(64)));
    std::fs::create_dir_all(&manifest)?;

    let cleanup = remove_unreferenced_scheduler_manifest(&manifest);
    let combined = format!("scheduler Create failed; {cleanup}");
    assert!(manifest.is_dir());
    assert!(cleanup.contains("retained"));
    assert!(combined.starts_with("scheduler Create failed;"));
    Ok(())
}

#[test]
pub(super) fn scheduler_preflight_failure_creates_no_manifest_directory() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let _prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let rejected_path = manifest_path_for_task_run_utf16_len(262);

    assert!(render_scheduler_task_run(
        Path::new(r"C:\e.exe"),
        &rejected_path,
        "Edda-Reconcile-0123456789abcdef0123456789abcdef",
    )
    .is_err());
    assert!(!fixture.store.join("scheduler-launch").exists());
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_rejects_unknown_duplicate_and_noncanonical_json(
) -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

    let mut unknown_field: serde_json::Value = serde_json::from_slice(&prepared.bytes)?;
    unknown_field
        .as_object_mut()
        .expect("manifest object")
        .insert("extra".into(), true.into());
    let path =
        write_scheduler_manifest_candidate(&fixture.store, &serde_json::to_vec(&unknown_field)?)?;
    assert!(load_scheduler_manifest(&path).is_err());

    let mut unknown_version = prepared.manifest.clone();
    unknown_version.schema_version = 2;
    let path =
        write_scheduler_manifest_candidate(&fixture.store, &serde_json::to_vec(&unknown_version)?)?;
    assert!(load_scheduler_manifest(&path).is_err());

    let canonical = String::from_utf8(prepared.bytes.clone())?;
    let duplicate = canonical.replacen('{', r#"{"schema_version":1,"#, 1);
    let path = write_scheduler_manifest_candidate(&fixture.store, duplicate.as_bytes())?;
    assert!(load_scheduler_manifest(&path).is_err());

    let mut noncanonical = prepared.bytes;
    noncanonical.push(b'\n');
    let path = write_scheduler_manifest_candidate(&fixture.store, &noncanonical)?;
    assert!(load_scheduler_manifest(&path).is_err());
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_rejects_oversize_and_digest_mismatch() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;

    let oversized = vec![b' '; 16 * 1024 + 1];
    let path = write_scheduler_manifest_candidate(&fixture.store, &oversized)?;
    assert!(load_scheduler_manifest(&path).is_err());

    let mismatch = prepared
        .path
        .with_file_name(format!("{}.json", "0".repeat(64)));
    edda_store::write_atomic(&mismatch, &prepared.bytes)?;
    assert!(load_scheduler_manifest(&mismatch).is_err());
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_revalidates_project_repo_and_codex() -> anyhow::Result<()> {
    let fixture = scheduler_manifest_fixture()?;
    assert!(prepare_scheduler_manifest(
        &fixture.store,
        &fixture.repo.join("missing"),
        &fixture.config
    )
    .is_err());
    let mut invalid_codex = fixture.config.clone();
    invalid_codex.codex_bin = fixture.repo.join("codex.cmd");
    assert!(prepare_scheduler_manifest(&fixture.store, &fixture.repo, &invalid_codex).is_err());

    let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
    let mut wrong_project = prepared.manifest.clone();
    wrong_project.project_id = "0".repeat(32);
    let path =
        write_scheduler_manifest_candidate(&fixture.store, &serde_json::to_vec(&wrong_project)?)?;
    assert!(load_scheduler_manifest(&path).is_err());

    edda_store::write_atomic(&prepared.path, &prepared.bytes)?;
    std::fs::remove_file(&fixture.codex)?;
    assert!(load_scheduler_manifest(&prepared.path).is_err());
    Ok(())
}

#[test]
pub(super) fn scheduler_manifest_rejects_store_root_and_reparse_escape() -> anyhow::Result<()> {
    {
        let fixture = scheduler_manifest_fixture()?;
        let prepared = prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config)?;
        std::fs::create_dir_all(fixture.store.join("scheduler-launch").join("v1"))?;
        let outside = fixture._root.path().join("outside");
        let escaped = outside.join(format!("{}.json", prepared.digest));
        edda_store::write_atomic(&escaped, &prepared.bytes)?;
        assert!(load_scheduler_manifest(&escaped).is_err());
    }

    let fixture = scheduler_manifest_fixture()?;
    let launch = fixture.store.join("scheduler-launch");
    let reparse_target = fixture._root.path().join("reparse-target");
    std::fs::create_dir(&reparse_target)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&reparse_target, &launch)?;
    #[cfg(windows)]
    if let Err(error) = std::os::windows::fs::symlink_dir(&reparse_target, &launch) {
        anyhow::ensure!(
            error.raw_os_error() == Some(1314),
            "create scheduler manifest directory symlink: {error}"
        );
        let output = Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&launch)
            .arg(&reparse_target)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "create scheduler manifest directory junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(prepare_scheduler_manifest(&fixture.store, &fixture.repo, &fixture.config).is_err());
    Ok(())
}

#[test]
pub(super) fn scheduler_cli_parses_manifest_reentry_and_rejects_conflicting_modes() {
    use clap::CommandFactory;

    let parsed = SchedulerCli::try_parse_from([
        "test",
        "--repo",
        r"C:\ai projects\sample",
        "--install-scheduler",
    ])
    .expect("scheduler arguments");
    assert_eq!(
        parsed.args.repo.as_deref(),
        Some(Path::new(r"C:\ai projects\sample"))
    );
    assert!(parsed.args.install_scheduler);
    assert!(
        SchedulerCli::try_parse_from(["test", "--install-scheduler", "--uninstall-scheduler"])
            .is_err()
    );
    assert!(SchedulerCli::try_parse_from([
        "test",
        "--install-scheduler",
        "--run-task",
        "7",
        "--attempt",
        "1"
    ])
    .is_err());

    let manifest = r"C:\store\scheduler-launch\v1\0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json";
    let reentry = SchedulerCli::try_parse_from(["test", "--scheduler-manifest", manifest])
        .expect("manifest scheduler re-entry arguments");
    assert_eq!(
        reentry.args.scheduler_manifest.as_deref(),
        Some(Path::new(manifest))
    );
    assert!(!SchedulerCli::command()
        .render_long_help()
        .to_string()
        .contains("--scheduler-manifest"));

    for conflict in [
        &["--install-scheduler"][..],
        &["--uninstall-scheduler"][..],
        &["--repo", r"C:\repo"][..],
        &["--codex-bin", r"C:\codex.exe"][..],
        &["--run-task", "7"][..],
        &["--attempt", "1"][..],
        &["--max-workers", "2"][..],
        &["--max-attempts", "5"][..],
        &["--lease-ttl-s", "17"][..],
    ] {
        let mut args = vec!["test", "--scheduler-manifest", manifest];
        args.extend_from_slice(conflict);
        assert!(SchedulerCli::try_parse_from(args).is_err(), "{conflict:?}");
    }
}

#[test]
pub(super) fn scheduler_codex_config_prefers_cli_then_environment() {
    let _environment = codex_bin_env_guard(r"C:\environment\codex.exe");

    let explicit = SchedulerCli::try_parse_from([
        "test",
        "--install-scheduler",
        "--codex-bin",
        r"C:\explicit\codex.exe",
    ])
    .expect("explicit Codex path");
    assert_eq!(
        ReconcileConfig::from_args(&explicit.args).codex_bin,
        PathBuf::from(r"C:\explicit\codex.exe")
    );

    let inherited = SchedulerCli::try_parse_from(["test", "--install-scheduler"])
        .expect("environment Codex path");
    assert_eq!(
        ReconcileConfig::from_args(&inherited.args).codex_bin,
        PathBuf::from(r"C:\environment\codex.exe")
    );
}

#[test]
pub(super) fn scheduler_codex_environment_guard_restores_after_unwind() {
    let previous = codex_bin_env();
    let result = std::panic::catch_unwind(|| {
        let _environment = codex_bin_env_guard(r"C:\environment\codex.exe");
        panic!("test unwind");
    });

    assert!(result.is_err());
    assert_eq!(codex_bin_env(), previous);
}
