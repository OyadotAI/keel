//! Files Keel writes into a project on the person's behalf — the team (`/api/adopt`), a subagent
//! (`/api/agents/create`), a skill (`/api/skills/*`) — and the three rules they share.
//!
//! **Hold the tree.** The checkpoint after a turn is `git add -A`, so anything written into a
//! tree a turn is writing is committed under that turn's prompt and reported as its work. So a
//! write takes the same writer claim a turn does ([`hold`]), and is refused while anyone else
//! holds it — and the refusal says who.
//!
//! **Never through a link, never over a file.** [`create_new`] is `O_CREAT|O_EXCL`: it fails on a
//! file that exists, on one that appeared a moment ago, and on a symlink even when it dangles —
//! which `path.exists()` followed by `fs::write` did not, so a repository could have Keel create
//! a file anywhere its user can write.
//!
//! **Commit its own paths, or leave the index as it was.** [`commit_only`] commits exactly the
//! paths it wrote, `--no-verify` because it is Keel's checkpoint and not the person's commit; when
//! the commit cannot happen (a merge in progress, no identity, an ignored folder) the paths are
//! unstaged again, so the person's next commit — perhaps their merge — does not carry them.

use crate::serve::{AppState, Held};
use axum::http::StatusCode;
use camino::Utf8Path;
use std::sync::Arc;

pub type Failure = (StatusCode, String);

/// Take `root`'s writer claim for one of Keel's own writes, or say who has it.
pub fn hold(state: &Arc<AppState>, root: &Utf8Path, what: &str) -> Result<Held, Failure> {
    // The tree is in the key, so the same write in two checkouts does not collide on the key;
    // in one checkout, the writer check below is what refuses it.
    let key = format!("keel:{what}:{root}");
    let token = state
        .claim(&key, root, true)
        .map_err(|e| match state.writer_of(root) {
            Some(holder) => (StatusCode::CONFLICT, busy(&holder).to_string()),
            // Not a writer: the claim could not be taken at all — a project folder moved away
            // under the window. Its own words, not "wait", which would never end.
            None => (StatusCode::INTERNAL_SERVER_ERROR, e),
        })?;
    Ok(Held::new(state.clone(), key, token))
}

fn busy(holder: &str) -> &'static str {
    if holder.starts_with("keel:") {
        "Keel is writing other files into this project right now — try again in a moment."
    } else if holder.starts_with("term:") {
        "A session running in a terminal is editing this working tree — try again when it goes \
         idle."
    } else {
        "A turn is editing this project's working tree — try again when that turn and its \
         checks finish."
    }
}

/// Run `change` holding an exclusive lock on `<path>.lock` — a file lock, so it holds across
/// processes: the app's daemon and a `keel serve` from a terminal share one HOME, and a mutex in
/// one of them lost 40 of 200 concurrent renames and brought a revoked device back. Each call
/// opens its own handle, so threads in one process exclude each other too.
pub fn locked<T>(path: &std::path::Path, change: impl FnOnce() -> T) -> Result<T, String> {
    let lock = path.with_extension("lock");
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    let out = change();
    let _ = file.unlock();
    Ok(out)
}

/// Replace a file whole: written beside it and renamed over it, so a reader sees the old file or
/// the new one and never half of either. A plain `fs::write` truncates first, and a reader in
/// that moment parses an empty store as "nothing saved" — which is how concurrent session
/// renames lost 192 of 200 nicknames.
pub fn replace(path: &std::path::Path, body: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = path.parent().ok_or("no parent folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    tmp.write_all(body).map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether anything — a file, a folder, a symlink even if it dangles — is at `path`. Any error
/// but absence names the path, because "could not check" is not "not there".
pub fn taken(path: &Utf8Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{path}: {e}")),
    }
}

/// Write a file that must not exist yet. `Ok(false)` when something is already there.
pub fn create_new(path: &Utf8Path, body: &[u8]) -> Result<bool, String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(format!("{path}: {e}")),
    };
    file.write_all(body).map_err(|e| format!("{path}: {e}"))?;
    Ok(true)
}

