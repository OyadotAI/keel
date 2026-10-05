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

use keel_workspace::Real;
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
    // no-blocking: subscribes to an in-memory bus; nothing touches the disk.
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
    // no-blocking: middleware; emitting is a broadcast in memory.
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
        // A skill or subagent is written to the project root and, for a skill, maybe committed.
        p if p.starts_with("/api/skills") || p.starts_with("/api/agents") => {
            emit("state.changed", None, serde_json::Value::Null);
            emit("git.changed", None, serde_json::Value::Null);
            emit("tree.changed", None, serde_json::Value::Null);
        }
        p if p.starts_with("/api/adopt") => {
            emit("state.changed", None, serde_json::Value::Null);
            emit("git.changed", wt, serde_json::Value::Null);
            emit("tree.changed", wt, serde_json::Value::Null);
        }
        p if p.starts_with("/api/open")
            || p.starts_with("/api/project")
            || p.starts_with("/api/readiness")
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
/// Woken by a watch on Claude Code's pid files and on every project directory that holds this
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
    // What the windows were last *sent*, not what was last read: drift is measured against what
    // they are showing.
    let mut last: Option<Vec<keel_workspace::Session>> = None;
    let mut sent = tokio::time::Instant::now();
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
            hold_terminal_claims(
                &state,
                now.iter()
                    .map(|s| (s.id.as_str(), s.cwd.as_deref(), s.live, s.busy)),
            );
            // A session writing its transcript changes its count and timestamp on every record,
            // so "the list differs" was true three times a second for as long as anything ran,
            // and every window redrew its sidebar each time. What a person reads off the list at
            // a glance — which sessions, what they are called, which are running — goes at once;
            // the counts ride along with that, or at most every thirty seconds.
            let reshaped = last.as_deref().map(sessions_shape) != Some(sessions_shape(&now));
            let drifted = last.as_ref() != Some(&now) && sent.elapsed() >= SESSIONS_DRIFT;
            if again || reshaped || drifted {
                if let Ok(data) = serde_json::to_value(&now) {
                    emit("sessions", None, data);
                }
                sent = tokio::time::Instant::now();
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

/// How stale a session's count and timestamp may get before the list goes out anyway.
const SESSIONS_DRIFT: std::time::Duration = std::time::Duration::from_secs(30);

/// What changes the list as a person reads it: which sessions, their titles and branches, and
/// whether each is running.
/// (id, title, branch, live, busy)
type Shape<'a> = (&'a str, Option<&'a str>, Option<&'a str>, bool, bool);

fn sessions_shape(list: &[keel_workspace::Session]) -> Vec<Shape<'_>> {
    list.iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.title.as_deref(),
                s.branch.as_deref(),
                s.live,
                s.busy,
            )
        })
        .collect()
}

/// A terminal `claude` busy in one of this project's trees holds that tree, whether or not a
/// window is following it.
///
/// The follower's claim covers the sessions somebody has open in Keel; this covers the rest,
/// which is where non-negotiable 11 had its gap — a lane could start writing a tree a terminal
/// was mid-turn in. Held while busy, released at idle unless a follower has it, in which case
/// the follower finishes the turn and releases it. Nothing here records facts: a tree held is
/// all an unfollowed session gets, and all it needs.
pub fn hold_terminal_claims<'a>(
    state: &AppState,
    sessions: impl Iterator<Item = (&'a str, Option<&'a str>, bool, bool)>,
) {
    let mut held = std::collections::HashSet::new();
    for (id, cwd, live, busy) in sessions {
        if state.owns_session(id) {
            continue;
        }
        if live && busy {
            held.insert(id);
            if let Some(checkout) = state.session_dir_checked(cwd) {
                // Refused when a lane is writing that tree: the lane was first, and the terminal
                // turn is the one that will be told so if a follower attaches.
                let _ = state.claim_terminal(id, &checkout);
            }
        }
    }
    // Dead, idle, or gone from the listing altogether: all of them give the tree back.
    state.release_terminal_except(&held);
}

