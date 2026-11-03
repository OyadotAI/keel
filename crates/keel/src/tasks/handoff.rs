//! Handoffs preserve exact revisions and checkout ownership without recreating or resetting files.
use super::{
    model::*,
    repository::{Repository, event},
    text, tracking,
};
use crate::serve::AppState;
use camino::Utf8Path;
use std::sync::Arc;

pub(super) fn evidence(repo: &Utf8Path, source: &Source) -> Vec<String> {
    let status = crate::repo::git_status(Utf8Path::new(&source.checkout));
    let mut evidence = status
        .changes
        .into_iter()
        .take(FILE_LIMIT)
        .map(|file| {
            format!(
                "Observed current Git change: {} · {}",
                file.status, file.path
            )
        })
        .collect::<Vec<_>>();
    if status.collapsed {
        evidence.push("Observed Git file list was truncated.".into());
    }
    for turn in crate::turns::read(repo, &source.session)
        .into_iter()
        .rev()
        .take(PROMPT_LIMIT)
    {
        if !turn.files.is_empty() {
            evidence.push(format!(
                "Observed changed files: {}",
                turn.files
                    .into_iter()
                    .take(FILE_LIMIT)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(gate) = turn.gate {
            evidence.push(format!(
                "Observed check: {} · {}",
                gate.status,
                gate.command.unwrap_or_default()
            ));
        }
        if let Some(commit) = turn.commit {
            evidence.push(format!("Observed commit: {commit}"));
        }
        if let Some(failed) = turn.failed {
            evidence.push(format!(
                "Observed provider exit: {} · {}",
                failed.code, failed.cause
            ));
        }
    }
    evidence
        .into_iter()
        .map(|s| text::clean(&s))
        .take(FILE_LIMIT)
        .collect()
}
pub(super) fn repo_state(checkout: &str) -> (String, String) {
    let checkout = Utf8Path::new(checkout);
    (
        crate::git::trimmed(checkout, &["branch", "--show-current"]).unwrap_or_default(),
        crate::git::trimmed(checkout, &["rev-parse", "HEAD"]).unwrap_or_default(),
    )
}
pub(super) fn capture(repo: &Utf8Path, lane: &str) -> Result<Task, String> {
    Repository::new(repo)?.change(|store| {
        let task=tracking::find(store,lane).ok_or("No task has been registered for this session yet.")?.clone();
        if task.owner.lane!=lane {return Err("Only the current task owner can save newer progress. The source handoff is preserved.".into());}
        if !Utf8Path::new(&task.owner.checkout).is_dir(){return Err("The original checkout is unavailable. Restore or reopen it before continuing.".into());}
        let task=store.tasks.get_mut(&task.id).unwrap();
        task.evidence=evidence(repo,&task.owner);
        task.revision=task.revision.saturating_add(1);task.updated_at=crate::turns::now();
        let (branch,head)=repo_state(&task.owner.checkout);
        let saved=Saved {revision:task.revision,at:task.updated_at.clone(),source:task.owner.clone(),branch:branch.clone(),head:head.clone(),incomplete:task.incomplete,text:text::preview(task,&branch,&head)};
        task.saved.push(saved);
        if task.saved.len()>REVISION_LIMIT {task.saved.remove(0);}
        let snapshot=task.clone();event(store,&snapshot,"Saved","Observed: an immutable handoff revision was saved.".into());Ok(snapshot)
    })
}
pub(super) fn occupied(state: &AppState, task: &Task) -> Result<(), String> {
    if crate::term::live(&task.owner.session)
        || crate::runtime::has_session(state, &task.owner.lane)
    {
        return Err("The source agent still owns this checkout. Stop source work, then retry the ownership check.".into());
    }
    if task.owner.provider == "claude"
        && keel_workspace::claude_home()
            .is_some_and(|home| keel_workspace::status(&home, &task.owner.session).is_some())
    {
        return Err("The source session is open outside Keel. Stop it there, then retry the ownership check.".into());
    }
    // Verified child lifecycle is independent of parent completion. Unknown work also blocks transfer.
    let provider =
        keel_workspace::Follows::from_query(Some(&task.owner.provider)).map_err(str::to_owned)?;
    let checkout = Utf8Path::new(&task.owner.checkout);
    if task.stopped_at.is_none()
        && let Some(parent) = provider.locate(checkout, &task.owner.session)
    {
        let mut observer = keel_workspace::subagents::Observer::new(
            parent,
            checkout.to_owned(),
            task.owner.session.clone(),
        );
        if observer.snapshot().invocations.iter().any(|i| {
            matches!(
                i.status,
                keel_workspace::subagents::Status::Working
                    | keel_workspace::subagents::Status::Waiting
                    | keel_workspace::subagents::Status::Unknown
            )
        }) {
            return Err("Subagent shutdown is not confirmed. Stop source work and its children, then retry the ownership check.".into());
        }
    }
    Ok(())
}
pub(super) fn reserve(
    state: &Arc<AppState>,
    lane: &str,
    revision: u64,
    provider: &str,
    receipt: &str,
) -> Result<Continuation, String> {
    let executable = if provider == "antigravity" {
        "agy"
    } else {
        provider
    };
    if crate::permissions::find_on_path(executable).is_none() {
        return Err(format!(
            "{provider} is not installed. Install it before continuing."
        ));
    }
    transfer(&state.repo(), lane, revision, provider, receipt, |task| {
        let ownership = super::ownership::hold(Utf8Path::new(&task.owner.checkout), true)?;
        occupied(state, task)?;
        let key = format!("keel:continue:{receipt}");
        let token = state.reserve_checkout(&key, Utf8Path::new(&task.owner.checkout))?;
        let held = crate::serve::Held::new(state.clone(), key, token);
        occupied(state, task)?;
        Ok((held, ownership))
    })
}

/// The lease outlives the atomic store replacement; retries return the original destination.
pub(super) fn transfer<G>(
    repo: &Utf8Path,
    lane: &str,
    revision: u64,
    provider: &str,
    receipt: &str,
    acquire: impl FnOnce(&Task) -> Result<G, String>,
) -> Result<Continuation, String> {
    if !tracking::valid(receipt)
        || receipt.len() > 64
        || !["claude", "codex", "antigravity"].contains(&provider)
    {
        return Err("Invalid continuation request".into());
    }
    let (result, held) = Repository::new(repo)?.change(|store| {
        let task = tracking::find(store, lane).ok_or("Task unavailable")?.clone();
        if let Some(previous) = &task.continuation {
            if previous.id == receipt && previous.revision == revision && previous.destination.provider == provider {
                return Ok((previous.clone(), None));
            }
            if task.owner.lane != lane { return Err("This task already continued in another lane. Open its current continuation.".into()); }
        }
        let saved = task.saved.iter().find(|s| s.revision == revision)
            .ok_or("That inspected revision is no longer retained. Inspect and save the latest handoff.")?.clone();
        if saved.source.lane != task.owner.lane {
            return Err("That handoff belongs to an earlier task owner. Save the latest progress before continuing.".into());
        }
        if !Utf8Path::new(&task.owner.checkout).is_dir() {
            return Err("The original checkout is unavailable. Restore or reopen it before continuing.".into());
        }
        let held = acquire(&task)?;
        let destination = Source {
            lane: format!("continuation-{receipt}"), provider: provider.into(), session: format!("pending-{receipt}"),
            checkout: task.owner.checkout.clone(), wt: task.owner.wt.clone(),
        };
        let result = Continuation { id: receipt.into(), revision, source: task.owner,
            destination: destination.clone(), prompt: text::continuation(&saved) };
        let task = store.tasks.get_mut(&task.id).unwrap();
        task.owner = destination.clone(); task.members.push(destination); task.notes = Notes::default();
        task.continuation = Some(result.clone()); task.incomplete = true; task.stopped_at = None;
        task.revision = task.revision.saturating_add(1);
        let snapshot = task.clone();
        event(store, &snapshot, "Continued", format!(
            "Observed: checkout ownership transferred from {} session {} using saved revision {revision}. Destination launch still requires its durable submission receipt.", result.source.provider, result.source.session));
        Ok((result, Some(held)))
    })?;
    drop(held);
    Ok(result)
}
