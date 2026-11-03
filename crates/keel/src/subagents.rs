//! Parent-scoped read-only child observation shares lifecycle readers without creating terminals.
use crate::serve::{Checkout, blocking};
use axum::{
    extract::Query,
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use camino::Utf8Path;
use keel_workspace::{
    Follows, Transcript,
    subagents::{Observer, Snapshot},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

type Handle = Arc<Mutex<Observer>>;
static READERS: OnceLock<Mutex<BTreeMap<String, Weak<Mutex<Observer>>>>> = OnceLock::new();
const OUTPUT_POLL: std::time::Duration = std::time::Duration::from_secs(1);
const OUTPUT_QUEUE: usize = 4;
const ID_LIMIT: usize = 256;

/// The authorized parent tail owns this lease; weak registration cannot keep closed lanes alive.
pub(crate) fn watch(parent: Transcript, directory: &Utf8Path, id: &str) -> Handle {
    let key = format!("{:?}:{}:{directory}:{id}", parent.agent(), parent.path());
    let mut readers = READERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    readers.retain(|_, v| v.strong_count() > 0);
    if let Some(reader) = readers.get(&key).and_then(Weak::upgrade) {
        return reader;
    }
    let reader = Arc::new(Mutex::new(Observer::new(
        parent,
        directory.to_owned(),
        id.to_string(),
    )));
    readers.insert(key, Arc::downgrade(&reader));
    reader
}

/// Disk discovery stays off Tokio; only confirmed parent exit interrupts unfinished executions.
pub(crate) async fn snapshot(
    reader: Handle,
    alive: Option<bool>,
    since: Option<String>,
) -> Option<Snapshot> {
    blocking(
        move || {
            let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
            reader.lifetime(since.as_deref());
            let mut snapshot = if alive == Some(false) {
                reader.interrupted()
            } else {
                reader.snapshot()
            };
            if alive.is_none() {
                for child in &mut snapshot.invocations {
                    if matches!(
                        child.status,
                        keel_workspace::subagents::Status::Working
                            | keel_workspace::subagents::Status::Waiting
                    ) {
                        child.status = keel_workspace::subagents::Status::Unknown;
                    }
                }
            }
            Some(snapshot)
        },
        None,
    )
    .await
}

/// A child is selected by execution ID within an authorized parent, never by arbitrary path.
#[derive(Deserialize)]
pub struct OutputQuery {
    parent: String,
    invocation: String,
    agent: Option<String>,
}

/// Inspect an existing child's output; closing this stream has no process or execution effects.
pub async fn output(Checkout(repo): Checkout, Query(query): Query<OutputQuery>) -> Response {
    if query.invocation.len() > ID_LIMIT || query.parent.len() > ID_LIMIT {
        return (StatusCode::BAD_REQUEST, "Invalid subagent identity.").into_response();
    }
    let agent = match Follows::from_query(query.agent.as_deref()) {
        Ok(a) => a,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let parent = query.parent.clone();
    let dir = repo.clone();
    let Some(transcript) = blocking(move || agent.locate(&dir, &parent), None).await else {
        return (StatusCode::NOT_FOUND, "Parent conversation is unavailable.").into_response();
    };
    let reader = watch(transcript, &repo, &query.parent);
    let first_reader = reader.clone();
    let id = query.invocation.clone();
    let initial = blocking(
        move || {
            first_reader
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .detail(&id)
        },
        None,
    )
    .await;
    let Some(initial) = initial else {
        return (
            StatusCode::NOT_FOUND,
            "That invocation does not belong to this parent conversation.",
        )
            .into_response();
    };
    let (tx, rx) =
        tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(OUTPUT_QUEUE);
    tokio::spawn(async move {
        let mut previous = None;
        let mut detail = Some(initial);
        loop {
            if let Some(current) = detail
                && previous.as_ref() != Some(&current)
            {
                let data = serde_json::to_string(&current).expect("child output");
                if tx
                    .send(Ok(Event::default().event("output").data(data)))
                    .await
                    .is_err()
                {
                    break;
                }
                previous = Some(current);
            }
            tokio::select! { _=tx.closed()=>break, _=tokio::time::sleep(OUTPUT_POLL)=>{} }
            let reader = reader.clone();
            let id = query.invocation.clone();
            detail = blocking(
                move || reader.lock().unwrap_or_else(|e| e.into_inner()).detail(&id),
                None,
            )
            .await;
            if detail.is_none() {
                let _ = tx
                    .send(Ok(Event::default()
                        .event("unavailable")
                        .data("Invocation no longer retained.")))
                    .await;
                break;
            }
        }
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}