/// The working tree, said only when it changed.
///
/// Woken by one recursive watch of the checkout — FSEvents on macOS, `ReadDirectoryChangesW` on
/// Windows, both one handle for the whole tree — and read with `git status` only for the checkouts
/// an event touched. It used to be `git status -uall` on every checkout every two seconds whether
/// or not anything moved, which on a large repository was steady CPU with every window idle. Never
/// kqueue: that is a descriptor per file, and a recursive watch of a checkout with
/// `node_modules` is tens of thousands of them. Elsewhere (Linux's inotify is a watch per
/// directory, with the same problem), and as a
/// fallback for anything a watch misses, a slow poll of everything.
async fn watch_tree(state: Arc<AppState>) {
    let dirty: Dirty = Default::default();
    let gitdirs: GitDirs = Default::default();
    let rescan = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let mut outside: Vec<std::path::PathBuf> = Vec::new();
    let mut seen: std::collections::HashMap<Option<String>, u64> = Default::default();
    let mut watching: Option<(Utf8PathBuf, Box<dyn notify::Watcher + Send>)> = None;
    let mut everything = true;
    loop {
        if state.project_open() {
            let root = state.repo();
            if watching.as_ref().map(|(r, _)| r) != Some(&root) {
                let handler = handler(
                    &root,
                    dirty.clone(),
                    gitdirs.clone(),
                    rescan.clone(),
                    wake.clone(),
                );
                watching = tree_watcher(&root, handler).map(|w| (root.clone(), w));
                outside.clear();
                everything = true;
            }
            everything |= rescan.swap(false, std::sync::atomic::Ordering::Relaxed);
            let mut checkouts: Vec<Option<String>> = {
                use crate::lock::Locked;
                dirty.locked().drain().collect()
            };
            if everything || watching.is_none() {
                let r = root.clone();
                let lanes =
                    crate::serve::blocking(move || crate::worktree::names(&r), Vec::new()).await;
                checkouts = std::iter::once(None)
                    .chain(lanes.into_iter().map(Some))
                    .collect();
                // Where each checkout's HEAD and index actually are. Not `<root>/.git`: a project
                // that is itself a linked worktree or a submodule keeps them outside the tree, and
                // git names a lane's admin directory `fix1` when a `fix` was ever registered.
                let (r, all) = (root.clone(), checkouts.clone());
                let found = crate::serve::blocking(move || git_dirs(&r, &all), Vec::new()).await;
                if let Some((_, w)) = watching.as_mut() {
                    for (dir, _) in &found {
                        if !outside.contains(dir)
                            && !found_under_root(dir, &root)
                            && w.watch(dir, notify::RecursiveMode::Recursive).is_ok()
                        {
                            outside.push(dir.clone());
                        }
                    }
                }
                *gitdirs.locked() = found;
            }
            for wt in checkouts {
                let dir = match &wt {
                    None => root.clone(),
                    Some(name) => root.join(crate::worktree::DIR).join(name),
                };
                let hash = crate::serve::blocking(move || status_hash(&dir), 0).await;
                let before = seen.insert(wt.clone(), hash);
                if let Some(before) = before
                    && before != hash
                {
                    emit("tree.changed", wt.as_deref(), serde_json::Value::Null);
                }
            }
        }
        let fallback = if watching.is_some() {
            TREE_FALLBACK
        } else {
            TREE_POLL
        };
        everything = tokio::select! {
            _ = wake.notified() => {
                // Debounced: one save is several events, and a build is thousands.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                false
            }
            _ = tokio::time::sleep(fallback) => true,
        };
    }
}

/// The checkouts an event touched since the last read: `None` the project, `Some` a lane.
type Dirty = Arc<std::sync::Mutex<std::collections::HashSet<Option<String>>>>;

/// With a watch: how often everything is read anyway, for what a watch can miss (an overflowed
/// event queue, a checkout created behind its back).
const TREE_FALLBACK: std::time::Duration = std::time::Duration::from_secs(30);
/// Without one: the old two-second poll.
const TREE_POLL: std::time::Duration = std::time::Duration::from_secs(2);

