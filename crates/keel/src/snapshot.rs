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

fn git(root: &Utf8Path, index: Option<&Utf8Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(root).args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if why.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            why
        })
    }
}

/// Photograph the working tree. Returns the tree id.
///
/// ponytail: O(files) per turn — `add -A` hashes what changed and stats the rest. Fine up to
/// monorepo scale; past that, snapshot only the turn's touched paths.
pub fn snapshot(root: &Utf8Path) -> Result<String, String> {
    let git_dir = git(root, None, &["rev-parse", "--git-dir"])?;
    let index = root
        .join(git_dir)
        .join(format!("keel-index-{}", std::process::id()));
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
}
