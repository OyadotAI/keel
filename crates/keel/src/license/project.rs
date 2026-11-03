//! Opened-root identity deduplicates real Git worktrees without ascending from arbitrary subfolders.

use camino::{Utf8Path, Utf8PathBuf};

/// Resolve only metadata belonging to the actual opened root, never an unrelated ancestor.
pub(super) fn identity(root: &Utf8Path) -> Utf8PathBuf {
    let root = root.canonicalize_utf8().unwrap_or_else(|_| root.to_owned());
    let git = root.join(".git");
    if git.is_dir() {
        return common(&git).unwrap_or(root);
    }
    if git.is_file() {
        return worktree(&root, &git).unwrap_or(root);
    }
    root
}

/// A worktree's gitdir pointer is interpreted locally and never enters telemetry state.
fn worktree(root: &Utf8Path, file: &Utf8Path) -> Option<Utf8PathBuf> {
    let contents = std::fs::read_to_string(file).ok()?;
    let value = contents.lines().next()?.strip_prefix("gitdir:")?.trim();
    if value.is_empty() {
        return None;
    }
    common(&root.join(value))
}

/// Original checkouts and linked worktrees converge on the same canonical common Git directory.
fn common(git: &Utf8Path) -> Option<Utf8PathBuf> {
    let relative = std::fs::read_to_string(git.join("commondir")).unwrap_or_else(|_| ".".into());
    git.join(relative.trim()).canonicalize_utf8().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_worktrees_and_their_original_project_share_one_identity() {
        let home = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::try_from(home.path().to_owned()).unwrap();
        let original = root.join("project");
        let worktree = root.join("lane");
        let gitdir = original.join(".git/worktrees/lane");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        std::fs::write(worktree.join(".git"), format!("gitdir: {gitdir}\n")).unwrap();
        assert_eq!(identity(&original), identity(&worktree));
    }

    #[test]
    fn an_opened_subfolder_does_not_silently_become_its_ancestor_repository() {
        let home = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::try_from(home.path().to_owned()).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let opened = root.join("independent-folder");
        std::fs::create_dir_all(&opened).unwrap();
        assert_eq!(identity(&opened), opened.canonicalize_utf8().unwrap());
        assert_ne!(identity(&root), identity(&opened));
    }
}
