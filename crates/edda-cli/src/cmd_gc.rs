use edda_ledger::blob_meta::{self, BlobClass};
use edda_ledger::blob_store::{blob_list, blob_list_archived};
use edda_ledger::tombstone::{self, DeleteReason};
use edda_ledger::{blob_archive, blob_remove, Ledger};
use std::collections::HashSet;
use std::path::Path;

const DEFAULT_BLOB_KEEP_DAYS: u32 = 90;
const DEFAULT_TRANSCRIPT_KEEP_DAYS: u32 = 30;
const DEFAULT_ARCHIVE_KEEP_DAYS: u32 = 180;

pub struct GcParams<'a> {
    pub repo_root: &'a Path,
    pub dry_run: bool,
    pub keep_days: Option<u32>,
    pub force: bool,
    pub global: bool,
    pub archive: bool,
    pub purge_archive: bool,
    pub archive_keep_days: Option<u32>,
    pub include_sessions: bool,
}

const DEFAULT_STATE_KEEP_DAYS: u32 = 7;

/// Candidate blob for removal/archival.
struct GcCandidate {
    hash: String,
    size: u64,
    class: BlobClass,
    reason: DeleteReason,
}

pub fn execute(params: &GcParams) -> anyhow::Result<()> {
    if params.purge_archive {
        return purge_archive(params);
    }

    let ledger = Ledger::open(params.repo_root)?;

    // Read config for retention settings
    let blob_keep_days = params.keep_days.unwrap_or_else(|| {
        read_config_u32(&ledger.paths.config_json, "gc.blob_keep_days")
            .unwrap_or(DEFAULT_BLOB_KEEP_DAYS)
    });
    let transcript_keep_days = params.keep_days.unwrap_or_else(|| {
        read_config_u32(&ledger.paths.config_json, "gc.transcript_keep_days")
            .unwrap_or(DEFAULT_TRANSCRIPT_KEEP_DAYS)
    });
    let quota_mb = read_config_u32(&ledger.paths.config_json, "gc.blob_quota_mb");

    // Phase 1: Scan events to collect active blob refs
    let events = ledger.iter_events()?;
    let mut active_refs: HashSet<String> = HashSet::new();
    for event in &events {
        for blob_ref in &event.refs.blobs {
            if let Some(hex) = blob_ref.strip_prefix("blob:sha256:") {
                active_refs.insert(hex.to_string());
            }
        }
    }
    println!(
        "Scanning events... {} events, {} blob refs",
        events.len(),
        active_refs.len()
    );

    // Phase 2: Scan blob store + load metadata
    let blobs = blob_list(&ledger.paths)?;
    let total_size: u64 = blobs.iter().map(|b| b.size).sum();
    let meta_map = blob_meta::load_blob_meta(&ledger.paths.blob_meta_json)?;

    println!(
        "Scanning blob store... {} blobs ({})",
        blobs.len(),
        format_size(total_size)
    );

    // Phase 3: Build candidate list with class-aware priority
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(blob_keep_days));

    let mut candidates: Vec<GcCandidate> = Vec::new();

    for blob in &blobs {
        let entry = blob_meta::get_meta(&meta_map, &blob.hash);

        // Skip pinned blobs — never touch
        if entry.pinned {
            continue;
        }

        // Skip artifact class — never auto-remove
        if entry.class == BlobClass::Artifact {
            continue;
        }

        // For referenced blobs: only remove if unreferenced
        if active_refs.contains(&blob.hash) {
            continue;
        }

        // Check file modification time against keep_days
        let blob_path = ledger.paths.blobs_dir.join(&blob.hash);
        let is_expired = match blob_path.metadata().and_then(|m| m.modified()) {
            Ok(modified) => {
                let modified_odt = time::OffsetDateTime::from(modified);
                modified_odt < cutoff
            }
            Err(_) => false,
        };

        if is_expired {
            candidates.push(GcCandidate {
                hash: blob.hash.clone(),
                size: blob.size,
                class: entry.class,
                reason: DeleteReason::Retention,
            });
        }
    }

    // Sort by GC priority: trace_noise first, then decision_evidence
    candidates.sort_by_key(|c| c.class.gc_priority());

    // Phase 3b: Quota enforcement — add more candidates if over quota
    if let Some(quota) = quota_mb {
        let quota_bytes = u64::from(quota) * 1024 * 1024;
        let candidate_size: u64 = candidates.iter().map(|c| c.size).sum();
        let size_after_gc = total_size.saturating_sub(candidate_size);

        if size_after_gc > quota_bytes {
            // Need to remove more blobs to meet quota
            let mut overage = size_after_gc - quota_bytes;
            // Collect additional candidates from remaining blobs (not already in list)
            let candidate_hashes: HashSet<&str> =
                candidates.iter().map(|c| c.hash.as_str()).collect();

            let mut extra: Vec<GcCandidate> = Vec::new();
            for blob in &blobs {
                if candidate_hashes.contains(blob.hash.as_str()) {
                    continue;
                }
                let entry = blob_meta::get_meta(&meta_map, &blob.hash);
                if entry.pinned || entry.class == BlobClass::Artifact {
                    continue;
                }
                extra.push(GcCandidate {
                    hash: blob.hash.clone(),
                    size: blob.size,
                    class: entry.class,
                    reason: DeleteReason::Quota,
                });
            }
            extra.sort_by_key(|c| c.class.gc_priority());

            for candidate in extra {
                if overage == 0 {
                    break;
                }
                overage = overage.saturating_sub(candidate.size);
                candidates.push(candidate);
            }
        }
    }

    let candidate_size: u64 = candidates.iter().map(|c| c.size).sum();

    println!();
    if candidates.is_empty() {
        println!("No removable blobs found.");
    } else {
        let action = if params.archive {
            "archival"
        } else {
            "removal"
        };
        println!(
            "Candidates for {}:\n  {} blob(s) ({})",
            action,
            candidates.len(),
            format_size(candidate_size)
        );
        // Breakdown by class
        let noise_count = candidates
            .iter()
            .filter(|c| c.class == BlobClass::TraceNoise)
            .count();
        let evidence_count = candidates
            .iter()
            .filter(|c| c.class == BlobClass::DecisionEvidence)
            .count();
        if noise_count > 0 {
            println!("    trace_noise: {noise_count}");
        }
        if evidence_count > 0 {
            println!("    decision_evidence: {evidence_count}");
        }
    }

    // Phase 4: Global transcript cleanup
    let mut transcript_candidates: Vec<(std::path::PathBuf, u64)> = Vec::new();
    if params.global {
        let pid = edda_store::project_id(params.repo_root);
        let transcripts_dir = edda_store::project_dir(&pid).join("transcripts");
        if transcripts_dir.exists() {
            let transcript_cutoff = time::OffsetDateTime::now_utc()
                - time::Duration::days(i64::from(transcript_keep_days));
            if let Ok(entries) = std::fs::read_dir(&transcripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Ok(meta) = path.metadata() {
                        if let Ok(modified) = meta.modified() {
                            let modified_odt = time::OffsetDateTime::from(modified);
                            if modified_odt < transcript_cutoff {
                                transcript_candidates.push((path, meta.len()));
                            }
                        }
                    }
                }
            }
        }

        let transcript_size: u64 = transcript_candidates.iter().map(|(_, s)| *s).sum();
        if transcript_candidates.is_empty() {
            println!("No expired transcripts found.");
        } else {
            println!(
                "  {} transcript(s) older than {} days ({})",
                transcript_candidates.len(),
                transcript_keep_days,
                format_size(transcript_size)
            );
        }
    }

    // Phase 4b: Session files cleanup (ledger, index, state)
    let mut session_candidates: Vec<(std::path::PathBuf, u64)> = Vec::new();
    if params.include_sessions && params.global {
        let pid = edda_store::project_id(params.repo_root);
        let session_keep_days = params.keep_days.unwrap_or_else(|| {
            read_config_u32(&ledger.paths.config_json, "gc.session_keep_days")
                .unwrap_or(transcript_keep_days)
        });
        let session_cutoff =
            time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(session_keep_days));

        // Session ledgers: {project_dir}/ledger/{session_id}.jsonl
        let ledger_dir = edda_store::project_dir(&pid).join("ledger");
        scan_expired_files(
            &ledger_dir,
            "jsonl",
            session_cutoff,
            &mut session_candidates,
        );

        // Index files: {project_dir}/index/{session_id}.jsonl
        let index_dir = edda_store::project_dir(&pid).join("index");
        scan_expired_files(&index_dir, "jsonl", session_cutoff, &mut session_candidates);

        // Stale state files: inject_hash.*, transcript_cursor.*, progress_last.*
        let state_dir = edda_store::project_dir(&pid).join("state");
        let state_cutoff = time::OffsetDateTime::now_utc()
            - time::Duration::days(i64::from(DEFAULT_STATE_KEEP_DAYS));
        scan_stale_state_files(&state_dir, state_cutoff, &mut session_candidates);

        let session_size: u64 = session_candidates.iter().map(|(_, s)| *s).sum();
        if session_candidates.is_empty() {
            println!("No expired session files found.");
        } else {
            println!(
                "  {} session file(s) older than {} days ({})",
                session_candidates.len(),
                session_keep_days,
                format_size(session_size)
            );
        }
    }

    // Phase 4c: Compact coordination.jsonl if over threshold
    if params.include_sessions && params.global {
        let pid = edda_store::project_id(params.repo_root);
        let compacted = compact_coordination_log(&pid, 1000, params.dry_run);
        if compacted > 0 {
            println!("  coordination.jsonl compacted: {compacted} → current state");
        }
    }

    // Phase 5: Execute or dry-run
    let total_items = candidates.len() + transcript_candidates.len() + session_candidates.len();
    if total_items == 0 {
        println!("\nNothing to clean up.");
        return Ok(());
    }

    let total_free: u64 = candidate_size
        + transcript_candidates.iter().map(|(_, s)| *s).sum::<u64>()
        + session_candidates.iter().map(|(_, s)| *s).sum::<u64>();

    if params.dry_run {
        let action = if params.archive { "archive" } else { "free" };
        println!(
            "\n[dry-run] Would {} {} ({} item(s))",
            action,
            format_size(total_free),
            total_items
        );
        return Ok(());
    }

    // Confirmation prompt (unless --force)
    if !params.force {
        let action = if params.archive { "Archive" } else { "Delete" };
        eprint!(
            "\n{} {} item(s) freeing {}? [y/N] ",
            action,
            total_items,
            format_size(total_free)
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Process blobs
    let mut freed: u64 = 0;
    let mut processed_count = 0;
    for candidate in &candidates {
        let result = if params.archive {
            blob_archive(&ledger.paths, &candidate.hash)
        } else {
            blob_remove(&ledger.paths, &candidate.hash)
        };
        match result {
            Ok(size) => {
                freed += size;
                processed_count += 1;
                // Write tombstone only when deleting (not archiving)
                if !params.archive {
                    let t = tombstone::make_tombstone(
                        &candidate.hash,
                        candidate.reason,
                        candidate.class,
                        false,
                        Some(size),
                    );
                    let _ = tombstone::append_tombstone(&ledger.paths, &t);
                }
            }
            Err(e) => eprintln!("  warning: failed to process blob {}: {e}", &candidate.hash),
        }
    }

    // Delete transcripts
    for (path, size) in &transcript_candidates {
        match std::fs::remove_file(path) {
            Ok(()) => {
                freed += size;
                processed_count += 1;
            }
            Err(e) => eprintln!("  warning: failed to remove {}: {e}", path.display()),
        }
    }

    // Delete session files (ledger, index, state)
    for (path, size) in &session_candidates {
        match std::fs::remove_file(path) {
            Ok(()) => {
                freed += size;
                processed_count += 1;
            }
            Err(e) => eprintln!("  warning: failed to remove {}: {e}", path.display()),
        }
    }

    let action = if params.archive { "Archived" } else { "Freed" };
    println!(
        "\n{} {} ({} item(s) processed)",
        action,
        format_size(freed),
        processed_count
    );

    Ok(())
}

/// Purge archived blobs past retention period.
fn purge_archive(params: &GcParams) -> anyhow::Result<()> {
    let ledger = Ledger::open(params.repo_root)?;
    let archive_keep_days = params.archive_keep_days.unwrap_or_else(|| {
        read_config_u32(&ledger.paths.config_json, "gc.archive_keep_days")
            .unwrap_or(DEFAULT_ARCHIVE_KEEP_DAYS)
    });

    let archived = blob_list_archived(&ledger.paths)?;
    let meta_map = blob_meta::load_blob_meta(&ledger.paths.blob_meta_json)?;
    if archived.is_empty() {
        println!("No archived blobs found.");
        return Ok(());
    }

    let cutoff =
        time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(archive_keep_days));

    let mut candidates: Vec<(&str, u64)> = Vec::new();
    for blob in &archived {
        let path = ledger.paths.archive_blobs_dir.join(&blob.hash);
        let is_expired = match path.metadata().and_then(|m| m.modified()) {
            Ok(modified) => time::OffsetDateTime::from(modified) < cutoff,
            Err(_) => false,
        };
        if is_expired {
            candidates.push((&blob.hash, blob.size));
        }
    }

    let total_size: u64 = archived.iter().map(|b| b.size).sum();
    let candidate_size: u64 = candidates.iter().map(|(_, s)| *s).sum();

    println!(
        "Archive: {} blob(s) ({})",
        archived.len(),
        format_size(total_size)
    );

    if candidates.is_empty() {
        println!(
            "No expired archived blobs (keep_days={})",
            archive_keep_days
        );
        return Ok(());
    }

    println!(
        "Expired: {} blob(s) older than {} days ({})",
        candidates.len(),
        archive_keep_days,
        format_size(candidate_size)
    );

    if params.dry_run {
        println!(
            "\n[dry-run] Would permanently delete {} ({} blob(s))",
            format_size(candidate_size),
            candidates.len()
        );
        return Ok(());
    }

    if !params.force {
        eprint!(
            "\nPermanently delete {} archived blob(s) ({})? [y/N] ",
            candidates.len(),
            format_size(candidate_size)
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut freed: u64 = 0;
    let mut deleted = 0;
    for (hash, size) in &candidates {
        let path = ledger.paths.archive_blobs_dir.join(hash);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                freed += size;
                deleted += 1;
                let entry = blob_meta::get_meta(&meta_map, hash);
                let t = tombstone::make_tombstone(
                    hash,
                    DeleteReason::PurgeArchive,
                    entry.class,
                    entry.pinned,
                    Some(*size),
                );
                let _ = tombstone::append_tombstone(&ledger.paths, &t);
            }
            Err(e) => eprintln!("  warning: failed to delete archived blob {hash}: {e}"),
        }
    }

    println!(
        "\nPurged {} ({} blob(s) deleted)",
        format_size(freed),
        deleted
    );

    Ok(())
}

