//! Branch mutations select exact refs and retain Git's ordinary refusal and merged-only rules.
use super::{git_branches, git_run};
use camino::Utf8Path;

/// New callers specify local/remote; legacy callers retain their previous inference path.
pub fn git_branch_target(
    root: &Utf8Path,
    action: &str,
    name: &str,
    kind: Option<&str>,
) -> Result<String, String> {
    crate::repo::valid_branch(name)?;
    if action == "delete" {
        return git_run(root, &["branch", "-d", name]);
    }
    if action == "create" {
        return git_run(root, &["switch", "-c", name]);
    }
    if action != "checkout" {
        return Err(format!("unknown branch action: {action}"));
    }
    let branches = git_branches(root)?;
    let remote = kind == Some("remote")
        || (kind.is_none() && !branches.local.iter().any(|b| b.name == name) && name.contains('/'));
    if kind.is_some_and(|k| !["local", "remote"].contains(&k)) {
        return Err("Unknown branch target kind.".into());
    }
    if !remote {
        let branch = branches
            .local
            .iter()
            .find(|b| b.name == name)
            .ok_or("That local branch is no longer available.")?;
        available(branch)?;
        return git_run(root, &["switch", name]);
    }
    let branch = branches.remote_details.iter().find(|b| b.name == name).ok_or("That remote-tracking branch is no longer recorded locally. Fetch explicitly to refresh it.")?;
    if branch.tracking.len() > 1 {
        return Err(
            "Several local branches track this ref. Choose the local branch you want.".into(),
        );
    }
    if let Some(local) = branch.tracking.first() {
        available(
            branches
                .local
                .iter()
                .find(|b| &b.name == local)
                .ok_or("The tracking branch changed. Refresh and try again.")?,
        )?;
        return git_run(root, &["switch", local]);
    }
    let prefix = format!("{}/", branch.remote);
    let short = name
        .strip_prefix(&prefix)
        .ok_or("Invalid remote-tracking ref.")?;
    if branches.local.iter().any(|b| b.name == short) {
        return Err(format!(
            "Local branch {short:?} already exists and does not track {name}. Choose that local branch or a different name."
        ));
    }
    git_run(
        root,
        &[
            "switch",
            "--track",
            "-c",
            short,
            &format!("refs/remotes/{name}"),
        ],
    )
}

fn available(branch: &super::Branch) -> Result<(), String> {
    if !branch.current
        && let Some(tree) = &branch.worktree
    {
        return Err(format!(
            "This branch is checked out in another worktree: {}. Open that lane or worktree instead.",
            tree.path
        ));
    }
    Ok(())
}