fn status_hash(dir: &camino::Utf8Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let status = crate::repo::git_status(dir);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    status.branch.hash(&mut h);
    for c in &status.changes {
        c.path.hash(&mut h);
        c.status.hash(&mut h);
    }
    h.finish()
}

/// Each checkout's git directory, resolved, longest first so a lane's
/// `<root>/.git/worktrees/<name>` wins over the project's `<root>/.git`.
type GitDirs = Arc<std::sync::Mutex<Vec<(std::path::PathBuf, Option<String>)>>>;

fn git_dirs(
    root: &camino::Utf8Path,
    checkouts: &[Option<String>],
) -> Vec<(std::path::PathBuf, Option<String>)> {
    let mut out: Vec<_> = checkouts
        .iter()
        .filter_map(|wt| {
            let dir = match wt {
                None => root.to_path_buf(),
                Some(name) => root.join(crate::worktree::DIR).join(name),
            };
            let git = crate::git::read(&dir, &["rev-parse", "--absolute-git-dir"])?;
            let git = std::path::PathBuf::from(git.trim());
            Some((keel_workspace::real_std(&git).unwrap_or(git), wt.clone()))
        })
        .collect();
    out.sort_by_key(|(d, _)| std::cmp::Reverse(d.as_os_str().len()));
    out
}

fn found_under_root(dir: &std::path::Path, root: &camino::Utf8Path) -> bool {
    let real = root.real().unwrap_or_else(|_| root.to_path_buf());
    dir.starts_with(real.as_std_path())
}

/// What a watch event means: which checkout it touched, or that everything must be read again.
fn handler(
    root: &camino::Utf8Path,
    dirty: Dirty,
    gitdirs: GitDirs,
    rescan: Arc<std::sync::atomic::AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) -> impl notify::EventHandler {
    // Events arrive with resolved paths; on macOS a temp dir or a symlinked home is not one.
    let real = root.real().unwrap_or_else(|_| root.to_path_buf());
    move |event: Result<notify::Event, notify::Error>| {
        let everything = || {
            rescan.store(true, std::sync::atomic::Ordering::Relaxed);
            wake.notify_one();
        };
        // An overflow or a rescan: something changed and nobody knows what. Every checkout.
        let Ok(event) = event else {
            return everything();
        };
        if event.need_rescan() {
            return everything();
        }
        let mut any = false;
        for path in &event.paths {
            if let Some(wt) = in_git_dir(path, &gitdirs.locked()) {
                if let Some(wt) = wt {
                    dirty.locked_insert(wt);
                    any = true;
                }
                continue;
            }
            let Ok(rel) = path.strip_prefix(real.as_std_path()) else {
                continue;
            };
            if let Some(wt) = checkout_of(rel) {
                dirty.locked_insert(wt);
                any = true;
            }
        }
        if any {
            wake.notify_one();
        }
    }
}

/// A path inside one of the git directories: `Some(Some(wt))` when it is that checkout's HEAD,
/// index or refs, `Some(None)` for anything else in there (objects, logs), `None` when it is in
/// none of them.
fn in_git_dir(
    path: &std::path::Path,
    dirs: &[(std::path::PathBuf, Option<String>)],
) -> Option<Option<Option<String>>> {
    let (dir, wt) = dirs.iter().find(|(d, _)| path.starts_with(d))?;
    let first = path
        .strip_prefix(dir)
        .ok()?
        .iter()
        .next()
        .and_then(|c| c.to_str());
    Some(matches!(first, Some("HEAD" | "index" | "refs")).then(|| wt.clone()))
}

/// One recursive watch of `root`. `None` where the platform has no watcher that is one handle
/// for a whole tree.
#[cfg(any(target_os = "macos", windows))]
fn tree_watcher(
    root: &camino::Utf8Path,
    handler: impl notify::EventHandler,
) -> Option<Box<dyn notify::Watcher + Send>> {
    use notify::Watcher;
    let mut w: Box<dyn Watcher + Send> =
        Box::new(notify::RecommendedWatcher::new(handler, notify::Config::default()).ok()?);
    w.watch(root.as_std_path(), notify::RecursiveMode::Recursive)
        .ok()?;
    Some(w)
}

