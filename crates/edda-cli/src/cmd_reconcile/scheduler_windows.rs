use anyhow::Context;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::manifest::{
    load_scheduler_manifest, scheduler_manifest_directory, validate_scheduler_manifest,
    SchedulerLaunchManifestV1, SCHEDULER_MANIFEST_TEMP_COUNTER,
};
use super::scheduler_xml::scheduler_direct_exec_values;
use super::SCHEDULER_MANIFEST_MAX_BYTES;

pub(super) const MISSING_TASK_HRESULT: u32 = 0x8007_0002;
pub(super) const SCHEDULER_OUTPUT_LIMIT: usize = 4096;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum SchedulerTaskState {
    Present,
    Missing,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ManifestCleanupDecision {
    RemoveNewArtifact,
    Retain,
}

pub(super) struct SchedulerOutput {
    pub(super) code: u32,
    pub(super) stdout_raw: Vec<u8>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) stdout_bytes: usize,
    pub(super) stderr_bytes: usize,
}

impl SchedulerOutput {
    #[cfg(test)]
    pub(super) fn for_test(code: u32, stdout: &str, stderr: &str) -> Self {
        Self::for_test_with_lengths(code, stdout, stderr, stdout.len(), stderr.len())
    }

    #[cfg(test)]
    pub(super) fn for_test_with_lengths(
        code: u32,
        stdout: &str,
        stderr: &str,
        stdout_bytes: usize,
        stderr_bytes: usize,
    ) -> Self {
        Self::for_test_bytes_with_lengths(
            code,
            stdout.as_bytes(),
            stderr.as_bytes(),
            stdout_bytes,
            stderr_bytes,
        )
    }

    #[cfg(test)]
    pub(super) fn for_test_bytes(code: u32, stdout: &[u8], stderr: &[u8]) -> Self {
        Self::for_test_bytes_with_lengths(code, stdout, stderr, stdout.len(), stderr.len())
    }

    #[cfg(test)]
    pub(super) fn for_test_bytes_with_stdout_len(
        code: u32,
        stdout: &[u8],
        stderr: &[u8],
        stdout_bytes: usize,
    ) -> Self {
        Self::for_test_bytes_with_lengths(code, stdout, stderr, stdout_bytes, stderr.len())
    }

    #[cfg(test)]
    pub(super) fn for_test_bytes_with_lengths(
        code: u32,
        stdout: &[u8],
        stderr: &[u8],
        stdout_bytes: usize,
        stderr_bytes: usize,
    ) -> Self {
        let stdout_raw = stdout[..stdout.len().min(SCHEDULER_OUTPUT_LIMIT)].to_vec();
        Self {
            code,
            stdout: String::from_utf8_lossy(&stdout_raw).into_owned(),
            stdout_raw,
            stderr: String::from_utf8_lossy(&stderr[..stderr.len().min(SCHEDULER_OUTPUT_LIMIT)])
                .into_owned(),
            stdout_bytes,
            stderr_bytes,
        }
    }

