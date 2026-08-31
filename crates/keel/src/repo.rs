//! Git, as the window uses it: status, diffs, branches, log, remotes, staging.
//!
//! The vocabulary this speaks — how a git is spawned, what a failed one says — is `crate::git`.
//! This is the layer above: what Keel asks git *for*.

use anyhow::Result;
use camino::Utf8Path;
use serde::Serialize;

use crate::git::{read as git, run as git_run};
use crate::tree::resolve;

/// One file with uncommitted changes.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub path: String,
    /// Two-character porcelain code, e.g. ` M`, `??`, `A `.
    pub status: String,
    /// Human label for the status.
    pub label: String,
    pub staged: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub changes: Vec<Change>,
    /// Every repository in the opened folder.
    ///
    /// One entry with an empty `dir` for an ordinary project, so nothing about the single-repo
    /// case changes. Two or more when the folder is a workspace — `backend/` and `frontend/`,
    /// each its own repository — which is the shape that used to report nothing at all.
    #[serde(default)]
    pub repos: Vec<RepoStatus>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct RepoStatus {
    /// Relative to the opened folder; empty when the folder is itself the repository.
    pub dir: String,
    pub branch: Option<String>,
    /// Paths relative to the *opened folder*, not to this repository — so the changes of a
    /// workspace can be one list, and `ChangeTree` groups them under `backend/` for free.
    pub changes: Vec<Change>,
}

/// Whether git is tracking this path. An untracked file has no version to be restored to.
fn tracked(root: &Utf8Path, path: &str) -> bool {
    git_run(root, &["ls-files", "--error-unmatch", "--", path]).is_ok()
}

/// Stage, unstage, or throw away one file's changes.
///
/// Discarding is the one that matters — reviewing a diff and deciding against it is most of what
/// the Changes panel is for, and doing it in the terminal means retyping a path you are already
/// looking at. It is also the only one that destroys anything, so an untracked file goes to the
/// Trash rather than being deleted: git has no copy of it, and "discard" should not mean "gone".
/// Turn a directory into a git repository.
///
/// Offered where the absence is noticed rather than reported as an error: a new project is not a
/// broken one, and "not a git repository" is a thing to fix in a click, not a thing to be told.
///
/// Only `git init`. No first commit, no author config, no `.gitignore` guessed from the language —
/// each of those is a decision that belongs to whoever owns the project, and doing them silently
/// is how a tool ends up in someone's history with an opinion they never had.
pub fn git_init(root: &Utf8Path) -> Result<(), String> {
    if root.join(".git").exists() {
        return Err("This is already a git repository.".into());
    }
    git_run(root, &["init"]).map(|_| ())
}

pub fn git_act(
    root: &Utf8Path,
    action: &str,
    path: &str,
    hunk: Option<usize>,
) -> Result<(), String> {
    // Same as the diff: stage or discard has to happen in the repository that owns the file.
    let (owner, inner) = crate::gitroots::resolve(root, path);
    let (root, path) = (owner.as_path(), inner.as_str());
    if path.is_empty() {
        return Err("no file".into());
    }
    // Resolved against the repository, so a path cannot climb out of it.
    let target = resolve(root, path)?;

    match action {
        "stage" => git_run(root, &["add", "--", path]).map(|_| ()),
        "unstage" => git_run(root, &["restore", "--staged", "--", path]).map(|_| ()),
        // One hunk, not the file. Reviewing a four-hunk edit and wanting three of them is the
        // ordinary case, and "discard the file and ask again" throws away the three.
        "discard-hunk" => {
            let n = hunk.ok_or("which hunk?")?;
            let raw = git_run(root, &["diff", "--no-color", "-U3", "--", path])?;
            let patch = one_hunk(&raw, n).ok_or(format!("no hunk {n} in {path}"))?;
            let mut child = crate::git::command(root)
                .args(["apply", "-R", "--recount", "--unidiff-zero", "-"])
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            child
                .stdin
                .take()
                .ok_or("no stdin")?
                .write_all(patch.as_bytes())
                .map_err(|e| e.to_string())?;
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
        "discard" => {
            if tracked(root, path) {
                git_run(root, &["restore", "--staged", "--worktree", "--", path]).map(|_| ())
            } else {
                crate::fsops::trash(target.as_std_path())
            }
        }
        _ => Err(format!("unknown action: {action}")),
    }
}

/// The file header and the `n`th `@@` block of a unified diff, as a patch of its own.
fn one_hunk(raw: &str, n: usize) -> Option<String> {
    let mut header = String::new();
    let mut hunks: Vec<String> = Vec::new();
    for line in raw.lines() {
        if line.starts_with("@@") {
            hunks.push(format!("{line}\n"));
        } else if let Some(last) = hunks.last_mut() {
            last.push_str(line);
            last.push('\n');
        } else {
            header.push_str(line);
            header.push('\n');
        }
    }
    hunks.get(n).map(|h| header + h)
}

/// One commit, for the list beside the working tree.
#[derive(Serialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    /// Seconds since the epoch.
    pub when: i64,
    pub files: u32,
    /// Whether the upstream branch has it. `true` when there is no upstream at all is a lie, so
    /// that case is `false` too: nothing has been pushed anywhere.
    pub pushed: bool,
}

