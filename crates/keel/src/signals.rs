//! Signalling a process Keel started — and everything it started.
//!
//! Every child the daemon spawns is given a process group of its own (`process_group(0)`), for
//! one reason: `claude` is not a leaf. It runs the tools it was asked to run, so a turn's real
//! process tree is `claude` with a `cargo test`, a `pnpm build` or a dev server underneath it.
//! Signalling the pid signals the one process at the top and leaves the tree standing —
//! reparented to init, holding the terminal, invisible.
//!
//! So there is one function, it takes a group, and the negative-pid incantation appears once.
//! It was written out by hand in three places and the fourth wrote it wrong: the SSE
//! disconnect path — the one that runs when the window closes, the lane closes or the app quits —
//! called `Child::start_kill()`, which is `kill(pid)` on the leader alone. `AppState::interrupt`
//! carried a comment explaining precisely why that is wrong, four files away.

/// Ask a tree to stop: what lets `claude` flush its transcript and a dev server give its port back.
#[cfg(unix)]
pub const INTERRUPT: i32 = libc::SIGINT;
/// Insist.
#[cfg(unix)]
pub const KILL: i32 = libc::SIGKILL;
// Windows has no signals to send another program's group: both mean "end the tree" there.
#[cfg(windows)]
pub const INTERRUPT: i32 = 2;
#[cfg(windows)]
pub const KILL: i32 = 9;

/// Make a command the leader of a group of its own, so [`group`] can reach everything it starts.
/// Every spawn site says this instead of `process_group(0)`, which does not exist on Windows.
pub trait Leads {
    fn lead_group(&mut self) -> &mut Self;
}

impl Leads for std::process::Command {
    fn lead_group(&mut self) -> &mut Self {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(self, 0);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(self, WINDOWS_GROUP);
        self
    }
}

impl Leads for tokio::process::Command {
    fn lead_group(&mut self) -> &mut Self {
        #[cfg(unix)]
        self.process_group(0);
        #[cfg(windows)]
        self.creation_flags(WINDOWS_GROUP);
        self
    }
}

/// `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`: a group of its own, and no console window
/// flashing up behind the app for a process nobody asked to see.
#[cfg(windows)]
const WINDOWS_GROUP: u32 = 0x0000_0200 | 0x0800_0000;

/// Signal the whole group `pid` leads.
///
/// `pid` must be a group leader Keel spawned. Negating it is what makes the kernel deliver to
/// every process in the group rather than to one.
#[cfg(unix)]
pub fn group(pid: u32, signal: i32) {
    if pid == 0 {
        return;
    }
    // Safety: a pid this process spawned as a group leader, negated to address the group. The
    // worst outcome of a stale pid is `ESRCH`, which is ignored — the kernel does not reuse a pid
    // while it is still our unreaped child.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

/// End the tree `pid` leads, on Windows.
///
/// `taskkill /T /F` — the whole tree, at once. Windows cannot deliver an interrupt to another
/// program's group from a process with no console, so there is no "ask" here, and `claude` may not
/// flush the turn's last records. ponytail: ending `claude` gracefully on Windows means its own
/// `--input-format stream-json` interrupt message, and Job Objects would make the tree exact.
/// Started and not waited for: callers include request handlers, which must not block.
#[cfg(windows)]
pub fn group(pid: u32, _signal: i32) {
    use std::os::windows::process::CommandExt;
    if pid == 0 {
        return;
    }
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Every process descended from `pid`, by parent link — including ones that moved to a process
/// group of their own. Claude Code starts its Bash tool's shells detached, each leading its own
/// group, so ending `claude`'s group alone could leave a tool's `sleep` or `cargo test` running.
/// Read once from `ps`; the walk is in memory.
#[cfg(unix)]
pub fn descendants(pid: u32) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    else {
        return Vec::new();
    };
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
    let mut found = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for &(child, of) in &pairs {
            if of == parent && !found.contains(&child) {
                found.push(child);
                frontier.push(child);
            }
        }
    }
    found
}

/// End `pid`'s tree and everything descended from it that left the group: the group asked, then
/// each straggler — and the group of each, which is where a detached tool shell's own children are.
#[cfg(unix)]
pub fn end_all(pid: u32, stragglers: &[u32]) {
    end_tree(pid);
    for &p in stragglers {
        // Safety: kill(2) on a pid, then on the group it may lead. A pid that is already gone is
        // ESRCH and ignored.
        unsafe {
            libc::kill(-(p as i32), KILL);
            libc::kill(p as i32, KILL);
        }
    }
}