    pub(super) fn xml(&self) -> anyhow::Result<Cow<'_, str>> {
        anyhow::ensure!(
            self.stdout_bytes <= SCHEDULER_OUTPUT_LIMIT,
            "scheduler Query XML is {} bytes; maximum bounded output is {}",
            self.stdout_bytes,
            SCHEDULER_OUTPUT_LIMIT
        );
        let bytes = self.stdout_raw.as_slice();
        anyhow::ensure!(
            !bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff])
                && !bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]),
            "scheduler Query XML uses an unsupported UTF-32 encoding"
        );
        let decode_utf16 = |encoded: &[u8], little_endian: bool| -> anyhow::Result<String> {
            anyhow::ensure!(
                encoded.len().is_multiple_of(2),
                "scheduler Query XML contains odd-length UTF-16"
            );
            let units = encoded
                .as_chunks::<2>()
                .0
                .iter()
                .map(|bytes| {
                    if little_endian {
                        u16::from_le_bytes([bytes[0], bytes[1]])
                    } else {
                        u16::from_be_bytes([bytes[0], bytes[1]])
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&units).context("scheduler Query XML contains malformed UTF-16")
        };
        if let Some(encoded) = bytes.strip_prefix(&[0xff, 0xfe]) {
            return Ok(Cow::Owned(decode_utf16(encoded, true)?));
        }
        if let Some(encoded) = bytes.strip_prefix(&[0xfe, 0xff]) {
            return Ok(Cow::Owned(decode_utf16(encoded, false)?));
        }
        if bytes.starts_with(&[0x3c, 0x00, 0x3f, 0x00]) {
            return Ok(Cow::Owned(decode_utf16(bytes, true)?));
        }
        if bytes.starts_with(&[0x00, 0x3c, 0x00, 0x3f]) {
            return Ok(Cow::Owned(decode_utf16(bytes, false)?));
        }
        let utf8 = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        Ok(Cow::Borrowed(
            std::str::from_utf8(utf8).context("scheduler Query XML is not valid UTF-8")?,
        ))
    }

    pub(super) fn description(&self) -> String {
        format!(
            "code=0x{:08x} ({}) stdout_bytes={} stderr_bytes={} stdout={:?} stderr={:?}",
            self.code,
            self.code as i32,
            self.stdout_bytes,
            self.stderr_bytes,
            self.stdout,
            self.stderr
        )
    }
}

pub(super) struct WindowsSchedulerSpec {
    pub(super) task_name: String,
    pub(super) create_args: Vec<String>,
    pub(super) query_args: Vec<String>,
}

pub(super) fn quote_windows_argument(path: &Path) -> anyhow::Result<String> {
    let value = path.to_str().context("scheduler path is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "scheduler path contains an unsupported character"
    );
    let trailing_backslashes = value
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    Ok(format!("\"{value}{}\"", "\\".repeat(trailing_backslashes)))
}

pub(super) fn windows_path_is_absolute(path: &Path) -> anyhow::Result<bool> {
    let value = path.to_str().context("scheduler path is not Unicode")?;
    let bytes = value.as_bytes();
    let separator = |byte| matches!(byte, b'\\' | b'/');
    let drive_rooted = |candidate: &[u8]| {
        candidate.len() >= 3
            && candidate[0].is_ascii_alphabetic()
            && candidate[1] == b':'
            && separator(candidate[2])
    };
    let unc_rooted = |candidate: &str| {
        let mut parts = candidate.split(['\\', '/']);
        parts.next().is_some_and(|part| !part.is_empty())
            && parts.next().is_some_and(|part| !part.is_empty())
    };

    if drive_rooted(bytes) {
        return Ok(true);
    }
    if bytes.len() < 2 || !separator(bytes[0]) || !separator(bytes[1]) {
        return Ok(false);
    }
    if bytes.len() >= 4 && bytes[2] == b'?' && separator(bytes[3]) {
        let rest = &value[4..];
        if drive_rooted(rest.as_bytes()) {
            return Ok(true);
        }
        let rest_bytes = rest.as_bytes();
        return Ok(rest_bytes.len() >= 4
            && rest_bytes[..3].eq_ignore_ascii_case(b"UNC")
            && separator(rest_bytes[3])
            && unc_rooted(&rest[4..]));
    }
    Ok(unc_rooted(&value[2..]))
}

pub(super) fn windows_manifest_path_components(path: &Path) -> anyhow::Result<(&str, &str)> {
    let value = path
        .to_str()
        .context("scheduler manifest path is not Unicode")?;
    let (parent, filename) = value
        .rsplit_once(['\\', '/'])
        .context("scheduler manifest path has no Windows parent")?;
    anyhow::ensure!(
        !parent.is_empty() && !filename.is_empty(),
        "scheduler manifest path has no Windows filename"
    );
    Ok((parent, filename))
}

