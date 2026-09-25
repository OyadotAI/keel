//! Every `git` the daemon runs, built in one place.
//!
//! There were three `git()` helpers in three files, each of them `Command::new("git")` and
//! nothing else, and "nothing else" is the problem: a bare git is an *interactive* program that
//! happens not to have asked you anything yet.
//!
//! - A repository whose remote is HTTPS with no cached credential asks for a username. With
//!   `output()` stdin is `/dev/null`, so that path fails fast — but an `askpass` helper does not
//!   read stdin. `SSH_ASKPASS`, Git Credential Manager and macOS's own keychain helper all put a
//!   *window* up, and the git process waits on it for as long as nobody clicks. The request
//!   behind it is a `spawn_blocking` thread, so the thread waits too, and the window shows a
//!   spinner with no way to cancel it.
//! - `commit.gpgsign = true` is the same shape and closer to home, because auto-commit runs after
//!   every accepted turn: gpg-agent raises a pinentry dialog, and a person who does not notice it
//!   has a Keel that appears to have stopped committing.
//!
//! So the environment says no once, here, rather than in thirty call sites that each have to
//! remember. Failing is fine — a credential that is not there is a thing Keel can report. Waiting
//! forever is not.

use camino::Utf8Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// What makes it non-interactive. One list, so the sync and async builders cannot drift.
///
/// `GIT_TERMINAL_PROMPT=0` covers the terminal, which does not exist here anyway. The askpass
/// entries are the ones that matter: a helper that exits 0 with no output is read as an empty
/// credential, so the operation fails immediately with something Keel can report, instead of
/// waiting on a dialog nobody knows is open.
const QUIET: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", "/usr/bin/true"),
    ("GCM_INTERACTIVE", "never"),
    // Keel's reads (`status` every two seconds) must not write the index: an optional refresh
    // takes `index.lock`, so the person's own `git add` failed now and then, and it fired the
    // repository's `post-index-change` hook on nobody's request.
    ("GIT_OPTIONAL_LOCKS", "0"),
];

/// `git`, in `root`, that will never stop to ask a human something.
pub fn command(root: &Utf8Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root).env_remove("SSH_ASKPASS");
    for (k, v) in QUIET {
        c.env(k, v);
    }
    c
}

/// The same, for the paths that stream a command's output as it runs.
pub fn tokio_command(root: &Utf8Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("git");
    c.current_dir(root).env_remove("SSH_ASKPASS");
    for (k, v) in QUIET {
        c.env(k, v);
    }
    c
}

/// The arguments that make a commit Keel's own bookkeeping rather than the person's.
///
/// `--no-verify` skips the repository's `pre-commit` and `commit-msg` hooks, and that is a
/// deliberate line rather than a shortcut:
///
/// - **It is arbitrary code from the repository.** `.claude/settings.json` hooks are quarantined
///   before anything runs because a hook is a shell command written by whoever wrote the repo.
///   `.git/hooks` is the same code from the same author, and auto-commit runs it on Keel's own
///   initiative — nobody asked for this commit, Keel decided to make one.
/// - **It has no ceiling.** A `husky` + `lint-staged` hook is tens of seconds; a hook that runs
///   the test suite is minutes. Measured with a trivial `sleep 8` hook, one auto-commit took
///   8.4 seconds; the app gives up at 90 and shows a failure for a commit that then happens
///   anyway. Once per turn, all of it after the work is already done.
/// - **It is duplicated work.** Keel has already run the project's own declared check command
///   against this turn and shown the result. The pre-commit hook is a second, slower, invisible
///   run of the same thing, and it is not the one anyone reads.
///
/// An explicit commit the person asked for keeps its hooks. This is only the automatic one.
///
/// Signing goes with it: a per-turn checkpoint that raises a pinentry dialog is a checkpoint that
/// stops the turn.
///
/// And every hook, not only the two `--no-verify` skips: `post-commit` still ran (measured, a
/// `sleep 8` there made one checkpoint take 8.5 s), and it is the same author's code for the same
/// reasons. `core.hooksPath` pointed at nothing turns them all off for this one command.
pub const AUTOMATIC: &[&str] = &[
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
];

