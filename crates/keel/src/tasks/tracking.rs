//! Launch registration binds reports to a project task and rejects writes from transferred sources.
use super::{
    model::*,
    repository::{Repository, event},
    text,
};
use camino::Utf8Path;
use keel_workspace::Real;
use sha2::{Digest, Sha256};

pub(super) fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
pub(super) fn find<'a>(store: &'a Store, lane: &str) -> Option<&'a Task> {
    store
        .tasks
        .values()
        .find(|t| t.members.iter().any(|s| s.lane == lane))
}
pub(super) fn key(source: &Source) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}:{}", source.provider, source.session).as_bytes())
    )
}
/// Register only after normal checkout validation; the source can never name a foreign task file.
pub(crate) fn register(repo: &Utf8Path, mut source: Source) -> Result<String, String> {
    if !valid(&source.lane)
        || !valid(&source.session)
        || !["claude", "codex", "antigravity"].contains(&source.provider.as_str())
    {
        return Err("Invalid task source".into());
    }
    let checkout = Utf8Path::new(&source.checkout)
        .real()
        .map_err(|e| e.to_string())?;
    let root = repo.real().map_err(|e| e.to_string())?;
    if !checkout.starts_with(&root) {
        return Err("Task checkout must belong to this project.".into());
    }
    source.checkout = checkout.to_string();
    Repository::new(repo)?.change(|store| register_in(store, source))
}

fn register_in(store: &mut Store, source: Source) -> Result<String, String> {
    let existing = store.tasks.values_mut().find(|t| {
        t.members.iter().any(|s| {
            s.provider == source.provider
                && s.session == source.session
                && s.checkout == source.checkout
        })
    });
    if let Some(task) = existing {
        if task.owner.provider != source.provider || task.owner.session != source.session {
            return Err(
                "This source was handed to another coding agent. Continue from its current owner."
                    .into(),
            );
        }
        if task.owner.lane != source.lane && crate::term::live(&source.session) {
            return Err("This provider session is still open in another lane. Stop it there before resuming.".into());
        }
        task.owner.lane = source.lane.clone();
        task.stopped_at = None;
        if !task.members.iter().any(|s| s.lane == source.lane) {
            task.members.push(source);
        }
        return Ok(task.id.clone());
    }
    if let Some(task) = find(store, &source.lane) {
        return Ok(task.id.clone());
    }
    guard_in(store, &source.lane, &source.checkout)?;
    // Every lane registers a task and none is ever removed by hand, so at the limit the one
    // touched longest ago goes — never one carrying a handoff, which another lane depends on.
    if store.tasks.len() >= TASK_LIMIT {
        let oldest = store
            .tasks
            .values()
            .filter(|t| t.continuation.is_none())
            .min_by(|a, b| a.updated_at.cmp(&b.updated_at))
            .map(|t| t.id.clone())
            .ok_or("Shared task storage is full of handed-off tasks. Finish or discard one, then retry.")?;
        store.tasks.remove(&oldest);
    }
    let id = key(&source);
    let task = Task {
        id: id.clone(),
        revision: 1,
        owner: source.clone(),
        members: vec![source],
        objective: String::new(),
        steering: Vec::new(),
        notes: Notes::default(),
        evidence: Vec::new(),
        incomplete: true,
        stopped_at: None,
        saved: Vec::new(),
        continuation: None,
        updated_at: crate::turns::now(),
    };
    event(
        store,
        &task,
        "Started",
        "Observed: Keel registered this provider session.".into(),
    );
    store.tasks.insert(id.clone(), task);
    Ok(id)
}
/// Native providers may choose a different session ID; rebind only the current owning lane.
pub(crate) fn session(repo: &Utf8Path, lane: &str, session: &str) -> Result<(), String> {
    if !valid(session) {
        return Err("Invalid session identity".into());
    }
    crate::browser::session_alias(lane, session)?;
    let repository = Repository::new(repo)?;
    if !repository
        .read()?
        .tasks
        .values()
        .any(|t| t.owner.lane == lane && t.owner.session != session)
    {
        return Ok(());
    }
    repository.change(|store| {
        let task = store
            .tasks
            .values_mut()
            .find(|t| t.owner.lane == lane)
            .ok_or("No task for this lane")?;
        task.owner.session = session.into();
        for member in &mut task.members {
            if member.lane == lane {
                member.session = session.into();
            }
        }
        Ok(())
    })
}
/// Capture bounded user-visible steering, never transcript system prompts or hidden reasoning.
pub(crate) fn input(
    repo: &Utf8Path,
    provider: &str,
    session: &str,
    prompt: &str,
) -> Result<(), String> {
    let repository = Repository::new(repo)?;
    let Some(id) = repository
        .read()?
        .tasks
        .values()
        .find(|t| t.owner.provider == provider && t.owner.session == session)
        .map(|t| t.id.clone())
    else {
        return Ok(());
    };
    let prompt = text::clean(prompt);
    if prompt.trim().is_empty() {
        return Ok(());
    }
    repository.change(|store| {
        let task = store.tasks.get_mut(&id).ok_or("Task unavailable")?;
        if task.owner.provider != provider || task.owner.session != session {
            return Ok(());
        }
        if task.steering.last() == Some(&prompt) || task.objective == prompt {
            return Ok(());
        }
        if task.objective.is_empty() {
            task.objective = prompt.clone();
        } else {
            task.steering.push(prompt.clone());
        }
        if task.steering.len() > PROMPT_LIMIT {
            task.steering.remove(0);
            task.incomplete = true;
        }
        task.revision = task.revision.saturating_add(1);
        task.incomplete = true;
        task.updated_at = crate::turns::now();
        let snapshot = task.clone();
        event(
            store,
            &snapshot,
            "Steering",
            format!("Observed user input:\n{prompt}"),
        );
        Ok(())
    })
}
/// Transferred checkouts accept agent starts only from their current owner; old sources remain readable.
pub(crate) fn guard(repo: &Utf8Path, lane: &str, checkout: &Utf8Path) -> Result<(), String> {
    let repository = Repository::new(repo)?;
    if !repository.root.join(".keel/tasks/state.json").exists() {
        return Ok(());
    }
    let checkout = checkout.real().map_err(|e| e.to_string())?;
    guard_in(&repository.read()?, lane, checkout.as_str())
}
fn guard_in(store: &Store, lane: &str, checkout: &str) -> Result<(), String> {
    for task in store
        .tasks
        .values()
        .filter(|t| t.continuation.is_some() && t.owner.checkout == checkout)
    {
        if lane != task.owner.lane && lane != format!("term:{}", task.owner.session) {
            return Err("This checkout was handed to another coding agent. Open its current continuation lane; the source is available for inspection.".into());
        }
    }
    Ok(())
}

