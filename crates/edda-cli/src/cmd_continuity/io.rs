use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

pub fn read_bounded(path: &Path, limit: usize, label: &str) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        anyhow::bail!("{label} exceeds the {limit}-byte bound");
    }
    Ok(bytes)
}

pub fn write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("export destination has no parent"))?;
    if !parent.is_dir() {
        anyhow::bail!("export destination parent does not exist");
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}
