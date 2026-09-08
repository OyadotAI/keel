//! Rewind: the working tree as it was before a turn.
//!
//! Every ADE that has this is loved for it, and the one that made it irreversible (Zed, #28676)
//! is hated for that. So a snapshot is cheap enough to take before every turn, and a restore
//! takes one of itself first, so that undoing the undo is one more click.
//!
//! A snapshot is a git *tree* written from a throwaway index: all tracked files, including those
//! matching ignore rules, plus nonignored untracked files. It is not a commit, stash or ref:
//! nothing appears in the user's history, and an unreferenced tree is reclaimed by `git gc`
//! after its grace period. The repository is the store, because it already knows how to hold a
//! version of every file.

use axum::Json;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::serve::Checkout;

/// Snapshot speaks to git in two ways: against an index of its own while building or restoring
/// a tree, and against the repository's own for discovery and validation.
fn git(root: &Utf8Path, index: Option<&Utf8Path>, args: &[&str]) -> Result<String, String> {
    match index {
        Some(index) => crate::git::with_index(root, index, args),
        None => crate::git::trimmed(root, args),
    }
}

/// Photograph the working tree. Returns the tree id.
///
/// ponytail: O(files) per turn — `add -A` hashes what changed and stats the rest. Fine up to
/// monorepo scale; past that, snapshot only the turn's touched paths.
pub fn snapshot(root: &Utf8Path) -> Result<String, String> {
    with_temporary_index(root, |index| {
        // Starting empty makes every file untracked, including tracked files that now match an
        // ignore rule. Seed from the real index so `add -A` retains Git's tracking decisions,
        // while recording the working copy without changing anything the user has staged.
        let real = root.join(git(root, None, &["rev-parse", "--git-path", "index"])?);
        match std::fs::copy(real, index) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        git(root, Some(index), &["add", "-A", "--", "."])?;
        git(root, Some(index), &["write-tree"])
    })
}

