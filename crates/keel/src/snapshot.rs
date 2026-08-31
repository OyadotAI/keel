//! Rewind: the working tree as it was before a turn.
//!
//! Every ADE that has this is loved for it, and the one that made it irreversible (Zed, #28676)
//! is hated for that. So a snapshot is cheap enough to take before every turn, and a restore
//! takes one of itself first, so that undoing the undo is one more click.
//!
//! A snapshot is a git *tree* written from a throwaway index: tracked and untracked files alike,
//! ignored files never. It is not a commit, not a stash and not a ref — nothing appears in the
//! user's history, and an unreferenced tree is reclaimed by `git gc` after its grace period. The
//! repository is the store, because it already knows how to hold a version of every file.

use axum::Json;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::serve::Checkout;

/// Snapshot speaks to git in two ways: against an index of its own while building a tree, and
/// against the repository's own for everything else.
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
    let result = git(root, Some(&index), &["add", "-A", "--", "."])
        .and_then(|_| git(root, Some(&index), &["write-tree"]));
    let _ = std::fs::remove_file(&index);
    result
}

/// Put the working tree back to a snapshot. Returns the snapshot taken first, which undoes this.
///
/// `clean -fd` without `-x`: files created since the snapshot go, ignored files stay. A rewind
/// that deleted `node_modules` and `.env` would be a rewind nobody used twice.
pub fn restore(root: &Utf8Path, tree: &str) -> Result<String, String> {
    if tree.len() != 40 || !tree.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("not a snapshot id".into());
    }
    git(root, None, &["cat-file", "-e", &format!("{tree}^{{tree}}")])
        .map_err(|_| "that snapshot is no longer in the repository".to_string())?;
    let undo = snapshot(root)?;
    git(root, None, &["read-tree", tree])?;
    git(root, None, &["checkout-index", "-a", "-f"])?;
    git(root, None, &["clean", "-fd"])?;
    Ok(undo)
}

#[derive(Serialize)]
pub struct Snapshot {
    pub tree: String,
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

pub async fn take(
    Checkout(repo): Checkout,
) -> Result<Json<Snapshot>, (axum::http::StatusCode, String)> {
    tokio::task::spawn_blocking(move || snapshot(&repo))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .map(|tree| Json(Snapshot { tree }))
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
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
