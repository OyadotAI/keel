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

/// Signal the whole group `pid` leads.
///
/// `pid` must be a group leader Keel spawned. Negating it is what makes the kernel deliver to
/// every process in the group rather than to one.
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

/// End a process tree: ask, then insist.
///
/// SIGINT first, and to the group, because that is what ends things cleanly — `claude` flushes its
/// transcript and the session stays resumable, a dev server closes its listening socket and gives
/// the port back, and anything either of them started gets the same chance. SIGKILL after, because
/// every caller of this is a path where nobody is listening any more, and something that decides
/// to ignore SIGINT must not become the thing this file exists to prevent.
pub fn end_tree(pid: u32) {
    group(pid, libc::SIGINT);
    group(pid, libc::SIGKILL);
}

#[cfg(test)]
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
}
