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
        // On a detached HEAD there is no branch to name, and recording nothing made `base_of`
        // fall back to whatever branch was checked out at finish — which merged the lane, and
        // main's history under it, into an unrelated release branch. The commit is recorded
        // instead, and `finish` refuses it as "not a branch" rather than guessing.
        match git(root, &["branch", "--show-current"]).unwrap_or_default() {
            b if b.is_empty() => git(root, &["rev-parse", "HEAD"]).unwrap_or_default(),
            b => b,
        }
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

/// Whether `name` is a local branch.
fn is_branch(root: &Utf8Path, name: &str) -> bool {
    git(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

/// Why a lane's recorded base cannot be finished into, when it cannot: a commit (cut on a
/// detached HEAD), or a branch that has since been renamed or deleted.
fn unfinishable(root: &Utf8Path, name: &str, base: &str) -> Option<String> {
    if base.is_empty() || is_branch(root, base) {
        return None;
    }
    let branch = branch_of(name);
    Some(
        if base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit()) {
            format!(
                "This feature was started on a detached HEAD (commit {}), not on a branch, so there is \
             no branch to finish it into. Merge {branch} by hand where you want it.",
                &base[..7]
            )
        } else {
            format!(
                "The branch this feature came from, “{base}”, no longer exists — renamed or deleted. \
             Merge {branch} by hand, or recreate “{base}” and finish it there."
            )
        },
    )
}

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
        && (base.starts_with('-')
            || !base
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c)))
    {
        return Err(format!("{base} is not a branch name"));
    }
    // A local branch, because that is what `finish` merges into: a remote-tracking ref or a bare
    // commit made a lane that could never be finished.
    if base != "HEAD" && !is_branch(root, base) {
        return Err(format!(
            "There is no local branch called “{base}” to start from."
        ));
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
    // A branch left by a lane whose checkout was deleted outside Keel: said in words, with the
    // way out, rather than as git's `fatal: a branch named … already exists`.
    if is_branch(root, &branch_of(name)) {
        return Err(format!(
            "A branch {} is left from an earlier lane whose checkout is gone. Discard that lane \
             first, or pick another name.",
            branch_of(name)
        ));
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
        let Some(from) = included_path(root, line) else {
            continue;
        };
        let Some(to) = included_path(into, line) else {
            continue;
        };
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

/// Both sides must remain below their checkout without following any symlink component. The
/// destination may already contain a tracked symlink even when the source is an ordinary file.
fn included_path(root: &Utf8Path, relative: &str) -> Option<Utf8PathBuf> {
    let mut path = root.to_path_buf();
    let mut has_name = false;
    for component in Utf8Path::new(relative).components() {
        let name = match component {
            camino::Utf8Component::CurDir => continue,
            camino::Utf8Component::Normal(name) => name,
            _ => return None,
        };
        if matches!(name, ".git" | ".keel") {
            return None;
        }
        has_name = true;
        path.push(name);
        if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            return None;
        }
    }
    has_name.then_some(path)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_name() == ".git"
            || entry.file_name() == ".keel"
            || target.symlink_metadata().is_ok_and(|m| m.is_symlink())
        {
            continue;
        }
        // `read_dir`'s file type does not follow links: an included symlink is skipped, never
        // walked or opened as a regular file.
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
    names(root)
}

/// The lane checkouts git has registered, and nothing else: one read-only git call. The watcher
/// wants this every six seconds and was calling `list`, which prunes (a write), then asks for
/// each lane's base, ahead count and a full status — five processes a lane for three names.
pub fn names(root: &Utf8Path) -> Vec<String> {
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
/// pretending otherwise would be a lie about what was saved. Every repository is attempted; a
/// partial failure reports both the saved repositories and the failures, leaving commits intact.
/// The person's commit (the button, a lane's finish): refuses only unresolved conflicts — a
/// merge whose conflicts are resolved is finished by exactly this commit.
pub fn commit_all(checkout: &Utf8Path, message: &str) -> Result<bool, String> {
    commit_every(checkout, message, false, None)
}

/// Keel's checkpoint after a turn: also refuses while a merge, rebase, cherry-pick, revert or
/// `am` is under way, because finishing the person's operation under a turn's prompt is not
/// Keel's to do.
///
/// Over the whole tree, so tests only now: a turn commits its own files, through
/// `checkpoint_only`. Kept because the refusals above are the automatic commit's either way, and
/// these are the tests that pin them.
#[cfg(test)]
pub fn checkpoint(checkout: &Utf8Path, message: &str) -> Result<bool, String> {
    commit_every(checkout, message, true, None)
}

/// Keel's checkpoint of one turn's own files and nothing else.
///
/// `add -A -- .` took the person's own uncommitted work into the turn's commit, under the
/// agent's prompt: their half-written `a.txt` and their `notes-mine.txt` were committed as "hi"
/// beside the one file the agent wrote. The turn knows what it changed — its files, against the
/// fingerprint taken when it began — and that is all its checkpoint holds.
pub fn checkpoint_only(
    checkout: &Utf8Path,
    message: &str,
    paths: &[String],
) -> Result<bool, String> {
    if paths.is_empty() {
        return Ok(false);
    }
    commit_every(checkout, message, true, Some(paths))
}

fn commit_every(
    checkout: &Utf8Path,
    message: &str,
    automatic: bool,
    only: Option<&[String]>,
) -> Result<bool, String> {
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
        let mut committed = Vec::new();
        let mut failures = Vec::new();
        for root in &found {
            // Each repository commits the turn's paths that are inside it, relative to itself.
            let mine: Option<Vec<String>> = only.map(|paths| {
                let prefix = if root.dir.is_empty() {
                    String::new()
                } else {
                    format!("{}/", root.dir)
                };
                paths
                    .iter()
                    .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
                    .collect()
            });
            if mine.as_ref().is_some_and(Vec::is_empty) {
                continue;
            }
            match commit_one(
                &checkout.join(&root.dir),
                message,
                automatic,
                mine.as_deref(),
            ) {
                Ok(true) => committed.push(root.dir.as_str()),
                Ok(false) => {}
                // One repository refusing must not lose the commit in the other.
                Err(e) => failures.push(format!("{}: {e}", root.dir)),
            }
        }
        if !failures.is_empty() {
            let failed = format!("Failed to commit: {}", failures.join("; "));
            return Err(if committed.is_empty() {
                failed
            } else {
                format!("Committed: {}. {failed}", committed.join(", "))
            });
        }
        return Ok(!committed.is_empty());
    }
    commit_one(checkout, message, automatic, only)
}

fn commit_one(
    checkout: &Utf8Path,
    message: &str,
    automatic: bool,
    only: Option<&[String]>,
) -> Result<bool, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("a commit needs a message".into());
    }
    // Never in the middle of the person's merge, rebase, cherry-pick or revert: `add -A` stages
    // the conflict markers and the commit finishes their operation under a turn's prompt.
    if let Some(what) = crate::writes::in_progress(checkout)
        && (automatic || what == "conflict")
    {
        return Err(if what == "conflict" {
            "there are unresolved conflicts — resolve them first".to_string()
        } else {
            format!("a {what} is in progress — finish or abort it first")
        });
    }
    // The index file itself, copied, so a commit that fails puts it back byte for byte: `add -A`
    // stages the person's untracked files too, and a failed commit used to leave them all staged.
    // A copy rather than `write-tree`/`read-tree`: that pair drops `add -N` entries and index
    // flags, and `write-tree` rewrites the index and fires `post-index-change`.
    let index = git(checkout, &["rev-parse", "--git-path", "index"])
        .ok()
        .map(|p| checkout.join(p.trim()));
    let saved = index.as_ref().and_then(|i| {
        let copy = tempfile::NamedTempFile::new_in(i.parent()?).ok()?;
        std::fs::copy(i, copy.path()).ok()?;
        Some(copy)
    });
    // `AUTOMATIC` on the `add` too: `post-index-change` is a hook, and `add` is what fires it.
    let mut add = crate::git::AUTOMATIC.to_vec();
    // The paths, when given, travel in a file: a turn that rewrote a thousand files is a command
    // line no shell takes.
    let mut wanted: Option<std::collections::HashSet<String>> = None;
    let listed = match only {
        Some(paths) => {
            let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
            // The quarantine is excluded here rather than as `:(exclude)` pathspecs, which git
            // refuses beside `--pathspec-from-file`.
            let quarantined = |p: &str| {
                keel_harness::trust::UNTRUSTED
                    .iter()
                    .chain(&[".keel/quarantine"])
                    .any(|q| p == *q || p.starts_with(&format!("{q}/")))
            };
            let keep: Vec<&String> = paths.iter().filter(|p| !quarantined(p)).collect();
            wanted = Some(keep.iter().map(|p| p.to_string()).collect());
            for p in keep {
                std::io::Write::write_all(&mut file, p.as_bytes()).map_err(|e| e.to_string())?;
                std::io::Write::write_all(&mut file, b"\0").map_err(|e| e.to_string())?;
            }
            Some(file)
        }
        None => None,
    };
    let from_file = listed
        .as_ref()
        .map(|f| format!("--pathspec-from-file={}", f.path().display()));
    match &from_file {
        Some(spec) => add.extend_from_slice(&["add", "-A", spec.as_str(), "--pathspec-file-nul"]),
        None => add.extend_from_slice(&["add", "-A", "--", "."]),
    }
    // What Keel quarantined is Keel's doing, not the turn's or the person's: committing it
    // deleted the repository's shared config on the branch under somebody's prompt.
    let excluded: Vec<String> = keel_harness::trust::UNTRUSTED
        .iter()
        .chain(&[".keel/quarantine"])
        .map(|p| format!(":(exclude){p}"))
        .collect();
    if from_file.is_none() {
        add.extend(excluded.iter().map(String::as_str));
    }
    git(checkout, &add)?;
    // Anything to commit — of the turn's paths, when it has them; the person may have staged
    // things of their own, which do not count and are not committed.
    let staged = git(checkout, &["diff", "--cached", "--name-only"])?;
    let any = staged
        .lines()
        .filter(|l| !l.trim().is_empty())
        .any(|l| wanted.as_ref().is_none_or(|w| w.contains(l)));
    if !any {
        return Ok(false);
    }
    // `crate::git::AUTOMATIC` and `--no-verify` for Keel's checkpoint only: see the note on
    // `AUTOMATIC` for why running the repository's own `pre-commit` hook after every turn is both
    // slow and a thing nobody asked for. A commit the person asked for — the Git panel's "Commit
    // all" comes through here too — is theirs, and keeps their hooks; it once skipped them as well.
    let mut args = if automatic {
        crate::git::AUTOMATIC.to_vec()
    } else {
        Vec::new()
    };
    args.extend_from_slice(&["commit", "-q"]);
    if automatic {
        args.push("--no-verify");
    }
    args.extend_from_slice(&["-m", message]);
    // With paths, `commit` takes those paths only — what the person staged themselves stays out.
    if let Some(spec) = &from_file {
        args.extend_from_slice(&[spec.as_str(), "--pathspec-file-nul"]);
    }
    git(checkout, &args).map(|_| true).inspect_err(|_| {
        if let (Some(index), Some(saved)) = (&index, saved) {
            // Renamed over it: the index is replaced whole, never half-written.
            let _ = saved.persist(index);
        }
    })
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
    if let Some(why) = unfinishable(root, name, &base) {
        return Err(why);
    }
    let here = git(root, &["branch", "--show-current"]).unwrap_or_default();
    if !base.is_empty() && base != here {
        let where_now = if here.is_empty() {
            "a detached HEAD".to_string()
        } else {
            format!("“{here}”")
        };
        return Err(format!(
            "This feature was branched from “{base}”, and the project is on {where_now}. \
             Switch to “{base}” and finish it there, so the work lands where it came from."
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
            "Could not merge {branch}: {}. Nothing was merged — the lane's changes are \
             committed on {branch}, and the project is as it was. Resolve it in a terminal.",
            why.trim_end_matches('.')
        ));
    }
    remove(root, name)
}

