//! Mutations acknowledge the selected review, never a replacement run or a still-running dismissal.
use super::*;
use serde_json::{Value, json};

const STOP_WAIT: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(100);

/// A delayed click names its review so it cannot cancel or dismiss a later one.
#[derive(Deserialize)]
pub struct Control {
    lane: String,
    id: Option<String>,
}

fn conflict(message: &str) -> (StatusCode, String) {
    (StatusCode::CONFLICT, message.into())
}

fn request_stop(dir: &Utf8Path, q: &Control) -> Result<Value, (StatusCode, String)> {
    let mut all = reviews().locked();
    let Some(entry) = find(&mut all, &q.lane, dir) else {
        return Ok(json!({"id":q.id,"missing":true,"finished":true,"cancel_requested":false}));
    };
    if q.id.as_ref().is_some_and(|id| id != &entry.review.id) {
        return Err(conflict(
            "The review changed. Refresh its status before cancelling.",
        ));
    }
    let active = running(entry);
    if active {
        entry.stop.store(true, Ordering::Relaxed);
    }
    Ok(json!({"id":entry.review.id,"finished":!active,"cancel_requested":active,"missing":false}))
}

/// Acknowledging a cancellation request does not claim the provider worker has already ended.
pub async fn api_stop(
    Checkout(dir): Checkout,
    Json(q): Json<Control>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // no-blocking: only inspects an in-memory record and sets its cancellation flag.
    request_stop(&dir, &q).map(Json)
}

fn dismiss(dir: &Utf8Path, q: &Control) -> Result<Value, (StatusCode, String)> {
    let mut all = reviews().locked();
    let Some(entry) = find(&mut all, &q.lane, dir) else {
        return Ok(json!({"id":q.id,"dismissed":true,"missing":true}));
    };
    if q.id.as_ref().is_some_and(|id| id != &entry.review.id) {
        return Err(conflict(
            "The review changed. Refresh its status before dismissing.",
        ));
    }
    if running(entry) {
        return Err(conflict(
            "The review is still stopping. Wait for it to finish, then dismiss it.",
        ));
    }
    let id = entry.review.id.clone();
    let wt = entry.review.wt.clone();
    all.retain(|e| e.review.id != id || e.dir != dir);
    drop(all);
    crate::events::emit("reviews.changed", wt.as_deref(), json!({"lane":q.lane}));
    Ok(json!({"id":id,"dismissed":true}))
}

/// A running or mismatched review stays visible; repeated dismissal of an absent run is safe.
pub async fn api_dismiss(
    Checkout(dir): Checkout,
    Json(q): Json<Control>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // no-blocking: removes only a finished in-memory record.
    dismiss(&dir, &q).map(Json)
}

/// All PR entry points wait for real worker completion before publishing the checkout.
pub(crate) async fn cancel_for_open(dir: &Utf8Path) -> Result<(), String> {
    let ids: Vec<String> = {
        let all = reviews().locked();
        all.iter()
            .filter(|e| e.dir == dir && running(e))
            .map(|e| {
                e.stop.store(true, Ordering::Relaxed);
                e.review.id.clone()
            })
            .collect()
    };
    let settled = async {
        loop {
            let active: Vec<_> = {
                let all = reviews().locked();
                all.iter()
                    .filter(|e| e.dir == dir && running(e))
                    .map(|e| e.review.id.clone())
                    .collect()
            };
            if active.iter().any(|id| !ids.contains(id)) {
                return Err("The review changed while stopping. Refresh its status before opening the pull request.".into());
            }
            if active.is_empty() {
                return Ok(());
            }
            tokio::time::sleep(STOP_POLL).await;
        }
    };
    tokio::time::timeout(STOP_WAIT, settled)
        .await
        .map_err(|_| {
            "The review is still stopping. Wait for it to finish, then retry opening.".to_string()
        })?
}

#[cfg(test)]
mod tests;
