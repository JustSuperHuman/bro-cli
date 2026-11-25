//! Shared locked, atomic storage for the browser workers' independent outboxes.
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub(super) fn update<T: Serialize + DeserializeOwned, R>(
    root: &Path,
    name: &str,
    change: impl FnOnce(&mut BTreeMap<String, T>) -> R,
) -> std::io::Result<R> {
    std::fs::create_dir_all(root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("{name}.lock")))?;
    lock.lock()?;
    let path = root.join(format!("{name}.json"));
    let before = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut pending = before
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(std::io::Error::other)?
        .unwrap_or_default();
    let result = change(&mut pending);
    let bytes = serde_json::to_vec(&pending).map_err(std::io::Error::other)?;
    if before.as_deref() == Some(bytes.as_slice()) {
        return Ok(result);
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, path)?;
    Ok(result)
}