/// The last `n` commits on the current branch.
///
/// A working tree that only ever grows is what makes people nervous about an agent; a list of
/// small commits beside it is what makes the same work look like progress.
pub fn git_log(root: &Utf8Path, n: usize) -> Vec<Commit> {
    let Some(raw) = git(
        root,
        &[
            "log",
            &format!("-{n}"),
            "--format=%h%x1f%s%x1f%ct",
            "--shortstat",
        ],
    ) else {
        return Vec::new();
    };
    // What the upstream does not have yet. No upstream means nothing is pushed.
    let unpushed: std::collections::HashSet<String> = git(root, &["rev-list", "@{u}..HEAD"])
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    let has_upstream = git(root, &["rev-parse", "--abbrev-ref", "@{u}"]).is_some();
    let full: Vec<String> = git(root, &["log", &format!("-{n}"), "--format=%H"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

    // Headers carry the separator; the stat line for a commit follows its header, blank lines
    // between, and the next header comes straight after the stat.
    let mut out: Vec<Commit> = Vec::new();
    for line in raw.lines() {
        if line.contains('\x1f') {
            let mut f = line.split('\x1f');
            let (Some(sha), Some(subject), Some(when)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let long = full.get(out.len()).cloned().unwrap_or_default();
            out.push(Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
                when: when.parse().unwrap_or(0),
                files: 0,
                pushed: has_upstream && !unpushed.contains(&long),
            });
        } else if line.contains("changed")
            && let Some(last) = out.last_mut()
            && let Some(n) = line.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            last.files = n;
        }
    }
    out
}

/// One commit's diff, file by file, in the shape the working-tree diff already has.
pub fn git_commit_diff(root: &Utf8Path, sha: &str) -> Result<Vec<DiffResponse>, String> {
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() > 40 {
        return Err("not a commit id".into());
    }
    let files = git_run(root, &["show", "--format=", "--name-only", sha])?;
    Ok(files
        .lines()
        .filter(|f| !f.is_empty())
        .map(|path| {
            let raw = git(
                root,
                &["show", "--format=", "--no-color", "-U3", sha, "--", path],
            )
            .unwrap_or_default();
            DiffResponse {
                path: path.to_string(),
                hunks: parse_hunks(&raw),
                untracked: false,
                note: None,
            }
        })
        .collect())
}

/// Send the branch to its remote, setting the upstream on the first push.
pub fn git_push(root: &Utf8Path) -> Result<String, String> {
    git_run(root, &["push", "-u", "origin", "HEAD"])
}

// ── branches and remotes: the rest of a git client ───────────────────────────

#[derive(Serialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Last commit's subject, for the list.
    pub subject: String,
}

#[derive(Serialize)]
pub struct Branches {
    pub current: Option<String>,
    pub local: Vec<Branch>,
    /// `origin/feature`, without the ones a local branch already tracks.
    pub remote: Vec<String>,
    pub remotes: Vec<String>,
    pub staged: u32,
    pub unstaged: u32,
}

/// Every branch, with how far each local one is from its upstream.
pub fn git_branches(root: &Utf8Path) -> Branches {
    let current = git(root, &["branch", "--show-current"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let raw = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%1f%(upstream:short)%1f%(upstream:track)%1f%(subject)",
            "refs/heads",
        ],
    )
    .unwrap_or_default();
    let mut tracked = std::collections::HashSet::new();
    let local: Vec<Branch> = raw
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\x1f');
            let name = f.next()?.to_string();
            let upstream = f.next().filter(|u| !u.is_empty()).map(str::to_string);
            let track = f.next().unwrap_or_default();
            let subject = f.next().unwrap_or_default().to_string();
            if let Some(u) = &upstream {
                tracked.insert(u.clone());
            }
            let count = |key: &str| -> u32 {
                track
                    .split(|c: char| !c.is_alphanumeric())
                    .collect::<Vec<_>>()
                    .windows(2)
                    .find(|w| w[0] == key)
                    .and_then(|w| w[1].parse().ok())
                    .unwrap_or(0)
            };
            Some(Branch {
                current: Some(&name) == current.as_ref(),
                ahead: count("ahead"),
                behind: count("behind"),
                name,
                upstream,
                subject,
            })
        })
        .collect();
    let remote: Vec<String> = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .unwrap_or_default()
    .lines()
    .map(str::to_string)
    .filter(|r| !r.ends_with("/HEAD") && !tracked.contains(r))
    .collect();
    let remotes: Vec<String> = git(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let status = git(root, &["status", "--porcelain"]).unwrap_or_default();
    let staged = status
        .lines()
        .filter(|l| l.len() > 1 && !l.starts_with([' ', '?']))
        .count() as u32;
    let unstaged = status
        .lines()
        .filter(|l| l.len() > 1 && (l.as_bytes()[1] != b' ' || l.starts_with("??")))
        .count() as u32;
    Branches {
        current,
        local,
        remote,
        remotes,
        staged,
        unstaged,
    }
}

