//! A working tree per lane.
//!
//! Two agents editing one checkout overwrite each other, and the only honest thing Keel could do
//! about it was warn. Every surviving agent IDE isolates lanes in git worktrees, and every one of
//! them is also bug-reported for how: `.env` missing from the new tree, a branch deleted with the
//! commits still on it, edits landing in the wrong checkout. So the rules here are the complaints
//! inverted: files the project lists in `.worktreeinclude` (Claude Code's own convention) are
//! copied in, a branch is only ever deleted with `-d`, and a discard says how many commits it
//! would lose before it loses them.
//!
//! Worktrees live in `.keel/worktrees/<name>` inside the project, ignored by a `.gitignore` written
//! beside them. Permissions, trust and approvals stay with the project root — they are decisions
//! about the repository, not about a checkout.

use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

/// Where a lane's checkout lives, relative to the project.
pub const DIR: &str = ".keel/worktrees";

/// A name that can only ever be one directory under [`DIR`].
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 41
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn path_of(root: &Utf8Path, name: &str) -> Result<Utf8PathBuf, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} is not a lane name"));
    }
    Ok(root.join(DIR).join(name))
}

fn branch_of(name: &str) -> String {
    format!("keel/{name}")
}

/// Where the lane started, remembered so that finishing it knows where to put it back.
///
/// In git's own per-branch config section. Git owns `branch.<name>.*`: it moves the section on
/// `branch -m` and removes it on `branch -d`, so this needs no cleanup of its own and cannot
/// outlive the branch it describes. It also survives what the app cannot — a daemon restart, a
/// project switch, a quit — which is why the base is not kept in the window that chose it.
fn base_key(name: &str) -> String {
    format!("branch.{}.keelbase", branch_of(name))
}

fn remember_base(root: &Utf8Path, name: &str, base: &str) {
    // `HEAD` is where the project was standing, which is a position, not a branch. Resolve it to
    // the name now — by the time the lane is finished the project has usually moved.
    let branch = if base == "HEAD" {
        git(root, &["branch", "--show-current"]).unwrap_or_default()
    } else {
        base.to_string()
    };
    if !branch.is_empty() {
        let _ = git(root, &["config", "--local", &base_key(name), &branch]);
    }
}

/// The branch a lane was cut from, or where the project is standing for one made before Keel
/// recorded it.
pub fn base_of(root: &Utf8Path, name: &str) -> String {
    git(root, &["config", "--local", "--get", &base_key(name)])
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| git(root, &["branch", "--show-current"]).unwrap_or_default())
}

use crate::git::trimmed as git;

#[derive(Serialize, Clone)]
pub struct Worktree {
    pub name: String,
    pub branch: String,
    /// The branch this lane was cut from, and the one `finish` merges it into. `ahead` is counted
    /// against it too — against the project's *current* branch, the count that decides whether a
    /// discard warns could be anything at all.
    pub base: String,
    pub path: String,
    /// Commits on the lane's branch that the project's branch does not have.
    pub ahead: u32,
    /// Uncommitted changes in the lane.
    pub dirty: bool,
}

/// Make a lane's checkout: a new branch off the project's HEAD, in its own directory.
/// A lane's checkout, branched from `from` — the branch the person picked when they started
/// the session, or `HEAD` when they did not. A lane off `main` while the project sits on a
/// half-finished branch is the common case, and it used to be impossible.
pub fn create_from(root: &Utf8Path, name: &str, from: Option<&str>) -> Result<Worktree, String> {
    let base = from
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or("HEAD");
    if base != "HEAD"
        && !base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
    {
        return Err(format!("{base} is not a branch name"));
    }
    create_at(root, name, base)
}

/// Where the project is standing. Only the tests want this now; every caller says what to
/// branch from.
#[cfg(test)]
pub fn create(root: &Utf8Path, name: &str) -> Result<Worktree, String> {
    create_at(root, name, "HEAD")
}

