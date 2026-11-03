//! Where projects Keel moved on this device used to be, read-only.
//!
//! Codex and Antigravity record a conversation's working directory inside their own files, and
//! Keel finds a conversation only when that directory is the project's — the path alone would let
//! any request read any conversation on the machine. A project Keel moved into its workspace keeps
//! its conversations under the old path, so the finders accept the old path too, and only the old
//! paths this record names. It lives in Keel's home and only the daemon's move writes it, after a
//! rename it performed itself: nothing in a repository can add a former path.
use crate::Real;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// The record's place under Keel's home.
pub const FILE: &str = ".keel/moved.json";

/// One move: every spelling of the folder before it (as given and resolved), and where it went.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Move {
    pub from: Vec<String>,
    pub to: String,
}

/// Every move this device's Keel made.
pub fn records() -> Vec<Move> {
    crate::home()
        .map(|home| records_at(&home.join(FILE)))
        .unwrap_or_default()
}

/// [`records`] from a file, for tests and for the writer that appends to it.
pub fn records_at(path: &Utf8Path) -> Vec<Move> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// `cwd` and every place it was before a move — `/new/app/lane` is also `/old/app/lane` — with
/// moves followed back through each other. Whole path components only: `/old/app-two` is not
/// under `/old/app`.
pub fn places_in(records: &[Move], cwd: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut out = vec![cwd.to_owned()];
    if let Ok(real) = cwd.real() {
        out.push(real);
    }
    // Each round can only add places from a record not yet used, so it ends.
    for _ in 0..records.len() {
        let mut more = Vec::new();
        for m in records {
            for place in &out {
                if let Ok(rest) = place.strip_prefix(&m.to) {
                    more.extend(vacant(m).map(|f| Utf8Path::new(f).join(rest)));
                }
            }
        }
        more.retain(|p| !out.contains(p));
        if more.is_empty() {
            break;
        }
        out.extend(more);
    }
    out.dedup();
    out
}

/// The spellings of a former path that are still empty. A folder Keel moved leaves nothing
/// behind, so anything at that path now — a fresh clone of the same name — is another project,
/// and must not be read as this one or have its sessions filed under it.
fn vacant(m: &Move) -> impl Iterator<Item = &String> {
    m.from
        .iter()
        .filter(|f| std::fs::symlink_metadata(f.as_str()).is_err())
}

/// [`places_in`] with this device's record.
pub fn places(cwd: &Utf8Path) -> Vec<Utf8PathBuf> {
    places_in(&records(), cwd)
}

/// Whether a directory an agent recorded is `cwd`, or where `cwd` was before Keel moved it.
pub fn same_place(recorded: &Utf8Path, cwd: &Utf8Path) -> bool {
    let resolved = |p: &Utf8Path| p.real().unwrap_or_else(|_| p.to_owned());
    let want = resolved(recorded);
    places(cwd).iter().any(|p| resolved(p) == want)
}

/// Where a path recorded before a move is now: the inverse of [`places_in`], for a session
/// list that files a transcript under the checkout it ran in.
pub fn current_in(records: &[Move], path: &Utf8Path) -> Utf8PathBuf {
    let mut now = path.to_owned();
    for _ in 0..records.len() {
        let next = records.iter().find_map(|m| {
            vacant(m)
                .find_map(|f| now.strip_prefix(f).ok())
                .map(|rest| Utf8Path::new(&m.to).join(rest))
        });
        match next {
            Some(next) if next != now => now = next,
            _ => break,
        }
    }
    now
}

/// [`current_in`] with this device's record.
pub fn current(path: &Utf8Path) -> Utf8PathBuf {
    current_in(&records(), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(from: &str, to: &str) -> Move {
        Move {
            from: vec![from.into()],
            to: to.into(),
        }
    }

    #[test]
    fn a_former_path_matches_whole_components_only() {
        let records = [moved("/old/app", "/ws/app")];
        let places = places_in(&records, Utf8Path::new("/ws/app/.keel/worktrees/x"));
        assert!(places.contains(&Utf8PathBuf::from("/old/app/.keel/worktrees/x")));
        let sibling = places_in(&records, Utf8Path::new("/ws/app-two"));
        assert_eq!(sibling, vec![Utf8PathBuf::from("/ws/app-two")]);
        assert_eq!(
            current_in(&records, Utf8Path::new("/old/app-two")),
            Utf8PathBuf::from("/old/app-two")
        );
    }

    #[test]
    fn moves_are_followed_through_each_other() {
        let records = [moved("/a/app", "/b/app"), moved("/b/app", "/c/app")];
        let places = places_in(&records, Utf8Path::new("/c/app"));
        assert!(places.contains(&Utf8PathBuf::from("/b/app")));
        assert!(places.contains(&Utf8PathBuf::from("/a/app")));
        assert_eq!(
            current_in(&records, Utf8Path::new("/a/app/src")),
            Utf8PathBuf::from("/c/app/src")
        );
    }

    /// A new project made at the old path is its own: neither read as the moved one, nor filed
    /// under it.
    #[test]
    fn a_former_path_that_is_in_use_again_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let old = Utf8PathBuf::from_path_buf(d.path().join("app")).unwrap();
        std::fs::create_dir(&old).unwrap();
        let records = [moved(old.as_str(), "/ws/app")];
        assert_eq!(
            places_in(&records, Utf8Path::new("/ws/app")),
            vec![Utf8PathBuf::from("/ws/app")]
        );
        assert_eq!(current_in(&records, &old), old);
    }

    #[test]
    fn a_missing_or_broken_record_is_no_former_paths() {
        let d = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(d.path().join("moved.json")).unwrap();
        assert!(records_at(&path).is_empty());
        std::fs::write(&path, "{not json").unwrap();
        assert!(records_at(&path).is_empty());
    }
}
