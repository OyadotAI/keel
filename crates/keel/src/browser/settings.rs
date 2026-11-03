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
    let (preference, installation, detection_error) = crate::serve::in_blocking(move || {
        let preference = preference::read()?;
        let (installation, error) = match detection::installed() {
            Ok(value) => (value, None),
            Err(error) => (None, Some(error)),
        };
        Ok((preference, installation, error))
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
        effective: bindings::effective(query.lane.as_deref()),
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Save {
    selected: Backend,
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
    use keel_workspace::Real;
    if state.repo().real().ok().as_ref() != Some(&binding.project)
        || binding.backend != Backend::LocalOya
    {
        return Err(bad("This session is not using local Oya.".into()));
    }
    let mut page = binding
        .page
        .try_lock()
        .map_err(|_| bad("A browser operation is still pending.".into()))?;
    page.show().await.map(Json).map_err(bad)
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
    use keel_workspace::Real;
    if state.repo().real().ok().as_ref() != Some(&binding.project)
        || binding.backend != Backend::LocalOya
    {
        return Err(bad("This session is not using local Oya.".into()));
    }
    let mut page = binding
        .page
        .try_lock()
        .map_err(|_| bad("A browser operation is still pending.".into()))?;
    page.reset();
    Ok(Json(true))
}