pub(super) fn windows_scheduler_task_name(project_id: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        project_id.len() == 32
            && project_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "project id must be 32 lowercase hexadecimal characters"
    );
    Ok(format!("Edda-Reconcile-{project_id}"))
}

pub(super) fn windows_scheduler_management_args(
    project_id: &str,
) -> anyhow::Result<(String, Vec<String>, Vec<String>)> {
    let task_name = windows_scheduler_task_name(project_id)?;
    let strings = |items: &[&str]| items.iter().map(|item| (*item).into()).collect();
    let query_args = strings(&["/Query", "/TN", &task_name, "/XML", "/HRESULT"]);
    let delete_args = strings(&["/Delete", "/TN", &task_name, "/F", "/HRESULT"]);
    Ok((task_name, query_args, delete_args))
}

pub(super) fn render_scheduler_task_run(
    exe: &Path,
    manifest_path: &Path,
    task_name: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        windows_path_is_absolute(exe)?,
        "scheduler executable must be absolute"
    );
    anyhow::ensure!(
        windows_path_is_absolute(manifest_path)?,
        "scheduler manifest path must be absolute"
    );
    let task_run = format!(
        "{} reconcile --scheduler-manifest {}",
        quote_windows_argument(exe)?,
        quote_windows_argument(manifest_path)?,
    );
    let units = task_run.encode_utf16().count();
    anyhow::ensure!(
        units <= 261,
        "scheduler task {task_name} /TR is {units} UTF-16 code units; maximum is 261"
    );
    Ok(task_run)
}

pub(super) fn windows_scheduler_spec(
    exe: &Path,
    manifest_path: &Path,
    project_id: &str,
) -> anyhow::Result<WindowsSchedulerSpec> {
    let (task_name, query_args, _) = windows_scheduler_management_args(project_id)?;
    let task_run = render_scheduler_task_run(exe, manifest_path, &task_name)?;
    let strings = |items: &[&str]| items.iter().map(|item| (*item).into()).collect();
    Ok(WindowsSchedulerSpec {
        create_args: strings(&[
            "/Create", "/SC", "MINUTE", "/MO", "1", "/TN", &task_name, "/TR", &task_run, "/RL",
            "LIMITED", "/F", "/HRESULT",
        ]),
        query_args,
        task_name,
    })
}

pub(super) fn scheduler_command_matches_executable(
    command: &str,
    executable: &Path,
) -> anyhow::Result<bool> {
    let expected = executable
        .to_str()
        .context("scheduler executable is not Unicode")?;
    Ok(command == expected || command == quote_windows_argument(executable)?)
}

pub(super) fn scheduler_query_references_manifest(
    xml: &str,
    executable: &Path,
    manifest: &Path,
) -> anyhow::Result<bool> {
    let arguments = format!(
        "reconcile --scheduler-manifest {}",
        quote_windows_argument(manifest)?
    );
    let actions = scheduler_direct_exec_values(xml)?;
    let [(command, actual_arguments)] = actions.as_slice() else {
        return Ok(false);
    };
    Ok(
        scheduler_command_matches_executable(command, executable)?
            && actual_arguments == &arguments,
    )
}

