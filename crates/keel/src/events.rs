//! What changed in the project, as one stream the window subscribes to.
//!
//! The app used to find out by asking: the session list every three seconds while History was
//! open, the background jobs every two or fifteen per lane for the life of the window, the dev
//! server forty times after starting it, git after every turn and every time the app came to the
//! front. Four idle lanes asked the daemon for the same empty job list 172,800 times a day. Nothing
//! could tell the app that a session had started in a terminal, that a file had been saved in an
//! editor, or that a question was waiting — so it polled for all of them, and still found out late.
//!
//! One `GET /api/events` per window instead. The events are coarse on purpose — "git changed,
//! read it again" rather than a patch — because the daemon already caches what a read costs, and a
//! diff protocol is a second thing to get wrong. The small ones carry their payload: the session
//! list, because it is what changes most and is a few KB.
//!
//! Emitted from three places: a middleware, for every mutating request, keyed on the path it
//! took — one place rather than a call in sixty handlers; the daemon's own hands, where it changes
//! things nobody asked it to (a job's output, a dev server's URL, the checkpoint after a turn);
//! and two watchers, for what happens outside Keel altogether — a session started in a terminal,
//! a file saved in an editor.

use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use camino::Utf8PathBuf;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::serve::AppState;

/// Events buffered per subscriber before the oldest are dropped. A window that lags this far
/// behind is told so by `Lagged`, and reads everything again.
const BUS: usize = 512;

#[derive(Serialize, Clone, Debug)]
pub struct Emitted {
    pub seq: u64,
    pub kind: &'static str,
    /// The checkout the event is about, for the per-checkout kinds. `None` is the project.
    pub wt: Option<String>,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

fn bus() -> &'static broadcast::Sender<Arc<Emitted>> {
    static BUS_: OnceLock<broadcast::Sender<Arc<Emitted>>> = OnceLock::new();
    BUS_.get_or_init(|| broadcast::channel(BUS).0)
}

fn next_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Say that something changed. Cheap enough to call from a drain loop: a clone of an `Arc` per
/// subscriber, and nothing at all when nobody is listening.
pub fn emit(kind: &'static str, wt: Option<&str>, data: serde_json::Value) {
    let _ = bus().send(Arc::new(Emitted {
        seq: next_seq(),
        kind,
        wt: wt.map(str::to_string),
        data,
    }));
}

pub fn subscribe() -> broadcast::Receiver<Arc<Emitted>> {
    bus().subscribe()
}