fn valid_branch(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c == '~' || c == '^' || c == ':')
    {
        return Err(format!("{name:?} is not a branch name"));
    }
    Ok(())
}

/// Switch, create, or delete a branch. Delete is `-d`: a branch git will not delete safely is
/// one whose commits would vanish, and that is not a panel button's decision to make.
pub fn git_branch_act(root: &Utf8Path, action: &str, name: &str) -> Result<String, String> {
    valid_branch(name)?;
    match action {
        // `switch` refuses when it would lose work, which is the behaviour a button needs.
        "checkout" => {
            if name.contains('/')
                && git(
                    root,
                    &["rev-parse", "--verify", &format!("refs/heads/{name}")],
                )
                .is_none()
            {
                // A remote branch: make the tracking branch on the way.
                let short = name.split_once('/').map(|x| x.1).unwrap_or(name);
                git_run(root, &["switch", "--track", "-c", short, name])
            } else {
                git_run(root, &["switch", name])
            }
        }
        "create" => git_run(root, &["switch", "-c", name]),
        "delete" => git_run(root, &["branch", "-d", name]),
        _ => Err(format!("unknown branch action: {action}")),
    }
}

/// Fetch, pull, push — or add `origin`, for a project that was never pushed anywhere.
pub fn git_remote_act(root: &Utf8Path, action: &str, url: Option<&str>) -> Result<String, String> {
    match action {
        "fetch" => git_run(root, &["fetch", "--all", "--prune"]),
        // `--ff-only`: a merge commit nobody asked for is not a pull, and a conflict is a
        // decision for a person with a terminal.
        "pull" => git_run(root, &["pull", "--ff-only"]),
        "push" => git_push(root),
        "add" => {
            let url = url
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or("a remote URL is needed")?;
            // Only URL-shaped remotes: `-` would be read as a flag, and a bare path is a
            // mistake typed into a box, not a remote anyone meant.
            let ok = url.starts_with("https://")
                || url.starts_with("ssh://")
                || url.starts_with("git@") && url.contains(':');
            if !ok {
                return Err("use an https://, ssh:// or git@host:owner/repo URL".into());
            }
            git_run(root, &["remote", "add", "origin", "--", url])
        }
        _ => Err(format!("unknown remote action: {action}")),
    }
}

/// Drop every uncommitted change: tracked files go back to HEAD, untracked ones go to the
/// Trash (not `git clean`, which deletes for good). Ignored files are left alone — a `.env`
/// is not a change. Returns how many of each it touched.
pub fn git_discard_all(root: &Utf8Path) -> Result<(u32, u32), String> {
    let tracked = git(root, &["diff", "--name-only", "HEAD"])
        .map(|s| s.lines().filter(|l| !l.is_empty()).count() as u32)
        .unwrap_or(0);
    git_run(root, &["restore", "--staged", "--worktree", "--", "."])?;
    let untracked: Vec<String> = git(root, &["ls-files", "--others", "--exclude-standard"])
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for rel in &untracked {
        crate::fsops::trash(root.join(rel).as_std_path())?;
    }
    Ok((tracked, untracked.len() as u32))
}

/// Add a path to the repository's `.gitignore`, and stop tracking it if it was tracked.
///
/// The file stays on disk: this is "git should stop watching this", which is what somebody
/// means when they point at `.env.local` in the panel — not "delete my configuration".
pub fn git_ignore_path(root: &Utf8Path, path: &str) -> Result<(), String> {
    let path = path.trim().trim_start_matches("./");
    if path.is_empty() || path.starts_with('/') || path.contains("..") {
        return Err("that is not a path in this repository".into());
    }
    let file = root.join(".gitignore");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == path) {
        let mut next = existing;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(path);
        next.push('\n');
        std::fs::write(&file, next).map_err(|e| e.to_string())?;
    }
    // Tracked files keep being reported until the index forgets them; --cached leaves the disk
    // alone. A path git never knew is not an error here.
    let _ = git_run(root, &["rm", "-r", "--cached", "-q", "--", path]);
    Ok(())
}

/// Stage or unstage everything.
pub fn git_stage_all(root: &Utf8Path, stage: bool) -> Result<(), String> {
    if stage {
        git_run(root, &["add", "-A"]).map(|_| ())
    } else {
        git_run(root, &["reset", "-q"]).map(|_| ())
    }
}