pub(super) fn manifest_cleanup_decision(
    query: &SchedulerOutput,
    executable: &Path,
    expected_manifest: &Path,
) -> anyhow::Result<ManifestCleanupDecision> {
    match classify_scheduler_query(query)? {
        SchedulerTaskState::Missing => return Ok(ManifestCleanupDecision::RemoveNewArtifact),
        SchedulerTaskState::Present => {}
    }
    let xml = query.xml()?;
    let actions = scheduler_direct_exec_values(xml.as_ref())?;
    anyhow::ensure!(
        actions.len() == 1,
        "scheduler Query must contain exactly one direct Exec action: {}",
        query.description()
    );
    let expected_arguments = format!(
        "reconcile --scheduler-manifest {}",
        quote_windows_argument(expected_manifest)?
    );
    if let [(command, arguments)] = actions.as_slice() {
        if scheduler_command_matches_executable(command, executable)?
            && arguments == &expected_arguments
        {
            return Ok(ManifestCleanupDecision::Retain);
        }
    }
    anyhow::ensure!(
        !actions.is_empty(),
        "scheduler Query did not contain an Exec action: {}",
        query.description()
    );
    let (expected_parent, expected_filename) = windows_manifest_path_components(expected_manifest)?;

    for (command, arguments) in actions {
        anyhow::ensure!(
            scheduler_command_matches_executable(&command, executable)?,
            "scheduler Query command did not match the direct Edda executable: {}",
            query.description()
        );
        let path = arguments
            .strip_prefix("reconcile --scheduler-manifest \"")
            .and_then(|value| value.strip_suffix('"'))
            .context("scheduler Query Arguments were not a strict manifest command")?;
        anyhow::ensure!(
            !path.contains('"'),
            "scheduler Query manifest path contains a quote"
        );
        let candidate = Path::new(path);
        let (candidate_parent, candidate_filename) = windows_manifest_path_components(candidate)?;
        let trusted_filename = candidate_filename
            .strip_suffix(".json")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        anyhow::ensure!(
            windows_path_is_absolute(candidate)?
                && candidate_parent == expected_parent
                && candidate_filename != expected_filename
                && trusted_filename,
            "scheduler Query did not prove a different trusted manifest path: {}",
            query.description()
        );
    }
    Ok(ManifestCleanupDecision::RemoveNewArtifact)
}

pub(super) fn remove_unreferenced_scheduler_manifest(path: &Path) -> String {
    let removal = (|| -> anyhow::Result<()> {
        let launch_directory = path
            .parent()
            .and_then(Path::parent)
            .context("scheduler manifest path has no launch directory")?;
        let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
        std::fs::remove_file(path)
            .with_context(|| format!("remove unreferenced scheduler manifest {}", path.display()))
    })();
    match removal {
        Ok(()) => format!(
            "new scheduler manifest removed after exact-task Query proved it unreferenced: {}",
            path.display()
        ),
        Err(error) => format!(
            "new scheduler manifest retained because exact-file cleanup failed for {}: {error:#}",
            path.display()
        ),
    }
}

pub(super) fn recover_scheduler_manifest_candidate(
    xml: &str,
    executable: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let actions = scheduler_direct_exec_values(xml)?;
    let [(command, arguments)] = actions.as_slice() else {
        return Ok(None);
    };
    if !scheduler_command_matches_executable(command, executable)? {
        return Ok(None);
    }
    let Some(value) = arguments
        .strip_prefix("reconcile --scheduler-manifest \"")
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Ok(None);
    };
    let candidate = PathBuf::from(value);
    if !(candidate.is_absolute() || windows_path_is_absolute(&candidate)?)
        || format!(
            "reconcile --scheduler-manifest {}",
            quote_windows_argument(&candidate)?
        ) != *arguments
    {
        return Ok(None);
    }
    Ok(Some(candidate))
}

