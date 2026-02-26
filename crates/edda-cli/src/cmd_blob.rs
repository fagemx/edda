use clap::Subcommand;
use edda_ledger::blob_meta::{self, BlobClass};
use edda_ledger::blob_store::{blob_list, blob_list_archived};
use edda_ledger::{EddaPaths, Ledger};
use std::path::Path;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum BlobCmd {
    /// Classify a blob (artifact, decision_evidence, trace_noise)
    Classify {
        /// Blob hash or prefix
        hash: String,
        /// Classification: artifact, decision_evidence, trace_noise
        #[arg(long)]
        class: String,
    },
    /// Pin a blob (prevent GC from removing it)
    Pin {
        /// Blob hash or prefix
        hash: String,
    },
    /// Unpin a blob (allow GC to remove it)
    Unpin {
        /// Blob hash or prefix
        hash: String,
    },
    /// Show blob info (hash, size, class, pinned, location)
    Info {
        /// Blob hash or prefix
        hash: String,
    },
    /// Show blob store statistics
    Stats,
    /// List tombstones (deleted blob records)
    Tombstones,
}

// ── Dispatch ──

pub fn run(cmd: BlobCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        BlobCmd::Classify { hash, class } => classify(repo_root, &hash, &class),
        BlobCmd::Pin { hash } => pin(repo_root, &hash),
        BlobCmd::Unpin { hash } => unpin(repo_root, &hash),
        BlobCmd::Info { hash } => info(repo_root, &hash),
        BlobCmd::Stats => stats(repo_root),
        BlobCmd::Tombstones => tombstones(repo_root),
    }
}

// ── Command Implementations ──

/// `edda blob classify <hash> --class <class>`
pub fn classify(repo_root: &Path, hash: &str, class_str: &str) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let class: BlobClass = class_str.parse()?;
    let resolved = resolve_hash(&paths, hash)?;

    let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json)?;
    blob_meta::set_class(&mut meta, &resolved, class, "user");
    blob_meta::save_blob_meta(&paths.blob_meta_json, &meta)?;

    println!(
        "Classified blob {} as {}",
        &resolved[..resolved.len().min(12)],
        class
    );
    Ok(())
}

/// `edda blob pin <hash>`
pub fn pin(repo_root: &Path, hash: &str) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let resolved = resolve_hash(&paths, hash)?;

    let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json)?;
    blob_meta::set_pinned(&mut meta, &resolved, true);
    blob_meta::save_blob_meta(&paths.blob_meta_json, &meta)?;

    println!("Pinned blob {}", &resolved[..resolved.len().min(12)]);
    Ok(())
}

/// `edda blob unpin <hash>`
pub fn unpin(repo_root: &Path, hash: &str) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let resolved = resolve_hash(&paths, hash)?;

    let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json)?;
    blob_meta::set_pinned(&mut meta, &resolved, false);
    blob_meta::save_blob_meta(&paths.blob_meta_json, &meta)?;

    println!("Unpinned blob {}", &resolved[..resolved.len().min(12)]);
    Ok(())
}

/// `edda blob info <hash>`
pub fn info(repo_root: &Path, hash: &str) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let resolved = resolve_hash(&paths, hash)?;
    let meta_map = blob_meta::load_blob_meta(&paths.blob_meta_json)?;
    let entry = blob_meta::get_meta(&meta_map, &resolved);

    let (location, size, blob_path) = if paths.blobs_dir.join(&resolved).exists() {
        let p = paths.blobs_dir.join(&resolved);
        let size = p.metadata()?.len();
        ("active", size, p)
    } else if paths.archive_blobs_dir.join(&resolved).exists() {
        let p = paths.archive_blobs_dir.join(&resolved);
        let size = p.metadata()?.len();
        ("archive", size, p)
    } else {
        anyhow::bail!("blob not found: {resolved}");
    };

    println!("Hash:     {resolved}");
    println!("Size:     {} ({} bytes)", format_size(size), size);
    println!("Class:    {}", entry.class);
    println!("Pinned:   {}", entry.pinned);
    println!("Location: {location}");
    if let Some(at) = &entry.classified_at {
        println!(
            "Classified: {} by {}",
            at,
            entry.classified_by.as_deref().unwrap_or("?")
        );
    }

    // Classification history
    if !entry.class_history.is_empty() {
        println!("\nClassification history:");
        for change in &entry.class_history {
            println!(
                "  {} -> {} (by {} at {})",
                change.from, change.to, change.by, change.at
            );
        }
    }

    // Retention explain
    println!();
    let blob_ref = format!("blob:sha256:{resolved}");
    let reasons = explain_retention(&paths, &blob_path, &entry, &blob_ref);
    println!("Retention: {}", reasons.join("; "));

    Ok(())
}

