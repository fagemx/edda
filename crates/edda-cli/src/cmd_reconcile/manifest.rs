use anyhow::Context;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::codex_target::{canonical_direct_codex_executable, canonical_main_repo};
use super::{ReconcileConfig, SCHEDULER_MANIFEST_MAX_BYTES};

#[cfg(any(windows, test))]
use std::sync::atomic::AtomicU64;

#[cfg(any(windows, test))]
pub(super) static SCHEDULER_MANIFEST_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SchedulerLaunchManifestV1 {
    pub(super) schema_version: u8,
    pub(super) project_id: String,
    pub(super) repo: PathBuf,
    pub(super) codex_bin: PathBuf,
    pub(super) max_workers: usize,
    pub(super) max_attempts: u32,
    pub(super) lease_ttl_s: u64,
}

#[cfg(any(windows, test))]
pub(super) struct PreparedSchedulerManifest {
    pub(super) manifest: SchedulerLaunchManifestV1,
    pub(super) bytes: Vec<u8>,
    pub(super) digest: String,
    pub(super) path: PathBuf,
}

pub(super) struct LoadedSchedulerManifest {
    #[cfg(any(windows, test))]
    pub(super) manifest: SchedulerLaunchManifestV1,
    pub(super) repo: PathBuf,
    pub(super) config: ReconcileConfig,
}

pub(super) fn scheduler_manifest_directory(
    store: &Path,
    must_exist: bool,
) -> anyhow::Result<PathBuf> {
    let store = std::path::absolute(store)
        .with_context(|| format!("resolve Edda store root {}", store.display()))?;
    let value = store.to_str().context("Edda store root is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "Edda store root contains an unsupported character"
    );
    let store = prospective_canonical_store_root(&store, must_exist)?;
    let launch = store.join("scheduler-launch");
    if launch.exists() {
        anyhow::ensure!(
            launch.canonicalize()? == launch,
            "scheduler manifest directory escapes the Edda store root"
        );
    }
    let directory = launch.join("v1");
    if directory.exists() {
        anyhow::ensure!(
            directory.canonicalize()? == directory,
            "scheduler manifest directory escapes the Edda store root"
        );
    } else {
        anyhow::ensure!(!must_exist, "scheduler manifest directory does not exist");
    }
    Ok(directory)
}

pub(super) fn prospective_canonical_store_root(
    store: &Path,
    must_exist: bool,
) -> anyhow::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut existing = store;
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("Edda store root has no existing ancestor")?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .context("Edda store root has no existing ancestor")?;
    }
    anyhow::ensure!(existing.is_dir(), "Edda store root must be a directory");
    anyhow::ensure!(
        !must_exist || missing.is_empty(),
        "Edda store root does not exist"
    );
    let mut canonical = existing
        .canonicalize()
        .with_context(|| format!("canonicalize Edda store root {}", existing.display()))?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

pub(super) fn scheduler_manifest_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn validate_scheduler_manifest(
    manifest: SchedulerLaunchManifestV1,
) -> anyhow::Result<LoadedSchedulerManifest> {
    anyhow::ensure!(
        manifest.schema_version == 1,
        "unknown scheduler manifest version"
    );
    let repo = canonical_main_repo(&manifest.repo)?;
    anyhow::ensure!(
        repo == manifest.repo,
        "scheduler manifest repository is not canonical"
    );
    anyhow::ensure!(
        manifest.project_id == edda_store::project_id_for_root(&repo),
        "scheduler manifest project id does not match repository"
    );
    let codex_bin = canonical_direct_codex_executable(&manifest.codex_bin, None)?;
    anyhow::ensure!(
        codex_bin == manifest.codex_bin,
        "scheduler manifest Codex executable is not canonical"
    );
    let config = ReconcileConfig {
        max_workers: manifest.max_workers,
        max_attempts: manifest.max_attempts,
        lease_ttl_s: manifest.lease_ttl_s,
        codex_bin,
    };
    Ok(LoadedSchedulerManifest {
        #[cfg(any(windows, test))]
        manifest,
        repo,
        config,
    })
}

