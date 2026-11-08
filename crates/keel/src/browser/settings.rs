//! Browser preference APIs report installation and connection separately without exposing credentials.
use super::{
    bindings, connection, detection,
    preference::{self, Backend},
};
use crate::serve::AppState;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
type Failure = (StatusCode, String);
#[derive(Default, Deserialize)]
pub(crate) struct Request {
    pub lane: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct Status {
    selected: Option<Backend>,
    revision: u64,
    installation: Option<detection::Installation>,
    detection_error: Option<String>,
    connection_state: String,
    connection_message: String,
    effective: bindings::Effective,
}
/// Detection and metadata handshakes open no pages and retain the user's profile untouched.
pub(crate) async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Request>,
) -> Result<Json<Status>, Failure> {
    let project = state.repo();
    let (here, lane) = (project.clone(), query.lane.clone());
    let (preference, installation, detection_error, effective) =
        crate::serve::in_blocking(move || {
            let preference = preference::read()?;
            let (installation, error) = match detection::installed() {
                Ok(value) => (value, None),
                Err(error) => (None, Some(error)),
            };
            // Keyed by the real path, as every other reader of a lane's choice is.
            use keel_workspace::Real;
            let here = here.real().unwrap_or(here);
            let effective = bindings::effective(&here, lane.as_deref());
            Ok((preference, installation, error, effective))
        })
        .await
        .map_err(bad)?;
    let (connection_state, connection_message) = if let Some(installation) = &installation {
        let installation = installation.clone();
        let connection =
            crate::serve::in_blocking(move || connection::discover(&project, &installation)).await;
        match connection {
            Ok(connection) => match connection::Client::new(connection) {
                Ok(mut client) => match client.initialize().await {
                    Ok(()) => ("ready".into(), "Connected to local Oya Browser.".into()),
                    Err(error) => ("unavailable".into(), error),
                },
                Err(error) => ("unavailable".into(), error),
            },
            Err(error) => ("setup_needed".into(), error),
        }
    } else {
        (
            "unavailable".into(),
            if detection_error.is_some() {
                "Couldn’t check for Oya Browser."
            } else {
                "Local Oya Browser wasn’t found."
            }
            .into(),
        )
    };
    Ok(Json(Status {
        selected: preference.selected,
        revision: preference.revision,
        installation,
        detection_error,
        connection_state,
        connection_message,
        effective,
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Save {
    /// `null` is "Ask each lane".
    selected: Option<Backend>,
    revision: u64,
}
/// Saving a default never replaces a running conversation's captured browser binding.
pub(crate) async fn save(Json(body): Json<Save>) -> Result<Json<serde_json::Value>, Failure> {
    crate::serve::in_blocking(move || preference::save(body.selected, body.revision))
        .await
        .map(|p| Json(serde_json::json!({"selected":p.selected,"revision":p.revision})))
        .map_err(bad)
}
/// Focus only this binding's owned tab through Oya's authenticated ownership-enforcing adapter.
pub(crate) async fn show(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Request>,
) -> Result<Json<String>, Failure> {
    let lane = query
        .lane
        .ok_or_else(|| bad("A lane is required.".into()))?;
    let binding =
        bindings::get(&lane).ok_or_else(|| bad("This session has no browser binding.".into()))?;
    uses_oya(&state, &binding).await?;
    let mut page = binding
        .page
        .try_lock()
        .map_err(|_| bad("A browser operation is still pending.".into()))?;
    page.show().await.map(Json).map_err(bad)
}
/// Only a lane whose browser is Oya, in this project, reaches Oya's tabs from here.
async fn uses_oya(state: &AppState, binding: &bindings::Binding) -> Result<(), Failure> {
    use keel_workspace::Real;
    let (repo, project, lane) = (state.repo(), binding.project.clone(), binding.lane.clone());
    let oya = crate::serve::in_blocking(move || {
        repo.real().ok().as_ref() == Some(&project)
            && preference::resolve(&project, &lane).ok().flatten() == Some(Backend::LocalOya)
    })
    .await;
    if oya {
        Ok(())
    } else {
        Err(bad("This lane is not using Oya Browser.".into()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lane {
    backend: Backend,
}
/// `POST /api/browser/lane?lane=`: the Browser tab's "Browse in" switch. Applies to the lane's next
/// browser step, in every session it runs; a step already running finishes where it started.
pub(crate) async fn set_lane(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Request>,
    Json(body): Json<Lane>,
) -> Result<Json<bool>, Failure> {
    use keel_workspace::Real;
    let lane = query
        .lane
        .ok_or_else(|| bad("A lane is required.".into()))?;
    let repo = state.repo();
    let changed = lane.clone();
    crate::serve::in_blocking(move || {
        let project = repo.real().map_err(|e| e.to_string())?;
        preference::set_lane(&project, &lane, body.backend)
    })
    .await
    .map_err(bad)?;
    bindings::changed(&changed);
    Ok(Json(true))
}

fn bad(error: String) -> Failure {
    (StatusCode::CONFLICT, error)
}

/// Explicit recovery invalidates old tab IDs without closing any Oya tabs or replaying an action.
pub(crate) async fn reconnect(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Request>,
) -> Result<Json<bool>, Failure> {
    let lane = query
        .lane
        .ok_or_else(|| bad("A lane is required.".into()))?;
    let binding =
        bindings::get(&lane).ok_or_else(|| bad("This session has no browser binding.".into()))?;
    uses_oya(&state, &binding).await?;
    let mut page = binding
        .page
        .try_lock()
        .map_err(|_| bad("A browser operation is still pending.".into()))?;
    page.reset();
    Ok(Json(true))
}