/// Commit what is staged, and only that.
pub fn git_commit_staged(root: &Utf8Path, message: &str) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("a commit needs a message".into());
    }
    if git(root, &["diff", "--cached", "--quiet"]).is_some() {
        return Err("Nothing is staged. Stage files first, or commit everything.".into());
    }
    git_run(root, &["commit", "-q", "-m", message]).map(|_| ())
}

/// Take the last commit apart, keeping its changes in the working tree.
///
/// `--soft`, never `--hard`: undoing a commit Keel made must not undo the work in it.
pub fn git_uncommit(root: &Utf8Path) -> Result<(), String> {
    let count = git(root, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if count < 2 {
        return Err("This is the first commit; there is nothing to go back to.".into());
    }
    git_run(root, &["reset", "--soft", "HEAD~1"]).map(|_| ())
}

/// Uncommitted changes across every repository in the opened folder.
///
/// The single-repo answer is unchanged, and `is_repo`/`branch`/`changes` still describe it, so
/// every existing caller keeps working. A workspace of two repositories fills `repos` and merges
/// their changes into `changes` with each path prefixed by the repository's directory.
pub fn git_status(root: &Utf8Path) -> GitStatus {
    let found = crate::gitroots::find(root);
    // The opened folder is the repository: the ordinary case, and the one that must not change.
    if matches!(found.as_slice(), [one] if one.dir.is_empty()) {
        let mut status = git_status_in(root);
        status.repos = vec![RepoStatus {
            dir: String::new(),
            branch: status.branch.clone(),
            changes: status.changes.clone(),
        }];
        return status;
    }
    if found.is_empty() {
        return GitStatus::default();
    }

    // A workspace. Each repository answers for itself, and its paths are rewritten to be relative
    // to the folder the person actually opened.
    let mut repos = Vec::new();
    let mut all = Vec::new();
    for root_dir in &found {
        let mut status = git_status_in(&root.join(&root_dir.dir));
        for change in &mut status.changes {
            change.path = format!("{}/{}", root_dir.dir, change.path);
        }
        all.extend(status.changes.clone());
        repos.push(RepoStatus {
            dir: root_dir.dir.clone(),
            branch: status.branch,
            changes: status.changes,
        });
    }
    GitStatus {
        // True: this folder *is* versioned, just not at its top. Reporting false here is what put
        // "Initialise a repository" in front of people whose repositories already existed.
        is_repo: true,
        branch: None,
        changes: all,
        repos,
    }
}

fn git_status_in(root: &Utf8Path) -> GitStatus {
    // `-uall` rather than the default. Without it git collapses an untracked directory to a single
    // entry ending in `/` — `.github/` instead of the three files under it — which is useless in a
    // list you click to open a file, and rendered as a row with no name at all, because the
    // basename of "a/b/" is the empty string.
    let Some(raw) = git(root, &["status", "--porcelain=v1", "-z", "-uall"]) else {
        return GitStatus::default();
    };

    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|b| b.trim().to_string());

    // NUL-separated so paths containing spaces or quotes survive intact.
    let changes = raw
        .split('\0')
        .filter(|entry| entry.len() > 3)
        .map(|entry| {
            let (status, path) = entry.split_at(2);
            let staged = !status.starts_with([' ', '?']);
            Change {
                // A submodule still arrives with a trailing slash, and nothing downstream should
                // have to know that.
                path: path.trim_start().trim_end_matches('/').to_string(),
                status: status.to_string(),
                label: label_for(status).to_string(),
                staged,
            }
        })
        .collect();

    GitStatus {
        is_repo: true,
        branch,
        changes,
        repos: Vec::new(),
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn ignoring_a_path_writes_gitignore_once_and_leaves_the_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join(".env.local"), "SECRET=1").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "seed"]);

        git_ignore_path(&root, ".env.local").unwrap();
        git_ignore_path(&root, ".env.local").unwrap();
        let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(ignore.matches(".env.local").count(), 1, "{ignore}");
        assert!(root.join(".env.local").exists(), "the file is not deleted");
        let status = git(&root, &["status", "--porcelain"]).unwrap();
        assert!(
            status.contains("D  .env.local"),
            "untracked in the index: {status}"
        );
        assert!(git_ignore_path(&root, "../escape").is_err());
    }

    /// A path that is not a file in this checkout is not a new file. The changes list can be a
    /// moment out of date — a commit in the terminal, a revert in another lane — and clicking one
    /// of its rows then asked for a diff of a path git has never heard of.
    #[test]
    fn a_path_that_is_not_there_is_not_reported_as_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "seed"]);

        let gone = git_diff(&root, "was-deleted-or-never-here.txt");
        assert!(!gone.untracked, "no NEW badge for a path with no file");
        assert!(gone.hunks.is_empty());

        // A file that is genuinely new still reads as new, whole.
        std::fs::write(root.join("new.txt"), "x\ny\n").unwrap();
        let fresh = git_diff(&root, "new.txt");
        assert!(fresh.untracked);
        assert_eq!(fresh.hunks.len(), 1);

        // A tracked file that matches HEAD is not new — and is no longer blank either. Auto-commit
        // puts a turn's work in a commit seconds after it lands, so "matches HEAD" is the ordinary
        // state of everything the agent just wrote; the diff now comes from the commit that holds
        // it, with a note saying which. See `a_committed_change_is_still_shown_and_says_where_it_is`.
        let same = git_diff(&root, "a.txt");
        assert!(!same.untracked);
        assert!(
            !same.hunks.is_empty(),
            "the committing commit is the baseline"
        );
        assert!(
            same.note
                .is_some_and(|n| n.starts_with("Already committed"))
        );
    }

    #[test]
    fn discard_all_restores_tracked_and_trashes_untracked_but_keeps_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one").unwrap();
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "seed"]);
        std::fs::write(root.join("a.txt"), "two").unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        let (t, u) = git_discard_all(&root).unwrap();
        assert_eq!((t, u), (1, 1));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one");
        assert!(!root.join("new.txt").exists());
        assert!(root.join(".env").exists(), "ignored files are not changes");
    }

    #[test]
    fn a_remote_can_be_added_once_and_only_as_a_url() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::process::Command::new("git")
            .current_dir(&root)
            .args(["init", "--quiet"])
            .output()
            .unwrap();
        assert!(git_remote_act(&root, "add", None).is_err());
        assert!(git_remote_act(&root, "add", Some("--upload-pack=x")).is_err());
        assert!(git_remote_act(&root, "add", Some("/tmp/somewhere")).is_err());
        git_remote_act(&root, "add", Some("git@github.com:o/r.git")).unwrap();
        assert!(git_branches(&root).remotes.contains(&"origin".to_string()));
        // A second origin is a real error from git, surfaced, not silently replaced.
        assert!(git_remote_act(&root, "add", Some("https://x/y")).is_err());
    }

    /// `git status --porcelain` reports an untracked *directory* as one entry ending in `/`, so a
    /// new folder of twelve files was one row whose basename was the empty string — a blank line
    /// in the changes list that opened nothing.
    #[test]
    fn untracked_directories_are_listed_as_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("seed"), "x").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/a.yml"), "a").unwrap();
        std::fs::write(root.join(".github/workflows/b.yml"), "b").unwrap();

        let status = git_status(&root);
        let paths: Vec<_> = status.changes.iter().map(|c| c.path.as_str()).collect();

        assert!(
            paths.contains(&".github/workflows/a.yml")
                && paths.contains(&".github/workflows/b.yml"),
            "expected the files, got {paths:?}"
        );
        for path in &paths {
            assert!(!path.ends_with('/'), "{path} is a directory, not a file");
            assert!(
                !path.rsplit('/').next().unwrap_or_default().is_empty(),
                "{path} has no basename, so its row would render blank"
            );
        }
    }

    /// Discard is the only panel action that destroys anything, so it is the one worth a test:
    /// the file has to come back exactly as it was committed, and staging has to be reversible.
    #[test]
    fn a_file_can_be_staged_unstaged_and_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "committed\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        let staged = |root: &Utf8Path| {
            git_status(root)
                .changes
                .iter()
                .find(|c| c.path == "a.txt")
                .map(|c| c.staged)
        };
        assert_eq!(staged(&root), Some(false), "an edit starts unstaged");

        git_act(&root, "stage", "a.txt", None).expect("stage");
        assert_eq!(staged(&root), Some(true));
        git_act(&root, "unstage", "a.txt", None).expect("unstage");
        assert_eq!(staged(&root), Some(false));

        git_act(&root, "discard", "a.txt", None).expect("discard");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "committed\n"
        );
        assert!(
            git_status(&root).changes.is_empty(),
            "the working tree is clean again"
        );

        // A path that is not in the repository never reaches git.
        assert!(git_act(&root, "discard", "../../etc/hosts", None).is_err());
        assert!(git_act(&root, "nonsense", "a.txt", None).is_err());
    }

    /// Branches: create, switch, list with tracking; stage-all and a staged-only commit.
    #[test]
    fn branches_can_be_made_switched_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        git_branch_act(&root, "create", "feature").unwrap();
        let b = git_branches(&root);
        assert_eq!(b.current.as_deref(), Some("feature"));
        assert_eq!(b.local.len(), 2);
        assert!(b.local.iter().any(|x| x.name == "main" && !x.current));

        std::fs::write(root.join("b.txt"), "x\n").unwrap();
        assert_eq!(git_branches(&root).unstaged, 1);
        assert!(git_commit_staged(&root, "nothing staged").is_err());
        git_stage_all(&root, true).unwrap();
        assert_eq!(git_branches(&root).staged, 1);
        git_commit_staged(&root, "add b").unwrap();
        assert_eq!(git_branches(&root).staged, 0);

        git_branch_act(&root, "checkout", "main").unwrap();
        assert!(!root.join("b.txt").exists(), "switched");
        assert!(
            git_branch_act(&root, "delete", "feature").is_err(),
            "unmerged: -d refuses"
        );
        assert!(git_branch_act(&root, "create", "bad name").is_err());
        assert!(git_branch_act(&root, "create", "-x").is_err());
    }

    /// The log reads back what was committed, and undoing keeps the work.
    #[test]
    fn the_log_lists_commits_and_uncommit_keeps_the_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);
        assert!(git_uncommit(&root).is_err(), "the first commit stays");

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("b.txt"), "new\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "turn 1: add b"]);

        let log = git_log(&root, 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "turn 1: add b");
        assert_eq!(log[0].files, 2);
        assert!(log[0].when > 0);

        git_uncommit(&root).unwrap();
        assert_eq!(git_log(&root, 10).len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("b.txt")).unwrap(),
            "new\n",
            "the work survives"
        );
    }

    /// The header's numbers, not the header's `@@`.
    #[test]
    fn hunk_headers_number_both_sides() {
        let raw = "@@ -48,6 +48,14 @@ describe(\"x\", () => {\n a\n+b\n c\n@@ -7 +7,2 @@\n x\n+y\n";
        let hunks = parse_hunks(raw);
        assert_eq!(hunks[0].lines[0].old, Some(48));
        assert_eq!(hunks[0].lines[0].new, Some(48));
        assert_eq!(hunks[0].lines[1].new, Some(49));
        assert_eq!(hunks[0].lines[2].old, Some(49));
        assert_eq!(hunks[1].lines[0].old, Some(7));
        assert_eq!(hunks[1].lines[1].new, Some(8));
    }

    /// Discarding one hunk leaves the other. The reason the action exists at all.
    #[test]
    fn one_hunk_can_be_discarded_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("a.txt"), &body).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        // Two edits far enough apart to be two hunks.
        let edited = body
            .replace("line 2\n", "LINE 2\n")
            .replace("line 28\n", "LINE 28\n");
        std::fs::write(root.join("a.txt"), &edited).unwrap();
        assert_eq!(git_diff(&root, "a.txt").hunks.len(), 2);

        git_act(&root, "discard-hunk", "a.txt", Some(0)).expect("discard the first");
        let now = std::fs::read_to_string(root.join("a.txt")).unwrap();
        assert!(now.contains("line 2\n"), "the first edit is gone");
        assert!(now.contains("LINE 28\n"), "the second survives");
        assert_eq!(git_diff(&root, "a.txt").hunks.len(), 1);

        assert!(
            git_act(&root, "discard-hunk", "a.txt", Some(5)).is_err(),
            "no such hunk"
        );
        assert!(git_act(&root, "discard-hunk", "a.txt", None).is_err());
    }

    /// The one every tester hit: auto-commit is on, so seconds after the agent writes a file the
    /// change is in a commit and `git diff` says nothing. Every diff in the window went blank, and
    /// a blank pane reads as "Keel lost my change".
    #[test]
    fn a_committed_change_is_still_shown_and_says_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        let uncommitted = git_diff(&root, "a.txt");
        assert!(!uncommitted.hunks.is_empty());
        assert_eq!(uncommitted.note, None, "the working tree needs no excuse");

        // Staged, then committed — the two states a turn passes through.
        run(&["add", "-A"]);
        let staged = git_diff(&root, "a.txt");
        assert!(!staged.hunks.is_empty(), "staged work is still work");
        assert_eq!(staged.note.as_deref(), Some("Staged, not yet committed."));

        run(&["commit", "--quiet", "-m", "turn 1: change a"]);
        let committed = git_diff(&root, "a.txt");
        assert!(
            committed
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .any(|l| l.kind == "add" && l.text == "two"),
            "the change is shown from the commit that holds it"
        );
        let note = committed.note.expect("says where the change went");
        assert!(note.contains("turn 1: change a"), "{note}");

        // A file the agent wrote and deleted again is not a bug in Keel, and says so.
        std::fs::write(root.join("scratch.txt"), "temp\n").unwrap();
        assert!(!git_diff(&root, "scratch.txt").hunks.is_empty());
        std::fs::remove_file(root.join("scratch.txt")).unwrap();
        let gone = git_diff(&root, "scratch.txt");
        assert!(gone.hunks.is_empty());
        assert_eq!(
            gone.note.as_deref(),
            Some("This file is not on disk any more.")
        );
    }
}

