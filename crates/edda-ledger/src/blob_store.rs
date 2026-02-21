use crate::blob_meta::{self, BlobClass};
use crate::paths::EddaPaths;
use edda_core::hash::sha256_hex;
use std::io::Write;
use std::path::PathBuf;

/// Metadata for a blob in the store.
pub struct BlobInfo {
    /// The hex hash (filename in blobs/).
    pub hash: String,
    /// File size in bytes.
    pub size: u64,
}

/// List all blobs in the blob store directory with their sizes.
pub fn blob_list(paths: &EddaPaths) -> anyhow::Result<Vec<BlobInfo>> {
    if !paths.blobs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut blobs = Vec::new();
    for entry in std::fs::read_dir(&paths.blobs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip temp files
        if name.starts_with(".tmp_") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                blobs.push(BlobInfo {
                    hash: name,
                    size: meta.len(),
                });
            }
        }
    }
    Ok(blobs)
}

/// Remove a blob file by its hash. Returns bytes freed.
pub fn blob_remove(paths: &EddaPaths, hash: &str) -> anyhow::Result<u64> {
    let path = paths.blobs_dir.join(hash);
    if !path.exists() {
        anyhow::bail!("blob not found: {hash}");
    }
    let size = path.metadata()?.len();
    std::fs::remove_file(&path)?;
    Ok(size)
}

/// Get size of a blob by hash.
pub fn blob_size(paths: &EddaPaths, hash: &str) -> anyhow::Result<u64> {
    let path = paths.blobs_dir.join(hash);
    if !path.exists() {
        anyhow::bail!("blob not found: {hash}");
    }
    Ok(path.metadata()?.len())
}

/// Write bytes to the blob store. Returns `blob:sha256:<hex>`.
/// Atomic: writes to a temp file first, then renames.
/// Idempotent: if the blob already exists, returns immediately.
pub fn blob_put(paths: &EddaPaths, bytes: &[u8]) -> anyhow::Result<String> {
    let hex = sha256_hex(bytes);
    let final_path = paths.blobs_dir.join(&hex);
    let blob_ref = format!("blob:sha256:{hex}");

    // Content-addressable: if it exists, it's identical
    if final_path.exists() {
        return Ok(blob_ref);
    }

    // Atomic write: tmp file → rename
    let tmp_path = paths.blobs_dir.join(format!(".tmp_{hex}"));
    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&tmp_path, &final_path)?;
    Ok(blob_ref)
}

/// Resolve a blob ref to its filesystem path.
/// Checks active blobs first, then falls back to archive.
/// Returns an error if the blob does not exist in either location.
pub fn blob_get_path(paths: &EddaPaths, blob_ref: &str) -> anyhow::Result<PathBuf> {
    let hex = blob_ref
        .strip_prefix("blob:sha256:")
        .ok_or_else(|| anyhow::anyhow!("invalid blob ref format: {blob_ref}"))?;
    let active_path = paths.blobs_dir.join(hex);
    if active_path.exists() {
        return Ok(active_path);
    }
    // Fallback: check archive
    let archive_path = paths.archive_blobs_dir.join(hex);
    if archive_path.exists() {
        return Ok(archive_path);
    }
    anyhow::bail!("blob not found: {blob_ref}");
}

/// Move a blob from active store to archive. Returns bytes archived.
/// Creates archive directory on demand.
pub fn blob_archive(paths: &EddaPaths, hash: &str) -> anyhow::Result<u64> {
    let src = paths.blobs_dir.join(hash);
    if !src.exists() {
        anyhow::bail!("blob not found in active store: {hash}");
    }
    let size = src.metadata()?.len();
    std::fs::create_dir_all(&paths.archive_blobs_dir)?;
    let dst = paths.archive_blobs_dir.join(hash);
    std::fs::rename(&src, &dst)?;
    Ok(size)
}

/// Check if a blob exists in the archive (but not in active store).
pub fn blob_is_archived(paths: &EddaPaths, hash: &str) -> bool {
    let active = paths.blobs_dir.join(hash);
    let archive = paths.archive_blobs_dir.join(hash);
    !active.exists() && archive.exists()
}