/// End a process tree: ask, then insist.
///
/// SIGINT first, and to the group, because that is what ends things cleanly — `claude` flushes its
/// transcript and the session stays resumable, a dev server closes its listening socket and gives
/// the port back, and anything either of them started gets the same chance. SIGKILL after, because
/// every caller of this is a path where nobody is listening any more, and something that decides
/// to ignore SIGINT must not become the thing this file exists to prevent.
pub fn end_tree(pid: u32) {
    group(pid, INTERRUPT);
    #[cfg(unix)]
    group(pid, KILL);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// A grandchild must not survive the child.
    ///
    /// This is the disconnect path in miniature: a leader with something running underneath it,
    /// ended the way Keel ends a turn nobody is listening to any more. Signalling the leader
    /// alone — which is what `Child::start_kill()` does — leaves the grandchild running.
    #[test]
    fn ending_a_tree_takes_all_of_it() {
        use std::os::unix::process::CommandExt;

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-here");
        // The leader starts a grandchild that will outlive it by minutes if nobody stops it, and
        // the grandchild writes a file at the end so its survival is observable.
        let script = format!(
            "( sleep 30; touch {} ) & echo $!; wait",
            marker.to_string_lossy()
        );
        let mut leader = Command::new("/bin/sh");
        leader.args(["-c", &script]).stdout(Stdio::piped());
        leader.process_group(0);
        let mut leader = leader.spawn().expect("could not spawn the leader");

        // Read the grandchild's pid off stdout so the test can ask about it directly.
        let mut line = String::new();
        {
            use std::io::{BufRead, BufReader};
            BufReader::new(leader.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
        }
        let grandchild: i32 = line.trim().parse().expect("no pid on stdout");
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
        assert!(alive(grandchild), "the grandchild did not start");

        end_tree(leader.id());
        let _ = leader.wait();

        // The signal is delivered to the group synchronously; a moment for the shell to fall over.
        for _ in 0..50 {
            if !alive(grandchild) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Do not leave it behind if the assertion is about to fail.
        unsafe { libc::kill(grandchild, libc::SIGKILL) };
        panic!("the grandchild outlived the turn — the group was not signalled");
    }

    /// The leader gone first — an agent that `/exit`s — and what it left in its group still
    /// ends: the group outlives its leader, and the terminal ends it after every exit.
    #[test]
    fn a_group_is_ended_after_its_leader_has_exited() {
        use std::os::unix::process::CommandExt;

        let mut leader = Command::new("/bin/sh");
        leader
            .args(["-c", "sleep 30 & echo $!"])
            .stdout(Stdio::piped());
        leader.process_group(0);
        let mut leader = leader.spawn().expect("could not spawn the leader");
        let mut line = String::new();
        {
            use std::io::{BufRead, BufReader};
            BufReader::new(leader.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
        }
        let left: i32 = line.trim().parse().expect("no pid on stdout");
        let _ = leader.wait(); // the leader has exited; `sleep` is still in its group
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
        assert!(alive(left), "what the leader left did not start");

        end_tree(leader.id());
        for _ in 0..50 {
            if !alive(left) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { libc::kill(left, libc::SIGKILL) };
        panic!("what the agent left behind outlived it");
    }

    /// A descendant that left the group — Claude Code's tool shells start detached — still ends.
    #[test]
    fn a_descendant_in_its_own_group_is_ended_too() {
        use std::os::unix::process::CommandExt;

        let mut leader = Command::new("/bin/sh");
        leader
            .args(["-c", "/usr/bin/python3 -c 'import os,time; os.setpgid(0,0); time.sleep(300)' & echo $!; sleep 300"])
            .stdout(Stdio::piped());
        leader.process_group(0);
        let mut leader = leader.spawn().expect("could not spawn the leader");
        let mut line = String::new();
        {
            use std::io::{BufRead, BufReader};
            BufReader::new(leader.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
        }
        let detached: i32 = line.trim().parse().expect("no pid on stdout");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let found = descendants(leader.id());
        assert!(
            found.contains(&(detached as u32)),
            "{found:?} does not hold {detached}"
        );

        end_all(leader.id(), &found);
        let _ = leader.wait();
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
        for _ in 0..50 {
            if !alive(detached) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { libc::kill(detached, libc::SIGKILL) };
        panic!("a detached descendant outlived its tree");
    }
}