pub(super) fn claim_and_remove_scheduler_manifest_under_lock(
    path: &Path,
    quarantine: &Path,
    expected_bytes: &[u8],
    repo: &Path,
    project_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.parent() == quarantine.parent(),
        "scheduler manifest quarantine must be in the same directory"
    );
    anyhow::ensure!(
        !quarantine.exists(),
        "scheduler manifest quarantine already exists"
    );
    std::fs::rename(path, quarantine).with_context(|| {
        format!(
            "atomically claim scheduler manifest {} as {}",
            path.display(),
            quarantine.display()
        )
    })?;

    let revalidation = (|| -> anyhow::Result<()> {
        let trusted_directory = scheduler_manifest_directory(&edda_store::store_root(), true)?;
        let source_metadata = quarantine.symlink_metadata().with_context(|| {
            format!(
                "inspect scheduler manifest quarantine {}",
                quarantine.display()
            )
        })?;
        anyhow::ensure!(
            source_metadata.file_type().is_file(),
            "scheduler manifest quarantine must be a regular file"
        );
        anyhow::ensure!(
            source_metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
            "scheduler manifest quarantine exceeds 16 KiB"
        );
        let canonical = quarantine.canonicalize().with_context(|| {
            format!(
                "canonicalize scheduler manifest quarantine {}",
                quarantine.display()
            )
        })?;
        let parent = quarantine
            .parent()
            .context("scheduler manifest quarantine has no parent")?
            .canonicalize()
            .context("canonicalize scheduler manifest quarantine parent")?;
        anyhow::ensure!(
            parent == trusted_directory && canonical.parent() == Some(trusted_directory.as_path()),
            "scheduler manifest quarantine is outside the trusted Edda store directory"
        );
        let bytes = std::fs::read(&canonical).with_context(|| {
            format!("read scheduler manifest quarantine {}", canonical.display())
        })?;
        anyhow::ensure!(
            bytes.len() as u64 <= SCHEDULER_MANIFEST_MAX_BYTES,
            "scheduler manifest quarantine exceeds 16 KiB"
        );
        anyhow::ensure!(
            bytes == expected_bytes,
            "scheduler manifest entry changed before the atomic quarantine claim"
        );
        let manifest: SchedulerLaunchManifestV1 = serde_json::from_slice(&bytes)
            .context("parse quarantined scheduler launch manifest")?;
        anyhow::ensure!(
            serde_json::to_vec(&manifest)? == bytes,
            "scheduler manifest quarantine JSON is not canonical"
        );
        let loaded = validate_scheduler_manifest(manifest)?;
        anyhow::ensure!(
            loaded.repo == repo && loaded.manifest.project_id == project_id,
            "scheduler manifest quarantine does not belong to the exact task project"
        );
        Ok(())
    })();
    if let Err(error) = revalidation {
        anyhow::bail!(
            "retain quarantine {} because the claimed scheduler manifest failed revalidation: {error:#}",
            quarantine.display()
        );
    }
    std::fs::remove_file(quarantine).with_context(|| {
        format!(
            "retain quarantine {} because exact-file removal failed",
            quarantine.display()
        )
    })
}

pub(super) fn remove_trusted_scheduler_manifest(
    path: &Path,
    repo: &Path,
    project_id: &str,
) -> String {
    let removal = (|| -> anyhow::Result<()> {
        let store = edda_store::store_root();
        let directory = scheduler_manifest_directory(&store, true)?;
        let launch_directory = directory
            .parent()
            .context("scheduler manifest path has no launch directory")?;
        let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
        anyhow::ensure!(
            scheduler_manifest_directory(&store, true)? == directory,
            "scheduler manifest directory changed during uninstall"
        );
        let loaded = load_scheduler_manifest(path)?;
        anyhow::ensure!(
            loaded.repo == repo && loaded.manifest.project_id == project_id,
            "scheduler manifest does not belong to the exact task project"
        );
        let expected_bytes = serde_json::to_vec(&loaded.manifest)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("scheduler manifest filename is not Unicode")?;
        let sequence =
            SCHEDULER_MANIFEST_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let quarantine = directory.join(format!(
            ".{filename}.{}.{}.uninstall-quarantine",
            std::process::id(),
            sequence
        ));
        claim_and_remove_scheduler_manifest_under_lock(
            path,
            &quarantine,
            &expected_bytes,
            repo,
            project_id,
        )
    })();
    match removal {
        Ok(()) => format!(
            "removed trusted scheduler manifest after exact-task absence proof: {}",
            path.display()
        ),
        Err(error) => format!(
            "scheduler manifest retained because exact-file validation or removal failed for {}: {error:#}",
            path.display()
        ),
    }
}