fn create_at(root: &Utf8Path, name: &str, base: &str) -> Result<Worktree, String> {
    let path = path_of(root, name)?;
    if path.exists() {
        return Err(format!("lane {name} already exists"));
    }
    // The two causes read identically to git (`fatal: Needed a single revision`) and could not be
    // told apart in the message, so a folder that was never a repository was told it had no
    // commits. The app offers to `git init` for the first and to commit for the second.
    //
    // `is_root`, not `rev-parse --git-dir`: that succeeds from any subdirectory of any
    // repository, so opening a plain folder that happens to sit inside one — a directory under a
    // dotfiles checkout is the everyday case — made a worktree of the *ancestor* repository
    // underneath it. `gitroots` already owns this question.
    if !crate::gitroots::is_root(root) {
        return Err(
            "This folder is not a git repository, so there is nothing to branch from.".to_string(),
        );
    }
    git(root, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "This repository has no commits yet, so there is nothing to branch from.")?;

    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Everything under here is a checkout of this repository, not content of it.
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").map_err(|e| e.to_string())?;
    }

    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            &branch_of(name),
            path.as_str(),
            base,
        ],
    )?;
    remember_base(root, name, base);
    copy_included(root, &path);
    Ok(Worktree {
        name: name.to_string(),
        branch: branch_of(name),
        base: base_of(root, name),
        path: path.to_string(),
        ahead: 0,
        dirty: false,
    })
}

/// Bring across what git leaves behind.
///
/// A worktree has no `.env`, no local config, nothing ignored — which is the most-reported reason
/// a fresh lane cannot run the project. `.worktreeinclude` is the list, one path per line, and
/// it is Claude Code's own file for exactly this purpose, so a project that has it for the CLI
/// has it for Keel.
fn copy_included(root: &Utf8Path, into: &Utf8Path) {
    let Ok(list) = std::fs::read_to_string(root.join(".worktreeinclude")) else {
        return;
    };
    for line in list.lines().map(str::trim) {
        // `.worktreeinclude` is repository content — the same author whose `.claude/settings.json`
        // is quarantined before the first invocation and rated Critical by the scanner. `..` was
        // the only escape it filtered, and it was not the only one: `Path::join` *replaces* the
        // base when what it is given is absolute, so an absolute line made `from` and `to` the
        // same path somewhere else entirely on the machine.
        if line.is_empty()
            || line.starts_with('#')
            || line.contains("..")
            || Utf8Path::new(line).is_absolute()
        {
            continue;
        }
        let from = root.join(line);
        let to = into.join(line);
        // `is_dir` follows symlinks, so a tracked `deps -> /` turned this into an unbounded walk
        // of the filesystem inside a `spawn_blocking` with nothing watching it. What the entry
        // *is* decides; what it points at does not.
        if from.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            continue;
        }
        if from.is_dir() {
            let _ = copy_dir(from.as_std_path(), to.as_std_path());
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&from, &to);
        }
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        // `read_dir`'s file type does not follow links, which is what we want: a symlink inside
        // an included directory is copied as a link, never walked.
        if entry.file_type()?.is_symlink() {
            continue;
        }
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The lane checkouts git knows about, by name.
///
/// `read_dir` was the wrong question. It asks the filesystem what is there, and git keeps its own
/// register: delete a checkout in Finder and git still holds it, so Keel could not see the lane
/// while `worktree add` refused its name forever with `fatal: '<path>' is a missing but already
/// registered worktree` — a state nothing in the app could reach or explain. `prune` clears
/// exactly that, and is a no-op when there is nothing stale.
fn registered(root: &Utf8Path) -> Vec<String> {
    let _ = git(root, &["worktree", "prune"]);
    let Ok(listing) = git(root, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    // Matched on the tail rather than by stripping `root`: git reports resolved paths, and on
    // macOS the repository's own path very often is not one — `/tmp` and `/var` are symlinks into
    // `/private`, so a prefix comparison against `root` finds nothing and every lane disappears.
    let marker = format!("/{DIR}/");
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| path.trim().split_once(&marker))
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default().to_string())
        .filter(|name| valid_name(name))
        .collect()
}