/// Elsewhere — Linux's inotify watches one directory per handle — there is none, and the poll is
/// what notices a change.
#[cfg(not(any(target_os = "macos", windows)))]
fn tree_watcher(
    _root: &camino::Utf8Path,
    _handler: impl notify::EventHandler,
) -> Option<Box<dyn notify::Watcher + Send>> {
    None
}

trait LockedInsert {
    fn locked_insert(&self, wt: Option<String>);
}

use crate::lock::Locked;

impl LockedInsert for std::sync::Mutex<std::collections::HashSet<Option<String>>> {
    fn locked_insert(&self, wt: Option<String>) {
        self.locked().insert(wt);
    }
}

/// Which checkout a changed path belongs to: `Some(None)` the project, `Some(Some(lane))` a lane,
/// `None` nothing `git status` would report — build output, dependencies, git's object store,
/// Keel's own records. Without this a `cargo build` would be a status read every 300 ms.
fn checkout_of(rel: &std::path::Path) -> Option<Option<String>> {
    let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
    match parts.as_slice() {
        [".keel", "worktrees", name, rest @ ..] if !rest.is_empty() => {
            inside(rest).then(|| Some(name.to_string()))
        }
        [".keel", ..] => None,
        // A lane's HEAD and index live in the project's git directory.
        [".git", "worktrees", name, "HEAD" | "index", ..] => Some(Some(name.to_string())),
        rest => inside(rest).then_some(None),
    }
}

