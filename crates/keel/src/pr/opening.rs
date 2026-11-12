//! One checkout reservation spans cancellation, reconciliation, push, creation, and cleanup.
use super::{PrQuery, github, repository, target};
use crate::serve::{AppState, Checkout, Held};
use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use serde_json::json;
use std::{convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

type Tx = mpsc::Sender<Result<Event, Infallible>>;

async fn say(tx: &Tx, event: &'static str, text: impl Into<String>) {
    let _ = tx
        .send(Ok(Event::default().event(event).data(text.into())))
        .await;
}

/// A disconnected window does not relinquish an in-flight publication to a duplicate request.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Query(query): Query<PrQuery>,
) -> impl axum::response::IntoResponse {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let result = open(state, repo, query, &tx).await;
        if let Err(error) = &result {
            say(&tx, "fatal", error.clone()).await;
        }
        say(&tx, "done", if result.is_ok() { "0" } else { "1" }).await;
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn open(
    state: Arc<AppState>,
    repo: camino::Utf8PathBuf,
    query: PrQuery,
    tx: &Tx,
) -> Result<(), String> {
    say(tx, "line", "Checking the pull request target…").await;
    let (s, r) = (state.clone(), repo.clone());
    let _held = tokio::task::spawn_blocking(move || {
        let key = format!("keel:pr:{r}");
        let token = s.reserve_checkout(&key, &r)?;
        Ok::<_, String>(Held::new(s, key, token))
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::prereview::cancel_for_open(&repo).await?;
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let target = target::capture(&repo, &query)?;
        let hosting = repository::capture(&mut |program, args| github::command(&repo, program, args))?;
        let opened = github::publish(&repo, &target, &hosting, &query, &mut |program, args| github::command(&repo, program, args), &mut |line| {
            let _ = tx.blocking_send(Ok(Event::default().event("line").data(line)));
        })?;
        super::remember(&repo, &target.branch, &opened.url);
        // What went into it, in the page's words: one commit is named, several are counted.
        let commits = super::ahead(&target.base, |args| crate::git::run(&repo, args));
        let files = [format!("origin/{}", target.base), target.base.clone()].iter().find_map(|base| {
            crate::git::run(&repo, &["diff", "--name-only", &format!("{base}...HEAD")]).ok()
        }).map(|names| names.lines().count());
        let title = crate::git::trimmed(&repo, &["log", "-1", "--format=%s"]).ok();
        let data = json!({"url":opened.url,"existing":opened.existing,"branch":target.branch,"head":opened.head,"requested_head":target.sha,"base":target.base,"commits":commits,"files":files,"title":title});
        let _ = tx.blocking_send(Ok(Event::default().event("pr").data(data.to_string())));
        let _ = tx.blocking_send(Ok(Event::default().event("line").data(opened.url)));
        Ok(())
    }).await.map_err(|e| e.to_string())?
}