/// `GET /api/events`: every change, as it happens, for the life of the window.
// no-blocking: forwards a channel.
pub async fn stream(State(_state): State<Arc<AppState>>) -> impl axum::response::IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(64);
    tokio::spawn(async move {
        let mut events = subscribe();
        // The first frame says which sequence number the window is reading from, so a
        // reconnect can tell whether it missed anything.
        let _ = tx
            .send(Ok(Event::default()
                .event("connected")
                .data(serde_json::json!({ "seq": next_seq() }).to_string())))
            .await;
        // A broadcast does not replay: a window that connects after the watcher's first pass
        // would wait for the list to change. Ask for it to be said again.
        poke_sessions();
        loop {
            let emitted = tokio::select! {
                _ = tx.closed() => return,
                e = events.recv() => match e {
                    Ok(e) => e,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Too far behind to say what changed: say that everything might have.
                        let _ = tx
                            .send(Ok(Event::default().event("lagged").data("{}")))
                            .await;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            };
            let Ok(json) = serde_json::to_string(&*emitted) else {
                continue;
            };
            if tx
                .send(Ok(Event::default().event(emitted.kind).data(json)))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// What a mutating request changed, from the path it took.
///
/// One place rather than a call in every handler, and keyed on the route because the route is
/// already the statement of what a request is about. A request that failed changed nothing and
/// says nothing.
pub async fn after_mutation(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let wt = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("wt=")))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let response = next.run(req).await;
    if !response.status().is_success() {
        return response;
    }
    let mutating = method == axum::http::Method::POST
        || path.starts_with("/api/plugins/install")
        || path.starts_with("/api/plugins/action")
        || path.starts_with("/api/mcp/")
        || path.starts_with("/api/cli/install");
    if !mutating {
        return response;
    }
    let wt = wt.as_deref();
    match path.as_str() {
        p if p.starts_with("/api/git/") => {
            emit("git.changed", wt, serde_json::Value::Null);
            emit("tree.changed", wt, serde_json::Value::Null);
        }
        p if p.starts_with("/api/worktree") => {
            emit("worktrees.changed", None, serde_json::Value::Null);
            emit("git.changed", None, serde_json::Value::Null);
        }
        p if p.starts_with("/api/fs/") => emit("tree.changed", wt, serde_json::Value::Null),
        p if p.starts_with("/api/permissions") => {
            emit("permissions.changed", None, serde_json::Value::Null)
        }
        p if p.starts_with("/api/dev") => emit("dev.changed", None, serde_json::Value::Null),
        p if p.starts_with("/api/monitors") => {
            emit("monitors.changed", None, serde_json::Value::Null)
        }
        "/api/session/rename" => poke_sessions(),
        p if p.starts_with("/api/open")
            || p.starts_with("/api/readiness")
            || p.starts_with("/api/adopt")
            || p.starts_with("/api/plugins")
            || p.starts_with("/api/mcp")
            || p.starts_with("/api/cli") =>
        {
            emit("state.changed", None, serde_json::Value::Null);
            poke_sessions();
        }
        _ => {}
    }
    response
}

fn sessions_poke() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// Set by `poke_sessions`, so the next pass sends the list whether or not it changed.
static SAY_AGAIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the sessions watcher to look now rather than at its next tick, and to say what it finds.
pub fn poke_sessions() {
    SAY_AGAIN.store(true, std::sync::atomic::Ordering::Relaxed);
    sessions_poke().notify_one();
}

/// The two watchers, for what happens outside Keel: sessions started in a terminal and files
/// saved in an editor. Started once per daemon, beside the server.
pub fn watch(state: Arc<AppState>) {
    tokio::spawn(watch_sessions(state.clone()));
    tokio::spawn(watch_tree(state));
}

/// The session list, sent whole whenever it changes.
///
/// Woken by kqueue on Claude Code's pid files and on every project directory that holds this
/// repository's transcripts; a 5 s fallback catches a dead pid whose file nobody removed. Sent
/// only on a change, so a quiet daemon sends nothing.
async fn watch_sessions(state: Arc<AppState>) {
    use notify::Watcher;
    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    let (wake_tx, mut wake) = tokio::sync::mpsc::channel::<()>(1);
    let mut watcher =
        notify::recommended_watcher(move |_: Result<notify::Event, notify::Error>| {
            let _ = wake_tx.try_send(());
        })
        .ok();
    let mut watched: Vec<Utf8PathBuf> = Vec::new();
    let mut last: Option<Vec<keel_workspace::Session>> = None;
    loop {
        if state.project_open() {
            let repo = state.repo();
            // The directories worth watching move with the project. Re-armed on every pass,
            // which is a no-op when nothing changed; capped, because a home directory with a
            // thousand projects is not a thing to open a thousand descriptors for.
            if let Some(w) = watcher.as_mut() {
                let mut dirs: Vec<Utf8PathBuf> = vec![home.join("sessions")];
                let (r, h) = (repo.clone(), home.clone());
                dirs.extend(
                    crate::serve::blocking(
                        move || {
                            keel_workspace::session_dirs(&r, &h)
                                .into_iter()
                                .map(|(d, scope)| {
                                    if scope == "below" {
                                        d
                                    } else {
                                        h.join("projects").join(keel_workspace::project_key(&d))
                                    }
                                })
                                .collect::<Vec<_>>()
                        },
                        Vec::new(),
                    )
                    .await
                    .into_iter()
                    .take(32),
                );
                for d in dirs {
                    if !watched.contains(&d)
                        && d.is_dir()
                        && w.watch(d.as_std_path(), notify::RecursiveMode::NonRecursive)
                            .is_ok()
                    {
                        watched.push(d);
                    }
                }
            }
            let (r, h) = (repo.clone(), home.clone());
            let now = crate::serve::blocking(
                move || keel_workspace::discover_sessions(&r, &h),
                Vec::new(),
            )
            .await;
            let again = SAY_AGAIN.swap(false, std::sync::atomic::Ordering::Relaxed);
            if again || last.as_ref() != Some(&now) {
                if let Ok(data) = serde_json::to_value(&now) {
                    emit("sessions", None, data);
                }
                last = Some(now);
            }
        }
        tokio::select! {
            _ = wake.recv() => {
                // Debounced: a session writing its transcript wakes this on every record.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                while wake.try_recv().is_ok() {}
            }
            _ = sessions_poke().notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }
}

/// The working tree, read every two seconds and said only when it changed.
///
/// One loop in one process, instead of one poll per lane per window. Not `notify`: kqueue is per
/// file descriptor, and a recursive watch of a checkout with `node_modules` is tens of thousands
/// of them. A `git status` on a warm repository is tens of milliseconds.
// ponytail: 2 s stat loop; FSEvents if it ever shows on a profile.
async fn watch_tree(state: Arc<AppState>) {
    let mut seen: std::collections::HashMap<Option<String>, u64> = Default::default();
    let mut pass: u64 = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if !state.project_open() {
            continue;
        }
        pass += 1;
        let root = state.repo();
        let mut checkouts: Vec<(Option<String>, Utf8PathBuf)> = vec![(None, root.clone())];
        // Lane checkouts every third pass: listing them is a git call of its own.
        if pass % 3 == 1 {
            let r = root.clone();
            let lanes = crate::serve::blocking(move || crate::worktree::list(&r), Vec::new()).await;
            checkouts.extend(
                lanes
                    .into_iter()
                    .map(|w| (Some(w.name.clone()), Utf8PathBuf::from(w.path))),
            );
        }
        for (wt, dir) in checkouts {
            let d = dir.clone();
            let hash = crate::serve::blocking(
                move || {
                    use std::hash::{Hash, Hasher};
                    let status = crate::repo::git_status(&d);
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    status.branch.hash(&mut h);
                    for c in &status.changes {
                        c.path.hash(&mut h);
                        c.status.hash(&mut h);
                    }
                    h.finish()
                },
                0,
            )
            .await;
            let before = seen.insert(wt.clone(), hash);
            if let Some(before) = before
                && before != hash
            {
                emit("tree.changed", wt.as_deref(), serde_json::Value::Null);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every subscriber gets every event, in order, and the sequence climbs.
    #[tokio::test]
    async fn two_subscribers_each_get_every_event() {
        let mut a = subscribe();
        let mut b = subscribe();
        emit("git.changed", Some("lane"), serde_json::Value::Null);
        emit("tree.changed", None, serde_json::json!({ "n": 1 }));
        for rx in [&mut a, &mut b] {
            let first = rx.recv().await.unwrap();
            let second = rx.recv().await.unwrap();
            assert_eq!(first.kind, "git.changed");
            assert_eq!(first.wt.as_deref(), Some("lane"));
            assert_eq!(second.kind, "tree.changed");
            assert!(second.seq > first.seq);
        }
    }

    #[test]
    fn an_event_serialises_without_an_empty_payload() {
        let e = Emitted {
            seq: 3,
            kind: "dev.changed",
            wt: None,
            data: serde_json::Value::Null,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"seq":3,"kind":"dev.changed","wt":null}"#
        );
    }
}