#[cfg(any(windows, test))]
pub(super) fn prepare_scheduler_manifest(
    store: &Path,
    repo: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<PreparedSchedulerManifest> {
    let repo = canonical_main_repo(repo)?;
    let codex_bin = canonical_direct_codex_executable(&config.codex_bin, None)?;
    let manifest = SchedulerLaunchManifestV1 {
        schema_version: 1,
        project_id: edda_store::project_id_for_root(&repo),
        repo,
        codex_bin,
        max_workers: config.max_workers,
        max_attempts: config.max_attempts,
        lease_ttl_s: config.lease_ttl_s,
    };
    let bytes = serde_json::to_vec(&manifest)?;
    let digest = scheduler_manifest_digest(&bytes);
    let path = scheduler_manifest_directory(store, false)?.join(format!("{digest}.json"));
    Ok(PreparedSchedulerManifest {
        manifest,
        bytes,
        digest,
        path,
    })
}

#[cfg(any(windows, test))]
pub(super) fn publish_scheduler_manifest(
    prepared: &PreparedSchedulerManifest,
) -> anyhow::Result<bool> {
    let directory = prepared
        .path
        .parent()
        .context("scheduler manifest path has no version directory")?;
    let launch_directory = directory
        .parent()
        .context("scheduler manifest path has no launch directory")?;
    let store = edda_store::store_root();
    anyhow::ensure!(
        scheduler_manifest_directory(&store, false)? == directory,
        "scheduler manifest path is outside the trusted Edda store directory"
    );
    let _lock = edda_store::lock_file(&launch_directory.join("manifest.lock"))?;
    anyhow::ensure!(
        scheduler_manifest_directory(&store, false)? == directory,
        "scheduler manifest directory changed during lock acquisition"
    );
    std::fs::create_dir_all(directory).with_context(|| {
        format!(
            "create scheduler manifest directory {}",
            directory.display()
        )
    })?;
    anyhow::ensure!(
        scheduler_manifest_directory(&store, true)? == directory,
        "scheduler manifest directory changed during publication"
    );

    if prepared.path.exists() {
        validate_existing_scheduler_manifest(prepared)?;
        return Ok(false);
    }

    let sequence =
        SCHEDULER_MANIFEST_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = directory.join(format!(
        ".{}.{}.{}.tmp",
        prepared.digest,
        std::process::id(),
        sequence
    ));
    edda_store::write_atomic(&temp, &prepared.bytes)
        .with_context(|| format!("write scheduler manifest temporary file {}", temp.display()))?;
    link_scheduler_manifest_noclobber(&temp, prepared)
}

#[cfg(any(windows, test))]
pub(super) fn validate_existing_scheduler_manifest(
    prepared: &PreparedSchedulerManifest,
) -> anyhow::Result<()> {
    let loaded = load_scheduler_manifest(&prepared.path)?;
    anyhow::ensure!(
        loaded.manifest == prepared.manifest && std::fs::read(&prepared.path)? == prepared.bytes,
        "existing scheduler manifest does not contain the expected bytes"
    );
    Ok(())
}

#[cfg(any(windows, test))]
pub(super) fn link_scheduler_manifest_noclobber(
    temp: &Path,
    prepared: &PreparedSchedulerManifest,
) -> anyhow::Result<bool> {
    match std::fs::hard_link(temp, &prepared.path) {
        Ok(()) => {
            std::fs::remove_file(temp).with_context(|| {
                format!(
                    "remove scheduler manifest temporary file {}",
                    temp.display()
                )
            })?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(temp).with_context(|| {
                format!(
                    "remove scheduler manifest temporary file {}",
                    temp.display()
                )
            })?;
            validate_existing_scheduler_manifest(prepared)?;
            Ok(false)
        }
        Err(error) => {
            let cleanup = std::fs::remove_file(temp);
            match cleanup {
                Ok(()) => Err(error).with_context(|| {
                    format!(
                        "atomically publish scheduler manifest {}",
                        prepared.path.display()
                    )
                }),
                Err(cleanup_error) => anyhow::bail!(
                    "atomically publish scheduler manifest {}: {error}; retain temporary file {} because cleanup failed: {cleanup_error}",
                    prepared.path.display(),
                    temp.display()
                ),
            }
        }
    }
}

pub(super) fn load_scheduler_manifest(path: &Path) -> anyhow::Result<LoadedSchedulerManifest> {
    anyhow::ensure!(
        path.is_absolute(),
        "scheduler manifest path must be absolute"
    );
    let value = path
        .to_str()
        .context("scheduler manifest path is not Unicode")?;
    anyhow::ensure!(
        !value.contains(['\0', '"']),
        "scheduler manifest path contains an unsupported character"
    );
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("scheduler manifest filename is not Unicode")?;
    let expected_digest = filename
        .strip_suffix(".json")
        .context("scheduler manifest filename must end in .json")?;
    anyhow::ensure!(
        expected_digest.len() == 64
            && expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "scheduler manifest filename must contain a 64-character lowercase SHA-256 digest"
    );

    let trusted_directory = scheduler_manifest_directory(&edda_store::store_root(), true)?;
    let source_metadata = path
        .symlink_metadata()
        .with_context(|| format!("inspect scheduler manifest {}", path.display()))?;
    anyhow::ensure!(
        source_metadata.file_type().is_file(),
        "scheduler manifest must be a regular file"
    );
    anyhow::ensure!(
        source_metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize scheduler manifest {}", path.display()))?;
    let parent = path
        .parent()
        .context("scheduler manifest has no parent")?
        .canonicalize()
        .context("canonicalize scheduler manifest parent")?;
    anyhow::ensure!(
        parent == trusted_directory && canonical.parent() == Some(trusted_directory.as_path()),
        "scheduler manifest is outside the trusted Edda store directory"
    );
    let metadata = canonical.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "scheduler manifest must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let bytes = std::fs::read(&canonical)
        .with_context(|| format!("read scheduler manifest {}", canonical.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= SCHEDULER_MANIFEST_MAX_BYTES,
        "scheduler manifest exceeds 16 KiB"
    );
    let manifest: SchedulerLaunchManifestV1 =
        serde_json::from_slice(&bytes).context("parse scheduler launch manifest")?;
    anyhow::ensure!(
        serde_json::to_vec(&manifest)? == bytes,
        "scheduler manifest JSON is not canonical"
    );
    anyhow::ensure!(
        scheduler_manifest_digest(&bytes) == expected_digest,
        "scheduler manifest digest does not match filename"
    );
    validate_scheduler_manifest(manifest)
}
