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
pub const AUTOMATIC: &[&str] = &["-c", "commit.gpgsign=false"];

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
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let stdout = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out.as_mut() {
            use std::io::Read;
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let stderr = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err.as_mut() {
            use std::io::Read;
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + ceiling;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                // The readers end when the pipes close, which killing does.
                let _ = child.kill();
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

    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
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