pub(super) fn uninstall_scheduler_task_with(
    repo: &Path,
    executable: &Path,
    project_id: &str,
    mut run: impl FnMut(&[String]) -> anyhow::Result<SchedulerOutput>,
) -> anyhow::Result<String> {
    let (task_name, query_args, delete_args) = windows_scheduler_management_args(project_id)?;
    let before =
        run(&query_args).with_context(|| format!("scheduler Query failed for task {task_name}"))?;
    if classify_scheduler_query(&before)
        .with_context(|| format!("scheduler Query failed for task {task_name}"))?
        == SchedulerTaskState::Missing
    {
        return Ok(format!(
            "scheduler task {} already absent for {}",
            task_name,
            repo.display()
        ));
    }
    let candidate = before
        .xml()
        .and_then(|xml| recover_scheduler_manifest_candidate(xml.as_ref(), executable));
    let deleted = run(&delete_args)
        .with_context(|| format!("scheduler Delete failed for task {task_name}"))?;
    anyhow::ensure!(
        deleted.code == 0 || deleted.code == MISSING_TASK_HRESULT,
        "scheduler Delete failed for {}: {}",
        task_name,
        deleted.description()
    );
    let after =
        run(&query_args).with_context(|| format!("scheduler Query failed for task {task_name}"))?;
    require_scheduler_state(
        &after,
        SchedulerTaskState::Missing,
        "post-Delete Query",
        &task_name,
    )?;
    let cleanup = match candidate {
        Ok(Some(path)) => remove_trusted_scheduler_manifest(&path, repo, project_id),
        Ok(None) => "scheduler manifest retained because the exact task did not prove one strict direct manifest command".into(),
        Err(error) => format!(
            "scheduler manifest retained because the pre-Delete Query was not trustworthy: {error:#}"
        ),
    };
    Ok(format!(
        "uninstalled scheduler task {} for {}; {}",
        task_name,
        repo.display(),
        cleanup
    ))
}

pub(super) fn classify_scheduler_query(
    output: &SchedulerOutput,
) -> anyhow::Result<SchedulerTaskState> {
    match output.code {
        0 => Ok(SchedulerTaskState::Present),
        MISSING_TASK_HRESULT => Ok(SchedulerTaskState::Missing),
        _ => anyhow::bail!("scheduler query failed: {}", output.description()),
    }
}

pub(super) fn require_scheduler_state(
    output: &SchedulerOutput,
    expected: SchedulerTaskState,
    operation: &str,
    task_name: &str,
) -> anyhow::Result<()> {
    let actual = classify_scheduler_query(output)
        .with_context(|| format!("scheduler {operation} failed for task {task_name}"))?;
    anyhow::ensure!(
        actual == expected,
        "scheduler {operation} expected {expected:?} for task {task_name}, got {actual:?}: {}",
        output.description()
    );
    Ok(())
}

#[cfg(windows)]
pub(super) fn run_schtasks(args: &[String]) -> anyhow::Result<SchedulerOutput> {
    let output = Command::new("schtasks.exe")
        .args(args)
        .output()
        .context("launch schtasks.exe")?;
    let signed_code = output
        .status
        .code()
        .context("schtasks.exe terminated by signal")?;
    let bounded = |bytes: &[u8]| bytes[..bytes.len().min(SCHEDULER_OUTPUT_LIMIT)].to_vec();
    let stdout_raw = bounded(&output.stdout);
    let stderr_raw = bounded(&output.stderr);
    Ok(SchedulerOutput {
        code: signed_code as u32,
        stdout: String::from_utf8_lossy(&stdout_raw).into_owned(),
        stdout_raw,
        stderr: String::from_utf8_lossy(&stderr_raw).into_owned(),
        stdout_bytes: output.stdout.len(),
        stderr_bytes: output.stderr.len(),
    })
}
