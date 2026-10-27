//! One durable name across lanes, native provider metadata, and History.
use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod codex_api;
mod repository;
#[cfg(test)]
mod tests;
use repository::{Identity, Repository};

const MAX_NAME_CHARS: usize = 80;
const MAX_SESSION_ID_CHARS: usize = 128;

/// A provider session in a checkout chosen through the project's normal worktree guard.
#[derive(Deserialize)]
pub struct RegisterBody {
    pub id: String,
    #[serde(default = "claude")]
    pub provider: String,
    pub wt: Option<String>,
}
fn claude() -> String {
    "claude".into()
}

/// Rename the native session, or durably defer it until the conversation exists.
#[derive(Deserialize)]
pub struct RenameBody {
    #[serde(flatten)]
    pub session: RegisterBody,
    pub name: String,
    /// Legacy local labels never replace an already explicit native name during migration.
    #[serde(default)]
    pub legacy: bool,
}

/// Keep the original rename response field while returning authoritative session metadata.
#[derive(Serialize)]
pub struct Renamed {
    name: String,
    #[serde(flatten)]
    session: keel_workspace::Session,
}

/// Register an opened conversation so closing its lane leaves a provider-aware History entry.
pub async fn register(
    State(state): State<Arc<crate::serve::AppState>>,
    Json(body): Json<RegisterBody>,
) -> Result<Json<keel_workspace::Session>, (StatusCode, String)> {
    let identity = identity(&state, &body).map_err(bad)?;
    crate::serve::in_blocking(move || Repository::local()?.register(identity))
        .await
        .map(Json)
        .map_err(bad)
}

/// Persist the requested name before provider I/O, keeping failures retryable across restarts.
pub async fn rename(
    State(state): State<Arc<crate::serve::AppState>>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Renamed>, (StatusCode, String)> {
    let identity = identity(&state, &body.session).map_err(bad)?;
    let name = body.name.trim().to_owned();
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(bad(
            "A session name must be one line and at most 80 characters".into(),
        ));
    }
    let requested = name.clone();
    crate::serve::in_blocking(move || Repository::local()?.rename(identity, &name, body.legacy))
        .await
        .map(|session| {
            Json(Renamed {
                name: requested,
                session,
            })
        })
        .map_err(bad)
}
fn bad(error: String) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error)
}
fn identity(state: &crate::serve::AppState, body: &RegisterBody) -> Result<Identity, String> {
    keel_workspace::Follows::from_query(Some(&body.provider)).map_err(str::to_owned)?;
    if body.id.len() > MAX_SESSION_ID_CHARS
        || body.id.is_empty()
        || !body
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("That is not a session id".into());
    }
    Ok(Identity {
        provider: body.provider.clone(),
        id: body.id.clone(),
        repo: state.repo().to_string(),
        cwd: state.checkout(body.wt.as_deref())?.to_string(),
        wt: body.wt.clone(),
    })
}

/// Every session-list transport uses this resolver, including native rename and deferred intent.
pub fn discover(repo: &Utf8Path, home: &Utf8Path) -> Vec<keel_workspace::Session> {
    let raw = keel_workspace::discover_sessions(repo, home);
    match Repository::local().and_then(|store| store.list(repo, raw.clone())) {
        Ok(list) => list,
        Err(_) => {
            tracing::warn!("Could not refresh session names");
            raw
        }
    }
}
