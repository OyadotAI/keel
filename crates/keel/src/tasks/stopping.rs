//! Explicit source shutdown records proof only after an owned process tree is confirmed ended.
use super::{
    model::*,
    repository::{Repository, event},
    tracking,
};
use crate::serve::AppState;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
const STOP_WAIT: Duration = Duration::from_secs(3);
const STOP_POLL: Duration = Duration::from_millis(25);

/// An explicit stop reaches only a process owned by this registered source generation.
pub(super) async fn stop(state: Arc<AppState>, source: Source) -> Result<(), String> {
    let repository = Repository::new(&state.repo())?;
    let current = repository.read()?;
    if tracking::find(&current, &source.lane).is_none_or(|t| t.owner != source) {
        return Err("Task ownership changed. Recheck the current source before stopping.".into());
    }
    let pid = crate::term::owned_pid(&source.lane)
        .or_else(|| crate::runtime::owned_pid(&state, &source.lane))
        .or_else(|| state.owned_pid(&source.lane))
        .ok_or("Keel cannot confirm ownership of this source process. Stop the session where it is running, then retry the ownership check.")?;
    crate::serve::in_blocking(move || {
        stop_tree(pid)?;
        repository.change(|store| stopped(store, &source))
    })
    .await
}
fn stopped(store: &mut Store, source: &Source) -> Result<(), String> {
    let found = tracking::find(store, &source.lane)
        .ok_or("Source task unavailable")?
        .id
        .clone();
    let task = store.tasks.get_mut(&found).unwrap();
    if task.owner != *source {
        return Err(
            "Task ownership changed while stopping; the latest handoff was preserved.".into(),
        );
    }
    task.stopped_at = Some(crate::turns::now());
    task.incomplete = true;
    task.revision = task.revision.saturating_add(1);
    let snapshot = task.clone();
    event(store, &snapshot, "Stopped", "Observed: Keel confirmed that the owned source process tree ended. Final progress may be missing.".into());
    Ok(())
}

fn stop_tree(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    let descendants = crate::signals::descendants(pid);
    #[cfg(windows)]
    let descendants: Vec<u32> = Vec::new();
    #[cfg(unix)]
    crate::signals::end_all(pid, &descendants);
    #[cfg(windows)]
    {
        let result = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(
                "Windows could not confirm that source work and its children stopped.".into(),
            );
        }
    }
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        if !keel_workspace::pid_alive(pid as i32)
            && descendants
                .iter()
                .all(|p| !keel_workspace::pid_alive(*p as i32))
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Couldn’t confirm that source work stopped. Retry stop; the saved handoff is preserved.".into());
        }
        std::thread::sleep(STOP_POLL);
    }
}
