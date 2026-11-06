//! One review deadline covers preparation, inference and validation, with time reserved for cleanup.
use camino::Utf8Path;
use std::{
    io,
    process::{Command, Output},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(crate) const MAX_MINUTES: u64 = 5;
pub(crate) const MAX_DURATION: Duration = Duration::from_secs(MAX_MINUTES * 60);
const RESERVE_FRACTION: u32 = 4;
const CLEANUP_RESERVE: Duration = Duration::from_secs(3);
static NEVER: AtomicBool = AtomicBool::new(false);

/// Copies share an absolute deadline; none of the review's blocking steps can renew it.
#[derive(Clone, Copy)]
pub(crate) struct Budget<'a> {
    pub started: Instant,
    pub deadline: Instant,
    pub work_until: Instant,
    pub stop: &'a AtomicBool,
    pub limit: Duration,
}

impl<'a> Budget<'a> {
    /// Reserve cleanup inside the caller's maximum rather than starting a fresh timer later.
    pub(crate) fn new(requested: Duration, stop: &'a AtomicBool) -> Self {
        let started = Instant::now();
        let limit = requested.min(MAX_DURATION);
        let deadline = started + limit;
        let reserve = CLEANUP_RESERVE.min(limit / RESERVE_FRACTION);
        Self {
            started,
            deadline,
            work_until: deadline - reserve,
            stop,
            limit,
        }
    }

    /// Work stops early enough to validate and remove its owned checkout before the deadline.
    pub(crate) fn check(&self) -> io::Result<()> {
        self.check_at(self.work_until)
    }

    /// A late result cannot pass even when inference itself completed before the limit.
    pub(crate) fn finish(&self) -> io::Result<()> {
        self.check_at(self.deadline)
    }

    fn check_at(&self, until: Instant) -> io::Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "The review was cancelled.",
            ));
        }
        if Instant::now() >= until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The focused review reached its execution limit. No verdict was produced.",
            ));
        }
        Ok(())
    }

    /// Every preparation command consumes the same remaining time and cancellation signal.
    pub(crate) fn output(&self, command: Command) -> io::Result<Output> {
        self.check()?;
        crate::git::output_before(command, self.work_until, self.stop)
    }

    /// Keep Git reads and checkout preparation inside the review's budget, including helpers.
    pub(crate) fn git(&self, repo: &Utf8Path, args: &[&str]) -> Result<String, String> {
        let mut command = crate::git::command(repo);
        command.args(args);
        let output = self.output(command).map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Cancellation still cleans owned scratch state, but never receives a new sixty-second wait.
pub(super) fn cleanup(repo: &Utf8Path, tree: &Utf8Path, deadline: Instant) -> Result<(), String> {
    let mut command = crate::git::command(repo);
    command.args(crate::git::AUTOMATIC).args([
        "worktree",
        "remove",
        "--force",
        "--force",
        tree.as_str(),
    ]);
    let until = deadline.min(Instant::now() + CLEANUP_RESERVE);
    let output = crate::git::output_before(command, until, &NEVER).map_err(|e| e.to_string())?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| String::from_utf8_lossy(&output.stderr).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_long_requests_cannot_extend_the_five_minute_limit() {
        let budget = Budget::new(MAX_DURATION + MAX_DURATION, &NEVER);
        assert_eq!(budget.limit, MAX_DURATION);
        assert_eq!(budget.deadline.duration_since(budget.started), MAX_DURATION);
    }

    #[test]
    fn copied_budgets_do_not_renew_the_deadline() {
        let budget = Budget::new(MAX_DURATION, &NEVER);
        let next = budget;
        assert_eq!(budget.work_until, next.work_until);
        assert_eq!(budget.deadline, next.deadline);
    }

    #[test]
    fn expired_work_cannot_spawn_a_new_command() {
        let budget = Budget::new(Duration::ZERO, &NEVER);
        let command = Command::new("a-program-that-must-never-start");
        assert_eq!(
            budget.output(command).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn cancellation_cannot_spawn_a_new_command() {
        let stop = AtomicBool::new(true);
        let budget = Budget::new(MAX_DURATION, &stop);
        let command = Command::new("a-program-that-must-never-start");
        assert_eq!(
            budget.output(command).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }
}