/// How long any one synchronous git may take before Keel stops it.
///
/// Chosen against the app's own ceiling rather than against git: `Client.ordinary` is 90 seconds,
/// so a git still running at 60 will not produce a result anybody sees — the window has already
/// given up and shown a generic timeout with nothing in it. Failing here instead means the person
/// gets "git … took longer than 60s and was stopped", which names the command.
///
/// It is not a performance budget. Every ordinary git in a large repository is well under a
/// second; this is the difference between slow and never.
const CEILING: Duration = Duration::from_secs(60);

/// Run it, and stop it if it will not stop itself.
///
/// The pipes are drained on their own threads, which is not optional: polling `try_wait` while a
/// chatty command fills a 64 KB pipe buffer deadlocks — git blocks writing, Keel blocks waiting,
/// and the timeout is the only thing that ends it. `git log` on a real repository is well past
/// 64 KB.
pub fn output(command: Command) -> std::io::Result<Output> {
    output_within(command, CEILING)
}

/// The same, with the ceiling named — so a test can assert the timeout without sitting through it.
pub fn output_within(mut command: Command, ceiling: Duration) -> std::io::Result<Output> {
    use std::os::unix::process::CommandExt;
    // Its own group, so a timeout ends everything it started (`crate::signals`): a `claude` or a
    // credential helper that forks and hangs left its children reparented to init.
    command.process_group(0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // The readers append to shared buffers as bytes arrive and say when their pipe closed,
    // rather than being joined: a command can exit and leave a child of its own holding the pipes
    // open (`sleep 300 &`, an auto-updater), and a join then waits as long as that child lives —
    // past any ceiling. One that left the group (`setsid`) cannot even be ended, so what already
    // arrived is what is returned.
    type Shared = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;
    let (out_buf, err_buf): (Shared, Shared) = Default::default();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    for (buf, pipe) in [
        (
            out_buf.clone(),
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ),
        (
            err_buf.clone(),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Some(mut p) = pipe {
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    match p.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            use crate::lock::Locked;
                            buf.locked().extend_from_slice(&chunk[..n]);
                        }
                    }
                }
            }
            let _ = tx.send(());
        });
    }
    drop(tx);

    let deadline = Instant::now() + ceiling;
    let pid = child.id();
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                // The readers end when the pipes close, which ending the whole group does.
                crate::signals::end_tree(pid);
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("took longer than {}s and was stopped", ceiling.as_secs()),
                ));
            }
            // Long enough not to spin, short enough that a fast command is not held up by it.
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    // Both pipes, within what is left of the ceiling. Past it, whatever the command left behind
    // in its group is holding them: end it, and take what arrived.
    // A moment, not the rest of the ceiling: output that is coming arrives within a pipe buffer
    // of the exit, and what is still open after that is a child the command left behind.
    const GRACE: Duration = Duration::from_millis(500);
    let deadline = deadline.min(Instant::now() + GRACE);
    let mut ended = false;
    for _ in 0..2 {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(50));
        if rx.recv_timeout(wait).is_ok() {
            continue;
        }
        if ended {
            break;
        }
        ended = true;
        crate::signals::end_tree(pid);
        if rx.recv_timeout(Duration::from_secs(1)).is_err() {
            break;
        }
    }
    let take = |b: &Shared| {
        use crate::lock::Locked;
        std::mem::take(&mut *b.locked())
    };
    let (stdout, stderr) = (take(&out_buf), take(&err_buf));
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

// ── The four ways the daemon asks git something ───────────────────────────────────────────────
//
// These were four near-identical bodies in four files: `repo::git`, `repo::git_run`,
// `worktree::git`, `snapshot::git`. Each had drifted a little — one trimmed stdout and the next
// did not, one said "git refused, without saying why" and the next said "git … failed" — and the
// drift was invisible because you had to open three files to see it. They differ for real
// reasons, so they stay four functions; they are four functions *here*, where the difference
// between them is a thing you can read in one screen.

