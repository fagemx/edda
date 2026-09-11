mod git;
mod io;
mod render;

use clap::Subcommand;
use edda_core::continuity::{
    build_capsule, CapsuleRepositoryV1, ContextCapsuleInputV1, ContextSourceV1, DataAuthority,
    PortableCapsuleBundleV1, MAX_CONTINUITY_BUNDLE_BYTES, MAX_CONTINUITY_INPUT_BYTES,
};
use edda_ledger::{CapsuleEntryV1, ImportDisposition, Ledger};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum ContinuityCmd {
    /// Save structured continuation state with trusted local repository metadata
    Save {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show one exact capsule as data only
    Show {
        capsule_id: String,
        #[arg(long)]
        json: bool,
    },
    /// List capsules in stable newest-first ledger order
    List {
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Export one capsule as a bounded portable bundle
    Export {
        capsule_id: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Import one portable bundle as data only
    Import {
        bundle: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Restore one exact or latest matching capsule without repository mutation
    Restore {
        capsule_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
struct SaveResult<'a> {
    status: &'static str,
    sync_status: &'static str,
    data_authority: DataAuthority,
    capsule_id: &'a str,
    event_id: &'a str,
    portable_repo_id: Option<&'a str>,
    warnings: Vec<String>,
}

pub fn run(command: ContinuityCmd, workspace_root: &Path, checkout: &Path) -> anyhow::Result<()> {
    match command {
        ContinuityCmd::Save { file, json } => {
            let result = save(workspace_root, checkout, &file, json);
            if let Err(error) = &result {
                report_save_failure(error, json);
            }
            result
        }
        ContinuityCmd::Show { capsule_id, json } => {
            show(workspace_root, checkout, &capsule_id, json)
        }
        ContinuityCmd::List { branch, json } => {
            list(workspace_root, checkout, branch.as_deref(), json)
        }
        ContinuityCmd::Export { capsule_id, out } => export(workspace_root, &capsule_id, &out),
        ContinuityCmd::Import { bundle, json } => {
            let result = import(workspace_root, checkout, &bundle, json);
            if let Err(error) = &result {
                report_import_refusal(error, json);
            }
            result
        }
        ContinuityCmd::Restore { capsule_id, json } => {
            restore(workspace_root, checkout, capsule_id.as_deref(), json)
        }
    }
}

fn report_save_failure(error: &anyhow::Error, json: bool) {
    let readback = error.downcast_ref::<edda_ledger::ContinuityReadbackError>();
    let status = if readback.is_some() {
        "SAVED_UNVERIFIED"
    } else {
        "SAVE_FAILED"
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": status,
                "sync_status": "SYNC_UNAVAILABLE",
                "data_authority": "data_only",
                "capsule_id": readback.map(|failure| failure.capsule_id.as_str()),
                "event_id": readback.map(|failure| failure.event_id.as_str()),
                "error": error.to_string(),
            })
        );
    } else {
        println!("{status}");
    }
}

fn report_import_refusal(error: &anyhow::Error, json: bool) {
    let readback = error.downcast_ref::<edda_ledger::ContinuityReadbackError>();
    let status = if readback.is_some() {
        "imported"
    } else {
        "refused"
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": status,
                "verified": false,
                "data_authority": "data_only",
                "capsule_id": readback.map(|failure| failure.capsule_id.as_str()),
                "local_event_id": readback.map(|failure| failure.event_id.as_str()),
                "error": error.to_string(),
            })
        );
    } else {
        println!("{}", status.to_ascii_uppercase());
        println!("DATA_ONLY");
    }
}

fn save(workspace_root: &Path, checkout: &Path, file: &Path, json: bool) -> anyhow::Result<()> {
    let bytes = io::read_bounded(file, MAX_CONTINUITY_INPUT_BYTES, "continuity input")?;
    let input: ContextCapsuleInputV1 = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("invalid ContextCapsuleV1 input: {error}"))?;
    let paths = edda_ledger::EddaPaths::discover(workspace_root);
    let identity =
        edda_store::continuity::derive_portable_repository_identity(checkout, &paths.config_json)?;
    let repository = CapsuleRepositoryV1 {
        portable_repo_id: identity.portable_repo_id.clone(),
        display_hint: identity.display_hint,
        local_only_reason: identity.local_only_reason.clone(),
    };
    let source = ContextSourceV1 {
        machine_alias: std::env::var("EDDA_MACHINE").ok(),
        actor: std::env::var("EDDA_ACTOR").ok(),
    };
    let capsule = build_capsule(
        input,
        source,
        repository,
        git::gather_git_metadata(checkout),
    )?;
    let ledger = Ledger::open(workspace_root)?;
    let entry = ledger.append_continuity_capsule(&capsule)?;

    let mut warnings = vec![
        "live continuity transport is unavailable until the #685 node carrier is implemented"
            .to_string(),
    ];
    if let Some(reason) = identity.local_only_reason {
        warnings.push(format!("LOCAL_ONLY: {reason}"));
    }
    if let Some(portable_repo_id) = &identity.portable_repo_id {
        if let Err(error) =
            edda_store::continuity::record_portable_alias(portable_repo_id, checkout)
        {
            warnings.push(format!(
                "portable repository alias was not recorded: {error}"
            ));
        }
    }
    let result = SaveResult {
        status: "SAVED_LOCAL",
        sync_status: "SYNC_UNAVAILABLE",
        data_authority: DataAuthority::DataOnly,
        capsule_id: &entry.capsule.capsule_id,
        event_id: &entry.local_event_id,
        portable_repo_id: entry.capsule.repository.portable_repo_id.as_deref(),
        warnings,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("SAVED_LOCAL {} {}", result.capsule_id, result.event_id);
        println!("SYNC_UNAVAILABLE");
        for warning in result.warnings {
            println!("WARNING: {warning}");
        }
    }
    Ok(())
}

