//! Thin handoff routes expose only project-scoped identities and immutable saved revisions.
use super::{handoff, model::*, repository::Repository, tracking};
use crate::serve::AppState;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
type Failure = (StatusCode, String);
fn bad(e: String) -> Failure {
    (StatusCode::CONFLICT, e)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub lane: String,
    pub provider: String,
    pub session: String,
}
#[derive(Serialize)]
pub(crate) struct View {
    task: Option<Task>,
    pub saved: Option<Saved>,
    events: Vec<Event>,
    truncated: bool,
    current_branch: String,
    current_head: String,
    checkout_exists: bool,
    ownership_error: Option<String>,
    guidance: crate::guidance::Delivery,
}
fn task_for(repository: &Repository, id: &Identity) -> Result<Option<Task>, String> {
    if !["claude", "codex", "antigravity"].contains(&id.provider.as_str())
        || !tracking::valid(&id.lane)
        || !tracking::valid(&id.session)
    {
        return Err("Invalid task identity".into());
    }
    let store = repository.read()?;
    Ok(store
        .tasks
        .values()
        .find(|t| {
            t.members
                .iter()
                .any(|s| s.provider == id.provider && s.session == id.session)
        })
        .cloned())
}
/// Inspect without creating a task or starting a provider, including while native context is unavailable.
pub(crate) async fn inspect(
    State(state): State<Arc<AppState>>,
    Query(id): Query<Identity>,
) -> Result<Json<View>, Failure> {
    crate::serve::in_blocking(move || {
        let repository = Repository::new(&state.repo())?;
        let task = task_for(&repository, &id)?;
        let store = repository.read()?;
        let checkout = task
            .as_ref()
            .map(|t| t.owner.checkout.as_str())
            .unwrap_or(repository.root.as_str());
        let (current_branch, current_head) = handoff::repo_state(checkout);
        let checkout_exists = camino::Utf8Path::new(checkout).is_dir();
        let ownership_error = task
            .as_ref()
            .and_then(|t| handoff::occupied(&state, t).err());
        let events = store
            .events
            .into_iter()
            .filter(|e| task.as_ref().is_some_and(|t| t.id == e.task))
            .collect();
        let saved = task.as_ref().and_then(|t| t.saved.last().cloned());
        let guidance = crate::guidance::delivery(&repository.root, &id.provider, &id.session);
        Ok(View {
            task,
            saved,
            events,
            truncated: store.truncated,
            current_branch,
            current_head,
            checkout_exists,
            ownership_error,
            guidance,
        })
    })
    .await
    .map(Json)
    .map_err(bad)
}
/// Save current evidence; earlier previews stay immutable and source ownership is enforced.
pub(crate) async fn save(
    State(state): State<Arc<AppState>>,
    Json(id): Json<Identity>,
) -> Result<Json<Task>, Failure> {
    crate::serve::in_blocking(move || {
        let repository = Repository::new(&state.repo())?;
        let task = task_for(&repository, &id)?.ok_or("No task is registered for this session.")?;
        if task.owner.provider != id.provider || task.owner.session != id.session {
            return Err("Save the latest handoff from the current task owner.".into());
        }
        handoff::capture(&state.repo(), &task.owner.lane)
    })
    .await
    .map(Json)
    .map_err(bad)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Continue {
    pub source: Identity,
    pub revision: u64,
    pub provider: String,
    pub receipt: String,
}
/// Reserve once; launch uses this exact receipt and handoff, and lost responses can request it again.
pub(crate) async fn continue_task(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Continue>,
) -> Result<Json<Continuation>, Failure> {
    crate::serve::in_blocking(move || {
        let repository = Repository::new(&state.repo())?;
        let task = task_for(&repository, &request.source)?.ok_or("Task unavailable")?;
        let lane = task
            .members
            .iter()
            .find(|s| s.provider == request.source.provider && s.session == request.source.session)
            .map(|s| s.lane.clone())
            .ok_or("Source unavailable")?;
        handoff::reserve(
            &state,
            &lane,
            request.revision,
            &request.provider,
            &request.receipt,
        )
    })
    .await
    .map(Json)
    .map_err(bad)
}

/// Stop only the verified current source; an HTTP success means process shutdown was confirmed.
pub(crate) async fn stop(
    State(state): State<Arc<AppState>>,
    Json(id): Json<Identity>,
) -> Result<Json<bool>, Failure> {
    let owner = state.clone();
    let source = crate::serve::in_blocking(move || {
        let task =
            task_for(&Repository::new(&owner.repo())?, &id)?.ok_or("Source task unavailable")?;
        if task.owner.provider != id.provider || task.owner.session != id.session {
            return Err("This source has already handed work to another agent.".into());
        }
        Ok(task.owner)
    })
    .await
    .map_err(bad)?;
    super::stopping::stop(state, source).await.map_err(bad)?;
    Ok(Json(true))
}
