//! One-time carry-over of the app's saved state from the `ai.oya.keel` bundle identifier.
//!
//! The rename to `ai.oya.factory` (0.4.89) gave the webview a new storage folder, and an update
//! opened onto an empty window: every project, lane and saved conversation was still on disk under
//! the old identifier, read by nothing. This runs before any webview exists and copies the old
//! storage across once. It copies, never moves — the old folder is left as it was — and whatever
//! the new identifier had already written is kept beside the copy, not deleted. Any failure is
//! reported and skipped: a migration that blocks launch is worse than one that did not run.
use std::path::{Path, PathBuf};

const OLD: &str = "ai.oya.keel";
const NEW: &str = "ai.oya.factory";
/// Present once the carry-over has been done (or found nothing to do), so it runs exactly once.
const MARKER: &str = ".migrated-from-ai.oya.keel";

/// One folder to carry over: replaced whole (the page's storage) or merged by entry (per-project
/// browser logins, which are separate stores keyed by project).
struct Part {
    rel: &'static [&'static str],
    whole: bool,
}

/// What the old identifier kept that the page or the Browser tab reads back.
#[cfg(target_os = "macos")]
const PARTS: &[Part] = &[
    // The page's localStorage, with its `salt`: WebKit names each origin's folder by a salted
    // hash, so the folder only resolves to `tauri://localhost` alongside the salt it was made with.
    Part {
        rel: &["WebsiteData", "Default"],
        whole: true,
    },
    // The Browser tab's per-project logins (`data_store_identifier`).
    Part {
        rel: &["WebsiteDataStore"],
        whole: false,
    },
];
#[cfg(windows)]
const PARTS: &[Part] = &[
    // WebView2 keys Local Storage by origin, so the folder is portable as it is. Browser-tab
    // profiles already stay under `ai.oya.keel` (`browser::browser_profile_directory`).
    Part {
        rel: &["EBWebView", "Default", "Local Storage"],
        whole: true,
    },
];
#[cfg(not(any(target_os = "macos", windows)))]
const PARTS: &[Part] = &[];

/// Where each platform's webview keeps an identifier's data.
fn base() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("WebKit"));
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[allow(unreachable_code)]
    None
}

/// Called first thing in `main`, before the window and its webview are created.
pub fn migrate() {
    let Some(base) = base() else { return };
    if let Err(e) = carry(&base.join(OLD), &base.join(NEW), PARTS) {
        eprintln!("Oya Factory: could not carry over saved state from {OLD}: {e}");
    }
}

/// Copies each part from `old` to `new` unless `new` already carries the marker.
fn carry(old: &Path, new: &Path, parts: &[Part]) -> std::io::Result<()> {
    if new.join(MARKER).exists() {
        return Ok(());
    }
    for part in parts {
        let from = part.rel.iter().fold(old.to_path_buf(), |p, c| p.join(c));
        let to = part.rel.iter().fold(new.to_path_buf(), |p, c| p.join(c));
        if !from.is_dir() {
            continue;
        }
        if part.whole {
            replace(&from, &to)?;
        } else {
            merge(&from, &to)?;
        }
    }
    std::fs::create_dir_all(new)?;
    std::fs::write(
        new.join(MARKER),
        format!("Copied from {OLD} on first launch as {NEW}.\n"),
    )
}

/// `to` becomes a copy of `from`. The copy is staged beside it and swapped in, so a failure halfway
/// leaves `to` as it was; what `to` held before is kept as `<name>.before-migration`.
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let staged = sibling(to, "migrating");
    let _ = std::fs::remove_dir_all(&staged);
    copy_dir(from, &staged)?;
    if to.exists() {
        let kept = sibling(to, "before-migration");
        let _ = std::fs::remove_dir_all(&kept);
        std::fs::rename(to, &kept)?;
    }
    std::fs::rename(&staged, to)
}

/// Each entry of `from` that `to` does not already have, copied whole.
fn merge(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if dest.exists() {
            continue;
        }
        if entry.file_type()?.is_dir() {
            let staged = sibling(&dest, "migrating");
            let _ = std::fs::remove_dir_all(&staged);
            copy_dir(&entry.path(), &staged)?;
            std::fs::rename(&staged, &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}

/// A plain recursive copy. Links are skipped: the webview writes none, and following one could
/// copy something outside its own folder.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE: Part = Part {
        rel: &["Data", "Default"],
        whole: true,
    };
    const MERGED: Part = Part {
        rel: &["Stores"],
        whole: false,
    };

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// The case that shipped: the new identifier already started fresh storage, and the old one
    /// holds the person's lanes. The old wins, the new is kept aside, the old is untouched.
    #[test]
    fn old_saved_state_replaces_a_fresh_store_and_nothing_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        write(&old.join("Data/Default/salt"), "s");
        write(&old.join("Data/Default/origin/LocalStorage/db"), "lanes");
        write(&new.join("Data/Default/origin2/LocalStorage/db"), "fresh");
        carry(&old, &new, &[WHOLE]).unwrap();
        assert_eq!(
            read(&new.join("Data/Default/origin/LocalStorage/db")),
            "lanes"
        );
        assert_eq!(read(&new.join("Data/Default/salt")), "s");
        assert_eq!(
            read(&new.join("Data/Default.before-migration/origin2/LocalStorage/db")),
            "fresh"
        );
        assert_eq!(
            read(&old.join("Data/Default/origin/LocalStorage/db")),
            "lanes",
            "copied, not moved"
        );
    }

    /// It runs once: after the marker, the new store is the person's and is never overwritten.
    #[test]
    fn it_runs_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        write(&old.join("Data/Default/db"), "old");
        carry(&old, &new, &[WHOLE]).unwrap();
        write(&new.join("Data/Default/db"), "used since");
        carry(&old, &new, &[WHOLE]).unwrap();
        assert_eq!(read(&new.join("Data/Default/db")), "used since");
    }

    /// Per-project stores are merged: one the new identifier already made is left alone.
    #[test]
    fn per_project_stores_are_added_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        write(&old.join("Stores/a/cookies"), "old a");
        write(&old.join("Stores/b/cookies"), "old b");
        write(&new.join("Stores/b/cookies"), "new b");
        carry(&old, &new, &[MERGED]).unwrap();
        assert_eq!(read(&new.join("Stores/a/cookies")), "old a");
        assert_eq!(read(&new.join("Stores/b/cookies")), "new b");
    }

    /// A fresh install has nothing to carry: it is marked done and launches as normal.
    #[test]
    fn nothing_to_carry_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        carry(&old, &new, &[WHOLE, MERGED]).unwrap();
        assert!(new.join(MARKER).exists());
        assert!(!new.join("Data").exists());
    }
}