/// Explain why a blob is retained or eligible for GC.
fn explain_retention(
    paths: &EddaPaths,
    blob_path: &Path,
    entry: &blob_meta::BlobMetaEntry,
    blob_ref: &str,
) -> Vec<String> {
    let mut reasons = Vec::new();

    if entry.pinned {
        reasons.push("pinned (never removed)".to_string());
    }

    if entry.class == BlobClass::Artifact {
        reasons.push("artifact class (never auto-removed)".to_string());
    }

    // Check event references
    if let Ok(ledger) = Ledger::open(&paths.root) {
        if let Ok(events) = ledger.iter_events() {
            let ref_count = events
                .iter()
                .filter(|e| e.refs.blobs.contains(&blob_ref.to_string()))
                .count();
            if ref_count > 0 {
                reasons.push(format!("referenced by {ref_count} event(s)"));
            } else {
                reasons.push("unreferenced (no events point to this blob)".to_string());
            }
        }
    }

    // Check mtime vs default retention
    let keep_days = read_config_u32(&paths.config_json, "gc.blob_keep_days").unwrap_or(90);
    if let Ok(meta) = blob_path.metadata() {
        if let Ok(modified) = meta.modified() {
            let modified_odt = time::OffsetDateTime::from(modified);
            let cutoff =
                time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(keep_days));
            if modified_odt < cutoff {
                reasons.push(format!("expired (older than {keep_days} days)"));
            } else {
                let age_days = (time::OffsetDateTime::now_utc() - modified_odt).whole_days();
                reasons.push(format!(
                    "within retention window ({age_days}d old, keep={keep_days}d)"
                ));
            }
        }
    }

    if reasons.is_empty() {
        reasons.push("no special retention rules apply".to_string());
    }

    reasons
}

/// `edda blob stats`
pub fn stats(repo_root: &Path) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }

    let active_blobs = blob_list(&paths)?;
    let archived_blobs = blob_list_archived(&paths)?;
    let meta_map = blob_meta::load_blob_meta(&paths.blob_meta_json)?;

    let active_size: u64 = active_blobs.iter().map(|b| b.size).sum();
    let archived_size: u64 = archived_blobs.iter().map(|b| b.size).sum();

    // Count by class
    let mut artifact_count = 0u32;
    let mut artifact_size = 0u64;
    let mut evidence_count = 0u32;
    let mut evidence_size = 0u64;
    let mut noise_count = 0u32;
    let mut noise_size = 0u64;
    let mut pinned_count = 0u32;

    for blob in &active_blobs {
        let entry = blob_meta::get_meta(&meta_map, &blob.hash);
        match entry.class {
            BlobClass::Artifact => {
                artifact_count += 1;
                artifact_size += blob.size;
            }
            BlobClass::DecisionEvidence => {
                evidence_count += 1;
                evidence_size += blob.size;
            }
            BlobClass::TraceNoise => {
                noise_count += 1;
                noise_size += blob.size;
            }
        }
        if entry.pinned {
            pinned_count += 1;
        }
    }

    println!("Blob Store Statistics\n");
    println!(
        "Active:   {} blob(s) ({})",
        active_blobs.len(),
        format_size(active_size)
    );
    println!(
        "Archived: {} blob(s) ({})",
        archived_blobs.len(),
        format_size(archived_size)
    );
    println!(
        "Total:    {} blob(s) ({})",
        active_blobs.len() + archived_blobs.len(),
        format_size(active_size + archived_size)
    );
    println!();
    println!("By class (active only):");
    println!(
        "  artifact:           {artifact_count:>4} ({:>8})",
        format_size(artifact_size)
    );
    println!(
        "  decision_evidence:  {evidence_count:>4} ({:>8})",
        format_size(evidence_size)
    );
    println!(
        "  trace_noise:        {noise_count:>4} ({:>8})",
        format_size(noise_size)
    );
    println!("  pinned:             {pinned_count:>4}");

    // Show quota usage if configured
    let config_path = &paths.config_json;
    if let Some(quota_mb) = read_config_u32(config_path, "gc.blob_quota_mb") {
        let quota_bytes = u64::from(quota_mb) * 1024 * 1024;
        let pct = if quota_bytes > 0 {
            (active_size as f64 / quota_bytes as f64 * 100.0) as u32
        } else {
            0
        };
        println!();
        println!(
            "Quota: {} / {} MB ({}%)",
            format_size(active_size),
            quota_mb,
            pct
        );
    }

    Ok(())
}

/// `edda blob tombstones`
pub fn tombstones(repo_root: &Path) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }

    let tombstones = edda_ledger::tombstone::list_tombstones(&paths)?;
    if tombstones.is_empty() {
        println!("No tombstones recorded.");
        return Ok(());
    }

    println!("Deleted blob records ({} total):\n", tombstones.len());
    for t in &tombstones {
        let size_str = t.size_bytes.map_or_else(|| "?".to_string(), format_size);
        println!(
            "  {} | {} | {} | {} | {}",
            &t.blob_hash[..t.blob_hash.len().min(12)],
            t.reason,
            t.last_known_class,
            size_str,
            t.deleted_at,
        );
    }

    Ok(())
}

