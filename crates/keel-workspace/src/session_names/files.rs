//! Checked metadata paths and atomic replacements shared by native naming adapters.
use camino::Utf8Path;
use std::io::Write;

/// Reject identifiers before they become filenames or provider request keys.
pub(super) fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("That is not a session id".into());
    }
    Ok(())
}

/// Do not follow an attacker-controlled metadata link outside the provider's private root.
pub(super) fn checked(root: &Utf8Path, path: &Utf8Path) -> Result<(), String> {
    let mut ancestor = path.to_owned();
    while ancestor != root {
        match std::fs::symlink_metadata(&ancestor) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Linked session metadata is not writable".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        ancestor = ancestor
            .parent()
            .ok_or("Session metadata escaped its provider root")?
            .to_owned();
    }
    Ok(())
}

/// Replace naming metadata beside itself, so readers never observe a partially written title.
pub(super) fn replace(root: &Utf8Path, path: &Utf8Path, body: &[u8]) -> Result<(), String> {
    checked(root, path)?;
    let parent = path.parent().ok_or("No metadata directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temp = parent.join(format!(".keel-name-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(body)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// SQLite reads must see a live WAL; immutable fallback is reserved for a closed database.
pub(super) fn database(path: &Utf8Path) -> Result<rusqlite::Connection, String> {
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    Ok(c)
}