/// Every lane checkout, with how far each has gone.
pub fn list(root: &Utf8Path) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = registered(root)
        .into_iter()
        .map(|name| {
            let path = root.join(DIR).join(&name);
            let branch = branch_of(&name);
            // Against where the lane started, not against where the project happens to be
            // standing now. A lane cut from `release` while the project sits on `main` had every
            // commit `main` was missing counted as its own, and that number is what a discard
            // shows the person before it throws the branch away.
            let base = base_of(root, &name);
            let ahead = git(root, &["rev-list", "--count", &format!("{base}..{branch}")])
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let dirty = git(&path, &["status", "--porcelain"])
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            Worktree {
                name,
                branch,
                base,
                path: path.to_string(),
                ahead,
                dirty,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Commit everything in a checkout. Nothing to commit is not an error.
/// Stage everything and commit. `Ok(false)` when there was nothing to commit — which
/// includes a nested repository with changes of its own: `status` reports it as "untracked
/// content" but `add -A` stages nothing for it, and git's "no changes added to commit" used to
/// surface as a red error under every turn.
/// Commit everything, in every repository the folder holds.
///
/// A workspace — `backend/` and `frontend/`, each its own repository — gets one commit in each,
/// with the same message. Not atomic, because two repositories cannot commit atomically and
/// pretending otherwise would be a lie about what was saved; but a turn that changed both halves
/// is saved in both, which is what people mean by "commit".
pub fn commit_all(checkout: &Utf8Path, message: &str) -> Result<bool, String> {
    let found = crate::gitroots::find(checkout);
    // Said in words, with the thing that fixes it, rather than passing git's own
    // "fatal: not a git repository (or any of the parent directories): .git" to somebody who
    // opened a folder and pressed Commit.
    if found.is_empty() {
        return Err(
            "This folder is not a git repository, so there is nowhere to commit. \
                    Initialise one from Changes, or open a folder that has one."
                .into(),
        );
    }
    // The ordinary case — the folder is the repository — commits here, as it always did.
    // Anything else is a workspace and each repository commits for itself.
    let workspace = !found.iter().any(|r| r.dir.is_empty());
    if workspace {
        let mut any = false;
        let mut failures = Vec::new();
        for root in &found {
            match commit_one(&checkout.join(&root.dir), message) {
                Ok(true) => any = true,
                Ok(false) => {}
                // One repository refusing must not lose the commit in the other.
                Err(e) => failures.push(format!("{}: {e}", root.dir)),
            }
        }
        if !failures.is_empty() && !any {
            return Err(failures.join("; "));
        }
        return Ok(any);
    }
    commit_one(checkout, message)
}

fn commit_one(checkout: &Utf8Path, message: &str) -> Result<bool, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("a commit needs a message".into());
    }
    git(checkout, &["add", "-A"])?;
    if git(checkout, &["diff", "--cached", "--name-only"])?
        .trim()
        .is_empty()
    {
        return Ok(false);
    }
    // `crate::git::AUTOMATIC` and `--no-verify`: this is Keel's checkpoint, not the person's
    // commit. See the note on `AUTOMATIC` for why running the repository's own `pre-commit` hook
    // after every turn is both slow and a thing nobody asked for. `git_commit_staged`, which is
    // the button, keeps its hooks.
    let mut args = crate::git::AUTOMATIC.to_vec();
    args.extend_from_slice(&["commit", "-q", "--no-verify", "-m", message]);
    git(checkout, &args).map(|_| true)
}

/// Merge the lane into the project's branch and remove the checkout.
///
/// Refuses rather than guesses: uncommitted work in the project would be mixed into the merge,
/// and a conflict is a decision for a person. On either, the lane is left exactly as it was.
pub fn finish(root: &Utf8Path, name: &str, message: &str) -> Result<(), String> {
    let path = path_of(root, name)?;
    if !path.exists() {
        return Err(format!("no lane {name}"));
    }
    // Into the branch the lane came from, and only while the project is standing on it.
    //
    // The merge runs in the project root against whatever HEAD is, which for a lane cut from
    // `release` while the project sits on `main` silently put release-derived work on main.
    // Checking the branch out on someone's behalf is not Keel's decision to make — moving a
    // person's working tree under them is exactly the surprise this file exists to avoid — so
    // this refuses and names the branch instead.
    let base = base_of(root, name);
    let here = git(root, &["branch", "--show-current"]).unwrap_or_default();
    if !base.is_empty() && base != here {
        let where_now = if here.is_empty() {
            "a detached HEAD".to_string()
        } else {
            format!("“{here}”")
        };
        return Err(format!(
            "This feature was branched from “{base}”, and the project is on {where_now}.              Switch to “{base}” and finish it there, so the work lands where it came from."
        ));
    }
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        return Err(
            "The project has uncommitted changes. Commit or discard them first, so the \
                    lane's work is not mixed into them."
                .into(),
        );
    }
    commit_all(&path, message)?;
    let branch = branch_of(name);
    if let Err(why) = git(root, &["merge", "--no-ff", "-q", "-m", message, &branch]) {
        let _ = git(root, &["merge", "--abort"]);
        return Err(format!(
            "Could not merge {branch}: {why}. The lane is untouched; resolve it in a terminal."
        ));
    }
    remove(root, name)
}