/// How many commits a discard would lose.
pub fn unmerged(root: &Utf8Path, name: &str) -> Result<u32, String> {
    let base = base_of(root, name);
    let branch = branch_of(name);
    git(root, &["rev-list", "--count", &format!("{base}..{branch}")])
        .map_err(|_| {
            format!(
                "Could not count what discarding {branch} would lose: the branch it came from, \
                 “{base}”, is not there any more. Discard it anyway only if you are sure."
            )
        })?
        .parse()
        .map_err(|e| format!("Could not count unmerged commits for {branch}: {e}"))
}

/// Throw a lane away. Refuses when its commits are not on the project's branch unless told the
/// count is acceptable — the number is in the refusal, so the person decides with it in front
/// of them.
pub fn discard(root: &Utf8Path, name: &str, force: bool) -> Result<(), String> {
    let path = path_of(root, name)?;
    if !path.exists() {
        return discard_without_checkout(root, name, force);
    }
    // A missing base or a failed Git command is not evidence that the lane has no work. Only
    // an explicit forced discard may proceed without a reliable count.
    let lost = if force { 0 } else { unmerged(root, name)? };
    // Uncommitted files count too. `worktree remove --force` is what makes a dirty checkout
    // removable at all, so a lane where the agent had written twenty files and committed none
    // reported nothing to lose and lost all of it — while this module's own header promised that
    // a discard says what it would lose before it loses it.
    // `-uall`: an untracked folder is one line otherwise, and 5,000 new files under `src/` were
    // reported as "1 uncommitted file" right before all of them were deleted.
    let uncommitted = git(&path, &["status", "--porcelain", "-uall"])?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
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
/// A lane whose checkout was deleted outside Keel: its branch is all that is left, and it has to
/// be removable, or `worktree add` refuses that name for good and nothing in the app can clear it.
/// The same promise as a discard with a checkout — the unmerged commits are counted and said
/// before anything is deleted.
fn discard_without_checkout(root: &Utf8Path, name: &str, force: bool) -> Result<(), String> {
    let branch = branch_of(name);
    if git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_err()
    {
        return Err(format!("no lane {name}"));
    }
    let _ = git(root, &["worktree", "prune"]);
    let lost = if force { 0 } else { unmerged(root, name)? };
    if lost > 0 && !force {
        return Err(format!(
            "This feature's checkout is gone, and its branch has {lost} unmerged commit{}, which \
             would be lost.",
            if lost == 1 { "" } else { "s" }
        ));
    }
    git(root, &["branch", "-D", &branch]).map(|_| ())
}

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
    // The lane's checkout is committed and the project is merged into: neither while an agent is
    // writing either. Finish committed a half-written file mid-turn as "Finish …".
    if let Ok(lane) = path_of(&root, &body.name)
        && lane.exists()
    {
        crate::serve::not_mid_turn(&state, &lane)?;
    }
    crate::serve::not_mid_turn(&state, &root)?;
    off_thread(move || finish(&root, &body.name, &body.message))
        .await
        .map(|()| Json(true))
}