/// Refuse a path whose folders pass through a symlink. `create_new` guards the last component
/// only; `create_dir_all` and a write both follow a linked `.claude` or `docs`, and a committed
/// `.claude -> ../../.claude` would put Keel's files in the person's home.
pub fn no_link_under(root: &Utf8Path, rel: &str) -> Result<(), String> {
    let mut at = root.to_owned();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symbolic link — left alone",
                    at.strip_prefix(root).unwrap_or(&at)
                ));
            }
            Ok(m) if !m.is_dir() => {
                return Err(format!(
                    "{} is a file, not a folder — left alone",
                    at.strip_prefix(root).unwrap_or(&at)
                ));
            }
            // A submodule or a nested checkout is another repository; writing there changes a
            // project nobody opened, and git refuses to commit it from here anyway.
            Ok(_) if at.join(".git").exists() => {
                return Err(format!(
                    "{} is another git repository — left alone",
                    at.strip_prefix(root).unwrap_or(&at)
                ));
            }
            Ok(_) => {}
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

/// Whether `rel` holds nothing of the person's that git has not already committed: absent
/// altogether, or tracked with index and working tree both equal to HEAD. Asked of the one file
/// about to be written rather than of a list of names, so a link to some other file, a staged
/// rename onto it, or an untracked file hidden by `status.showUntrackedFiles` all read as theirs.
pub fn clean_before(root: &Utf8Path, rel: &str) -> bool {
    let out = |args: &[&str]| crate::git::run(root, args).ok();
    // `<mode> <blob>` from the first line of `ls-tree` / `ls-files -s` output.
    let entry = |line: Option<String>, blob_at: usize| -> Option<(String, String)> {
        let line = line?;
        let fields: Vec<&str> = line.split_whitespace().collect();
        Some((
            fields.first()?.to_string(),
            fields.get(blob_at)?.to_string(),
        ))
    };
    let path = root.join(rel);
    let on_disk = std::fs::symlink_metadata(&path).ok();
    let index = entry(out(&["ls-files", "-s", "--", rel]), 1);
    if on_disk.is_none() && index.is_none() {
        return true;
    }
    // Content and mode, not git's opinion of them: `assume-unchanged` and `skip-worktree` make
    // `git diff` report an edited file as clean, and those edits are exactly the private kind;
    // and a `chmod +x` is a change too.
    let head = entry(out(&["ls-tree", "HEAD", "--", rel]), 2);
    let disk = on_disk.and_then(|m| {
        use std::os::unix::fs::PermissionsExt;
        let mode = if m.permissions().mode() & 0o111 != 0 {
            "100755"
        } else {
            "100644"
        };
        let blob = crate::git::trimmed(root, &["hash-object", "--", rel]).ok()?;
        Some((mode.to_string(), blob))
    });
    head.is_some() && head == index && head == disk
}

/// The outcome of [`commit_only`]: the commit, and anything the person should be told.
#[derive(Debug, Default)]
pub struct Committed {
    pub sha: Option<String>,
    pub note: Option<String>,
}

