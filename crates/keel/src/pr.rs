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

    // A branch with no commits on it is not a thing to open a pull request from, and it is the
    // ordinary state right after `git init` — so it gets its own sentence rather than being
    // folded into "not a git repository", which is both wrong and unactionable.
    if !has_commits(&repo) {
        return refuse(
            "This repository has no commits yet. Make one first — a pull request is a request to \
             merge commits, and there are none.",
        );
    }

    // A pull request is a branch, and `main` is not one. Catching it here is the difference
    // between a clear refusal and `gh` failing four steps in with something about a head ref.
    let branch = match branch_of(&repo) {
        Some(b) => b,
        None => return refuse("Could not read the current branch."),
    };
    if matches!(branch.as_str(), "main" | "master" | "trunk") {
        return refuse(&format!(
            "You are on `{branch}`. Make a branch for the change first — a pull request needs \
             somewhere to merge from."
        ));
    }
    if !uncommitted(&repo).is_empty() {
        return refuse(
            "There are uncommitted changes. Commit them first — a pull request only carries what \
             is committed, and opening one now would leave the rest behind.",
        );
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

fn uncommitted(repo: &camino::Utf8Path) -> String {
    let mut c = crate::git::command(repo);
    c.args(["status", "--porcelain"]);
    crate::git::output(c)
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
