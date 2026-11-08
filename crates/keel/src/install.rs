//! Installing a missing agent CLI: the one place that answers, per agent and per OS, how.
//!
//! Keel's people mostly arrive from hosted builders and do not know what a CLI is, so "Claude Code
//! is not installed — go and install it" is the end of their first run. Every lane that needs an
//! agent asks here first, in chat and in the terminal alike, and the vendor's own installer runs
//! with no click: the command is fixed text in Keel, nothing in a repository can change it, so a
//! project's trust adds no safety to it (the person decided this on 2026-11-08).

use crate::lock::Locked;
use crate::signals::Leads;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often an install, or a wait for one, looks at whether its caller stopped it.
const POLL: Duration = Duration::from_millis(200);

/// How long a vendor installer may run. Each one downloads a single binary; ten minutes is a slow
/// connection, and past it the install is not going to finish while anyone is watching.
const CEILING: Duration = Duration::from_secs(10 * 60);

/// A vendor's own installer for one agent CLI on this OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installer {
    /// The name a person knows it by.
    pub who: &'static str,
    /// The command, shown before it runs: piping a vendor script into a shell is reasonable, doing
    /// it invisibly is not.
    pub command: &'static str,
}

/// Every agent Keel drives, and its installer on macOS/Linux and on Windows. macOS and Windows are
/// at parity: an agent missing from either column fails `every_agent_installs_on_every_os`.
/// Each is the vendor's native installer, writing under the home directory, so there is no
/// password prompt to get stuck behind; where each lands on Windows is read from its own script
/// and kept on `PATH` by `path::adopt_shell_path`. curl is pinned to HTTPS, so a redirect can
/// never hand a script over plain http.
const TABLE: &[(&str, &str, &str, &str)] = &[
    // (program, who, unix, windows)
    // Git first: every lane is a git checkout, and Claude Code on Windows runs its commands in the
    // Git Bash that ships with it. On macOS `/usr/bin/git` is always there — a shim that puts up
    // Apple's own "install the developer tools" dialog when used — so this row is the Windows one
    // in practice: winget, whose installer shows the system's admin prompt (never a silent sudo).
    (
        "git",
        "Git",
        "xcode-select --install",
        "winget install --id Git.Git -e --source winget --silent --accept-package-agreements --accept-source-agreements",
    ),
    (
        "claude",
        "Claude Code",
        "curl --proto =https --tlsv1.2 -fsSL https://claude.ai/install.sh | bash",
        // %USERPROFILE%\.local\bin
        "irm https://claude.ai/install.ps1 | iex",
    ),
    (
        "codex",
        "Codex",
        "curl --proto =https --tlsv1.2 -fsSL https://chatgpt.com/codex/install.sh | sh",
        // %LOCALAPPDATA%\Programs\OpenAI\Codex\bin
        "irm https://chatgpt.com/codex/install.ps1 | iex",
    ),
    (
        "agy",
        "Antigravity",
        "curl --proto =https --tlsv1.2 -fsSL https://antigravity.google/cli/install.sh | bash",
        // %LOCALAPPDATA%\agy\bin
        "irm https://antigravity.google/cli/install.ps1 | iex",
    ),
];

/// The installer for `program` on this OS, whether or not it is installed.
pub fn for_program(program: &str) -> Option<Installer> {
    let (_, who, unix, windows) = TABLE.iter().find(|(p, ..)| *p == program)?;
    // `xcode-select` is macOS's: on Linux git comes from the distribution, which Keel leaves alone.
    if program == "git" && cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let command = if cfg!(windows) { windows } else { unix };
    Some(Installer { who, command })
}

/// What a lane running `program` needs on the machine, in the order to install it.
pub fn needed(program: &str) -> [&str; 2] {
    ["git", program]
}

/// The installer to run when `program` is not on `PATH`; `None` when it is there or Keel knows none.
pub fn missing(program: &str) -> Option<Installer> {
    crate::permissions::find_on_path(program)
        .is_none()
        .then(|| for_program(program))
        .flatten()
}

/// One install at a time. Two lanes opening on a fresh machine would otherwise run one installer
/// twice into the same `~/.local/bin`; the second waits here and then finds the program present.
/// ponytail: one global lock, not per agent — installs are rare and short next to a turn.
static ONE: Mutex<()> = Mutex::new(());

/// Install `program`, each output line handed to `line` as it comes. Blocking: call it from a PTY
/// thread or through `serve::blocking`. `Err` says which installer failed and why, in words.
///
/// `stopped` is asked every poll: Stop, a closed window or a closed tab ends the installer's whole
/// tree within a fifth of a second, rather than leaving a lane busy, or a thread held, for the
/// rest of a ten-minute ceiling.
pub fn run(
    program: &str,
    mut line: impl FnMut(&str),
    stopped: impl Fn() -> bool,
) -> Result<(), String> {
    let Some(installer) = for_program(program) else {
        return Err(format!(
            "Keel has no installer for `{program}` on this system"
        ));
    };
    let _one = loop {
        // Waiting behind another lane's install answers to Stop as the install itself does.
        match ONE.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(_)) => break ONE.locked(),
            Err(std::sync::TryLockError::WouldBlock) if stopped() => return Err(STOPPED.into()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(POLL),
        }
    };
    if crate::permissions::find_on_path(program).is_some() {
        return Ok(());
    }
    line(&format!("$ {}", installer.command));
    execute(installer, &mut line, &stopped)?;
    if crate::permissions::find_on_path(program).is_none() {
        return Err(format!(
            "the {} installer finished, but `{program}` still isn't where Keel looks for it",
            installer.who
        ));
    }
    Ok(())
}