/// Commit exactly `paths` — files Keel just wrote, each clean before it did — and nothing else.
///
/// The person's index is theirs: nothing is committed while a merge, rebase, cherry-pick or
/// revert is under way (a commit there would change what their commit holds); ignored paths are
/// left out and named; and when the commit fails, what `add` staged is unstaged again — retried
/// past a moment's `index.lock`, because the daemon's own watcher runs `git status` every few
/// seconds, and an unstage that silently lost that race left the file in the person's next commit.
pub fn commit_only(root: &Utf8Path, paths: &[String], message: &str) -> Committed {
    let say = |note: String| Committed {
        sha: None,
        note: Some(note),
    };
    if crate::git::run(root, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return say("not a git repository — not committed".into());
    }
    if let Some(what) = in_progress(root) {
        return say(format!("a {what} is in progress — not committed"));
    }
    let ignored = ignored(root, paths);
    let paths: Vec<String> = paths
        .iter()
        .filter(|p| !ignored.contains(p))
        .cloned()
        .collect();
    // A path can be half ignored: a skill folder whose `*.py` the repository ignores commits
    // its markdown and not its script, and a teammate who clones gets a skill that cannot run.
    let mut left_out = ignored.clone();
    if !paths.is_empty() {
        // Never with an empty pathspec, which would list every ignored file in the repository.
        left_out.extend(ignored_within(root, &paths));
    }
    let ignored_note = (!left_out.is_empty()).then(|| {
        const SHOWN: usize = 5;
        let more = left_out.len().saturating_sub(SHOWN);
        let mut list = left_out
            .iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if more > 0 {
            list.push_str(&format!(" and {more} more"));
        }
        format!("ignored by .gitignore, so not committed: {list}")
    });
    if paths.is_empty() {
        return Committed {
            sha: None,
            note: ignored_note,
        };
    }
    let with = |head: &[&str]| -> Vec<String> {
        head.iter()
            .map(|s| s.to_string())
            .chain(std::iter::once("--".to_string()))
            .chain(paths.iter().cloned())
            .collect()
    };
    // Every git here is Keel's own, so every one runs without the repository's hooks — `add` and
    // `reset` fire `post-index-change`, not only `commit` its three.
    let run = |args: Vec<String>| {
        let args: Vec<&str> = crate::git::AUTOMATIC
            .iter()
            .copied()
            .chain(args.iter().map(String::as_str))
            .collect();
        locked_retry(|| crate::git::run(root, &args))
    };
    let commit = ["commit", "-q", "--no-verify", "-m", message, "--only"];
    // A lock still held after the retries when `add` ran means it never got the index: nothing
    // to undo. At the commit step it is different — `add` staged, so the backout below runs.
    let staged = match run(with(&["add"])) {
        Err(e) if e.contains("index.lock") => return say(short(root, &reason(&e))),
        Err(e) => Err(e),
        Ok(_) => run(with(&commit)),
    };
    if let Err(e) = staged {
        // Back out of the index what `add` put there — a failed `add` can still have staged some
        // of the list (a sparse checkout stages what it can, then exits non-zero). Every path
        // was clean before Keel wrote it, so HEAD's version — or no entry, with no HEAD — is
        // exactly what the index held.
        let has_head = crate::git::trimmed(root, &["rev-parse", "-q", "--verify", "HEAD"]).is_ok();
        let undo = if has_head {
            run(with(&["reset", "-q"]))
        } else {
            run(with(&["rm", "--cached", "-r", "-q", "--ignore-unmatch"]))
        };
        let mut note = reason(&e);
        if let Err(u) = undo {
            note.push_str(&format!(
                "; and they could not be unstaged ({}) — check `git status` before committing",
                reason(&u)
            ));
        }
        return say(short(root, &note));
    }
    Committed {
        sha: crate::git::trimmed(root, &["rev-parse", "--short", "HEAD"]).ok(),
        note: ignored_note,
    }
}

/// A git that found `index.lock` held tried at a bad moment, not a bad thing; a second later it
/// is usually free. Bounded, so a lock left by a crashed git fails with its own message.
fn locked_retry(mut attempt: impl FnMut() -> Result<String, String>) -> Result<String, String> {
    const TRIES: u32 = 20;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(100);
    let mut last = attempt();
    for _ in 1..TRIES {
        match &last {
            Err(e) if e.contains("index.lock") => {
                std::thread::sleep(PAUSE);
                last = attempt();
            }
            _ => break,
        }
    }
    last
}

/// A merge, rebase, cherry-pick or revert under way, by the file git keeps for it.
fn in_progress(root: &Utf8Path) -> Option<&'static str> {
    [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ]
    .into_iter()
    .find(|(file, _)| {
        crate::git::trimmed(root, &["rev-parse", "--git-path", file])
            .is_ok_and(|p| root.join(p).exists())
    })
    .map(|(_, what)| what)
}

