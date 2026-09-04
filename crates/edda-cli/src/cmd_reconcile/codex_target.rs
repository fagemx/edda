use anyhow::Context;
use std::path::{Path, PathBuf};

pub(super) fn canonical_main_repo(repo: &Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(repo.is_absolute(), "--repo must be absolute");
    let repo = repo
        .canonicalize()
        .with_context(|| format!("canonicalize repository {}", repo.display()))?;
    anyhow::ensure!(repo.is_dir(), "--repo must name a directory");
    edda_ledger::EddaPaths::find_root(&repo)
        .context("--repo must name an initialized Edda workspace")?
        .canonicalize()
        .context("canonicalize Edda workspace root")
}

/// Test mirror of [`canonical_main_repo`] that anchors the workspace walk
/// inside the caller's own tree (GH-646).
///
/// The production walk is unbounded: from a fixture tempdir it climbs
/// through `%TEMP%` up to `$HOME`, where the fleet coordination workspace
/// lives, and resolves THERE instead of failing. Tests that assert
/// "not an initialized workspace" must bound the climb at the fixture root
/// so the premise is created by the fixture, not by the environment.
#[cfg(test)]
pub(super) fn canonical_main_repo_bounded(repo: &Path, ceiling: &Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(repo.is_absolute(), "--repo must be absolute");
    let repo = repo
        .canonicalize()
        .with_context(|| format!("canonicalize repository {}", repo.display()))?;
    let ceiling = ceiling
        .canonicalize()
        .with_context(|| format!("canonicalize ceiling {}", ceiling.display()))?;
    anyhow::ensure!(repo.is_dir(), "--repo must name a directory");
    edda_ledger::EddaPaths::find_root_bounded(&repo, &ceiling)
        .context("--repo must name an initialized Edda workspace (bounded walk)")?
        .canonicalize()
        .context("canonicalize Edda workspace root")
}

pub(super) fn validate_canonical_direct_codex_target(canonical: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        canonical.is_absolute()
            && canonical.is_file()
            && canonical
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")),
        "canonical Codex executable {} must be an absolute native .exe file",
        canonical.display()
    );
    Ok(())
}

pub(super) fn canonical_direct_codex_executable(
    command: &Path,
    search_path: Option<&std::ffi::OsStr>,
) -> anyhow::Result<PathBuf> {
    let has_parent = command
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    let path = search_path
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let candidates = if command.is_absolute() || has_parent {
        vec![command.to_path_buf()]
    } else {
        path.as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .map(|directory| directory.join(command))
            .collect()
    };

    for candidate in candidates {
        let candidate = match candidate
            .extension()
            .and_then(|extension| extension.to_str())
        {
            None => candidate.with_extension("exe"),
            Some(extension) if extension.eq_ignore_ascii_case("exe") => candidate,
            Some(_) => continue,
        };
        if !candidate.is_file() {
            continue;
        }
        let canonical = candidate
            .canonicalize()
            .with_context(|| format!("canonicalize Codex executable {}", candidate.display()))?;
        validate_canonical_direct_codex_target(&canonical)?;
        return Ok(canonical);
    }
    anyhow::bail!(
        "Codex executable {} must resolve to an absolute native .exe file",
        command.display()
    )
}