pub async fn api_discard(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DiscardBody>,
) -> Result<Json<bool>, (StatusCode, String)> {
    let root = state.repo();
    // Not the checkout an agent is writing: a forced discard deleted it under the running agent.
    if let Ok(lane) = path_of(&root, &body.name)
        && lane.exists()
    {
        crate::serve::not_mid_turn(&state, &lane)?;
    }
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
    crate::serve::not_mid_turn(&state, &checkout)?;
    off_thread(move || commit_all(&checkout, &body.message))
        .await
        .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_git() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
        ] {
            crate::git::run(&root, args).unwrap();
        }
        std::fs::write(root.join("f"), "base\n").unwrap();
        crate::git::run(&root, &["add", "f"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "base"]).unwrap();
        (dir, root)
    }

    /// A checkpoint in the middle of the person's merge committed the conflict markers and
    /// finished their merge under a turn's prompt.
    #[test]
    fn a_checkpoint_never_finishes_the_persons_merge() {
        let (_d, root) = scratch_git();
        crate::git::run(&root, &["checkout", "-qb", "other"]).unwrap();
        std::fs::write(root.join("f"), "other\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "other"]).unwrap();
        crate::git::run(&root, &["checkout", "-q", "-"]).unwrap();
        std::fs::write(root.join("f"), "mine\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "mine"]).unwrap();
        let _ = crate::git::run(&root, &["merge", "other"]);
        std::fs::write(root.join("agent.txt"), "x").unwrap();
        let err = checkpoint(&root, "a turn").unwrap_err();
        assert!(
            err.contains("merge is in progress") || err.contains("unresolved conflicts"),
            "{err}"
        );
        assert_eq!(
            crate::git::trimmed(&root, &["log", "-1", "--format=%s"]).unwrap(),
            "mine"
        );
        assert!(root.join(".git/MERGE_HEAD").exists());
    }

    /// `stash pop` leaves a conflict and no MERGE_HEAD; the checkpoint committed the markers.
    #[test]
    fn a_conflict_with_no_merge_in_progress_is_not_committed() {
        let (_d, root) = scratch_git();
        std::fs::write(root.join("f"), "stashed\n").unwrap();
        crate::git::run(&root, &["stash", "-q"]).unwrap();
        std::fs::write(root.join("f"), "committed\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "c2"]).unwrap();
        let _ = crate::git::run(&root, &["stash", "pop"]);
        let err = commit_all(&root, "a turn").unwrap_err();
        assert!(err.contains("unresolved conflicts"), "{err}");
        assert_eq!(
            crate::git::trimmed(&root, &["log", "-1", "--format=%s"]).unwrap(),
            "c2"
        );
    }

    /// The person resolved their merge and pressed Commit: that commit is how a merge finishes,
    /// and only the automatic checkpoint may refuse it.
    #[test]
    fn a_resolved_merge_is_finished_by_the_persons_commit_not_by_a_checkpoint() {
        let (_d, root) = scratch_git();
        crate::git::run(&root, &["checkout", "-qb", "other"]).unwrap();
        std::fs::write(root.join("f"), "other\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "other"]).unwrap();
        crate::git::run(&root, &["checkout", "-q", "-"]).unwrap();
        std::fs::write(root.join("f"), "mine\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "mine"]).unwrap();
        let _ = crate::git::run(&root, &["merge", "other"]);
        std::fs::write(root.join("f"), "resolved\n").unwrap();
        crate::git::run(&root, &["add", "f"]).unwrap();
        assert!(checkpoint(&root, "a turn").is_err());
        assert_eq!(commit_all(&root, "merge other"), Ok(true));
        assert!(!root.join(".git/MERGE_HEAD").exists());
    }

    /// `git bisect reset` drops a commit made on bisect's detached HEAD.
    #[test]
    fn a_checkpoint_never_lands_on_a_bisect() {
        let (_d, root) = scratch_git();
        for n in 0..3 {
            std::fs::write(root.join("f"), format!("{n}\n")).unwrap();
            crate::git::run(&root, &["commit", "-qam", &format!("c{n}")]).unwrap();
        }
        crate::git::run(&root, &["bisect", "start"]).unwrap();
        crate::git::run(&root, &["bisect", "bad", "HEAD"]).unwrap();
        let _ = crate::git::run(&root, &["bisect", "good", "HEAD~3"]);
        std::fs::write(root.join("u.txt"), "x").unwrap();
        let err = checkpoint(&root, "a turn").unwrap_err();
        assert!(err.contains("bisect"), "{err}");
    }

    /// What Keel quarantined is not the turn's work: committing it deleted the repository's
    /// shared config on the branch.
    #[test]
    fn a_checkpoint_never_commits_the_quarantine() {
        let (_d, root) = scratch_git();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(root.join(".mcp.json"), "{}").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "config"]).unwrap();
        keel_harness::quarantine(&root).unwrap();
        std::fs::write(root.join("work.txt"), "x").unwrap();
        assert_eq!(checkpoint(&root, "a turn"), Ok(true));
        let shown = crate::git::run(&root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert_eq!(shown.trim(), "work.txt", "{shown}");
    }

    /// A failed checkpoint put everything the person had — untracked files included — in the
    /// index; the index goes back to exactly what it was, `add -N` entries and all.
    #[test]
    fn a_failed_checkpoint_leaves_the_index_as_it_was() {
        let (_d, root) = scratch_git();
        std::fs::write(root.join("staged.txt"), "s").unwrap();
        crate::git::run(&root, &["add", "staged.txt"]).unwrap();
        std::fs::write(root.join("ita.txt"), "i").unwrap();
        crate::git::run(&root, &["add", "-N", "ita.txt"]).unwrap();
        std::fs::write(root.join("untracked.txt"), "u").unwrap();
        let before = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        // A held ref lock makes the commit fail for certain, after `add` has staged everything.
        let branch = crate::git::trimmed(&root, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        std::fs::write(root.join(format!(".git/refs/heads/{branch}.lock")), "").unwrap();
        assert!(commit_all(&root, "a turn").is_err());
        assert_eq!(
            crate::git::run(&root, &["status", "--porcelain"]).unwrap(),
            before
        );
    }
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
        assert_eq!(
            unmerged(&root, "off-release").unwrap(),
            0,
            "nothing committed yet"
        );
        commit_all(&Utf8PathBuf::from(&wt.path), "the lane's work").unwrap();
        assert_eq!(unmerged(&root, "off-release").unwrap(), 1);

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

        assert_eq!(
            unmerged(&root, "feature").unwrap(),
            0,
            "it is merged into its base"
        );
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
    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn included_paths_cannot_follow_source_or_destination_symlinks() {
        let (_dir, root) = repo();
        let outside = tempfile::tempdir().unwrap();
        let into = tempfile::tempdir().unwrap();
        let into = Utf8Path::from_path(into.path()).unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, "private content\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked")).unwrap();
        std::fs::create_dir(root.join("config")).unwrap();
        std::fs::write(root.join("config/local"), "copied config\n").unwrap();
        std::fs::create_dir(into.join("config")).unwrap();
        std::os::unix::fs::symlink(&secret, into.join("config/local")).unwrap();
        std::fs::write(
            root.join(".worktreeinclude"),
            "linked/secret\nconfig\n.env\n",
        )
        .unwrap();

        copy_included(&root, into);
        assert!(!into.join("linked/secret").exists());
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "private content\n"
        );
        assert_eq!(
            std::fs::read_to_string(into.join(".env")).unwrap(),
            "SECRET=1\n"
        );
    }

    #[test]
    fn included_paths_allow_a_leading_current_directory() {
        let (_dir, root) = repo();
        std::fs::create_dir(root.join("config")).unwrap();
        std::fs::write(root.join("config/local"), "local config\n").unwrap();
        std::fs::write(root.join(".worktreeinclude"), "./.env\n./config/local\n").unwrap();

        let wt = create(&root, "relative").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        assert_eq!(
            std::fs::read_to_string(path.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("config/local")).unwrap(),
            "local config\n"
        );
        for rejected in [".", "./", "./../escape", "/absolute", "./.git/config"] {
            assert!(included_path(&root, rejected).is_none(), "{rejected}");
        }
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

    /// A turn's checkpoint holds the turn's files. It held the person's too: their unsaved
    /// edit and their own new file were committed under the agent's prompt.
    #[test]
    fn a_checkpoint_commits_the_turns_files_and_not_the_persons() {
        let (_d, root) = repo();
        std::fs::write(root.join("a.txt"), "one\nmine, unfinished\n").unwrap();
        std::fs::write(root.join("notes-mine.txt"), "notes\n").unwrap();
        std::fs::write(root.join("staged-mine.txt"), "staged\n").unwrap();
        git(&root, &["add", "staged-mine.txt"]).unwrap();
        std::fs::write(root.join("agent.txt"), "the turn wrote this\n").unwrap();

        assert!(checkpoint_only(&root, "the turn", &["agent.txt".into()]).unwrap());
        let shown = git(&root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert_eq!(
            shown.trim(),
            "agent.txt",
            "the commit took more than the turn's file"
        );
        let status = git(&root, &["status", "--porcelain"]).unwrap();
        assert!(
            status.contains("a.txt") && status.contains("notes-mine.txt"),
            "{status}"
        );
        assert!(
            status.contains("A  staged-mine.txt"),
            "the person's staging was disturbed: {status}"
        );
    }

    /// A lane cut on a detached HEAD has no branch to go back to. Finishing it merged it into
    /// whatever was checked out by then; it refuses and says why now.
    #[test]
    fn a_lane_from_a_detached_head_is_not_finished_into_another_branch() {
        let (_d, root) = repo();
        git(&root, &["checkout", "-q", "--detach"]).unwrap();
        let wt = create(&root, "det").unwrap();
        std::fs::write(Utf8PathBuf::from(&wt.path).join("x.txt"), "x\n").unwrap();
        git(&root, &["checkout", "-q", "-b", "release"]).unwrap();
        let err = finish(&root, "det", "Finish det").unwrap_err();
        assert!(err.contains("detached HEAD"), "{err}");
        assert_ne!(
            git(&root, &["log", "--format=%s", "-1"]).unwrap(),
            "Finish det",
            "it merged anyway"
        );
    }

    /// A base renamed after the lane was cut: named, not merged somewhere else, not raw git.
    #[test]
    fn a_lane_whose_base_was_renamed_says_so() {
        let (_d, root) = repo();
        git(&root, &["branch", "release"]).unwrap();
        create_from(&root, "rel", Some("release")).unwrap();
        git(&root, &["branch", "-m", "release", "release-2026"]).unwrap();
        let err = finish(&root, "rel", "Finish").unwrap_err();
        assert!(err.contains("no longer exists"), "{err}");
        let err = discard(&root, "rel", false).unwrap_err();
        assert!(!err.contains("fatal"), "{err}");
    }

    /// Only a local branch can be finished into, so only a local branch can be started from.
    #[test]
    fn a_lane_starts_from_a_local_branch_or_not_at_all() {
        let (_d, root) = repo();
        assert!(create_from(&root, "a", Some("--force")).is_err());
        assert!(create_from(&root, "b", Some("origin/release")).is_err());
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        assert!(create_from(&root, "c", Some(&head)).is_err());
    }

    /// 5,000 new files in one folder were "1 uncommitted file" right before they were deleted.
    #[test]
    fn a_discard_counts_every_new_file() {
        let (_d, root) = repo();
        let wt = create(&root, "many").unwrap();
        let src = Utf8PathBuf::from(&wt.path).join("src");
        std::fs::create_dir_all(&src).unwrap();
        for i in 0..20 {
            std::fs::write(src.join(format!("f{i}.txt")), "x").unwrap();
        }
        let err = discard(&root, "many", false).unwrap_err();
        assert!(err.contains("20 uncommitted files"), "{err}");
    }

    /// A checkout deleted outside Keel left a branch nothing could remove, and `worktree add`
    /// then refused that name for good. It can be discarded now, with the same count first.
    #[test]
    fn a_lane_whose_checkout_was_deleted_can_still_be_discarded() {
        let (_d, root) = repo();
        let wt = create(&root, "gone").unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        commit_all(&path, "work").unwrap();
        std::fs::remove_dir_all(&path).unwrap();

        let err = discard(&root, "gone", false).unwrap_err();
        assert!(err.contains("1 unmerged commit"), "{err}");
        discard(&root, "gone", true).unwrap();
        assert!(git(&root, &["rev-parse", "--verify", "keel/gone"]).is_err());
        create(&root, "gone").expect("the name is free again");
    }

    #[test]
    fn discard_refuses_when_the_base_branch_no_longer_exists() {
        let (_dir, root) = repo();
        git(&root, &["branch", "release"]).unwrap();
        let wt = create_from(&root, "feature", Some("release")).unwrap();
        let path = Utf8PathBuf::from(&wt.path);
        std::fs::write(path.join("a.txt"), "unmerged work\n").unwrap();
        commit_all(&path, "work").unwrap();
        git(&root, &["branch", "-d", "release"]).unwrap();

        let error = discard(&root, "feature", false).unwrap_err();
        assert!(error.contains("release"), "{error}");
        assert!(
            path.exists(),
            "uncertain commit counts must not permit removal"
        );
        assert!(git(&root, &["rev-parse", "--verify", "keel/feature"]).is_ok());
        discard(&root, "feature", true).unwrap();
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
        assert!(checkpoint(&root, "an automatic checkpoint").unwrap());
        assert!(
            !root.join("hook-ran").exists(),
            "the repository's pre-commit hook ran inside Keel's automatic commit"
        );

        // "Commit all" in the Git panel is the person's commit too; it used to skip the hooks
        // because it shared the checkpoint's path.
        std::fs::write(root.join("d.txt"), "four\n").unwrap();
        assert!(commit_all(&root, "commit all, from the Git panel").unwrap());
        assert!(
            root.join("hook-ran").exists(),
            "the person's Commit all skipped the repository's pre-commit hook"
        );
        std::fs::remove_file(root.join("hook-ran")).unwrap();

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