/// Files under `paths` that exist and that `.gitignore` keeps out of the commit.
fn ignored_within(root: &Utf8Path, paths: &[String]) -> Vec<String> {
    let mut args = vec![
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    crate::git::run(root, &args)
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A note without the project's absolute path in it: the person knows where their project is.
fn short(root: &Utf8Path, note: &str) -> String {
    note.replace(&format!("{root}/"), "")
        .replace(root.as_str(), ".")
}

fn ignored(root: &Utf8Path, paths: &[String]) -> Vec<String> {
    let mut args = vec!["check-ignore", "--"];
    args.extend(paths.iter().map(String::as_str));
    // Exit 1 means none are ignored; either way stdout is the list.
    crate::git::run(root, &args)
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Git's message without its hints and without its `fatal:` or `error:` — every line of it,
/// joined, because git wraps one sentence over several lines and puts the list it announced on the
/// lines after the colon. Capped, so a list of a thousand paths does not become the note.
fn reason(stderr: &str) -> String {
    const MAX: usize = 200;
    // The one git failure a brand-new Mac always hits, said in a line rather than git's twelve.
    if stderr.contains("Author identity unknown") || stderr.contains("unable to auto-detect email")
    {
        return "git has no author identity here — set user.name and user.email".into();
    }
    let joined = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
        .collect::<Vec<_>>()
        .join(" ");
    let out = joined
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .to_string();
    if out.chars().count() > MAX {
        format!("{}…", out.chars().take(MAX).collect::<String>())
    } else if out.is_empty() {
        stderr.trim().to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
        ] {
            crate::git::run(&root, args).unwrap();
        }
        (dir, root)
    }

    /// A dangling symlink is how a repository would point Keel's write somewhere else.
    #[test]
    fn a_new_file_never_goes_through_a_link() {
        let (_d, root) = repo();
        let outside = tempfile::tempdir().unwrap();
        let target = Utf8Path::from_path(outside.path())
            .unwrap()
            .join("pwned.md");
        std::os::unix::fs::symlink(&target, root.join("pm.md")).unwrap();
        assert_eq!(create_new(&root.join("pm.md"), b"x"), Ok(false));
        assert!(!target.exists());
        assert!(taken(&root.join("pm.md")).unwrap());
    }

    /// The person's merge must not come to carry a skill because Keel's commit failed.
    #[test]
    fn a_failed_commit_leaves_the_index_as_it_was() {
        let (_d, root) = repo();
        std::fs::write(root.join("f"), "base\n").unwrap();
        crate::git::run(&root, &["add", "f"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "base"]).unwrap();
        crate::git::run(&root, &["checkout", "-qb", "other"]).unwrap();
        std::fs::write(root.join("f"), "other\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "other"]).unwrap();
        crate::git::run(&root, &["checkout", "-q", "-"]).unwrap();
        std::fs::write(root.join("f"), "mine\n").unwrap();
        crate::git::run(&root, &["commit", "-qam", "mine"]).unwrap();
        let _ = crate::git::run(&root, &["merge", "other"]);
        std::fs::write(root.join("new.md"), "x").unwrap();

        let done = commit_only(&root, &["new.md".into()], "Add");
        assert_eq!(
            done.note.as_deref(),
            Some("a merge is in progress — not committed"),
            "{done:?}"
        );
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert!(status.contains("?? new.md"), "{status}");
        assert!(status.contains("UU f"), "{status}");
    }

    /// The watcher's `git status` holds `index.lock` for a moment; an unstage that lost that race
    /// once left a file in the person's next commit.
    #[test]
    fn a_held_lock_is_waited_for_not_ignored() {
        let mut calls = 0;
        let got = locked_retry(|| {
            calls += 1;
            if calls < 3 {
                Err("fatal: Unable to create '.git/index.lock': File exists.".into())
            } else {
                Ok("ok".into())
            }
        });
        assert_eq!((got, calls), (Ok("ok".to_string()), 3));
    }

    #[test]
    fn ignored_paths_are_named_and_the_rest_committed() {
        let (_d, root) = repo();
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/a.md"), "x").unwrap();
        std::fs::write(root.join("b.md"), "x").unwrap();
        let done = commit_only(&root, &[".claude/a.md".into(), "b.md".into()], "Add");
        assert!(done.sha.is_some(), "{done:?}");
        assert_eq!(
            done.note.as_deref(),
            Some("ignored by .gitignore, so not committed: .claude/a.md")
        );
        let shown = crate::git::run(&root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert_eq!(shown.trim(), "b.md");
    }

    #[test]
    fn a_path_through_a_linked_folder_is_refused() {
        let (_d, root) = repo();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), root.join(".claude")).unwrap();
        assert!(no_link_under(&root, ".claude/agents/pm.md").is_err());
        std::fs::write(root.join("docs"), "a file").unwrap();
        assert!(
            no_link_under(&root, "docs/PRODUCTION.md")
                .unwrap_err()
                .contains("is a file")
        );
        assert!(no_link_under(&root, "CLAUDE.md").is_ok());
        assert!(no_link_under(&root, "new/dir/x.md").is_ok());
    }

    #[test]
    fn with_no_commits_a_failure_unstages_too() {
        let (_d, root) = repo();
        crate::git::run(&root, &["config", "user.email", ""]).unwrap();
        crate::git::run(&root, &["config", "user.useConfigOnly", "true"]).unwrap();
        std::fs::write(root.join("new.md"), "x").unwrap();
        // With no identity git refuses the commit; with no HEAD there is nothing to reset to.
        if commit_only(&root, &["new.md".into()], "Add").sha.is_none() {
            let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
            assert!(status.contains("?? new.md"), "{status}");
        }
    }

    fn committed(root: &Utf8Path, name: &str, body: &str) {
        std::fs::write(root.join(name), body).unwrap();
        crate::git::run(root, &["add", "-A"]).unwrap();
        crate::git::run(root, &["commit", "-qm", name]).unwrap();
    }

    /// Asked of the one file about to be written, so none of git's ways of hiding the person's
    /// work from a name-based `status` let Keel's commit take it.
    #[test]
    fn clean_before_sees_every_way_the_file_is_theirs() {
        let (_d, root) = repo();
        assert!(
            clean_before(&root, "CLAUDE.md"),
            "absent is Keel's to create"
        );
        committed(&root, "CLAUDE.md", "# x\n");
        assert!(clean_before(&root, "CLAUDE.md"));
        std::fs::write(root.join("CLAUDE.md"), "# x\nwip\n").unwrap();
        assert!(!clean_before(&root, "CLAUDE.md"), "edited");
        crate::git::run(&root, &["add", "CLAUDE.md"]).unwrap();
        assert!(!clean_before(&root, "CLAUDE.md"), "staged");
        crate::git::run(&root, &["commit", "-qm", "wip"]).unwrap();

        // A staged `git mv` onto the name.
        committed(&root, "AGENTS.md", "# a\n");
        crate::git::run(&root, &["rm", "-q", "CLAUDE.md"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "rm"]).unwrap();
        crate::git::run(&root, &["mv", "AGENTS.md", "CLAUDE.md"]).unwrap();
        assert!(!clean_before(&root, "CLAUDE.md"), "renamed onto");

        // Untracked, with untracked files hidden from status.
        let (_d2, other) = repo();
        crate::git::run(&other, &["config", "status.showUntrackedFiles", "no"]).unwrap();
        committed(&other, "README", "r");
        std::fs::write(other.join("CLAUDE.md"), "# private\n").unwrap();
        assert!(!clean_before(&other, "CLAUDE.md"), "untracked and hidden");
    }

    /// `assume-unchanged` and `skip-worktree` make `git diff` call an edited file clean; the
    /// content says otherwise, and those are exactly the edits meant to stay private.
    #[test]
    fn index_bits_do_not_hide_the_persons_edits() {
        for bit in ["--assume-unchanged", "--skip-worktree"] {
            let (_d, root) = repo();
            committed(&root, "CLAUDE.md", "# c\n");
            crate::git::run(&root, &["update-index", bit, "CLAUDE.md"]).unwrap();
            std::fs::write(root.join("CLAUDE.md"), "# c\nPRIVATE\n").unwrap();
            assert!(!clean_before(&root, "CLAUDE.md"), "{bit}");
        }
    }

    #[test]
    fn a_mode_change_is_the_persons_edit() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, root) = repo();
        committed(&root, "CLAUDE.md", "# c\n");
        std::fs::set_permissions(
            root.join("CLAUDE.md"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(!clean_before(&root, "CLAUDE.md"));
    }

    #[test]
    fn a_missing_identity_is_one_line() {
        let msg = "Author identity unknown\n\n*** Please tell me who you are.\n\nRun\n\n  git config --global user.email \"you@example.com\"\nfatal: unable to auto-detect email address";
        assert_eq!(
            reason(msg),
            "git has no author identity here — set user.name and user.email"
        );
    }

    /// Two daemons on one HOME, or two threads in one: every read-modify-write is kept.
    #[test]
    fn a_file_lock_keeps_every_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("count");
        std::fs::write(&file, "0").unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let file = file.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        locked(&file, || {
                            let n: u32 = std::fs::read_to_string(&file).unwrap().parse().unwrap();
                            replace(&file, (n + 1).to_string().as_bytes()).unwrap();
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "200");
    }

    /// In a sparse checkout `add` stages what it can and then fails; what it staged goes back.
    #[test]
    fn a_partial_add_is_backed_out() {
        let (_d, root) = repo();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("app/a"), "a").unwrap();
        std::fs::write(root.join("docs/x"), "d").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "d"]).unwrap();
        if crate::git::run(&root, &["sparse-checkout", "set", "--cone", "app"]).is_err() {
            return; // a git without sparse-checkout cannot reach this path
        }
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/new.md"), "x").unwrap();
        std::fs::write(root.join("keel.md"), "x").unwrap();
        let done = commit_only(&root, &["keel.md".into(), "docs/new.md".into()], "Add");
        let staged = crate::git::run(&root, &["diff", "--cached", "--name-only"]).unwrap();
        if done.sha.is_none() {
            assert!(staged.trim().is_empty(), "left staged: {staged} ({done:?})");
            assert!(!done.note.unwrap_or_default().contains("  "));
        }
    }

    /// With every requested path ignored, the note is about those paths, not the repository.
    #[test]
    fn an_ignored_folder_note_stays_about_that_folder() {
        let (_d, root) = repo();
        committed(&root, ".gitignore", ".claude/\nnode_modules/\n.env.local\n");
        std::fs::write(root.join(".env.local"), "K=1").unwrap();
        std::fs::create_dir_all(root.join(".claude/skills/s")).unwrap();
        std::fs::write(root.join(".claude/skills/s/SKILL.md"), "x").unwrap();
        let done = commit_only(&root, &[".claude/skills/s".into()], "Add");
        assert_eq!(
            done.note.as_deref(),
            Some("ignored by .gitignore, so not committed: .claude/skills/s")
        );
    }

    /// `--no-verify` skips two hooks; Keel's checkpoint runs none, `post-commit` included.
    #[test]
    fn no_hook_runs_on_keels_commit() {
        let (_d, root) = repo();
        let hook = root.join(".git/hooks/post-commit");
        std::fs::write(&hook, "#!/bin/sh\ntouch ran\n").unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::write(root.join("a"), "x").unwrap();
        assert!(commit_only(&root, &["a".into()], "Add").sha.is_some());
        assert!(!root.join("ran").exists(), "post-commit ran");
    }

    #[test]
    fn another_repository_inside_is_not_written_into() {
        let (_d, root) = repo();
        std::fs::create_dir_all(root.join("docs/.git")).unwrap();
        let err = no_link_under(&root, "docs/PRODUCTION.md").unwrap_err();
        assert!(err.contains("another git repository"), "{err}");
    }

    /// A skill folder whose `*.py` the repository ignores commits without its script; say so.
    #[test]
    fn a_half_ignored_folder_names_what_was_left_out() {
        let (_d, root) = repo();
        committed(&root, ".gitignore", "*.py\n");
        std::fs::create_dir_all(root.join("skill/scripts")).unwrap();
        std::fs::write(root.join("skill/SKILL.md"), "x").unwrap();
        std::fs::write(root.join("skill/scripts/run.py"), "x").unwrap();
        let done = commit_only(&root, &["skill".into()], "Add");
        assert!(done.sha.is_some(), "{done:?}");
        assert_eq!(
            done.note.as_deref(),
            Some("ignored by .gitignore, so not committed: skill/scripts/run.py")
        );
    }

    #[test]
    fn outside_a_repository_it_says_so_plainly() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::write(root.join("a"), "x").unwrap();
        let done = commit_only(root, &["a".into()], "Add");
        assert_eq!(
            done.note.as_deref(),
            Some("not a git repository — not committed")
        );
    }

    #[test]
    fn the_refusal_names_who_holds_the_tree() {
        let (_d, root) = repo();
        let state = Arc::new(AppState::new(root.clone()));
        let turn = state.claim("lane", &root, true).unwrap();
        let err = hold(&state, &root, "x").err().unwrap();
        assert!(err.1.starts_with("A turn is editing"), "{}", err.1);
        state.release("lane", turn);
        let mine = hold(&state, &root, "x").unwrap();
        let err = hold(&state, &root, "y").err().unwrap();
        assert!(err.1.starts_with("Keel is writing"), "{}", err.1);
        drop(mine);
        assert!(hold(&state, &root, "y").is_ok());
    }
}
