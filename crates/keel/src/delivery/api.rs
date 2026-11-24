//! Authenticated project routes translate delivery services without owning persistence or dispatch logic.
use super::{Failure, Service, StartIssue, service};
use crate::serve::AppState;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

/// Retry loads the original private request and cannot substitute a newer checkout or form.
pub(crate) async fn retry_publication(
    State(state): State<Arc<AppState>>,
    Path((task, id)): Path<(String, String)>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    let receipt = service.read_receipt(&id).await?;
    if receipt.kind != "publication" || receipt.input["task"] != task {
        return Err(Failure::Invalid(
            "This publication request belongs to another task.".into(),
        ));
    }
    let input: super::PublishInput = serde_json::from_value(receipt.input["request"].clone())
        .map_err(|_| Failure::Unavailable("Original publication request is unreadable.".into()))?;
    tokio::spawn(async move {
        let _ = service.publish(task, input).await;
    });
    Ok(Json(receipt))
}

/// Explicit preparation seeds one persisted PR draft shared by delivery and review.
pub(crate) async fn form(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
) -> Result<Json<super::PrForm>, Failure> {
    read(state, move |service| service.seed_form(&task)).await
}
/// Draft updates use a revision and never replace an already captured publication intent.
pub(crate) async fn save_form(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
    Json(input): Json<super::FormInput>,
) -> Result<Json<super::PrForm>, Failure> {
    read(state, move |service| service.save_form(&task, input)).await
}
/// Publication survives the response lifetime; status is read through the durable receipt.
pub(crate) async fn publish(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
    Json(input): Json<super::PublishInput>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    let owner = service.clone();
    let request = input.clone();
    let target = task.clone();
    let receipt = crate::serve::in_blocking(move || {
        owner.execution.reserve(
            &request.receipt,
            "publication",
            serde_json::json!({"task":target,"request":request}),
            owner.clock.now(),
        )
    })
    .await
    .map_err(Failure::from)?;
    tokio::spawn(async move {
        let key = input.receipt.clone();
        if let Err(error) = service.clone().publish(task, input).await {
            let captured = service
                .read_receipt(&key)
                .await
                .ok()
                .is_some_and(|r| r.result.is_some());
            let _ = service
                .save_phase(
                    &key,
                    if captured {
                        crate::execution::Phase::Unconfirmed
                    } else {
                        crate::execution::Phase::FailedBeforeAcceptance
                    },
                    Some(error.message().into()),
                )
                .await;
        }
    });
    Ok(Json(receipt))
}
/// Definitions and history are local host authority, independent of saved window lanes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryQuery {
    after: Option<String>,
}
/// History is paged by time and ID; the active slot is always returned independently.
pub(crate) async fn schedules(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<super::schedules::SchedulePage>, Failure> {
    read(state, move |service| {
        service.schedules_page(query.after.as_deref())
    })
    .await
}
/// Saving never enables a definition; Enable is an explicit separate action.
pub(crate) async fn save_schedule(
    State(state): State<Arc<AppState>>,
    Json(input): Json<super::ScheduleInput>,
) -> Result<Json<crate::execution::Schedule>, Failure> {
    read(state, move |service| service.save_schedule(input)).await
}
/// Pause affects future ticks; Stop addresses the current occurrence separately.
pub(crate) async fn schedule_action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<super::ScheduleAction>,
) -> Result<Json<serde_json::Value>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    tokio::spawn(async move { service.schedule_action(&id, input).await })
        .await
        .map_err(|_| {
            Failure::Unavailable(
                "Recurring action is unconfirmed. Inspect its saved history.".into(),
            )
        })?
        .map(Json)
}

