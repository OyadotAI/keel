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

/// How many commits `HEAD` has that `base` lacks, or `None` when git cannot say (no such base,
/// no commits). One answer for the precheck and for the share card: counted twice, the card
/// offered a pull request the precheck then refused as "nothing to open".
pub(crate) fn ahead(base: &str, run: impl Fn(&[&str]) -> Result<String, String>) -> Option<u64> {
    [format!("origin/{base}"), base.to_string()]
        .iter()
        .find_map(|b| {
            run(&["rev-list", "--count", &format!("{b}..HEAD")])
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        })
}

/// The branch a checkout's work would merge into: a lane's recorded base, else the remote's
/// default. Lifted from the review's target so the card and the review name the same one.
pub(crate) fn base_for(root: &camino::Utf8Path, dir: &camino::Utf8Path) -> String {
    dir.strip_prefix(root.join(crate::worktree::DIR))
        .ok()
        .map(|wt| crate::worktree::base_of(root, wt.as_str()))
        .unwrap_or_else(|| crate::prereview::remote_default(dir))
}

/// A pull request Keel opened from a branch, and the commit GitHub has for that branch as far as
/// this checkout knows (`origin/<branch>`). Equal to `HEAD` means everything here is in it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Shared {
    pub url: String,
    pub head: String,
}

/// Record the pull request on its branch, in `branch.<name>.keelpr`. Git owns that section — it
/// moves with `branch -m` and goes with `branch -d` — so the link survives a relaunch and cannot
/// outlive its branch. It was run memory, and a closed result row took the only link with it.
pub(crate) fn remember(repo: &camino::Utf8Path, branch: &str, url: &str) {
    let _ = crate::git::run(repo, &["config", &format!("branch.{branch}.keelpr"), url]);
}

/// The pull request recorded on `branch`, if what is recorded is still a GitHub pull request
/// link: the config is a file anything can write, and the page opens this address.
pub(crate) fn shared(repo: &camino::Utf8Path, branch: &str) -> Option<Shared> {
    let url = crate::git::trimmed(
        repo,
        &["config", "--get", &format!("branch.{branch}.keelpr")],
    )
    .ok()
    .filter(|url| github::valid_link(url))?;
    // No such branch on the remote any more — merged and deleted, usually — is no pull request
    // to add to: the next share opens a new one.
    let pushed = format!("refs/remotes/origin/{branch}");
    let head = crate::git::trimmed(repo, &["rev-parse", "--verify", "--quiet", &pushed]).ok()?;
    Some(Shared { url, head })
}

/// What the share card needs beyond the changes: where `HEAD` is, how far ahead of its base, and
/// the pull request already opened from this branch.
pub(crate) fn sharing(
    root: &camino::Utf8Path,
    dir: &camino::Utf8Path,
    branch: Option<&str>,
) -> (Option<String>, Option<u64>, Option<Shared>) {
    let head = crate::git::trimmed(dir, &["rev-parse", "HEAD"]).ok();
    let base = base_for(root, dir);
    let count = ahead(&base, |args| crate::git::run(dir, args));
    (head, count, branch.and_then(|b| shared(dir, b)))
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
    if ahead(base, &run) == Some(0) {
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

    /// The card's count and the precheck's refusal are one number.
    #[test]
    fn ahead_agrees_with_precheck() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let count = |base: &str| ahead(base, |args| crate::git::run(&repo, args));
        git(&repo, &["init", "-q", "-b", "main"]);
        assert_eq!(count("main"), None);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["checkout", "-qb", "feat"]);
        assert_eq!(count("main"), Some(0));
        assert!(precheck(&repo, "main").is_err());
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "the change"]);
        assert_eq!(count("main"), Some(1));
        assert!(precheck(&repo, "main").is_ok());
        assert_eq!(count("no-such-branch"), None);
    }

    /// A pull request is remembered on its branch and goes with it; anything that is not a
    /// GitHub pull request link is not handed to the page.
    #[test]
    fn an_opened_pr_is_remembered_on_its_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["checkout", "-qb", "keel/a-change"]);
        assert_eq!(shared(&repo, "keel/a-change"), None);
        remember(&repo, "keel/a-change", "https://github.com/o/r/pull/7");
        let (head, count, pr) = sharing(&repo, &repo, Some("keel/a-change"));
        assert_eq!(count, Some(0));
        // A branch the remote does not have (merged and deleted) has no pull request to add to.
        assert_eq!(pr, None);
        // Once the branch is on the remote, the pull request's head is the checkout's.
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/keel/a-change", "HEAD"],
        );
        assert_eq!(shared(&repo, "keel/a-change").map(|pr| pr.head), head);
        remember(&repo, "keel/a-change", "https://evil.example/pull/7");
        assert_eq!(shared(&repo, "keel/a-change"), None);
        remember(&repo, "keel/a-change", "https://github.com/o/r/pull/7");
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["branch", "-qD", "keel/a-change"]);
        assert_eq!(shared(&repo, "keel/a-change"), None);
    }
}