/// Ask git something and take the answer, or nothing. Stderr is dropped.
///
/// For reads whose failure is an answer: "is this a repository", "what branch is this". A caller
/// that needs to say *why* it failed wants [`run`].
pub fn read(root: &Utf8Path, args: &[&str]) -> Option<String> {
    let mut c = command(root);
    c.args(args);
    let out = output(c).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Do something, and keep what git said when it would not.
///
/// "Could not discard" with no reason is the kind of message people screenshot and send to you.
/// Stdout is returned as git wrote it, because some callers parse it by line.
pub fn run(root: &Utf8Path, args: &[&str]) -> Result<String, String> {
    let mut c = command(root);
    c.args(args);
    finish(output(c), args, false)
}

/// The same, with stdout trimmed — for the callers whose answer is a single token: a sha, a
/// branch name, a path.
pub fn trimmed(root: &Utf8Path, args: &[&str]) -> Result<String, String> {
    let mut c = command(root);
    c.args(args);
    finish(output(c), args, true)
}

/// [`trimmed`], against an index file of its own.
///
/// `snapshot` builds a tree without touching the repository's real index, which is what lets a
/// turn be photographed while somebody is mid-`git add` in another window.
pub fn with_index(root: &Utf8Path, index: &Utf8Path, args: &[&str]) -> Result<String, String> {
    let mut c = command(root);
    c.args(args).env("GIT_INDEX_FILE", index);
    finish(output(c), args, true)
}

/// One place that decides what a failed git *says*.
fn finish(out: std::io::Result<Output>, args: &[&str], trim: bool) -> Result<String, String> {
    let out = out.map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stdout = if trim { stdout.trim() } else { &stdout[..] }.to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(match (why.is_empty(), stdout.is_empty()) {
        // Git usually explains itself on stderr. When it does not, whatever it put on stdout is
        // more use than a sentence Keel made up.
        (false, _) => why,
        (true, false) => stdout,
        (true, true) => format!("git {} failed, without saying why", args.join(" ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keel reads git every two seconds; a read that refreshed the index took `index.lock` from
    /// the person's own `git add` and fired `post-index-change`.
    #[test]
    fn a_read_writes_nothing_and_runs_no_hook() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        run(root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("a"), "x").unwrap();
        run(root, &["add", "a"]).unwrap();
        let hook = root.join(".git/hooks/post-index-change");
        std::fs::write(&hook, "#!/bin/sh\ntouch fired\n").unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(root.join("a"), "x").unwrap(); // same content, new mtime: a stale stat
        run(root, &["status", "--porcelain"]).unwrap();
        assert!(
            !root.join("fired").exists(),
            "a read ran the repository's hook"
        );
    }

    /// A child that left the group (an auto-updater calls `setsid`) cannot be ended; what the
    /// command already printed is returned rather than thrown away.
    #[test]
    fn output_survives_a_child_that_escaped_the_group() {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg("echo hi; perl -e 'setpgrp(0,0); sleep 30' & exit 0");
        let started = Instant::now();
        let out = output_within(c, Duration::from_secs(20)).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }

    /// A command that exits and leaves a child holding its pipes answers at once, not at the
    /// ceiling.
    #[test]
    fn a_leftover_child_does_not_hold_a_finished_command() {
        let mut c = Command::new("sh");
        c.arg("-c").arg("echo hi; sleep 30 & exit 0");
        let started = Instant::now();
        let out = output_within(c, Duration::from_secs(20)).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }

    /// A timeout ends the whole tree: a command that forked and hung left its child reparented
    /// to init with its pipes held open.
    #[test]
    fn a_timeout_ends_what_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(format!("sleep 300 & echo $! > {}; wait", pidfile.display()));
        let err = output_within(c, Duration::from_millis(500)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        // Safety: signal 0 only asks whether the pid exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the grandchild outlived the timeout");
    }

    /// A command that will not end is ended, rather than held forever.
    #[test]
    fn a_hung_git_is_stopped() {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 600"]);
        let started = Instant::now();
        let e = output_within(c, Duration::from_millis(300)).expect_err("it should have been cut");
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
        assert!(e.to_string().contains("was stopped"), "{e}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it did not wait it out"
        );
    }

    /// And a command that ends on its own is not cut short by the machinery.
    #[test]
    fn an_ordinary_command_is_untouched() {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "printf done; printf oops >&2; exit 3"]);
        let out = output_within(c, Duration::from_secs(5)).unwrap();
        assert_eq!(out.stdout, b"done");
        assert_eq!(out.stderr, b"oops");
        assert_eq!(out.status.code(), Some(3), "the exit code survives");
    }

    /// The reason the pipes are drained on threads: more output than a pipe buffer holds.
    #[test]
    fn output_larger_than_a_pipe_buffer_does_not_deadlock() {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "yes hello | head -c 400000"]);
        let out = output(c).expect("it completed");
        assert_eq!(out.stdout.len(), 400_000, "every byte came back");
        assert!(out.status.success());
    }
}