/// Merge/discard routes must identify the current owning lane after an ownership transfer.
pub(crate) fn destructive_guard(
    repo: &Utf8Path,
    lane: Option<&str>,
    checkout: &Utf8Path,
) -> Result<(), String> {
    guard(repo, lane.unwrap_or_default(), checkout)
}

/// Save machine-observed milestones alongside agent claims, without tool-output or reasoning bodies.
pub(crate) fn observed(
    repo: &Utf8Path,
    session: &str,
    fact: &crate::turns::Fact,
) -> Result<(), String> {
    use crate::turns::Fact;
    let (kind, summary) = match fact {
        Fact::Started { .. } => ("Started", "Observed: a task turn started.".to_string()),
        Fact::Files { files } => (
            "Files",
            format!(
                "Observed changed files: {}",
                files
                    .iter()
                    .take(FILE_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Fact::Gate {
            status, command, ..
        } => (
            "Checks",
            format!(
                "Observed check: {status} · {}",
                command.as_deref().unwrap_or_default()
            ),
        ),
        Fact::Commit { sha } => ("Commit", format!("Observed commit: {sha}")),
        Fact::Failed { code, cause, .. } => (
            "Interrupted",
            format!("Observed provider exit: {code} · {cause}"),
        ),
        Fact::Ended { .. } => (
            "Stopped",
            "Observed: the turn ended; provider or children may still be running.".into(),
        ),
        _ => return Ok(()),
    };
    let repository = Repository::new(repo)?;
    if !repository.root.join(".keel/tasks/state.json").exists() {
        return Ok(());
    }
    repository.change(|store| {
        let Some(task) = store
            .tasks
            .values_mut()
            .find(|t| t.owner.session == session)
        else {
            return Ok(());
        };
        let summary = text::clean(&summary);
        if task.evidence.last() == Some(&summary) {
            return Ok(());
        }
        task.evidence.push(summary.clone());
        if task.evidence.len() > FILE_LIMIT {
            task.evidence.remove(0);
        }
        if kind == "Started" || kind == "Interrupted" {
            task.incomplete = true;
        }
        if kind == "Stopped" && task.notes != Notes::default() {
            task.incomplete = false;
        }
        task.revision = task.revision.saturating_add(1);
        task.updated_at = crate::turns::now();
        let snapshot = task.clone();
        event(store, &snapshot, kind, summary);
        Ok(())
    })
}