fn read_config_u32(config_path: &Path, key: &str) -> Option<u32> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get(key)?.as_u64().map(|n| n as u32)
}

/// Scan a directory for expired JSONL (or other extension) files older than cutoff.
fn scan_expired_files(
    dir: &Path,
    ext: &str,
    cutoff: time::OffsetDateTime,
    out: &mut Vec<(std::path::PathBuf, u64)>,
) {
    if !dir.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                let modified_odt = time::OffsetDateTime::from(modified);
                if modified_odt < cutoff {
                    out.push((path, meta.len()));
                }
            }
        }
    }
}

/// Scan state directory for stale per-session files.
/// Matches: inject_hash.*, transcript_cursor.*, progress_last.*, ingest.*.lock
/// Preserves: active_tasks.json, files_modified.json, recent_commits.json
fn scan_stale_state_files(
    state_dir: &Path,
    cutoff: time::OffsetDateTime,
    out: &mut Vec<(std::path::PathBuf, u64)>,
) {
    if !state_dir.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(state_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let stale_prefixes = [
        "inject_hash.",
        "transcript_cursor.",
        "progress_last.",
        "ingest.",
        "session.",
    ];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_session_scoped = stale_prefixes.iter().any(|p| name.starts_with(p));
        if !is_session_scoped {
            continue;
        }
        let path = entry.path();
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                let modified_odt = time::OffsetDateTime::from(modified);
                if modified_odt < cutoff {
                    out.push((path, meta.len()));
                }
            }
        }
    }
}

