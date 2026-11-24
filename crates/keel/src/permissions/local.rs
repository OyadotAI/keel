//! Execution records reuse the permission store's platform ownership and no-follow boundaries.
use camino::Utf8PathBuf;
use std::io::{Read, Write};

/// Explicit unlock prevents a concurrent fork's inherited descriptor from retaining a finished lease.
pub(crate) struct Lease(std::fs::File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Cross-process ownership is nonblocking; an active owner can be observed without waiting behind it.
pub(crate) fn try_lease(name: &str) -> Result<Option<Lease>, String> {
    let file = super::private_lock(name)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Lease(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn component(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

/// Keep execution authority outside project content, with the same private-directory checks as trust.
pub(crate) fn local_path(area: &str, name: &str) -> Result<Utf8PathBuf, String> {
    if !component(area) || !component(name) {
        return Err("Invalid local storage name.".into());
    }
    let dir = super::permissions_dir()?.with_file_name(area);
    super::private::create_dir(&dir).map_err(|e| e.to_string())?;
    let metadata = std::fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || !super::private::owned_and_private(&metadata, false)
        || !super::private::trusted_location(&dir)
    {
        return Err(
            "Execution storage must be a private, user-owned directory without links.".into(),
        );
    }
    Ok(dir.join(name))
}

fn open(area: &str, name: &str) -> Result<Option<std::fs::File>, String> {
    let path = local_path(area, name)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    super::private::no_follow(&mut options);
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || !super::private::owned_and_private(&metadata, true) {
        return Err(
            "Execution records must be regular, private user-owned files without links.".into(),
        );
    }
    Ok(Some(file))
}

/// Bounded reads distinguish absence from damage; damaged execution state never becomes an empty store.
pub(crate) fn read_local(area: &str, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(file) = open(area, name)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(
            "Execution storage exceeds its size limit. Existing records were preserved.".into(),
        );
    }
    Ok(Some(bytes))
}

/// Fsync before replacement makes a saved dispatch receipt durable before any runtime side effect.
pub(crate) fn replace_local(area: &str, name: &str, body: &[u8]) -> Result<(), String> {
    let path = local_path(area, name)?;
    let _ = open(area, name)?;
    let dir = path
        .parent()
        .ok_or("Execution storage has no parent directory.")?;
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    file.write_all(body)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    let _ = open(area, name)?;
    file.persist(&path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
