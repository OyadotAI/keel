//! Opening a pull request from the Changes panel.
//!
//! Driven through `gh`, the same as cloning: Keel never holds a long-lived GitHub token of its
//! own, and `gh` already knows how to authenticate, find the remote and pick a base branch.
//!
//! Three steps, streamed, and each is announced before it runs — a button that goes quiet for
//! twenty seconds while it pushes is a button people press twice.

use axum::{
    extract::Query,
    response::sse::{Event, Sse},
};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use crate::serve::Checkout;

#[derive(Deserialize)]
pub struct PrQuery {
    /// Title. Empty means let `gh` fill it from the commits.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Open it as a draft.
    #[serde(default)]
    pub draft: bool,
    /// The branch to merge into: the one the review before it compared against, so the verdict
    /// is about the diff the pull request shows. Empty means `gh`'s default.
    #[serde(default)]
    pub base: String,
}

fn refuse(message: &str) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let message = message.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default().event("fatal").data(message)))
            .await;
        let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn create(
    Checkout(repo): Checkout,
    Query(q): Query<PrQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    if !repo.join(".git").exists() {
        return refuse("This is not a git repository. Initialise one from Changes first.");
    }

    // Three gits, read off the executor: one slow `git status` here held up every other request.
    let checked = {
        let (repo, asked) = (repo.clone(), q.base.trim().to_string());
        tokio::task::spawn_blocking(move || {
            // The base `gh` will open against: the one asked for, else the remote's default.
            let base = if asked.is_empty() {
                crate::prereview::remote_default(&repo)
            } else {
                asked
            };
            precheck(&repo, &base)
        })
        .await
        .unwrap_or_else(|_| Err("The checks before a pull request failed to run.".into()))
    };
    let branch = match checked {
        Ok(b) => b,
        Err(why) => return refuse(&why),
    };
    // Opening one is the answer to the review before it, whichever way it was reached: "Open
    // without review" while one is running stops it, for every window. After the checks, so a
    // refused open leaves the review running.
    crate::prereview::stop_for(&repo);
    // A branch name or nothing: it becomes an argument to `gh`.
    let base = q.base.trim().to_string();
    if !base.is_empty()
        && (base.starts_with('-')
            || !base
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c)))
    {
        return refuse(&format!("{base} is not a branch name."));
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let say = |text: String| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(Ok(Event::default().event("line").data(text))).await;
            }
        };

        say(format!("$ git push -u origin {branch}")).await;
        let mut push = crate::git::tokio_command(&repo);
        push.args(["push", "-u", "origin", &branch]);
        if crate::plugins::pipe(&mut push, &tx).await != 0 {
            let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
            return;
        }

        let mut args: Vec<String> = vec!["pr".into(), "create".into()];
        if q.title.trim().is_empty() {
            // `--fill` writes the title and body from the commits, which is the right default:
            // the commit messages are already the description someone wrote.
            args.push("--fill".into());
        } else {
            args.push("--title".into());
            args.push(q.title.clone());
            args.push("--body".into());
            args.push(q.body.clone());
        }
        if q.draft {
            args.push("--draft".into());
        }
        if !base.is_empty() {
            args.push("--base".into());
            args.push(base);
        }

        say(format!("$ gh {}", args.join(" "))).await;
        let mut create = Command::new("gh");
        create.current_dir(&repo).args(&args);
        let code = crate::plugins::pipe(&mut create, &tx).await;

        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

/// Whether `repo` can have a pull request opened from it, and the branch it would be from. The
/// review before a pull request asks the same question first, in the same words.
pub fn precheck(repo: &camino::Utf8Path, base: &str) -> Result<String, String> {
    // A branch with no commits on it is not a thing to open a pull request from, and it is the
    // ordinary state right after `git init` — so it gets its own sentence rather than being
    // folded into "not a git repository", which is both wrong and unactionable.
    if !has_commits(repo) {
        return Err(
            "This repository has no commits yet. Make one first — a pull request is a request to \
             merge commits, and there are none."
                .into(),
        );
    }
    // A pull request is a branch, and `main` is not one. Catching it here is the difference
    // between a clear refusal and `gh` failing four steps in with something about a head ref.
    let branch = branch_of(repo).ok_or("Could not read the current branch.")?;
    if matches!(branch.as_str(), "main" | "master" | "trunk") {
        return Err(format!(
            "You are on `{branch}`. Make a branch for the change first — a pull request needs \
             somewhere to merge from."
        ));
    }
    if !uncommitted(repo).is_empty() {
        return Err(
            "There are uncommitted changes. Commit them first — a pull request only carries what \
             is committed, and opening one now would leave the rest behind."
                .into(),
        );
    }
    // Nothing the base does not already have: `gh` pushed and then refused with "could not
    // compute title or body defaults: could not find any commits between origin/main and …",
    // which says neither what is wrong nor what to do.
    if ahead_of(repo, base) == Some(0) {
        return Err(format!(
            "`{branch}` has no commits that `{base}` doesn't already have, so there is nothing to \
             open a pull request for. Commit the change on this branch first — or, if it was \
             already merged, there is nothing left to open."
        ));
    }
    Ok(branch)
}

/// How many commits `HEAD` has that `base` does not, against the remote's copy of `base` as `gh`
/// compares, else the local one; `None` when neither exists, which `gh` is left to say.
fn ahead_of(repo: &camino::Utf8Path, base: &str) -> Option<u64> {
    [format!("origin/{base}"), base.to_string()]
        .iter()
        .find_map(|b| {
            let mut c = crate::git::command(repo);
            c.args(["rev-list", "--count", &format!("{b}..HEAD")]);
            let out = crate::git::output(c).ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().ok())?
        })
}

/// The current branch.
///
/// `git branch --show-current`, not `rev-parse --abbrev-ref HEAD`. The latter exits non-zero on a
/// repository with no commits — an unborn HEAD — which is the ordinary state one second after
/// `git init`, and it was being reported as "this is not a git repository".
fn branch_of(repo: &camino::Utf8Path) -> Option<String> {
    let mut c = crate::git::command(repo);
    c.args(["branch", "--show-current"]);
    let out = crate::git::output(c).ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Whether anything has been committed. False on an unborn HEAD.
fn has_commits(repo: &camino::Utf8Path) -> bool {
    let mut c = crate::git::command(repo);
    c.args(["rev-parse", "--verify", "HEAD"]);
    crate::git::output(c)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub(crate) fn uncommitted(repo: &camino::Utf8Path) -> String {
    let mut c = crate::git::command(repo);
    c.args(["status", "--porcelain"]);
    crate::git::output(c)
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
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