fn label_for(status: &str) -> &'static str {
    match status.trim() {
        "M" | "MM" => "modified",
        "A" => "added",
        "D" => "deleted",
        "R" => "renamed",
        "??" => "untracked",
        "C" => "copied",
        "U" | "UU" => "conflicted",
        _ => "changed",
    }
}

#[derive(Default, Serialize)]
pub struct DiffResponse {
    pub path: String,
    pub hunks: Vec<Hunk>,
    /// True when the file is untracked, so there is no baseline to diff against.
    pub untracked: bool,
    /// Where these hunks came from when they are not the working tree, or why there are none.
    ///
    /// Keel commits a passing turn by itself, so by the time anyone clicks the file the change is
    /// usually *already committed* and `git diff` is empty. A blank pane then reads as "Keel lost
    /// my change"; this says which commit holds it.
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize)]
pub struct DiffLine {
    /// `add`, `del`, or `ctx`.
    pub kind: &'static str,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// Parse `git diff` for one path into hunks the browser can render side by side with line numbers.
pub fn git_diff(root: &Utf8Path, path: &str) -> DiffResponse {
    // In a workspace the path carries the repository it came from — `backend/src/api.ts` — and git
    // has to be run inside that repository, under the name it knows the file by.
    let (root, path) = crate::gitroots::resolve(root, path);
    let (root, path) = (root.as_path(), path.as_str());
    // An untracked file has no baseline; show it as entirely added rather than an empty diff.
    //
    // It has to exist for that to mean anything. "git does not know this path" also answers for a
    // path git cannot match at all — one from a stale changes list, or from another checkout —
    // and calling that untracked put a NEW badge on a diff with nothing under it.
    let untracked =
        root.join(path).exists() && git(root, &["ls-files", "--error-unmatch", path]).is_none();

    let (raw, note) = if untracked {
        match std::fs::read_to_string(root.join(path)) {
            Ok(content) => {
                let n = content.lines().count();
                let body = content
                    .lines()
                    .map(|l| format!("+{l}\n"))
                    .collect::<String>();
                (format!("@@ -0,0 +1,{n} @@\n{body}"), None)
            }
            // Written and then deleted again, which is what a scratch file is. Saying so beats an
            // empty pane that looks like a bug in Keel.
            Err(_) => (
                String::new(),
                Some("This file is not on disk any more.".to_string()),
            ),
        }
    } else {
        let unstaged = git(root, &["diff", "--no-color", "-U3", "--", path]).unwrap_or_default();
        if !unstaged.trim().is_empty() {
            (unstaged, None)
        } else {
            let staged = git(root, &["diff", "--cached", "--no-color", "-U3", "--", path])
                .unwrap_or_default();
            if !staged.trim().is_empty() {
                (staged, Some("Staged, not yet committed.".to_string()))
            } else {
                committed_diff(root, path)
            }
        }
    };

    let (hunks, trimmed) = capped(parse_hunks(&raw));
    DiffResponse {
        path: path.to_string(),
        // The cap's own note wins: "this is not all of it" is the more important thing to say.
        note: trimmed.or(note),
        hunks,
        untracked,
    }
}

/// The most diff Keel will send for one file.
///
/// An agent regenerating a lockfile is an ordinary turn, and `pnpm-lock.yaml` is tens of thousands
/// of lines. Measured: a 30,000-line untracked file came back as 3.8 MB of JSON — fast on this
/// side (46 ms) and then decoded, intraline-marked and laid out on the app's main actor, which is
/// the thread with 16 ms. Nobody reads the twelve-thousandth line of a lockfile; they read that
/// the lockfile changed.
///
/// It cuts *inside* a hunk, not only between hunks. The first version kept whole hunks on the
/// theory that a truncated one has line numbers that lie — which is backwards: what is kept is a
/// prefix, and a prefix's numbers are exactly as true as they were. Whole-hunks-only also failed
/// on the one case that motivated the cap, because a newly written file is a single hunk with
/// every line in it, so the 3.8 MB went out unchanged with a note on top saying it had not.
const MAX_DIFF_LINES: usize = 3_000;

fn capped(hunks: Vec<Hunk>) -> (Vec<Hunk>, Option<String>) {
    let total: usize = hunks.iter().map(|h| h.lines.len()).sum();
    if total <= MAX_DIFF_LINES {
        return (hunks, None);
    }
    let mut kept: Vec<Hunk> = Vec::new();
    let mut lines = 0;
    for mut h in hunks {
        if lines >= MAX_DIFF_LINES {
            break;
        }
        h.lines.truncate(MAX_DIFF_LINES - lines);
        lines += h.lines.len();
        kept.push(h);
    }
    (
        kept,
        Some(format!(
            "Showing the first {lines} of {total} changed lines — more than this pane will draw. \
             Open the file to see the rest."
        )),
    )
}

/// The commit that last touched this file, when the working tree has nothing to show.
///
/// The everyday case rather than the corner: auto-commit is on by default, so a turn's work is in
/// a commit seconds after it is written, and every diff in the window went blank at that moment.
fn committed_diff(root: &Utf8Path, path: &str) -> (String, Option<String>) {
    // Nothing in the working tree, nothing staged, and nothing in history either. The reason
    // matters: a file that was written and deleted again says so, where a stale path from another
    // checkout has genuinely nothing to report. Untracked-and-gone reaches here rather than the
    // arm above because a path with no file on disk must not wear a NEW badge over an empty diff.
    let nothing = || {
        if root.join(path).exists() {
            (String::new(), Some("No changes to show.".to_string()))
        } else {
            (
                String::new(),
                Some("This file is not on disk any more.".to_string()),
            )
        }
    };
    let Some(head) = git(root, &["log", "-1", "--format=%h %s", "--", path]) else {
        return nothing();
    };
    let head = head.trim();
    let Some((sha, subject)) = head.split_once(' ') else {
        return nothing();
    };
    let raw = git(
        root,
        &["show", "--no-color", "-U3", "--format=", sha, "--", path],
    )
    .unwrap_or_default();
    (
        raw,
        Some(format!("Already committed, in {sha} — {subject}")),
    )
}

/// `@@` blocks with numbered lines, from any unified diff.
fn parse_hunks(raw: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);