/// Run one installer to its end, its ceiling, or `stopped`.
fn execute(
    installer: Installer,
    line: &mut impl FnMut(&str),
    stopped: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut child = shell(installer.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Codex's script asks "[y/N]" — from /dev/tty when the daemon has one, which would wait
        // forever on a terminal nobody is looking at — unless told nobody is there to answer.
        .env("CODEX_NON_INTERACTIVE", "1")
        .lead_group()
        .spawn()
        .map_err(|e| format!("couldn't start the {} installer: {e}", installer.who))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("piped");
    // Drained on a thread so a chatty installer never fills the pipe and stalls mid-write.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            let _ = tx.send(String::from_utf8_lossy(&buf).trim_end().to_string());
            buf.clear();
        }
    });
    let deadline = Instant::now() + CEILING;
    loop {
        if stopped() {
            crate::signals::end_tree(pid);
            let _ = child.wait();
            return Err(STOPPED.into());
        }
        match rx.recv_timeout(POLL) {
            Ok(text) => line(&text),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) if Instant::now() < deadline => {}
            Err(_) => {
                crate::signals::end_tree(pid);
                let _ = child.wait();
                return Err(format!(
                    "the {} installer was still running after {} minutes, so Keel stopped it",
                    installer.who,
                    CEILING.as_secs() / 60
                ));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("the {} installer: {e}", installer.who))?;
    if !status.success() {
        return Err(failed(installer.who, status.code()));
    }
    Ok(())
}

/// What `run` returns when its caller went away: not a failure anyone needs telling about.
pub const STOPPED: &str = "the install was stopped";

/// What a failed installer says. curl's 6 and 7 (no such host, no connection) are by far the most
/// common reason and mean something a person can fix, so they get words of their own.
fn failed(who: &str, code: Option<i32>) -> String {
    match code {
        Some(6 | 7) => format!(
            "the {who} installer couldn't reach the internet — check the connection and try again"
        ),
        Some(code) => format!("the {who} installer exited with code {code}"),
        None => format!("the {who} installer was stopped before it finished"),
    }
}

/// The shell a vendor documents its installer for, with stderr folded into stdout so the person
/// sees the reason a failure gives.
fn shell(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(format!("& {{ {command} }} *>&1"));
        c
    }
    #[cfg(not(windows))]
    {
        // bash, which the vendors' scripts need anyway, for pipefail: a curl that could not connect
        // fails the pipeline instead of `bash` reading an empty script and reporting success.
        let mut c = Command::new("bash");
        c.args(["-o", "pipefail", "-c"])
            .arg(format!("{{ {command}; }} 2>&1"));
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every agent Keel drives installs on macOS and on Windows: the table cannot name a gap.
    #[test]
    fn every_agent_installs_on_every_os() {
        let programs: Vec<_> = TABLE.iter().map(|(p, ..)| *p).collect();
        assert_eq!(programs, ["git", "claude", "codex", "agy"]);
        for (program, _, unix, windows) in &TABLE[1..] {
            assert!(unix.starts_with("curl --proto =https"), "{program}: {unix}");
            assert!(windows.starts_with("irm https://"), "{program}: {windows}");
        }
        assert_eq!(
            for_program("sh"),
            None,
            "no installer guessed for a program Keel doesn't drive"
        );
        assert!(TABLE[0].3.starts_with("winget install --id Git.Git"));
        assert_eq!(needed("codex"), ["git", "codex"], "git before the agent");
    }

    #[cfg(unix)]
    #[test]
    fn the_installer_for_this_os_is_the_vendors_own() {
        assert!(
            for_program("claude")
                .unwrap()
                .command
                .contains("https://claude.ai/install.sh")
        );
        assert!(
            for_program("agy")
                .unwrap()
                .command
                .contains("antigravity.google/cli/install.sh")
        );
        assert_eq!(
            missing("sh"),
            None,
            "something on PATH is not installed again"
        );
    }

    #[test]
    fn a_failed_install_names_the_installer_and_why() {
        assert!(failed("Codex", Some(7)).contains("couldn't reach the internet"));
        assert_eq!(
            failed("Codex", Some(1)),
            "the Codex installer exited with code 1"
        );
        assert!(failed("Codex", None).contains("stopped"));
    }

    /// Stop ends an install at once, tree and all, and says it was stopped rather than failed.
    #[cfg(unix)]
    #[test]
    fn a_stopped_install_ends_now() {
        let fake = Installer {
            who: "Fake",
            command: "echo starting; sleep 30",
        };
        let started = Instant::now();
        let (seen, asked) = (std::cell::RefCell::new(Vec::new()), std::cell::Cell::new(0));
        let stopped = || {
            asked.set(asked.get() + 1);
            asked.get() > 3
        };
        let result = execute(
            fake,
            &mut |l: &str| seen.borrow_mut().push(l.to_string()),
            &stopped,
        );
        assert_eq!(result, Err(STOPPED.into()));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(seen.borrow().as_slice(), ["starting"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_installer_says_so() {
        let fake = Installer {
            who: "Fake",
            command: "echo nope; exit 3",
        };
        let result = execute(fake, &mut |_: &str| {}, &|| false);
        assert_eq!(result, Err("the Fake installer exited with code 3".into()));
    }

    /// The shell folds stderr in and a failing pipeline fails: what `run` relies on to say why.
    #[cfg(unix)]
    #[test]
    fn the_shell_reports_a_failed_pipeline_and_its_stderr() {
        let out = shell("echo said >&2; false | cat").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "said");
        assert!(!out.status.success());
    }
}