/// Write bytes to the blob store with classification metadata.
/// Returns `blob:sha256:<hex>`.
pub fn blob_put_classified(
    paths: &EddaPaths,
    bytes: &[u8],
    class: BlobClass,
) -> anyhow::Result<String> {
    let blob_ref = blob_put(paths, bytes)?;
    let hex = blob_ref
        .strip_prefix("blob:sha256:")
        .expect("blob_put always returns blob:sha256: prefix");
    // Write classification to blob_meta.json
    let mut meta = blob_meta::load_blob_meta(&paths.blob_meta_json)?;
    blob_meta::set_class(&mut meta, hex, class, "auto");
    blob_meta::save_blob_meta(&paths.blob_meta_json, &meta)?;
    Ok(blob_ref)
}

/// List archived blobs with their sizes.
pub fn blob_list_archived(paths: &EddaPaths) -> anyhow::Result<Vec<BlobInfo>> {
    if !paths.archive_blobs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut blobs = Vec::new();
    for entry in std::fs::read_dir(&paths.archive_blobs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                blobs.push(BlobInfo {
                    hash: name,
                    size: meta.len(),
                });
            }
        }
    }
    Ok(blobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let blob_ref = blob_put(&p, b"hello world").unwrap();
        assert!(blob_ref.starts_with("blob:sha256:"));

        let path = blob_get_path(&p, &blob_ref).unwrap();
        assert!(path.exists());
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"hello world");

        // Idempotent: second put returns same ref
        let blob_ref2 = blob_put(&p, b"hello world").unwrap();
        assert_eq!(blob_ref, blob_ref2);

        // No tmp files should remain
        let tmp_files: Vec<_> = std::fs::read_dir(&p.blobs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp_"))
            .collect();
        assert!(tmp_files.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn get_missing_blob_errors() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_miss_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        assert!(blob_get_path(&p, "blob:sha256:deadbeef").is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn invalid_ref_format_errors() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_fmt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        assert!(blob_get_path(&p, "not_a_blob_ref").is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_list_returns_all_hashes() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_list_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        blob_put(&p, b"aaa").unwrap();
        blob_put(&p, b"bbb").unwrap();
        blob_put(&p, b"ccc").unwrap();

        let list = blob_list(&p).unwrap();
        assert_eq!(list.len(), 3);
        for info in &list {
            assert!(info.size > 0);
            assert!(!info.hash.starts_with(".tmp_"));
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_remove_frees_space() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_rm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let blob_ref = blob_put(&p, b"remove me").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();
        let freed = blob_remove(&p, hex).unwrap();
        assert!(freed > 0);

        // Should be gone now
        assert!(blob_get_path(&p, &blob_ref).is_err());
        assert_eq!(blob_list(&p).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_remove_nonexistent_errors() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_rmerr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        assert!(blob_remove(&p, "deadbeef").is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_size_returns_correct_value() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_sz_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let data = b"hello world size test";
        let blob_ref = blob_put(&p, data).unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();
        let size = blob_size(&p, hex).unwrap();
        assert_eq!(size, data.len() as u64);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_archive_and_fallback() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_arch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let blob_ref = blob_put(&p, b"archive me").unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        // Archive the blob
        let archived_size = blob_archive(&p, hex).unwrap();
        assert!(archived_size > 0);

        // Should not be in active store
        assert!(!p.blobs_dir.join(hex).exists());
        // Should be in archive
        assert!(p.archive_blobs_dir.join(hex).exists());
        assert!(blob_is_archived(&p, hex));

        // blob_get_path should still resolve via archive fallback
        let resolved = blob_get_path(&p, &blob_ref).unwrap();
        assert_eq!(resolved, p.archive_blobs_dir.join(hex));

        // blob_list should NOT include archived blobs
        assert_eq!(blob_list(&p).unwrap().len(), 0);

        // blob_list_archived should include it
        let archived = blob_list_archived(&p).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].hash, hex);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blob_put_classified_writes_meta() {
        let tmp = std::env::temp_dir().join(format!("edda_blob_clsfy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();

        let blob_ref = blob_put_classified(&p, b"classified data", BlobClass::Artifact).unwrap();
        let hex = blob_ref.strip_prefix("blob:sha256:").unwrap();

        // Verify metadata was written
        let meta = crate::blob_meta::load_blob_meta(&p.blob_meta_json).unwrap();
        let entry = crate::blob_meta::get_meta(&meta, hex);
        assert_eq!(entry.class, BlobClass::Artifact);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