    for line in raw.lines() {
        if line.starts_with("@@") {
            // @@ -old,count +new,count @@ …  — the numbers only. The `@@` used to be the first
            // token, failed to parse, and every hunk's old side started at line 1.
            let nums: Vec<u32> = line
                .split_once(" @@")
                .map(|(head, _)| head)
                .unwrap_or(line)
                .split(['-', '+', ',', ' ', '@'])
                .filter_map(|s| s.parse().ok())
                .collect();
            old_no = nums.first().copied().unwrap_or(1);
            // A hunk with one old line has no count: `-7 +7,2`. Old is first, new is the one
            // after old's count when there is one.
            let old_has_count = line
                .split_whitespace()
                .nth(1)
                .is_some_and(|t| t.contains(','));
            new_no = nums
                .get(if old_has_count { 2 } else { 1 })
                .copied()
                .unwrap_or(1);
            hunks.push(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };

        let (kind, text) = match line.chars().next() {
            Some('+') => ("add", &line[1..]),
            Some('-') => ("del", &line[1..]),
            Some(' ') => ("ctx", &line[1..]),
            _ => continue,
        };

        let (old, new) = match kind {
            "add" => (None, Some(new_no)),
            "del" => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if kind != "add" {
            old_no += 1;
        }
        if kind != "del" {
            new_no += 1;
        }

        hunk.lines.push(DiffLine {
            kind,
            old,
            new,
            text: text.to_string(),
        });
    }
    hunks
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    /// A huge diff is trimmed, and says so.
    ///
    /// A regenerated lockfile is an ordinary turn. Before the cap, 30,000 lines went over the
    /// wire as 3.8 MB of JSON and were laid out on the app's main actor.
    #[test]
    fn a_diff_too_big_to_draw_is_trimmed_and_says_so() {
        let big: String = (0..40_000).map(|i| format!("+line {i}\n")).collect();
        let (kept, note) = capped(parse_hunks(&format!("@@ -0,0 +1,40000 @@\n{big}")));

        let shown: usize = kept.iter().map(|h| h.lines.len()).sum();
        assert_eq!(shown, MAX_DIFF_LINES, "capped, not merely annotated");
        let note = note.expect("it must say the diff was trimmed");
        assert!(note.contains("3000 of 40000 changed lines"), "{note}");
    }

    /// The case the cap exists for: a new file is one hunk holding every line of it. Keeping
    /// whole hunks only would let this straight through, which is what the first version did.
    #[test]
    fn a_single_oversized_hunk_is_cut_inside() {
        let big: String = (0..20_000).map(|i| format!("+line {i}\n")).collect();
        let (kept, note) = capped(parse_hunks(&format!("@@ -0,0 +1,20000 @@\n{big}")));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].lines.len(), MAX_DIFF_LINES);
        assert!(note.is_some());
    }

    /// What survives is a prefix, so its line numbers are the file's own.
    #[test]
    fn the_lines_that_survive_keep_their_numbers() {
        let big: String = (0..10_000).map(|i| format!("+line {i}\n")).collect();
        let (kept, _) = capped(parse_hunks(&format!("@@ -0,0 +1,10000 @@\n{big}")));
        assert_eq!(kept[0].lines.first().unwrap().new, Some(1));
        assert_eq!(
            kept[0].lines.last().unwrap().new,
            Some(MAX_DIFF_LINES as u32),
            "the last line kept is numbered where it actually is"
        );
    }

    /// An ordinary diff is untouched — no note, no trimming.
    #[test]
    fn an_ordinary_diff_carries_no_note() {
        let (kept, note) = capped(parse_hunks("@@ -1,2 +1,2 @@\n-one\n+two\n ctx\n"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].lines.len(), 3);
        assert!(note.is_none());
    }
}