/// Saved corrections belong to the stable task, independent of a particular diff mount.
pub(crate) async fn add_correction(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
    Json(input): Json<super::CorrectionInput>,
) -> Result<Json<super::Correction>, Failure> {
    read(state, move |service| service.add_correction(&task, input)).await
}
/// Edits require the last observed task revision and preserve delivery history.
pub(crate) async fn edit_correction(
    State(state): State<Arc<AppState>>,
    Path((task, id)): Path<(String, String)>,
    Json(input): Json<super::CorrectionEdit>,
) -> Result<Json<super::Correction>, Failure> {
    read(state, move |service| {
        service.edit_correction(&task, &id, input)
    })
    .await
}
/// One private receipt owns queued feedback even if its initiating window closes.
pub(crate) async fn send_batch(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
    Json(input): Json<super::BatchInput>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    tokio::spawn(async move { service.send_batch(task, input).await })
        .await
        .map_err(|_| {
            Failure::Unavailable(
                "Batch acceptance is unconfirmed. Inspect its saved receipt.".into(),
            )
        })?
        .map(Json)
}
/// Explicit refresh does not resolve items on an incomplete response.
pub(crate) async fn refresh_feedback(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
) -> Result<Json<super::Feedback>, Failure> {
    read(state, move |service| {
        let mut reader = crate::github_read::Gh::new(&service.root);
        service.refresh_feedback(&task, &mut reader)
    })
    .await
}

async fn read<T: Send + 'static>(
    state: Arc<AppState>,
    action: impl FnOnce(Arc<Service>) -> Result<T, Failure> + Send + 'static,
) -> Result<Json<T>, Failure> {
    crate::serve::in_blocking(move || action(service(&state)?))
        .await
        .map(Json)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    after: Option<String>,
    limit: Option<usize>,
}
/// Task discovery does not need a previously saved window lane or start any provider.
pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Result<Json<super::discovery::Page>, Failure> {
    read(state, move |service| {
        service.list(
            query.after.as_deref(),
            query.limit.unwrap_or(super::constants::PAGE_LIMIT),
        )
    })
    .await
}
/// Detail preserves saved evidence even when its current owner is temporarily unconfirmed.
pub(crate) async fn task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<super::service::TaskView>, Failure> {
    read(state, move |service| service.task(&id)).await
}

/// Recovery validates the original checkout and returns its owner without creating a new writer.
pub(crate) async fn recover(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<crate::tasks::Source>, Failure> {
    read(state, move |service| service.owner(&id)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaneQuery {
    lane: String,
}
/// Handoff aliases resolve through original task membership rather than duplicated React metadata.
pub(crate) async fn by_lane(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LaneQuery>,
) -> Result<Json<Option<super::service::TaskView>>, Failure> {
    read(state, move |service| {
        crate::tasks::for_lane(&service.root, &query.lane)
            .map_err(Failure::Unavailable)?
            .map(|id| service.task(&id))
            .transpose()
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preview {
    url: String,
}
/// Explicit preview reads GitHub only after project identity checks; it performs no runtime launch.
pub(crate) async fn preview(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Preview>,
) -> Result<Json<super::IssuePreview>, Failure> {
    read(state, move |service| {
        let mut host = crate::github_read::Gh::new(&service.root);
        let project = host.project().map_err(Failure::Unavailable)?;
        service.preview(&input.url, &mut host, &project)
    })
    .await
}

/// A dropped HTTP request cannot cancel the durable preparation/acceptance state machine halfway through.
pub(crate) async fn start(
    State(state): State<Arc<AppState>>,
    Json(input): Json<StartIssue>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    tokio::spawn(async move{service.start_issue(input).await}).await.map_err(|_|Failure::Unavailable("Task creation is unconfirmed. Check the saved receipt before starting another task.".into()))?.map(Json)
}

/// Status inspection reconciles the saved receipt without sending the provider another request.
pub(crate) async fn receipt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    service.operation_status(&id).await.map(Json)
}

/// The accepted check receipt returns immediately while its daemon-owned controller runs.
pub(crate) async fn checks(
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
    Json(input): Json<super::CheckInput>,
) -> Result<Json<crate::execution::Receipt>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    tokio::spawn(async move { service.run_checks(task, input).await })
        .await
        .map_err(|_| {
            Failure::Unavailable("Check creation is unconfirmed. Inspect its saved receipt.".into())
        })?
        .map(Json)
}

/// Cancellation names one receipt and cannot stop a later source turn in the same lane.
pub(crate) async fn stop_checks(
    State(state): State<Arc<AppState>>,
    Path((task, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, Failure> {
    let service = crate::serve::in_blocking(move || service(&state)).await?;
    service
        .stop_checks(&task, &id)
        .await
        .map(|stopped| Json(serde_json::json!({"stopped":stopped})))
}