/// Whether a path inside one checkout can change what `git status` says about it.
fn inside(parts: &[&str]) -> bool {
    const NOISE: &[&str] = &[
        "node_modules",
        "target",
        ".next",
        ".turbo",
        ".svelte-kit",
        "dist",
        "build",
        ".cache",
        "__pycache__",
        ".venv",
        ".wrangler",
        ".DS_Store",
    ];
    if let Some(at) = parts.iter().position(|p| *p == ".git") {
        // HEAD for a branch switch, index for staging, refs for a commit. Nothing else in there.
        return matches!(parts.get(at + 1), Some(&"HEAD" | &"index" | &"refs"));
    }
    !parts.iter().any(|p| NOISE.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save is the checkout it is in; build output, git's object store and Keel's own records
    /// are nobody's, or a `cargo build` would be a `git status` every 300 ms.
    #[test]
    fn a_changed_path_names_its_checkout_or_nothing() {
        let of = |p: &str| checkout_of(std::path::Path::new(p));
        assert_eq!(of("src/main.rs"), Some(None));
        assert_eq!(of(".keel/worktrees/fix/src/a.ts"), Some(Some("fix".into())));
        assert_eq!(of(".git/worktrees/fix/index"), Some(Some("fix".into())));
        assert_eq!(of(".git/HEAD"), Some(None));
        assert_eq!(of(".git/refs/heads/main"), Some(None));
        assert_eq!(of(".git/objects/ab/cdef"), None);
        assert_eq!(of(".keel/turns/s.json"), None);
        assert_eq!(of("target/debug/keel"), None);
        assert_eq!(of("web/node_modules/x/index.js"), None);
        assert_eq!(of(".keel/worktrees/fix/node_modules/x.js"), None);
        assert_eq!(of("nested/.git/index"), Some(None));
    }

    /// A project that is itself a linked worktree keeps its HEAD and index in the main
    /// repository's git directory, outside the tree being watched. Staging there must still name
    /// the project, and git's object store must name nobody.
    #[test]
    fn a_git_directory_outside_the_tree_still_names_its_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let base = camino::Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
        let main = base.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let git = |d: &camino::Utf8Path, args: &[&str]| {
            assert!(
                crate::git::command(d)
                    .args(args)
                    .status()
                    .unwrap()
                    .success(),
                "{args:?}"
            );
        };
        git(&main, &["init", "-q"]);
        git(
            &main,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let project = base.join("project");
        git(&main, &["worktree", "add", "-q", project.as_str()]);

        let dirs = git_dirs(&project, &[None]);
        let (gitdir, _) = dirs.first().expect("the project's git directory");
        assert!(
            !gitdir.starts_with(project.as_std_path()),
            "outside the tree"
        );
        assert_eq!(in_git_dir(&gitdir.join("index"), &dirs), Some(Some(None)));
        assert_eq!(in_git_dir(&gitdir.join("logs/HEAD"), &dirs), Some(None));
        assert_eq!(in_git_dir(project.join("a.rs").as_std_path(), &dirs), None);
    }

    /// The watch actually fires, on the real backend, and marks the checkout the save was in —
    /// and stays quiet for a write it was told to ignore.
    #[cfg(any(target_os = "macos", windows))]
    #[tokio::test]
    async fn a_save_wakes_the_tree_watch_and_build_output_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let dirty: Dirty = Default::default();
        let wake = Arc::new(tokio::sync::Notify::new());
        let rescan = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = handler(
            &root,
            dirty.clone(),
            Default::default(),
            rescan,
            wake.clone(),
        );
        let _w = tree_watcher(&root, h).expect("a watcher");
        // FSEvents starts delivering a moment after the stream is created.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let quiet = std::time::Duration::from_millis(800);

        std::fs::write(root.join("target/out.o"), "x").unwrap();
        assert!(
            tokio::time::timeout(quiet, wake.notified()).await.is_err(),
            "build output woke the watch"
        );

        std::fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), wake.notified())
            .await
            .expect("a save wakes the watch");
        use crate::lock::Locked;
        assert!(dirty.locked().contains(&None));
    }

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

    /// Non-negotiable 11 for a session nobody has open: busy holds the tree, idle gives it back.
    #[test]
    fn an_unfollowed_busy_terminal_session_holds_its_tree() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, because `session_dir_checked` compares a canonicalised cwd against the
        // repository, and a temp dir on macOS is `/var/…` for `/private/var/…`.
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
        let state = AppState::new(repo.clone());
        let cwd = repo.as_str();

        hold_terminal_claims(&state, [("s-1", Some(cwd), true, true)].into_iter());
        assert!(
            state.claim("lane-a", &repo, true).is_err(),
            "the terminal holds the tree while it is busy"
        );
        let reader = state
            .claim("lane-r", &repo, false)
            .expect("a reader may sit beside it");
        state.release("lane-r", reader);

        hold_terminal_claims(&state, [("s-1", Some(cwd), true, false)].into_iter());
        let writer = state
            .claim("lane-a", &repo, true)
            .expect("idle gives it back");
        state.release("lane-a", writer);

        // A follower owns the claim; the watcher leaves it alone at idle.
        state.attach_follower("s-2");
        hold_terminal_claims(&state, [("s-2", Some(cwd), true, true)].into_iter());
        hold_terminal_claims(&state, [("s-2", Some(cwd), true, false)].into_iter());
        assert!(
            state.claim("lane-b", &repo, true).is_err(),
            "the follower will release it"
        );
        state.detach_follower("s-2");
        hold_terminal_claims(&state, [("s-2", Some(cwd), true, false)].into_iter());
        assert!(state.claim("lane-b", &repo, true).is_ok());
    }

    /// A `claude` killed mid-turn never says idle — its pid file goes and the session leaves the
    /// listing. The tree was held until the daemon restarted.
    #[test]
    fn a_terminal_session_that_died_gives_its_tree_back() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
        let state = AppState::new(repo.clone());
        hold_terminal_claims(
            &state,
            [("s-9", Some(repo.as_str()), true, true)].into_iter(),
        );
        assert!(state.claim("lane-a", &repo, true).is_err());
        // Dead but still listed.
        hold_terminal_claims(
            &state,
            [("s-9", Some(repo.as_str()), false, true)].into_iter(),
        );
        let w = state
            .claim("lane-a", &repo, true)
            .expect("a dead session gives it back");
        state.release("lane-a", w);
        // Busy again, then gone from the listing entirely.
        hold_terminal_claims(
            &state,
            [("s-9", Some(repo.as_str()), true, true)].into_iter(),
        );
        hold_terminal_claims(&state, std::iter::empty());
        assert!(
            state.claim("lane-a", &repo, true).is_ok(),
            "a vanished session gives it back"
        );
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
