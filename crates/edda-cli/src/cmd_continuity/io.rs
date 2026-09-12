use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const TEMP_ATTEMPTS: u64 = 128;

pub fn read_bounded(path: &Path, limit: usize, label: &str) -> anyhow::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        anyhow::bail!("{label} exceeds the {limit}-byte bound");
    }
    Ok(bytes)
}

/// Publishes a complete file without replacing an existing destination.
///
/// The completed temporary file is linked into place atomically in the same
/// directory. Filesystems without hard-link support fail before creating the
/// destination; callers never observe a partial destination under its final
/// name.
pub fn write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_new_with(path, bytes, complete_file::<File>)
}

fn write_new_with<F>(path: &Path, bytes: &[u8], complete: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
{
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        anyhow::bail!("export destination parent does not exist");
    }

    let (temporary_path, mut temporary) = create_temporary(parent, path.file_name())?;
    let completion = complete(&mut temporary, bytes);
    drop(temporary);
    if let Err(error) = completion {
        return Err(failure_with_cleanup(&temporary_path, error));
    }

    if let Err(error) = std::fs::hard_link(&temporary_path, path) {
        return Err(failure_with_cleanup(&temporary_path, error));
    }
    // Publication succeeded. A cleanup failure leaves only a complete
    // same-directory hard-link, so it must not turn success into an unsafe
    // retry that collides with the already-published destination.
    let _ = std::fs::remove_file(&temporary_path);
    Ok(())
}

trait CompletionIo {
    fn write_all_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn flush_bytes(&mut self) -> std::io::Result<()>;
    fn sync_bytes(&mut self) -> std::io::Result<()>;
}

impl CompletionIo for File {
    fn write_all_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_all(bytes)
    }

    fn flush_bytes(&mut self) -> std::io::Result<()> {
        self.flush()
    }

    fn sync_bytes(&mut self) -> std::io::Result<()> {
        self.sync_all()
    }
}

fn complete_file<W: CompletionIo>(file: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    file.write_all_bytes(bytes)?;
    file.flush_bytes()?;
    file.sync_bytes()
}

fn failure_with_cleanup(path: &Path, operation: std::io::Error) -> anyhow::Error {
    match std::fs::remove_file(path) {
        Ok(()) => operation.into(),
        Err(cleanup) => anyhow::anyhow!(
            "export failed before publication ({operation}); temporary cleanup also failed ({cleanup})"
        ),
    }
}

fn create_temporary(
    parent: &Path,
    destination_name: Option<&std::ffi::OsStr>,
) -> anyhow::Result<(PathBuf, File)> {
    let process = std::process::id();
    let start = TEMP_SEQUENCE.fetch_add(TEMP_ATTEMPTS, Ordering::Relaxed);
    for offset in 0..TEMP_ATTEMPTS {
        let path = parent.join(format!(
            ".edda-continuity-export-{process}-{}.tmp",
            start + offset
        ));
        if path.file_name() == destination_name {
            continue;
        }
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("unable to reserve a temporary export file")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum FailureStage {
        Write,
        Flush,
        Sync,
    }

    struct FailingCompletion<'a> {
        file: &'a mut File,
        stage: FailureStage,
    }

    impl CompletionIo for FailingCompletion<'_> {
        fn write_all_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            if matches!(self.stage, FailureStage::Write) {
                return Err(std::io::Error::other("injected write failure"));
            }
            self.file.write_all(bytes)
        }

        fn flush_bytes(&mut self) -> std::io::Result<()> {
            if matches!(self.stage, FailureStage::Flush) {
                return Err(std::io::Error::other("injected flush failure"));
            }
            self.file.flush()
        }

        fn sync_bytes(&mut self) -> std::io::Result<()> {
            if matches!(self.stage, FailureStage::Sync) {
                return Err(std::io::Error::other("injected sync failure"));
            }
            self.file.sync_all()
        }
    }

    fn fail_completion_at(
        stage: FailureStage,
    ) -> impl FnOnce(&mut File, &[u8]) -> std::io::Result<()> {
        move |file, bytes| complete_file(&mut FailingCompletion { file, stage }, bytes)
    }

    #[test]
    fn completion_failures_leave_no_final_destination_and_retry_safely() {
        for stage in [FailureStage::Write, FailureStage::Flush, FailureStage::Sync] {
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("bundle.json");

            write_new_with(&destination, b"complete", fail_completion_at(stage))
                .expect_err("injected completion failure must be returned");

            assert!(!destination.exists());
            assert!(std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none());
            write_new(&destination, b"complete").unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), b"complete");
        }
    }

    #[test]
    fn publication_never_clobbers_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("bundle.json");
        std::fs::write(&destination, b"existing").unwrap();

        write_new(&destination, b"replacement").expect_err("existing output must be refused");

        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
