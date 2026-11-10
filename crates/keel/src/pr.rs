//! Pull-request creation captures an immutable target and reconciles existing PRs before retries.
mod github;
mod opening;
mod repository;
mod target;
pub use opening::create;
use serde::Deserialize;

/// User intent is separate from the commit/branch guards captured before a push.
#[derive(Deserialize, Default)]
pub struct PrQuery {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub branch: String,
}

/// Whether `repo` can have a pull request opened from it, and the branch it would be from. The
/// review before a pull request asks the same question first, in the same words.
pub fn precheck(repo: &camino::Utf8Path, base: &str) -> Result<String, String> {
    precheck_using(base, |args| crate::git::run(repo, args))
}

/// A review's preflight shares its deadline rather than allowing each Git query sixty seconds.
pub(crate) fn precheck_before(
    repo: &camino::Utf8Path,
    base: &str,
    budget: &crate::ci::Budget<'_>,
) -> Result<String, String> {
    precheck_using(base, |args| budget.git(repo, args))
}

fn precheck_using(
    base: &str,
    run: impl Fn(&[&str]) -> Result<String, String>,
) -> Result<String, String> {
    // A branch with no commits on it is not a thing to open a pull request from, and it is the
    // ordinary state right after `git init` — so it gets its own sentence rather than being
    // folded into "not a git repository", which is both wrong and unactionable.
    if run(&["rev-parse", "--verify", "HEAD"]).is_err() {
        return Err(
            "This repository has no commits yet. Make one first — a pull request is a request to \
             merge commits, and there are none."
                .into(),
        );
    }
    // A pull request is a branch, and `main` is not one. Catching it here is the difference
    // between a clear refusal and `gh` failing four steps in with something about a head ref.
    let branch = run(&["branch", "--show-current"])
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or("Could not read the current branch.")?;
    if matches!(branch.as_str(), "main" | "master" | "trunk") {
        return Err(format!(
            "You are on `{branch}`. Make a branch for the change first — a pull request needs \
             somewhere to merge from."
        ));
    }
    // What Keel itself keeps out of every commit is not the person's uncommitted work.
    let status = run(&["status", "--porcelain"])?;
    let uncommitted = status.lines().any(|line| {
        let path = line
            .get(3..)
            .unwrap_or("")
            .rsplit(" -> ")
            .next()
            .unwrap_or("");
        !path.is_empty() && !crate::worktree::kept_out(path.trim_matches('"'))
    });
    if uncommitted {
        return Err(
            "There are uncommitted changes. Commit them first — a pull request only carries what \
             is committed, and opening one now would leave the rest behind."
                .into(),
        );
    }
    // Nothing the base does not already have: `gh` pushed and then refused with "could not
    // compute title or body defaults: could not find any commits between origin/main and …",
    // which says neither what is wrong nor what to do.
    if [format!("origin/{base}"), base.to_string()]
        .iter()
        .find_map(|b| {
            run(&["rev-list", "--count", &format!("{b}..HEAD")])
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        })
        == Some(0)
    {
        return Err(format!(
            "`{branch}` has no commits that `{base}` doesn't already have, so there is nothing to \
             open a pull request for. Commit the change on this branch first — or, if it was \
             already merged, there is nothing left to open."
        ));
    }
    Ok(branch)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &camino::Utf8Path, args: &[&str]) {
        let mut c = crate::git::command(repo);
        c.args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args);
        assert!(
            crate::git::output(c).unwrap().status.success(),
            "git {args:?}"
        );
    }

    /// A quarantined file is never committed, so it cannot be what stops a pull request: the
    /// precheck said "commit first" about a `.mcp.json` the commit would never take.
    #[test]
    fn what_keel_keeps_out_of_commits_does_not_block_a_pull_request() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".mcp.json"), "{}").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["checkout", "-qb", "feat"]);
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "the change"]);
        std::fs::remove_file(repo.join(".mcp.json")).unwrap();
        assert_eq!(precheck(&repo, "main").unwrap(), "feat");
        std::fs::write(repo.join("c.txt"), "c\n").unwrap();
        assert!(precheck(&repo, "main").unwrap_err().contains("uncommitted"));
    }

    /// A branch with nothing its base lacks is refused before anything is pushed, in words that
    /// say what to do — `gh` pushed and then failed with "could not find any commits between".
    #[test]
    fn a_branch_with_no_commits_beyond_its_base_is_refused_before_the_push() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["checkout", "-qb", "feat/profile-groups"]);
        let why = precheck(&repo, "main").unwrap_err();
        assert!(
            why.contains("`feat/profile-groups` has no commits that `main` doesn't already have"),
            "{why}"
        );
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "the change"]);
        assert_eq!(precheck(&repo, "main").unwrap(), "feat/profile-groups");
        // A base nobody has is not a reason to refuse: `gh` says what is wrong with it.
        assert!(precheck(&repo, "no-such-branch").is_ok());
    }
}