/// How many commits a discard would lose.
pub fn unmerged(root: &Utf8Path, name: &str) -> u32 {
    list(root)
        .into_iter()
        .find(|w| w.name == name)
        .map(|w| w.ahead)
        .unwrap_or(0)
}

/// Throw a lane away. Refuses when its commits are not on the project's branch unless told the
/// count is acceptable — the number is in the refusal, so the person decides with it in front
/// of them.
pub fn discard(root: &Utf8Path, name: &str, force: bool) -> Result<(), String> {
    let path = path_of(root, name)?;
    if !path.exists() {
        return Err(format!("no lane {name}"));
    }
    let lost = unmerged(root, name);
    // Uncommitted files count too. `worktree remove --force` is what makes a dirty checkout
    // removable at all, so a lane where the agent had written twenty files and committed none
    // reported nothing to lose and lost all of it — while this module's own header promised that
    // a discard says what it would lose before it loses it.
    let uncommitted = git(&path, &["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    if (lost > 0 || uncommitted > 0) && !force {
        let commits = match lost {
            0 => None,
            1 => Some("1 unmerged commit".to_string()),
            n => Some(format!("{n} unmerged commits")),
        };
        let files = match uncommitted {
            0 => None,
            1 => Some("1 uncommitted file".to_string()),
            n => Some(format!("{n} uncommitted files")),
        };
        let what = [commits, files]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" and ");
        return Err(format!("This feature has {what}, which would be lost."));
    }
    git(root, &["worktree", "remove", "--force", path.as_str()])?;
    // `-D` only here, and only once the person has been told the count above — `finish` never
    // uses it.
    //
    // Always `-D`, though, and that is not the same decision it looks like. `-d` asks a
    // *different* question than the one the person was answering: it judges the branch against
    // its upstream, so a lane fully merged into its base still refuses when `origin/keel/<name>`
    // is merely behind it — measured here on a lane that was 0 ahead of `main`, with git's own
    // message admitting it "is merged to HEAD". By then the checkout is already gone, so the
    // refusal left a branch with no worktree and returned an error saying nothing had happened.
    // The count above is the promise; it is made against the base, and it is the one that
    // decides.
    git(root, &["branch", "-D", &branch_of(name)])
        .map(|_| ())
        // The worktree is gone whatever git says about the branch. Reporting a bare git failure
        // here reads as "the discard did not work", and the next thing the person does is try
        // again against a checkout that no longer exists.
        .map_err(|why| {
            format!(
                "The checkout is gone, but the branch {} could not be deleted: {why}. \
                 Remove it with `git branch -D {}` if you still want it gone.",
                branch_of(name),
                branch_of(name)
            )
        })
}

/// Remove a merged lane. `-d`, never `-D`: a branch git will not delete safely is one whose
/// commits would vanish, and that is the data-loss bug every other tool has shipped.
fn remove(root: &Utf8Path, name: &str) -> Result<(), String> {
    let path = path_of(root, name)?;
    git(root, &["worktree", "remove", "--force", path.as_str()])?;
    git(root, &["branch", "-d", &branch_of(name)]).map(|_| ())
}

// ── handlers ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NameBody {
    pub name: String,
    /// The branch to start from. Absent means where the project is now.
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Deserialize)]
pub struct FinishBody {
    pub name: String,
    pub message: String,
}

#[derive(Deserialize)]
pub struct DiscardBody {
    pub name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
/// The Commit button. Keel's own checkpoint after a turn no longer comes through here: the
/// daemon makes it inside the turn, under the lane's claim — see `turns::finish`.
pub struct CommitBody {
    pub message: String,
}

fn bad(e: String) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e)
}

async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, (StatusCode, String)> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .map_err(bad)
}

pub async fn api_create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<NameBody>,
) -> Result<Json<Worktree>, (StatusCode, String)> {
    let root = state.repo();
    let made = off_thread(move || create_from(&root, &body.name, body.from.as_deref())).await;
    // The failure that stopped a customer working. Reported so it is countable: the shape of the
    // git error only — it quotes branch names and paths.
    if made.is_err() {
        sentry::capture_message(
            "isolated checkout could not be created",
            sentry::Level::Error,
        );
    }
    made.map(Json)
}

