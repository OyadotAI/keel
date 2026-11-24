//! Strict discovery recovers an existing owner without inventing a new lane or launching work.
use super::{model::*, repository::Repository};
use camino::Utf8Path;
use keel_workspace::Real;

const PAGE_LIMIT: usize = 100;

/// Durable delivery records use the same provider/session identity as ordinary task registration.
pub(crate) fn key_for(source: &Source) -> String {
    super::tracking::key(source)
}

/// Discovery is independent of the window's saved lanes and never starts a provider.
pub(crate) fn list(
    repo: &Utf8Path,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Task>, String> {
    Ok(Repository::new(repo)?
        .read_strict()?
        .tasks
        .into_values()
        .filter(|t| after.is_none_or(|after| t.id.as_str() > after))
        .take(limit.min(PAGE_LIMIT))
        .collect())
}

/// Strict lookup reports missing/corrupt ownership instead of silently replacing a managed task.
pub(crate) fn get(repo: &Utf8Path, id: &str) -> Result<Task, String> {
    lookup(repo, id)?.ok_or_else(|| {
        "Task ownership is unavailable. Restore its task notes before continuing.".into()
    })
}

/// Absence is distinct from unreadable ownership when the launch service bootstraps a new task.
pub(crate) fn lookup(repo: &Utf8Path, id: &str) -> Result<Option<Task>, String> {
    Ok(Repository::new(repo)?.read_strict()?.tasks.remove(id))
}

/// Runtime operations always consult the original task service rather than a UI owner cache.
pub(crate) fn owner(repo: &Utf8Path, id: &str) -> Result<Source, String> {
    get(repo, id).map(|task| task.owner)
}

/// Lane aliases follow existing task membership, so a provider handoff keeps the original task ID.
pub(crate) fn for_lane(repo: &Utf8Path, lane: &str) -> Result<Option<String>, String> {
    Ok(Repository::new(repo)?
        .read_strict()?
        .tasks
        .values()
        .find(|task| task.members.iter().any(|source| source.lane == lane))
        .map(|task| task.id.clone()))
}

/// Machine facts belong only to the registered session/checkout pair, including prior handoff members.
pub(crate) fn for_session(
    repo: &Utf8Path,
    session: &str,
    checkout: &Utf8Path,
) -> Result<Option<Task>, String> {
    let canonical = checkout.real().map_err(|e| e.to_string())?;
    let store = Repository::new(repo)?.read_strict()?;
    let mut found = store.tasks.values().filter(|task| {
        task.members
            .iter()
            .any(|source| source.session == session && source.checkout == canonical.as_str())
    });
    let first = found.next().cloned();
    if found.next().is_some() {
        return Err("This session has ambiguous task ownership.".into());
    }
    Ok(first)
}

/// Referenced tasks must survive the advisory store's ordinary retention policy.
pub(crate) fn pin(repo: &Utf8Path, id: &str) -> Result<(), String> {
    let repository = Repository::new(repo)?;
    repository
        .read_strict()?
        .tasks
        .get(id)
        .ok_or("Task ownership is unavailable.")?;
    repository.change(|store| {
        store.version = PINNED_VERSION;
        store
            .tasks
            .get_mut(id)
            .ok_or("Task ownership is unavailable.")?
            .pinned = true;
        Ok(())
    })
}

/// Reopening validates the recorded checkout and common repository without starting a replacement.
pub(crate) fn recover(repo: &Utf8Path, id: &str) -> Result<Source, String> {
    let owner = owner(repo, id)?;
    let path = Utf8Path::new(&owner.checkout)
        .real()
        .map_err(|_| "Original checkout unavailable — restore or reopen it to continue.")?;
    if !path.starts_with(repo.real().map_err(|e| e.to_string())?) {
        return Err("The original checkout is outside this project.".into());
    }
    let args = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
    let expected = crate::git::run(repo, &args)?;
    if crate::git::run(&path, &args)?.trim() != expected.trim() {
        return Err("The original checkout belongs to another repository.".into());
    }
    Ok(owner)
}
