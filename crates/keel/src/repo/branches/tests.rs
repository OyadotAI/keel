//! Scratch repositories verify recorded metadata, remote selection, and safe refusal paths.
use super::*;
fn repository() -> (tempfile::TempDir, camino::Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
    git_run(&root, &["init", "-b", "main"]).unwrap();
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/nonexistent"),
    ] {
        git_run(&root, &["config", key, value]).unwrap();
    }
    git_run(&root, &["commit", "--allow-empty", "-m", "Initial fixture"]).unwrap();
    (dir, root)
}

#[test]
fn metadata_distinguishes_unborn_detached_and_other_worktrees_without_fetching() {
    let (_dir, root) = repository();
    git_run(&root, &["branch", "other"]).unwrap();
    let path = root.join("other worktree 日本語");
    git_run(&root, &["worktree", "add", path.as_str(), "other"]).unwrap();
    let data = git_branches(&root).unwrap();
    assert!(data.local.iter().all(|b| b.when.is_some()));
    assert_eq!(
        data.local
            .iter()
            .find(|b| b.name == "other")
            .unwrap()
            .worktree
            .as_ref()
            .unwrap()
            .path,
        keel_workspace::Real::real(path.as_path()).unwrap().as_str()
    );
    assert!(
        git_branch_target(&root, "checkout", "other", Some("local"))
            .unwrap_err()
            .contains("another worktree")
    );
    git_run(&root, &["switch", "--detach"]).unwrap();
    let detached = git_branches(&root).unwrap();
    assert!(detached.current.is_none());
    assert!(detached.head.is_some());
    let empty = tempfile::tempdir().unwrap();
    let empty = Utf8Path::from_path(empty.path()).unwrap();
    git_run(empty, &["init"]).unwrap();
    assert!(git_branches(empty).unwrap().local.is_empty());
    assert!(git_branches(Utf8Path::new("/nonexistent")).is_err());
}

#[test]
fn exact_remote_targets_reuse_tracking_locals_and_refuse_name_collisions() {
    let (_dir, root) = repository();
    git_run(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/never-fetch",
        ],
    )
    .unwrap();
    git_run(
        &root,
        &["update-ref", "refs/remotes/origin/feature", "HEAD"],
    )
    .unwrap();
    git_branch_target(&root, "checkout", "origin/feature", Some("remote")).unwrap();
    assert_eq!(
        git_branches(&root).unwrap().current.as_deref(),
        Some("feature")
    );
    git_run(&root, &["switch", "main"]).unwrap();
    // Short upstream names become ambiguous when a local ref resembles a remote ref.
    git_run(&root, &["branch", "origin/feature"]).unwrap();
    git_branch_target(&root, "checkout", "origin/feature", Some("remote")).unwrap();
    git_run(&root, &["switch", "main"]).unwrap();
    git_run(&root, &["branch", "collision"]).unwrap();
    git_run(
        &root,
        &["update-ref", "refs/remotes/origin/collision", "HEAD"],
    )
    .unwrap();
    assert!(
        git_branch_target(&root, "checkout", "origin/collision", Some("remote"))
            .unwrap_err()
            .contains("already exists")
    );
    assert_eq!(
        git_branches(&root).unwrap().current.as_deref(),
        Some("main")
    );
    let data = git_branches(&root).unwrap();
    assert!(
        data.remote_details
            .iter()
            .any(|b| b.name == "origin/feature" && b.tracking == ["feature"])
    );
    assert!(!data.remote.iter().any(|b| b == "origin/feature"));
}

#[test]
fn a_conflicting_switch_preserves_head_and_local_changes() {
    let (_dir, root) = repository();
    std::fs::write(root.join("file"), "base").unwrap();
    git_run(&root, &["add", "file"]).unwrap();
    git_run(&root, &["commit", "-m", "base"]).unwrap();
    git_run(&root, &["switch", "-c", "feature"]).unwrap();
    std::fs::write(root.join("file"), "feature").unwrap();
    git_run(&root, &["commit", "-am", "feature"]).unwrap();
    git_run(&root, &["switch", "main"]).unwrap();
    std::fs::write(root.join("file"), "unsaved work").unwrap();
    assert!(git_branch_target(&root, "checkout", "feature", Some("local")).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("file")).unwrap(),
        "unsaved work"
    );
    assert_eq!(
        git_branches(&root).unwrap().current.as_deref(),
        Some("main")
    );
    assert!(git_branch_target(&root, "delete", "feature", Some("local")).is_err());
}