/// Resolve a hash prefix to a full hash. Errors if ambiguous or not found.
fn resolve_hash(paths: &EddaPaths, prefix: &str) -> anyhow::Result<String> {
    // Try exact match first
    if paths.blobs_dir.join(prefix).exists() || paths.archive_blobs_dir.join(prefix).exists() {
        return Ok(prefix.to_string());
    }

    // Try prefix match in active + archive
    let mut matches = Vec::new();
    for dir in [&paths.blobs_dir, &paths.archive_blobs_dir] {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(prefix)
                    && !name.starts_with(".tmp_")
                    && !matches.contains(&name)
                {
                    matches.push(name);
                }
            }
        }
    }

    match matches.len() {
        0 => anyhow::bail!("blob not found: {prefix}"),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => {
            let preview: Vec<_> = matches
                .iter()
                .take(5)
                .map(|h| &h[..h.len().min(16)])
                .collect();
            anyhow::bail!(
                "ambiguous prefix '{prefix}': {} matches ({}...)",
                matches.len(),
                preview.join(", ")
            );
        }
    }
}

fn read_config_u32(config_path: &Path, key: &str) -> Option<u32> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get(key)?.as_u64().map(|n| n as u32)
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_ledger::blob_store::blob_put;
    use edda_ledger::EddaPaths;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, EddaPaths) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_blob_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        (tmp, paths)
    }

    #[test]
    fn classify_persists_blob_meta() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"test data").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        classify(&tmp, hex, "artifact").unwrap();

        let meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        assert_eq!(meta[hex].class, BlobClass::Artifact);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn classify_with_prefix() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"prefix test").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();
        let prefix = &hex[..8];

        classify(&tmp, prefix, "decision_evidence").unwrap();

        let meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        assert_eq!(meta[hex].class, BlobClass::DecisionEvidence);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn classify_invalid_class_errors() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"bad class").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        let result = classify(&tmp, hex, "nonexistent");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pin_unpin_round_trip() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"pin me").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        pin(&tmp, hex).unwrap();
        let meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        assert!(meta[hex].pinned);

        unpin(&tmp, hex).unwrap();
        let meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        assert!(!meta[hex].pinned);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn info_runs_without_error() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"info test").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        // Classify + pin so info exercises all paths
        classify(&tmp, hex, "artifact").unwrap();
        pin(&tmp, hex).unwrap();

        info(&tmp, hex).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn info_nonexistent_blob_errors() {
        let (tmp, _paths) = setup_workspace();

        let result = info(
            &tmp,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn stats_on_empty_workspace() {
        let (tmp, _paths) = setup_workspace();

        // Should run without error even with no blobs
        stats(&tmp).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn stats_with_classified_blobs() {
        let (tmp, paths) = setup_workspace();

        let r1 = blob_put(&paths, b"artifact data").unwrap();
        let r2 = blob_put(&paths, b"noise data").unwrap();
        let h1 = r1.strip_prefix("blob:sha256:").unwrap();
        let h2 = r2.strip_prefix("blob:sha256:").unwrap();

        classify(&tmp, h1, "artifact").unwrap();
        classify(&tmp, h2, "trace_noise").unwrap();

        stats(&tmp).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tombstones_empty() {
        let (tmp, _paths) = setup_workspace();

        // No tombstones → runs without error
        tombstones(&tmp).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hash_exact_match() {
        let (_tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"exact").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        let resolved = resolve_hash(&paths, hex).unwrap();
        assert_eq!(resolved, hex);

        let _ = std::fs::remove_dir_all(&_tmp);
    }

    #[test]
    fn resolve_hash_prefix_match() {
        let (_tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"prefix resolve").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();
        let prefix = &hex[..8];

        let resolved = resolve_hash(&paths, prefix).unwrap();
        assert_eq!(resolved, hex);

        let _ = std::fs::remove_dir_all(&_tmp);
    }

    #[test]
    fn resolve_hash_not_found() {
        let (_tmp, paths) = setup_workspace();

        let result = resolve_hash(&paths, "deadbeef00000000");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blob not found"));

        let _ = std::fs::remove_dir_all(&_tmp);
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn reclassify_records_history_via_cli() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"reclassify test").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        classify(&tmp, hex, "trace_noise").unwrap();
        classify(&tmp, hex, "artifact").unwrap();
        classify(&tmp, hex, "decision_evidence").unwrap();

        let meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        let entry = &meta[hex];
        assert_eq!(entry.class, BlobClass::DecisionEvidence);
        assert_eq!(entry.class_history.len(), 2);
        assert_eq!(entry.class_history[0].from, BlobClass::TraceNoise);
        assert_eq!(entry.class_history[0].to, BlobClass::Artifact);
        assert_eq!(entry.class_history[1].from, BlobClass::Artifact);
        assert_eq!(entry.class_history[1].to, BlobClass::DecisionEvidence);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn info_on_archived_blob() {
        let (tmp, paths) = setup_workspace();
        let blob_ref = blob_put(&paths, b"archive me").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        // Archive the blob
        edda_ledger::blob_store::blob_archive(&paths, hex).unwrap();

        // info should still work (archive fallback)
        info(&tmp, hex).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