fn with_temporary_index<T>(
    root: &Utf8Path,
    work: impl FnOnce(&Utf8Path) -> Result<T, String>,
) -> Result<T, String> {
    if !crate::gitroots::is_root(root) {
        return Err("This folder is not a git repository, so it cannot be rewound.".into());
    }
    let git_dir = git(root, None, &["rev-parse", "--git-dir"])?;
    // Per *call*, not per process. It used to be `keel-index-{pid}`, and snapshots are not rare
    // or serialised: one is taken at the start of every turn, and `restore` takes one of its own
    // before it writes. Two of them at once in a shared working tree — two lanes, or a rewind
    // beside a turn — meant the second `remove_file` deleted the first's index while its
    // `add -A` was still writing it, and the first `write-tree` then photographed whatever the
    // second had staged. The failure is silent and lands later, as a rewind that restores a tree
    // that was never there.
    let n = {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
    let index = root
        .join(git_dir)
        .join(format!("keel-index-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let result = work(&index);
    let _ = std::fs::remove_file(&index);
    result
}

/// Put the working tree back to a snapshot. Returns the snapshot taken first, which undoes this.
///
/// Apply the difference between the current snapshot and the requested one using an isolated
/// index. Git handles removed paths and file/directory transitions. A preflight check refuses
/// ignored-path collisions, and the isolated index leaves the user's staged version alone.
pub fn restore(root: &Utf8Path, tree: &str) -> Result<String, String> {
    if tree.len() != 40 || !tree.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("not a snapshot id".into());
    }
    git(root, None, &["cat-file", "-e", &format!("{tree}^{{tree}}")])
        .map_err(|_| "that snapshot is no longer in the repository".to_string())?;
    // Git permits ignored files to be overwritten even during a non-forced tree merge. Those
    // files are deliberately absent from our undo snapshot, so refuse overlapping paths first.
    // Collapsing ignored directories keeps dependency trees out of this listing.
    let ignored = crate::git::run(
        root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )?;
    if !ignored.is_empty() {
        let paths = crate::git::run(root, &["ls-tree", "-r", "--name-only", "-z", tree])?;
        let files: std::collections::HashSet<_> =
            paths.split('\0').filter(|p| !p.is_empty()).collect();
        let directories: std::collections::HashSet<_> = files
            .iter()
            .flat_map(|p| Utf8Path::new(p).ancestors().skip(1))
            .map(Utf8Path::as_str)
            .collect();
        for ignored in ignored.split('\0').filter(|p| !p.is_empty()) {
            let path = ignored.trim_end_matches('/');
            if directories.contains(path)
                || Utf8Path::new(path)
                    .ancestors()
                    .any(|p| files.contains(p.as_str()))
            {
                return Err(format!(
                    "Rewinding would overwrite ignored path {path}. Move it aside first."
                ));
            }
        }
    }
    let undo = snapshot(root)?;
    with_temporary_index(root, |index| {
        git(root, Some(index), &["read-tree", &undo])?;
        git(root, Some(index), &["update-index", "--refresh"])?;
        git(root, Some(index), &["read-tree", "-m", "-u", &undo, tree])?;
        Ok(())
    })?;
    Ok(undo)
}

#[derive(Deserialize)]
pub struct RestoreBody {
    pub tree: String,
}

#[derive(Serialize)]
pub struct Restored {
    /// The snapshot of what was there before the restore — restore this to undo it.
    pub undo: String,
}

pub async fn put_back(
    Checkout(repo): Checkout,
    Json(body): Json<RestoreBody>,
) -> Result<Json<Restored>, (axum::http::StatusCode, String)> {
    tokio::task::spawn_blocking(move || restore(&repo, &body.tree))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .map(|undo| Json(Restored { undo }))
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
        ] {
            git(&root, None, args).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "secret\n").unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, None, &["add", "-A"]).unwrap();
        git(&root, None, &["commit", "--quiet", "-m", "seed"]).unwrap();
        (dir, root)
    }

    /// Edit a tracked file, add a new one, then rewind: both come back, and an ignored file that
    /// was never photographed is left alone. Then undo the rewind and the edits are back.
    #[test]
    fn a_turn_can_be_rewound_and_the_rewind_undone() {
        let (_d, root) = repo();
        std::fs::write(root.join("secret"), "token").unwrap();
        let before = snapshot(&root).unwrap();
        assert_eq!(before.len(), 40);

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "hello\n").unwrap();
        std::fs::write(root.join("secret"), "rotated").unwrap();

        let undo = restore(&root, &before).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(
            !root.join("new.txt").exists(),
            "a file created after the snapshot is gone"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("secret")).unwrap(),
            "rotated",
            "ignored files are never touched"
        );
        assert!(
            git(&root, None, &["log", "--oneline"])
                .unwrap()
                .lines()
                .count()
                == 1,
            "nothing was committed"
        );

        restore(&root, &undo).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn only_a_real_snapshot_is_restored() {
        let (_d, root) = repo();
        assert!(restore(&root, "HEAD").is_err());
        assert!(restore(&root, "../../etc").is_err());
        assert!(
            restore(&root, &"0".repeat(40)).is_err(),
            "well-formed but absent"
        );
    }

    #[test]
    fn snapshots_include_tracked_files_even_when_they_match_ignore_rules() {
        let (_dir, root) = repo();
        std::fs::write(root.join(".gitignore"), "secret\na.txt\n").unwrap();
        std::fs::write(root.join("a.txt"), "tracked despite ignore\n").unwrap();
        let before = snapshot(&root).unwrap();
        assert_eq!(
            git(&root, None, &["show", &format!("{before}:a.txt")]).unwrap(),
            "tracked despite ignore"
        );
    }

    #[test]
    fn snapshots_do_not_use_an_ancestor_repository() {
        let (_dir, root) = repo();
        let inner = root.join("plain-folder");
        std::fs::create_dir(&inner).unwrap();
        assert!(snapshot(&inner).is_err());
    }

    #[test]
    fn rewinding_preserves_the_users_staged_version() {
        let (_dir, root) = repo();
        let before = snapshot(&root).unwrap();
        std::fs::write(root.join("a.txt"), "staged work\n").unwrap();
        git(&root, None, &["add", "a.txt"]).unwrap();
        std::fs::write(root.join("a.txt"), "later unstaged work\n").unwrap();
        let staged = git(&root, None, &["write-tree"]).unwrap();

        let undo = restore(&root, &before).unwrap();
        assert_eq!(git(&root, None, &["write-tree"]).unwrap(), staged);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n"
        );
        restore(&root, &undo).unwrap();
        assert_eq!(git(&root, None, &["write-tree"]).unwrap(), staged);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "later unstaged work\n"
        );
    }

    #[test]
    fn rewinding_handles_file_directory_transitions_and_can_undo_them() {
        let (_dir, root) = repo();
        let before = snapshot(&root).unwrap();
        std::fs::remove_file(root.join("a.txt")).unwrap();
        std::fs::create_dir(root.join("a.txt")).unwrap();
        std::fs::write(root.join("a.txt/new.txt"), "nested work\n").unwrap();

        let undo = restore(&root, &before).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n"
        );
        restore(&root, &undo).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt/new.txt")).unwrap(),
            "nested work\n"
        );
    }

    #[test]
    fn rewinding_refuses_to_overwrite_an_ignored_file() {
        let (_dir, root) = repo();
        std::fs::write(root.join("new.txt"), "snapshot content\n").unwrap();
        let before = snapshot(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "secret\nnew.txt\n").unwrap();
        std::fs::write(root.join("new.txt"), "private content\n").unwrap();

        assert!(restore(&root, &before).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "private content\n"
        );
    }

    /// Snapshots taken at the same time must not photograph each other's index.
    ///
    /// One is taken at the start of every turn and one inside every `restore`, so with more than
    /// one lane they overlap as a matter of course. With a per-process index name this failed
    /// outright or returned a tree built from another caller's staging.
    #[test]
    fn concurrent_snapshots_of_one_tree_agree() {
        let (_dir, root) = repo();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("b.txt"), "two\n").unwrap();

        let expected = snapshot(&root).expect("a baseline");
        let results: Vec<Result<String, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let root = root.clone();
                    scope.spawn(move || snapshot(&root))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for r in &results {
            let tree = r.as_ref().expect("every snapshot must succeed");
            assert_eq!(
                tree, &expected,
                "a snapshot saw another caller's index: {results:?}"
            );
        }
    }
}