pub async fn api_list(State(state): State<Arc<AppState>>) -> Json<Vec<Worktree>> {
    let root = state.repo();
    Json(
        tokio::task::spawn_blocking(move || list(&root))
            .await
            .unwrap_or_default(),
    )
}

pub async fn api_finish(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FinishBody>,
) -> Result<Json<bool>, (StatusCode, String)> {
    let root = state.repo();
    off_thread(move || finish(&root, &body.name, &body.message))
        .await
        .map(|()| Json(true))
}

pub async fn api_discard(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DiscardBody>,
) -> Result<Json<bool>, (StatusCode, String)> {
    let root = state.repo();
    off_thread(move || discard(&root, &body.name, body.force))
        .await
        .map(|()| Json(true))
}

/// Commit everything in the request's checkout.
pub async fn api_commit(
    State(state): State<Arc<AppState>>,
    crate::serve::Checkout(checkout): crate::serve::Checkout,
    Json(body): Json<CommitBody>,
) -> Result<Json<bool>, (StatusCode, String)> {
    let _ = &state;
    off_thread(move || commit_all(&checkout, &body.message))
        .await
        .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for args in [
            &["init", "--quiet", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
        ] {
            git(&root, args).unwrap();
        }
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(
            root.join(".worktreeinclude"),
            ".env\n# comment\n../escape\n",
        )
        .unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "--quiet", "-m", "seed"]).unwrap();
        (dir, root)
    }

    /// A name is one directory under `.keel/worktrees`, or it is refused.
    #[test]
    fn a_lane_can_branch_from_a_branch_that_is_not_where_the_project_is() {
        let (_d, root) = repo();
        git(&root, &["commit", "-qam", "seed"]).ok();
        git(&root, &["checkout", "-qb", "release"]).unwrap();
        std::fs::write(root.join("only-on-release.txt"), "x").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-qm", "release work"]).unwrap();
        git(&root, &["checkout", "-q", "main"]).unwrap();

        let wt = create_from(&root, "from-release", Some("release")).unwrap();
        assert!(
            camino::Utf8Path::new(&wt.path)
                .join("only-on-release.txt")
                .exists(),
            "the lane starts where it was told to, not where the project is"
        );
        assert!(create_from(&root, "bad", Some("release; rm -rf /")).is_err());
    }

    /// Where the lane started is remembered, so finishing it puts the work back where it came
    /// from rather than wherever the project happens to be standing.
    #[test]
    fn a_lane_finishes_into_the_branch_it_came_from() {
        let (_d, root) = repo();
        git(&root, &["checkout", "-qb", "release"]).unwrap();
        std::fs::write(root.join("on-release.txt"), "x").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-qm", "release work"]).unwrap();
        git(&root, &["checkout", "-q", "main"]).unwrap();

        let wt = create_from(&root, "off-release", Some("release")).unwrap();
        assert_eq!(wt.base, "release");
        std::fs::write(Utf8PathBuf::from(&wt.path).join("new.txt"), "y").unwrap();

        // Counted against `release`, not against `main` — which is missing `release work` and
        // would have called this lane 2 commits ahead when it is 1.
        assert_eq!(unmerged(&root, "off-release"), 0, "nothing committed yet");
        commit_all(&Utf8PathBuf::from(&wt.path), "the lane's work").unwrap();
        assert_eq!(unmerged(&root, "off-release"), 1);

        let refused = finish(&root, "off-release", "merge").unwrap_err();
        assert!(refused.contains("release"), "{refused}");
        assert!(refused.contains("main"), "{refused}");
        assert!(
            git(&root, &["rev-parse", "--verify", "keel/off-release"]).is_ok(),
            "the lane was touched by a refusal"
        );

        git(&root, &["checkout", "-q", "release"]).unwrap();
        finish(&root, "off-release", "merge").unwrap();
        assert!(root.join("new.txt").exists(), "the work did not land");
        git(&root, &["checkout", "-q", "main"]).unwrap();
        assert!(
            !root.join("new.txt").exists(),
            "the work landed on main too"
        );
        assert!(
            git(
                &root,
                &["config", "--local", "--get", &base_key("off-release")]
            )
            .is_err(),
            "git did not take the recorded base away with the branch"
        );
    }

    /// The header promises a discard says what it would lose. It only ever counted commits, and
    /// `worktree remove --force` is exactly what makes a dirty checkout removable.
    #[test]
    fn discard_counts_uncommitted_work_as_work() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        std::fs::write(
            Utf8PathBuf::from(&wt.path).join("unsaved.txt"),
            "hours of it",
        )
        .unwrap();

        let refused = discard(&root, "feature", false).unwrap_err();
        assert!(refused.contains("1 uncommitted file"), "{refused}");
        assert!(
            Utf8PathBuf::from(&wt.path).join("unsaved.txt").exists(),
            "the refusal removed it anyway"
        );
        discard(&root, "feature", true).unwrap();
    }

    /// The count a discard shows is measured against the lane's base, and it is the only question
    /// that decides. `-d` asks git's own, different one — is this branch merged into its
    /// *upstream* — and answers no for a lane that is fully in `main` whenever
    /// `origin/keel/<name>` is merely behind it. Measured on a real lane: 0 ahead of `main`, and
    /// git refusing while saying it "is merged to HEAD". The checkout is removed first, so that
    /// refusal used to leave a branch with no worktree behind an error saying nothing happened.
    #[test]
    fn a_discard_that_promised_nothing_would_be_lost_finishes_the_job() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        // An upstream that is behind the branch: exactly the shape `-d` refuses on.
        git(
            &root,
            &["update-ref", "refs/remotes/origin/keel/feature", "HEAD~0"],
        )
        .ok();
        git(&root, &["config", "branch.keel/feature.remote", "origin"]).unwrap();
        git(
            &root,
            &[
                "config",
                "branch.keel/feature.merge",
                "refs/heads/keel/feature",
            ],
        )
        .unwrap();
        std::fs::write(Utf8PathBuf::from(&wt.path).join("x.txt"), "x").unwrap();
        commit_all(&Utf8PathBuf::from(&wt.path), "work").unwrap();
        git(
            &root,
            &["merge", "--no-ff", "-q", "-m", "in", "keel/feature"],
        )
        .unwrap();

        assert_eq!(unmerged(&root, "feature"), 0, "it is merged into its base");
        discard(&root, "feature", false).unwrap();
        assert!(!Utf8PathBuf::from(&wt.path).exists());
        assert!(
            git(&root, &["rev-parse", "--verify", "keel/feature"]).is_err(),
            "the branch outlived the discard that said it would go"
        );
    }

    /// Keel's view of its lanes and git's must not be able to drift apart.
    ///
    /// This listed the *directory*, so a checkout removed outside Keel vanished from the app while
    /// git kept it registered — and `worktree add` then refused that name forever with a message
    /// nothing in the app could reach, explain, or clear.
    #[test]
    fn a_checkout_removed_behind_keels_back_is_pruned_not_stuck() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        assert_eq!(list(&root).len(), 1, "the lane is not listed at all");

        std::fs::remove_dir_all(&wt.path).unwrap();
        assert!(
            list(&root).is_empty(),
            "a checkout that is gone is still listed"
        );
        assert!(
            create(&root, "feature").is_err(),
            "the branch is still there, so the name is still taken"
        );

        git(&root, &["branch", "-D", "keel/feature"]).unwrap();
        assert!(
            create(&root, "feature").is_ok(),
            "the name never became usable again"
        );
    }

    /// `.worktreeinclude` is repository content, and the only escape it filtered was `..`.
    #[test]
    fn worktreeinclude_cannot_reach_outside_the_repository() {
        let (_d, root) = repo();
        let outside = root.parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join("precious.txt"),
            "not yours
",
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.as_std_path(), root.join("link").as_std_path()).unwrap();
        std::fs::write(
            root.join(".worktreeinclude"),
            format!(
                "{outside}
link
.env
"
            ),
        )
        .unwrap();

        let wt = create(&root, "feature").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        assert_eq!(
            std::fs::read_to_string(outside.join("precious.txt")).unwrap(),
            "not yours
",
            "an absolute entry reached a file outside the repository"
        );
        assert!(!path.join("link").exists(), "a symlink entry was followed");
        assert!(
            path.join(".env").exists(),
            "an ordinary entry stopped working"
        );
    }

    #[test]
    fn a_lane_name_cannot_leave_the_worktrees_directory() {
        for bad in [
            "../x",
            "a/b",
            "",
            "-x",
            "A",
            "x y",
            ".hidden",
            &"a".repeat(50),
        ] {
            assert!(!valid_name(bad), "{bad:?} accepted");
        }
        assert!(valid_name("fix-billing-tests"));
        assert!(valid_name("lane-3"));
    }

    /// The lane is a real checkout on its own branch, the ignored file the project asked for
    /// arrives, and the project's own status stays clean.
    #[test]
    fn a_lane_is_a_checkout_with_the_included_files() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        assert_eq!(wt.branch, "keel/feature");
        assert_eq!(
            git(&path, &["branch", "--show-current"]).unwrap(),
            "keel/feature"
        );
        assert_eq!(
            std::fs::read_to_string(path.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert!(
            git(&root, &["status", "--porcelain"]).unwrap().is_empty(),
            "the checkout does not show up as untracked content of the project"
        );
        assert!(
            create(&root, "feature").is_err(),
            "no second lane of the same name"
        );
    }

    /// Finish merges the lane's work and removes it; the project must be clean first.
    #[test]
    fn finishing_a_lane_merges_it_and_only_deletes_a_merged_branch() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();

        std::fs::write(root.join("b.txt"), "stray\n").unwrap();
        assert!(
            finish(&root, "feature", "feature: a").is_err(),
            "project dirty"
        );
        std::fs::remove_file(root.join("b.txt")).unwrap();

        finish(&root, "feature", "feature: a").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(!path.exists());
        assert!(git(&root, &["rev-parse", "--verify", "keel/feature"]).is_err());
        assert!(list(&root).is_empty());
    }

    /// A conflict leaves the lane exactly as it was.
    #[test]
    fn a_conflict_refuses_and_leaves_the_lane() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        std::fs::write(path.join("a.txt"), "lane\n").unwrap();
        std::fs::write(root.join("a.txt"), "root\n").unwrap();
        git(&root, &["commit", "-qam", "root moved on"]).unwrap();

        let err = finish(&root, "feature", "feature").unwrap_err();
        assert!(err.contains("merge"), "{err}");
        assert!(path.exists(), "the lane survives");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "root\n"
        );
        assert!(
            git(&root, &["status", "--porcelain"]).unwrap().is_empty(),
            "merge aborted"
        );
    }

    /// Discard names the commits it would lose, and loses nothing until told to.
    #[test]
    fn discard_refuses_to_lose_commits_silently() {
        let (_d, root) = repo();
        let wt = create(&root, "feature").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        commit_all(&path, "work").unwrap();
        assert_eq!(list(&root)[0].ahead, 1);

        let err = discard(&root, "feature", false).unwrap_err();
        assert!(err.contains("1 unmerged commit"), "{err}");
        assert!(path.exists());

        discard(&root, "feature", true).unwrap();
        assert!(!path.exists());
        assert!(git(&root, &["rev-parse", "--verify", "keel/feature"]).is_err());
    }

    /// The automatic commit does not run the repository's hooks; the button does.
    ///
    /// A `pre-commit` hook is arbitrary code the repository's author wrote, and auto-commit runs
    /// after every accepted turn on Keel's own initiative. Measured with a `sleep 8` hook: 8.4s
    /// per turn before, 0.07s after. A commit the person asked for is a different act and keeps
    /// its hooks.
    #[test]
    fn the_automatic_commit_skips_the_repositorys_hooks() {
        let (_dir, root) = repo();
        std::fs::write(
            root.join(".git/hooks/pre-commit"),
            "#!/bin/sh\ntouch \"$(git rev-parse --show-toplevel)/hook-ran\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join(".git/hooks/pre-commit"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        std::fs::write(root.join("b.txt"), "two\n").unwrap();
        assert!(commit_all(&root, "an automatic checkpoint").unwrap());
        assert!(
            !root.join("hook-ran").exists(),
            "the repository's pre-commit hook ran inside Keel's automatic commit"
        );

        // The explicit one is the person's own act, and keeps every hook the repository has.
        std::fs::write(root.join("c.txt"), "three\n").unwrap();
        git(&root, &["add", "-A"]).unwrap();
        crate::repo::git_commit_staged(&root, "a commit the person asked for").unwrap();
        assert!(
            root.join("hook-ran").exists(),
            "an explicit commit must still run the repository's hooks"
        );
    }
}