/// Compact coordination.jsonl if it exceeds the line threshold.
/// Returns the number of original lines (0 if no compaction needed).
fn compact_coordination_log(project_id: &str, max_lines: usize, dry_run: bool) -> usize {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("coordination.jsonl");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= max_lines {
        return 0;
    }

    if dry_run {
        return lines.len();
    }

    // Replay events to compute current state, then re-serialize
    // Use the peers module to compute board state, then rewrite
    let board = edda_bridge_claude::peers::compute_board_state_for_compaction(project_id);
    let mut new_lines = Vec::new();
    for line in board {
        new_lines.push(line);
    }

    let compacted = new_lines.join("\n");
    let tmp_path = path.with_extension("jsonl.tmp");
    if std::fs::write(&tmp_path, format!("{compacted}\n")).is_ok() {
        let _ = std::fs::rename(&tmp_path, &path);
    }

    lines.len()
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
    use edda_core::event::new_note_event;
    use edda_ledger::blob_store::blob_put;
    use edda_ledger::EddaPaths;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, EddaPaths) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_gc_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        (tmp, paths)
    }

    #[test]
    fn gc_removes_only_unreferenced_blobs() {
        let (tmp, paths) = setup_workspace();
        let ledger = Ledger::open(&tmp).unwrap();

        let ref_a = blob_put(&paths, b"referenced blob").unwrap();
        let ref_b = blob_put(&paths, b"orphan blob").unwrap();

        let mut event = new_note_event("main", None, "system", "test", &[]).unwrap();
        event.refs.blobs.push(ref_a.clone());
        ledger.append_event(&event, false).unwrap();

        let hex_b = ref_b.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_b));

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_b).is_err());

        // Verify tombstone was written
        let tombstones = tombstone::list_tombstones(&paths).unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].blob_hash, hex_b);
        assert_eq!(tombstones[0].reason, DeleteReason::Retention);
        assert_eq!(tombstones[0].last_known_class, BlobClass::TraceNoise);
        assert!(!tombstones[0].was_pinned);
        assert!(tombstones[0].size_bytes.unwrap() > 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_dry_run_does_not_delete() {
        let (tmp, paths) = setup_workspace();

        let ref_a = blob_put(&paths, b"orphan").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_a));

        let params = GcParams {
            repo_root: &tmp,
            dry_run: true,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_respects_keep_days() {
        let (tmp, paths) = setup_workspace();

        let ref_a = blob_put(&paths, b"recent orphan").unwrap();

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(90),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_skips_pinned() {
        let (tmp, paths) = setup_workspace();

        let ref_a = blob_put(&paths, b"pinned orphan").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_a));

        // Pin the blob
        let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        blob_meta::set_pinned(&mut meta, hex_a, true);
        blob_meta::save_blob_meta(&paths.blob_meta_json, &meta).unwrap();

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Pinned blob should survive
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_skips_artifact() {
        let (tmp, paths) = setup_workspace();

        let ref_a = blob_put(&paths, b"artifact orphan").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_a));

        // Classify as artifact
        let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        blob_meta::set_class(&mut meta, hex_a, BlobClass::Artifact, "test");
        blob_meta::save_blob_meta(&paths.blob_meta_json, &meta).unwrap();

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Artifact should survive
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_priority_order() {
        let (tmp, paths) = setup_workspace();

        // Create 2 blobs: one trace_noise, one decision_evidence
        let ref_noise = blob_put(&paths, b"noise data").unwrap();
        let ref_evidence = blob_put(&paths, b"evidence data").unwrap();
        let hex_noise = ref_noise.strip_prefix("blob:sha256:").unwrap();
        let hex_evidence = ref_evidence.strip_prefix("blob:sha256:").unwrap();

        set_file_time_old(&paths.blobs_dir.join(hex_noise));
        set_file_time_old(&paths.blobs_dir.join(hex_evidence));

        // Classify evidence blob
        let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json).unwrap();
        blob_meta::set_class(&mut meta, hex_evidence, BlobClass::DecisionEvidence, "test");
        blob_meta::save_blob_meta(&paths.blob_meta_json, &meta).unwrap();

        // Run GC — both should be removed (both unreferenced + expired)
        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Both should be gone (both unreferenced and expired)
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_noise).is_err());
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_evidence).is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_archive_mode() {
        let (tmp, paths) = setup_workspace();

        let ref_a = blob_put(&paths, b"archive me").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_a));

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: true,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Should NOT be in active store
        assert!(!paths.blobs_dir.join(hex_a).exists());
        // Should be in archive
        assert!(paths.archive_blobs_dir.join(hex_a).exists());
        // blob_get_path should still resolve via fallback
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_ok());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_quota_enforcement() {
        let (tmp, paths) = setup_workspace();

        // Create blobs totaling > quota
        let _ref_a = blob_put(&paths, &[0u8; 600]).unwrap(); // 600 bytes
        let _ref_b = blob_put(&paths, &[1u8; 600]).unwrap(); // 600 bytes
                                                             // Total: 1200 bytes

        // Set quota to 1 byte (force cleanup of all non-protected blobs)
        let config = serde_json::json!({"gc.blob_quota_mb": 0});
        std::fs::write(
            &paths.config_json,
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(9999),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // All blobs should be removed (quota enforcement overrides keep_days)
        let remaining = blob_list(&paths).unwrap();
        assert_eq!(remaining.len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gc_purge_archive() {
        let (tmp, paths) = setup_workspace();

        // Create and archive a blob
        let ref_a = blob_put(&paths, b"will be purged").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        edda_ledger::blob_archive(&paths, hex_a).unwrap();

        // Backdate the archived blob
        set_file_time_old(&paths.archive_blobs_dir.join(hex_a));

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: None,
            force: true,
            global: false,
            archive: false,
            purge_archive: true,
            archive_keep_days: Some(0),
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Should be completely gone
        assert!(!paths.archive_blobs_dir.join(hex_a).exists());

        // Verify tombstone was written for purged blob
        let tombstones = tombstone::list_tombstones(&paths).unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].blob_hash, hex_a);
        assert_eq!(tombstones[0].reason, DeleteReason::PurgeArchive);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn backward_compat_no_meta() {
        let (tmp, paths) = setup_workspace();

        // Create orphan blob WITHOUT any blob_meta.json
        let ref_a = blob_put(&paths, b"no meta orphan").unwrap();
        let hex_a = ref_a.strip_prefix("blob:sha256:").unwrap();
        set_file_time_old(&paths.blobs_dir.join(hex_a));

        // Ensure no blob_meta.json exists
        assert!(!paths.blob_meta_json.exists());

        let params = GcParams {
            repo_root: &tmp,
            dry_run: false,
            keep_days: Some(0),
            force: true,
            global: false,
            archive: false,
            purge_archive: false,
            archive_keep_days: None,
            include_sessions: false,
        };
        execute(&params).unwrap();

        // Should be removed (defaults to trace_noise, unpinned)
        assert!(edda_ledger::blob_store::blob_get_path(&paths, &ref_a).is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn format_size_works() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024 + 512 * 1024), "1.5 MB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    fn set_file_time_old(path: &std::path::Path) {
        use std::fs::FileTimes;
        let old_time =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let times = FileTimes::new().set_modified(old_time);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(times).unwrap();
    }

    #[test]
    fn scan_stale_state_cleans_session_heartbeats() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        // Stale heartbeat file (session.{sid}.json)
        let hb_file = state_dir.join("session.old-session-id.json");
        std::fs::write(&hb_file, r#"{"session_id":"old-session-id"}"#).unwrap();
        set_file_time_old(&hb_file);

        // Fresh heartbeat (should not be cleaned)
        let fresh_hb = state_dir.join("session.fresh-session.json");
        std::fs::write(&fresh_hb, r#"{"session_id":"fresh-session"}"#).unwrap();

        // Shared file (should not be cleaned)
        let coordination = state_dir.join("coordination.jsonl");
        std::fs::write(&coordination, "{}").unwrap();
        set_file_time_old(&coordination);

        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let mut candidates = Vec::new();
        scan_stale_state_files(&state_dir, cutoff, &mut candidates);

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].0.ends_with("session.old-session-id.json"));
    }

    // ── Session GC tests ──

    #[test]
    fn scan_expired_files_removes_old() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ledger");
        std::fs::create_dir_all(&dir).unwrap();

        // Create old and new files
        let old_file = dir.join("old-session.jsonl");
        std::fs::write(&old_file, "old data").unwrap();
        set_file_time_old(&old_file);

        let new_file = dir.join("new-session.jsonl");
        std::fs::write(&new_file, "new data").unwrap();

        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let mut candidates = Vec::new();
        scan_expired_files(&dir, "jsonl", cutoff, &mut candidates);

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].0.ends_with("old-session.jsonl"));
    }

    #[test]
    fn scan_expired_files_skips_wrong_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir_all(&dir).unwrap();

        let txt_file = dir.join("data.txt");
        std::fs::write(&txt_file, "text").unwrap();
        set_file_time_old(&txt_file);

        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let mut candidates = Vec::new();
        scan_expired_files(&dir, "jsonl", cutoff, &mut candidates);

        assert!(candidates.is_empty());
    }

    #[test]
    fn scan_stale_state_preserves_active_files() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        // Session-scoped: should be cleaned
        let hash_file = state_dir.join("inject_hash.sess-1");
        std::fs::write(&hash_file, "abcd").unwrap();
        set_file_time_old(&hash_file);

        let cursor_file = state_dir.join("transcript_cursor.sess-1.json");
        std::fs::write(&cursor_file, "{}").unwrap();
        set_file_time_old(&cursor_file);

        // Preserved: not a session-scoped prefix
        let tasks_file = state_dir.join("active_tasks.json");
        std::fs::write(&tasks_file, "[]").unwrap();
        set_file_time_old(&tasks_file);

        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let mut candidates = Vec::new();
        scan_stale_state_files(&state_dir, cutoff, &mut candidates);

        // Only session-scoped files
        assert_eq!(candidates.len(), 2);
        let names: Vec<String> = candidates
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"inject_hash.sess-1".to_string()));
        assert!(names.contains(&"transcript_cursor.sess-1.json".to_string()));
        assert!(!names.contains(&"active_tasks.json".to_string()));
    }
}