fn show(
    workspace_root: &Path,
    checkout: &Path,
    capsule_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open_existing(workspace_root)?;
    let entry = ledger
        .continuity_capsule(capsule_id)?
        .ok_or_else(|| anyhow::anyhow!("continuity capsule not found: {capsule_id}"))?;
    let current_git = git::gather_git_metadata(checkout);
    render::print_capsule(
        &render::capsule_output(&entry, checkout, &current_git),
        json,
    )
}

fn list(
    workspace_root: &Path,
    checkout: &Path,
    branch: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open_existing(workspace_root)?;
    let paths = edda_ledger::EddaPaths::discover(workspace_root);
    let current_id =
        edda_store::continuity::derive_portable_repository_identity(checkout, &paths.config_json)?
            .portable_repo_id;
    let entries = matching_entries(ledger.continuity_capsules()?, current_id.as_deref(), branch);
    render::print_list(&entries, json)
}

fn export(workspace_root: &Path, capsule_id: &str, out: &Path) -> anyhow::Result<()> {
    let ledger = Ledger::open_existing(workspace_root)?;
    let bundle = ledger.export_continuity_bundle(capsule_id)?;
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    if bytes.len() > MAX_CONTINUITY_BUNDLE_BYTES {
        anyhow::bail!("portable bundle exceeds its size bound");
    }
    io::write_new(out, &bytes)?;
    if std::fs::read(out)? != bytes {
        anyhow::bail!("portable bundle destination failed exact read-back");
    }
    println!("EXPORTED {} {}", bundle.origin_capsule_id, out.display());
    Ok(())
}

fn import(workspace_root: &Path, checkout: &Path, path: &Path, json: bool) -> anyhow::Result<()> {
    let bytes = io::read_bounded(path, MAX_CONTINUITY_BUNDLE_BYTES, "portable bundle")?;
    let bundle: PortableCapsuleBundleV1 = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("invalid portable bundle schema: {error}"))?;
    edda_core::continuity::validate_bundle(&bundle)?;
    let paths = edda_ledger::EddaPaths::discover(workspace_root);
    let current =
        edda_store::continuity::derive_portable_repository_identity(checkout, &paths.config_json)?;
    if current.portable_repo_id.as_deref() != Some(bundle.portable_repo_id.as_str()) {
        anyhow::bail!("portable bundle belongs to a different repository");
    }
    let ledger = Ledger::open(workspace_root)?;
    let result = ledger.import_continuity_bundle(&bundle)?;
    let mut warnings = Vec::new();
    if let Err(error) =
        edda_store::continuity::record_portable_alias(&bundle.portable_repo_id, checkout)
    {
        warnings.push(format!(
            "portable repository alias was not recorded: {error}"
        ));
    }
    let status = match result.disposition {
        ImportDisposition::Imported => "imported",
        ImportDisposition::Skipped => "skipped",
    };
    let output = serde_json::json!({
        "status": status,
        "data_authority": "data_only",
        "capsule_id": result.entry.capsule.capsule_id,
        "origin_event_id": result.entry.origin_event_id,
        "local_event_id": result.entry.local_event_id,
        "warnings": warnings,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", status.to_ascii_uppercase());
        println!("DATA_ONLY");
    }
    Ok(())
}

fn restore(
    workspace_root: &Path,
    checkout: &Path,
    capsule_id: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open_existing(workspace_root)?;
    let current_git = git::gather_git_metadata(checkout);
    let entry = if let Some(capsule_id) = capsule_id {
        ledger.continuity_capsule(capsule_id)?
    } else {
        let paths = edda_ledger::EddaPaths::discover(workspace_root);
        let current_id = edda_store::continuity::derive_portable_repository_identity(
            checkout,
            &paths.config_json,
        )?
        .portable_repo_id;
        matching_entries(
            ledger.continuity_capsules()?,
            current_id.as_deref(),
            current_git.branch.as_deref(),
        )
        .into_iter()
        .next()
    }
    .ok_or_else(|| anyhow::anyhow!("no matching continuity capsule found"))?;
    render::print_capsule(
        &render::capsule_output(&entry, checkout, &current_git),
        json,
    )
}

fn matching_entries(
    entries: Vec<CapsuleEntryV1>,
    portable_repo_id: Option<&str>,
    branch: Option<&str>,
) -> Vec<CapsuleEntryV1> {
    entries
        .into_iter()
        .filter(|entry| {
            entry.legacy_partial
                || entry.capsule.repository.portable_repo_id.as_deref() == portable_repo_id
        })
        .filter(|entry| {
            branch.is_none()
                || entry.capsule.git.branch.as_deref() == branch
                || (entry.legacy_partial && entry.capsule.git.branch.is_none())
        })
        .collect()
}
