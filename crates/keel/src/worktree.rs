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

use crate::git::trimmed as git;

#[derive(Serialize, Clone)]
pub struct Worktree {
    pub name: String,
    pub branch: String,
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
    if git(root, &["rev-parse", "--git-dir"]).is_err() {
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
    copy_included(root, &path);
    Ok(Worktree {
        name: name.to_string(),
        branch: branch_of(name),
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
        if line.is_empty() || line.starts_with('#') || line.contains("..") {
            continue;
        }
        let from = root.join(line);
        let to = into.join(line);
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
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Every lane checkout, with how far each has gone.
pub fn list(root: &Utf8Path) -> Vec<Worktree> {
    let Ok(entries) = std::fs::read_dir(root.join(DIR)) else {
        return Vec::new();
    };
    let base = git(root, &["branch", "--show-current"]).unwrap_or_default();
    let mut out: Vec<Worktree> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_name(n))
        .map(|name| {
            let path = root.join(DIR).join(&name);
            let branch = branch_of(&name);
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
    if lost > 0 && !force {
        return Err(if lost == 1 {
            "1 commit on this lane is not merged and would be lost.".into()
        } else {
            format!("{lost} commits on this lane are not merged and would be lost.")
        });
    }
    git(root, &["worktree", "remove", "--force", path.as_str()])?;
    // `-D` only here, and only after the person was told the count. `finish` never uses it.
    git(
        root,
        &[
            "branch",
            if lost > 0 { "-D" } else { "-d" },
            &branch_of(name),
        ],
    )
    .map(|_| ())
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
    crate::serve::Checkout(checkout): crate::serve::Checkout,
    Json(body): Json<CommitBody>,
) -> Result<Json<bool>, (StatusCode, String)> {
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
        assert!(err.starts_with("1 commit "), "{err}");
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
