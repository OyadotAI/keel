//! The local IDE server.
//!
//! A JSON and SSE API, and nothing else — the Swift app in `app/` is the only thing that draws it.
//! There was a web UI compiled in here, with Monaco alongside it; both are gone, and with them the
//! handlers that existed only to feed an editor (`/api/file`, `/api/file/original`, `/api/save`).
//!
//! Binds to loopback by default, and only leaves it when a device has been paired — see `pair.rs`,
//! which also owns the bearer token that anything off this machine has to carry. Keel reads the
//! developer's repositories, their Claude Code sessions and their cloud credentials, so reaching
//! any of that from another machine is a decision, never a default.

use crate::lock::Locked;
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::header,
    routing::get,
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

pub struct AppState {
    /// A `Mutex`, not an `RwLock`: every reader clones it immediately and holds it for
    /// nanoseconds, so there is no reader concurrency to win — and `lock::Locked` exists for
    /// `Mutex` alone, which is the rule that keeps a panic from bricking every later request.
    repo: std::sync::Mutex<Utf8PathBuf>,
    /// Whether `repo` is a project someone chose, or the empty stand-in used before one is open.
    open: std::sync::atomic::AtomicBool,
    /// The port this Keel is serving on. The approval hook is spawned by `claude`, in a separate
    /// process, and this is how it finds its way back.
    port: std::sync::atomic::AtomicU16,
    /// The turn each conversation is currently running, so Stop can signal it — and so that
    /// starting one can be refused.
    ///
    /// Keyed by conversation for the same reason the approval queue is: two lanes are two turns,
    /// and stopping one must not touch the other.
    running: std::sync::Mutex<std::collections::HashMap<String, Turn>>,
    /// Hands out a token per claim, so a turn that ends late releases its own slot and never a
    /// newer turn's.
    tokens: std::sync::atomic::AtomicU64,
    /// The last turn each lane ran, as `(session, turn key)`, kept past the claim's release so a
    /// fact that arrives after the turn — the manual "Run checks", a late design verdict — still
    /// has a turn to belong to.
    last_turn: std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
    /// How many tail streams are following each session. A followed session's terminal claim
    /// belongs to its follower, which finishes the turn; an unfollowed one's belongs to the
    /// sessions watcher, which only holds the tree.
    followers: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

/// One running turn: which checkout it is in, whether it can write to it, and what to signal.
///
/// The checkout and the flag are here because "one turn per lane" was never the whole invariant.
/// The other half — one *writer* per working tree — was kept in one window's Swift array, and a
/// window cannot see another window. Each daemon tracks its lanes here, and the checkout's
/// advisory lock coordinates writers across daemons used by detached windows.
struct Turn {
    token: u64,
    /// `None` until the child is spawned. The slot is reserved first so two requests arriving
    /// together cannot both get past the check.
    pid: Option<u32>,
    /// Stop also applies while the turn is preparing and has no child to signal yet.
    interrupted: bool,
    /// The child is reaped before checks/commit release the checkout claim. Never signal its
    /// old PID during that finishing phase; the OS can already have reused it.
    finished: bool,
    checkout: Utf8PathBuf,
    /// An advisory lock on the checkout directory's inode. Separate windows can use separate
    /// daemons, so the in-memory map alone cannot enforce one writer. Closing the descriptor
    /// releases the lock, including on panic or daemon exit; no stale lock file remains.
    _writer_lock: Option<CheckoutWriter>,
    /// False for a plan turn, which writes nothing and may sit beside one that does.
    writes: bool,
    /// The conversation the turn is in, once the provider has said (`system/init`).
    session: Option<String>,
    /// The `uuid` of the transcript record that opened the turn, once it has been read.
    key: Option<String>,
    /// Facts that arrived before the turn was keyed, with when they did.
    early: Vec<(String, crate::turns::Fact)>,
}

struct CheckoutWriter(std::fs::File);

impl Drop for CheckoutWriter {
    fn drop(&mut self) {
        // A concurrent fork can temporarily inherit our descriptor until exec closes it.
        // Explicit unlock releases this claim even while that inherited copy still exists.
        let _ = self.0.unlock();
    }
}

impl AppState {
    pub fn new(repo: Utf8PathBuf) -> Self {
        Self {
            repo: std::sync::Mutex::new(repo),
            open: std::sync::atomic::AtomicBool::new(true),
            port: std::sync::atomic::AtomicU16::new(7777),
            running: std::sync::Mutex::new(std::collections::HashMap::new()),
            tokens: std::sync::atomic::AtomicU64::new(1),
            last_turn: std::sync::Mutex::new(std::collections::HashMap::new()),
            followers: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Launched with nothing open — from the Dock, on a first run, or with a remembered project
    /// that has since moved.
    pub fn empty() -> Self {
        Self {
            repo: std::sync::Mutex::new(crate::prefs::no_project()),
            open: std::sync::atomic::AtomicBool::new(false),
            port: std::sync::atomic::AtomicU16::new(7777),
            running: std::sync::Mutex::new(std::collections::HashMap::new()),
            tokens: std::sync::atomic::AtomicU64::new(1),
            last_turn: std::sync::Mutex::new(std::collections::HashMap::new()),
            followers: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn project_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn port(&self) -> u16 {
        self.port.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The repository currently open. Cloned rather than borrowed so no handler holds the lock
    /// across an await point.
    pub fn repo(&self) -> Utf8PathBuf {
        self.repo.locked().clone()
    }

    /// The checkout a request is about: the project, or one of its lane worktrees.
    ///
    /// Permissions, trust and approvals never come through here — they are decisions about the
    /// repository and read `repo()` — so a lane cannot carry a different allowlist than its
    /// project, by construction rather than by care.
    pub fn checkout(&self, wt: Option<&str>) -> Result<Utf8PathBuf, String> {
        let root = self.repo();
        match wt.filter(|s| !s.is_empty()) {
            None => Ok(root),
            Some(name) => {
                let path = crate::worktree::path_of(&root, name)?;
                if path.is_dir() {
                    Ok(path)
                } else {
                    Err(format!(
                        "The feature “{name}” has no checkout in this project. Its worktree was \
                         removed, or it belongs to a project that was open before this one."
                    ))
                }
            }
        }
    }

    /// The directory a session was launched from, when it is not the repository: a parent
    /// (at most two up, never home) or a subdirectory. Anything else is the repository.
    pub fn session_dir(&self, cwd: Option<&str>) -> Utf8PathBuf {
        self.session_dir_checked(cwd).unwrap_or_else(|| self.repo())
    }

    /// The same question, with the refusal kept rather than swallowed.
    ///
    /// `None` means the session was started somewhere this project cannot reach — another
    /// project's checkout, most often, because History lists sessions from a shared parent
    /// directory and two repositories under `~/Dev` are each other's neighbours.
    ///
    /// Resuming one anyway is what `session_dir` used to do, quietly, by returning the current
    /// repository: the conversation came back holding a hundred paths in a directory the agent
    /// could no longer read, and every `Read` came back "you haven't granted permissions to read
    /// from …". Verified in a real transcript — one session id, `cwd` changing from one project's
    /// worktree to another's mid-file, and every tool call after the switch refused.
    pub fn session_dir_checked(&self, cwd: Option<&str>) -> Option<Utf8PathBuf> {
        let repo = self.repo();
        let Some(cwd) = cwd.filter(|c| !c.is_empty()).map(Utf8PathBuf::from) else {
            return Some(repo);
        };
        let Ok(cwd) = cwd.canonicalize_utf8() else {
            return Some(repo);
        };
        let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
        let allowed = keel_workspace::session_dirs(&repo, &home)
            .into_iter()
            .any(|(d, scope)| scope != "below" && d == cwd)
            || cwd.starts_with(&repo);
        allowed.then_some(cwd)
    }

    /// Resolve all ways a session can refer to the same working tree. A resumed session may
    /// run in a subdirectory or an allowed parent of the selected project; neither gets a
    /// separate writer slot. A linked worktree's own `.git` file is the nearest boundary.
    fn claim_checkout(&self, checkout: &Utf8Path) -> Result<Utf8PathBuf, String> {
        let mut checkout = checkout
            .canonicalize_utf8()
            .map_err(|e| format!("Could not open this working tree: {e}"))?;
        if let Ok(project) = self.repo().canonicalize_utf8()
            && project.starts_with(&checkout)
        {
            checkout = project;
        }
        for ancestor in checkout.ancestors() {
            if std::fs::symlink_metadata(ancestor.join(".git")).is_ok() {
                return Ok(ancestor.to_owned());
            }
        }
        Ok(checkout)
    }

    /// Take the lane, and the right to write its checkout, for one turn.
    ///
    /// Both refusals were previously guarded only in `SessionModel.start`, which is one window's
    /// view:
    ///
    /// * **Two turns in one lane** is two agents on one checkout with one of them invisible to
    ///   Stop — `running` held one pid per lane and the second `insert` simply overwrote the
    ///   first, so the first `claude` ran on with nothing left that could signal it.
    /// * **Two writers in one working tree** is the multi-agent pillar's load-bearing constraint.
    ///   Both of the things that end a turn are tree-wide — the auto-commit is `git add -A` and a
    ///   rewind restores the whole tree — so whichever finishes first sweeps the other's
    ///   half-written files into a commit labelled with the wrong prompt. Tearing a lane into its
    ///   own window is a first-class gesture here, and the moment it is used the Swift check is
    ///   looking at the wrong array.
    ///
    /// Returns the token that releases it.
    pub fn claim(&self, lane: &str, checkout: &Utf8Path, writes: bool) -> Result<u64, String> {
        let checkout = self.claim_checkout(checkout)?;
        let mut map = self.running.locked();
        if map.contains_key(lane) {
            return Err("This feature already has a turn running.".into());
        }
        if let Some((holder, _)) = map
            .iter()
            .find(|(_, t)| writes && t.writes && t.checkout == checkout)
        {
            // Who holds it changes what the person can do about it: a lane can be finished, a
            // terminal session goes idle on its own.
            let who = if holder.starts_with("term:") {
                "A session running in a terminal is editing this working tree right now."
            } else {
                "Another feature is already editing this working tree — in this window or in \
                 another one."
            };
            let wait = if holder.starts_with("term:") {
                "wait for that session to go idle"
            } else {
                "wait for the other to finish"
            };
            return Err(format!(
                "{who} Two agents writing one checkout commit each other's half-finished files, \
                 so this turn has not started. Give this one its own branch, or {wait}."
            ));
        }
        let writer_lock = if writes {
            let lock = std::fs::File::open(&checkout)
                .map_err(|e| format!("Could not reserve this working tree for editing: {e}"))?;
            lock.try_lock().map_err(|e| match e {
                std::fs::TryLockError::WouldBlock => {
                    "Another Keel window is already editing this working tree. Give this \
                     feature its own branch, or wait for the other turn and its checks to finish."
                        .to_string()
                }
                std::fs::TryLockError::Error(e) => {
                    format!("Could not reserve this working tree for editing: {e}")
                }
            })?;
            Some(CheckoutWriter(lock))
        } else {
            None
        };
        let token = self
            .tokens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        map.insert(
            lane.to_string(),
            Turn {
                token,
                pid: None,
                interrupted: false,
                finished: false,
                checkout,
                _writer_lock: writer_lock,
                writes,
                session: None,
                key: None,
                early: Vec::new(),
            },
        );
        Ok(token)
    }

    /// The conversation a lane's turn is in, once the provider has said.
    pub fn bound(&self, lane: &str, token: u64, session: &str) {
        if let Some(turn) = self.running.locked().get_mut(lane)
            && turn.token == token
        {
            turn.session = Some(session.to_string());
        }
    }

    /// The turn has a key: the `uuid` of the transcript record that opened it. Returns the facts
    /// that arrived before there was one, for the caller to write down.
    pub fn keyed(
        &self,
        lane: &str,
        token: u64,
        session: &str,
        key: &str,
    ) -> Vec<(String, crate::turns::Fact)> {
        let mut map = self.running.locked();
        let Some(turn) = map.get_mut(lane).filter(|t| t.token == token) else {
            return Vec::new();
        };
        turn.session = Some(session.to_string());
        turn.key = Some(key.to_string());
        self.last_turn
            .locked()
            .insert(lane.to_string(), (session.to_string(), key.to_string()));
        std::mem::take(&mut turn.early)
    }

    /// `(session, turn)` for a lane: its running turn once keyed, else the last turn it ran.
    pub fn turn_key(&self, lane: &str) -> Option<(String, String)> {
        if let Some(turn) = self.running.locked().get(lane)
            && let (Some(session), Some(key)) = (&turn.session, &turn.key)
        {
            return Some((session.clone(), key.clone()));
        }
        self.last_turn.locked().get(lane).cloned()
    }

    /// Hold a fact for a turn that has no key yet. False when the lane has no turn running, or
    /// holds as many as it may.
    pub fn hold_early(&self, lane: &str, fact: crate::turns::Fact) -> bool {
        let mut map = self.running.locked();
        match map.get_mut(lane) {
            Some(turn) if turn.key.is_none() && turn.early.len() < crate::turns::MAX_EARLY => {
                turn.early.push((crate::turns::now(), fact));
                true
            }
            _ => false,
        }
    }

    /// A terminal's claim on the tree its session runs in, under `term:<session>`. Idempotent:
    /// the turn after the first finds its claim already held and gets the same token.
    pub fn claim_terminal(&self, session: &str, checkout: &Utf8Path) -> Result<u64, String> {
        let lane = crate::turns::terminal_lane(session);
        if let Some(turn) = self.running.locked().get(&lane) {
            return Ok(turn.token);
        }
        let token = self.claim(&lane, checkout, true)?;
        if let Some(turn) = self.running.locked().get_mut(&lane) {
            turn.session = Some(session.to_string());
        }
        Ok(token)
    }

    pub fn attach_follower(&self, session: &str) {
        *self
            .followers
            .locked()
            .entry(session.to_string())
            .or_insert(0) += 1;
    }

    pub fn detach_follower(&self, session: &str) {
        let mut map = self.followers.locked();
        if let Some(n) = map.get_mut(session) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(session);
            }
        }
    }

    pub fn is_followed(&self, session: &str) -> bool {
        self.followers.locked().get(session).is_some_and(|n| *n > 0)
    }

    /// Give back a terminal session's claim, unless a follower holds it: the follower finishes
    /// the turn and releases it itself.
    pub fn release_terminal_unfollowed(&self, session: &str) {
        if self.is_followed(session) {
            return;
        }
        self.running
            .locked()
            .remove(&crate::turns::terminal_lane(session));
    }

    /// Whether a lane in some window is driving this session. A follower of such a session
    /// forwards what it sees and runs no lifecycle of its own: the lane's chat task owns it.
    pub fn owns_session(&self, session: &str) -> bool {
        self.running
            .locked()
            .iter()
            .any(|(lane, t)| !lane.starts_with("term:") && t.session.as_deref() == Some(session))
    }

    /// Whether a turn *other than this session's* is allowed to write this checkout. Read by
    /// the automatic commit, which holds its own claim and must not refuse itself.
    pub fn writer_in_other(&self, checkout: &Utf8Path, session: &str) -> bool {
        let checkout = self
            .claim_checkout(checkout)
            .unwrap_or_else(|_| checkout.to_owned());
        self.running
            .locked()
            .values()
            .any(|t| t.writes && t.checkout == checkout && t.session.as_deref() != Some(session))
    }

    /// Record the process, so Stop has something to signal.
    pub fn started(&self, lane: &str, token: u64, pid: u32) {
        if let Some(turn) = self.running.locked().get_mut(lane)
            && turn.token == token
        {
            turn.pid = Some(pid);
            // Close the race between the final cancellation check and registering the child.
            if turn.interrupted {
                crate::signals::group(pid, libc::SIGINT);
            }
        }
    }

    /// A stopped or superseded preparation must never start another agent.
    pub fn cancelled(&self, lane: &str, token: u64) -> bool {
        self.running
            .locked()
            .get(lane)
            .is_none_or(|turn| turn.token != token || turn.interrupted || turn.finished)
    }

    /// Forget the reaped child while retaining exclusive access for checks and auto-commit.
    pub fn finished(&self, lane: &str, token: u64) {
        if let Some(turn) = self.running.locked().get_mut(lane)
            && turn.token == token
        {
            turn.pid = None;
            turn.finished = true;
        }
    }

    /// Give the lane back, but only if it is still ours: a turn that ends after the next one has
    /// started must not release the new turn's slot.
    pub fn release(&self, lane: &str, token: u64) {
        let mut map = self.running.locked();
        if map.get(lane).map(|t| t.token) == Some(token) {
            map.remove(lane);
        }
    }

    /// Whether any turn is currently allowed to write this checkout.
    ///
    /// Read by the auto-commit, which is `git add -A` in the whole tree: committing while another
    /// lane's agent is mid-write is how one turn's commit comes to hold another turn's
    /// half-finished files under the wrong message.
    #[cfg(test)]
    pub fn writer_in(&self, checkout: &Utf8Path) -> bool {
        let checkout = self
            .claim_checkout(checkout)
            .unwrap_or_else(|_| checkout.to_owned());
        self.running
            .locked()
            .values()
            .any(|t| t.writes && t.checkout == checkout)
    }

    /// Stop the turn in one conversation, the way ⌃C would.
    ///
    /// SIGINT, not SIGTERM and not SIGKILL: `claude` handles an interrupt by finishing the turn it
    /// is in and writing its transcript. SIGTERM abandons the turn, and the transcript is the whole
    /// record of what the agent did — losing it is losing the evidence Keel exists to show.
    /// Returns whether there was a running or preparing turn to stop.
    pub fn interrupt(&self, lane: &str) -> bool {
        let mut map = self.running.locked();
        let Some(turn) = map.get_mut(lane).filter(|turn| !turn.finished) else {
            return false;
        };
        turn.interrupted = true;
        // The group, not the process: `claude` spawns the tools it runs, and a bare `kill(pid)`
        // leaves a `cargo test` it started alive and holding the terminal. See `crate::signals`.
        if let Some(pid) = turn.pid {
            crate::signals::group(pid, libc::SIGINT);
        }
        true
    }

    pub fn set_repo(&self, path: Utf8PathBuf) {
        crate::prefs::Prefs::remember(&path);
        *self.repo.locked() = path;
        self.open.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The checkout a request names with `?wt=<lane>`, or the project when it names none.
///
/// One extractor rather than a `wt` field on every query struct: the handlers that take a
/// checkout are the ones that read or change files, and they all resolve it the same way.
pub struct Checkout(pub Utf8PathBuf);

impl axum::extract::FromRequestParts<Arc<AppState>> for Checkout {
    type Rejection = (axum::http::StatusCode, String);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let wt = parts
            .uri
            .query()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("wt=")));
        state
            .checkout(wt)
            .map(Checkout)
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
    }
}

/// A claim that is given back however the turn ends.
///
/// The turn task has five ways out — a refusal, a failed spawn, a hung-up client, the child
/// exiting, a panic — and releasing at each of them is a list that only has to be wrong once. A
/// lane left claimed can never take another turn: the window looks idle and every send is
/// refused, which is the "never stuck" failure with the worst shape, because nothing on screen
/// says what is holding it.
pub struct Held {
    state: Arc<AppState>,
    lane: String,
    token: u64,
}

impl Held {
    pub fn new(state: Arc<AppState>, lane: String, token: u64) -> Self {
        Self { state, lane, token }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.state.release(&self.lane, self.token);
    }
}

/// Everything the UI needs, in one request.
///
/// A single round trip rather than four keeps the first paint honest: the page never renders a
/// half-populated view where one pane has loaded and another has not.
#[derive(Serialize)]
struct StateResponse {
    repo: String,
    /// `false` until a project is opened. The UI shows the welcome flow instead of the IDE, and
    /// asks for nothing that needs a repository until there is one.
    project_open: bool,
    /// Whether the welcome flow has ever been completed on this machine.
    onboarded: bool,
    scan: keel_scanner::Report,
    workspace: keel_workspace::Workspace,
    policy: crate::policy::Policy,
}

pub async fn run(repo: Utf8PathBuf, port: u16) -> Result<()> {
    serve(AppState::new(repo), port).await
}

/// Start with whatever was open last, or with nothing.
///
/// This is how the application is launched from the Dock, where there is no working directory to
/// infer a project from — the Finder hands a process `/` and it would be a strange thing to open.
pub async fn run_app(port: u16) -> Result<()> {
    let state = match crate::prefs::Prefs::load().resume() {
        Some(project) => AppState::new(project),
        None => AppState::empty(),
    };
    serve(state, port).await
}

/// Whether a Keel is already answering on this port.
///
/// Asked before binding rather than after failing: something else on 7777 is a different problem
/// from a second launch, and the two deserve different messages.
fn already_running(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    // A hand-written request rather than an HTTP client: this runs before the server exists and
    // asks one question of one loopback port. A dependency for that would be the larger cost.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(700)));
    if sock
        .write_all(b"GET /api/state HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }

    let mut buf = [0u8; 2048];
    let mut seen = Vec::new();
    while let Ok(n) = sock.read(&mut buf) {
        if n == 0 {
            break;
        }
        seen.extend_from_slice(&buf[..n]);
        // Enough to tell Keel from whatever else might hold the port. The field is Keel's own.
        if seen.len() > 8192 {
            break;
        }
    }
    let body = String::from_utf8_lossy(&seen);
    body.starts_with("HTTP/1.") && body.contains("project_open")
}

// Nothing here opens a window any more, and nothing should. The page these used to open —
// `Launch`, `open_ui` and a list of Chromium bundles to run in app mode — was deleted with `ui/`,
// but the code that navigated to it was kept, so `keel serve` greeted you with a 404 and `keel
// app` did the same inside a WKWebView. The daemon is an HTTP surface; the window is the Mac app.

async fn serve(state: AppState, port: u16) -> Result<()> {
    state.port.store(port, std::sync::atomic::Ordering::Relaxed);
    let url = format!("http://127.0.0.1:{port}");

    // Launching a second time from the Dock must raise the window that is already open, not fail
    // with "address in use" behind an icon that then does nothing.
    if already_running(port) {
        println!("\n  Keel is already running — {url}\n");
        return Ok(());
    }

    let state = Arc::new(state);

    let app = Router::new()
        .route("/api/state", get(api_state))
        .route("/api/events", get(crate::events::stream))
        .route("/api/sessions", get(api_sessions))
        .route("/api/tree", get(api_tree))
        .route("/api/frontend/importers", get(api_importers))
        .route("/api/session/tail", get(api_session_tail))
        .route(
            "/api/session/rename",
            axum::routing::post(crate::names::rename),
        )
        .route("/api/raw", get(api_raw))
        .route("/api/chat", get(crate::agent::chat))
        .route("/api/chat/stop", axum::routing::post(crate::agent::stop))
        .route(
            "/api/attach",
            axum::routing::post(crate::agent::attach)
                // No route overrides axum's 2 MB default, which a phone screenshot clears easily.
                // Scoped to this route: the limit exists for attachments, not for every handler.
                .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .route("/api/git/status", get(api_git_status))
        .route("/api/git/diff", get(api_git_diff))
        .route("/api/git/act", axum::routing::post(api_git_act))
        .route("/api/git/init", axum::routing::post(api_git_init))
        .route("/api/git/log", get(api_git_log))
        .route("/api/git/commit/diff", get(api_git_commit_diff))
        .route("/api/git/branches", get(api_git_branches))
        .route("/api/git/branch", axum::routing::post(api_git_branch))
        .route("/api/git/remote", axum::routing::post(api_git_remote))
        .route(
            "/mcp",
            axum::routing::post(crate::askmcp::post).get(crate::askmcp::get),
        )
        .route(
            "/api/memory",
            axum::routing::post(crate::review::api_memory),
        )
        .route("/api/readiness/ignore", axum::routing::post(api_ignore))
        .route("/api/git/ignore", axum::routing::post(api_git_ignore))
        .route("/api/review", get(crate::review::api_review))
        .route(
            "/api/review/save",
            axum::routing::post(crate::review::api_review_save),
        )
        .route("/api/adopt", axum::routing::post(crate::review::api_adopt))
        .route("/api/git/stage-all", axum::routing::post(api_git_stage_all))
        .route(
            "/api/git/discard-all",
            axum::routing::post(api_git_discard_all),
        )
        .route(
            "/api/git/commit-staged",
            axum::routing::post(api_git_commit_staged),
        )
        .route("/api/git/push", axum::routing::post(api_git_push))
        .route("/api/git/uncommit", axum::routing::post(api_git_uncommit))
        .route(
            "/api/git/commit",
            axum::routing::post(crate::worktree::api_commit),
        )
        .route("/api/worktree", get(crate::worktree::api_list))
        .route(
            "/api/worktree/create",
            axum::routing::post(crate::worktree::api_create),
        )
        .route(
            "/api/worktree/finish",
            axum::routing::post(crate::worktree::api_finish),
        )
        .route(
            "/api/worktree/discard",
            axum::routing::post(crate::worktree::api_discard),
        )
        .route(
            "/api/git/restore",
            axum::routing::post(crate::snapshot::put_back),
        )
        .route("/api/turns", axum::routing::post(crate::turns::record))
        .route("/api/permissions", get(crate::permissions::list))
        .route(
            "/api/permissions/trust",
            axum::routing::post(crate::permissions::trust),
        )
        .route(
            "/api/permissions/add",
            axum::routing::post(crate::permissions::add),
        )
        .route(
            "/api/permissions/remove",
            axum::routing::post(crate::permissions::remove),
        )
        .route("/api/fs/create", axum::routing::post(crate::fsops::create))
        .route("/api/fs/rename", axum::routing::post(crate::fsops::rename))
        .route("/api/fs/stat", axum::routing::post(crate::fsops::stat))
        .route("/api/fs/delete", axum::routing::post(crate::fsops::delete))
        .route("/api/fs/reveal", axum::routing::post(crate::fsops::reveal))
        .route("/api/term/ws", get(crate::term::ws))
        .route("/api/monitors", get(crate::monitor::api_list))
        .route(
            "/api/monitors/stop",
            axum::routing::post(crate::monitor::api_stop),
        )
        .route(
            "/api/monitors/ack",
            axum::routing::post(crate::monitor::api_ack),
        )
        .route("/api/dev", get(crate::dev::status))
        .route("/api/dev/start", axum::routing::post(crate::dev::start))
        .route("/api/dev/stop", axum::routing::post(crate::dev::stop))
        .route("/api/verify", get(crate::verify::run))
        .route("/api/verify/plan", get(crate::verify::plan))
        .route("/api/connections", get(crate::connect::status))
        .route(
            "/api/connect/github",
            axum::routing::post(crate::connect::connect_github),
        )
        .route(
            "/api/connect/cloudflare",
            axum::routing::post(crate::connect::connect_cloudflare),
        )
        .route(
            "/api/disconnect",
            axum::routing::post(crate::connect::disconnect),
        )
        .route("/api/github/repos", get(crate::connect::repos))
        .route("/api/github/pr", get(crate::pr::create))
        .route("/api/plugins", get(crate::plugins::list))
        .route("/api/plugins/install", get(crate::plugins::install))
        .route("/api/plugins/action", get(crate::plugins::action))
        .route("/api/plugins/details", get(crate::plugins::details))
        .route(
            "/api/plugins/marketplaces",
            get(crate::plugins::marketplaces),
        )
        .route(
            "/api/plugins/refresh",
            get(crate::plugins::refresh_marketplaces),
        )
        .route("/api/approve/ask", axum::routing::post(crate::approve::ask))
        .route("/api/approve/poll", get(crate::approve::poll))
        .route(
            "/api/approve/answer",
            axum::routing::post(crate::approve::answer),
        )
        .route(
            "/api/agents/create",
            axum::routing::post(crate::agents::create),
        )
        .route("/api/mcp/add", get(crate::mcp::add))
        .route("/api/mcp/remove", get(crate::mcp::remove))
        .route(
            "/api/aws/sso",
            axum::routing::post(crate::aws::configure_sso),
        )
        .route("/api/open-url", axum::routing::post(crate::fsops::open_url))
        .route("/api/claude", get(crate::clitools::claude_status))
        .route("/api/claude/install", get(crate::clitools::install_claude))
        .route("/api/claude/login", get(crate::clitools::login_claude))
        .route("/api/cli", get(crate::clitools::status))
        .route("/api/cli/install", get(crate::clitools::install))
        .route("/api/cli/install-all", get(crate::clitools::install_all))
        .route("/api/cli/login", get(crate::clitools::login))
        .route(
            "/api/github/clone",
            axum::routing::post(crate::connect::clone),
        )
        .route("/api/open", axum::routing::post(crate::connect::open_repo))
        .route("/api/browse", get(crate::connect::browse))
        .route("/api/browse-files", get(crate::connect::browse_files))
        .route("/api/pair/begin", axum::routing::post(crate::pair::begin))
        .route(
            "/api/pair/complete",
            axum::routing::post(crate::pair::complete),
        )
        .route("/api/pair/devices", get(crate::pair::devices))
        .route(
            "/api/pair/devices/{id}",
            axum::routing::delete(crate::pair::revoke),
        )
        .route(
            "/api/project/new",
            axum::routing::post(crate::project::create),
        )
        // Every request passes this, and for the loopback callers that are the only ones today
        // it is one `is_loopback()` and nothing else.
        .layer(axum::middleware::from_fn(crate::pair::guard))
        .layer(axum::middleware::from_fn(report_failures))
        .layer(axum::middleware::from_fn(crate::events::after_mutation))
        .with_state(state.clone());

    // What happens outside Keel — a session started in a terminal, a file saved in an editor —
    // reaches the window through these.
    crate::events::watch(state);

    let (ip, reach) = bind_address();
    let addr = SocketAddr::from((ip, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr} (is another keel already running?)"))?;

    println!("\n  Keel — {url}{reach}\n  Ctrl-C to stop\n");

    // `ConnectInfo` is what lets the guard tell a loopback caller from a stranger. Without it the
    // guard cannot answer its only question, so this is not an optional flourish.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("serving")?;
    Ok(())
}

/// Every failure the daemon returns, reported once, from one place.
///
/// Sixty handlers each returned `(StatusCode, String)` to the app and to nobody else, so the only
/// evidence that a tester's Keel was failing was the tester saying so — and by then they have
/// stopped trusting it. This is the cheapest possible fix for that: one layer, every route.
///
/// The matched route *pattern* travels, never the concrete path — `/api/pair/devices/{id}` would
/// otherwise carry a device id — and the body never travels at all, because a handler's message
/// quotes paths, branch names and the person's own text.
async fn report_failures(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // No `MatchedPath` means no such route — a probe, a favicon, a stale client, a typo in a
    // curl. That is the caller being wrong, not Keel failing, and reporting it means anything
    // that touches the port fills the issue list with 404s nobody can act on.
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str().to_string());
    let response = next.run(req).await;
    let status = response.status();
    if let (Some(route), true) = (route, status.is_client_error() || status.is_server_error()) {
        sentry::with_scope(
            |scope| {
                scope.set_tag("route", &route);
                scope.set_tag("status", status.as_u16().to_string());
            },
            || {
                sentry::capture_message(
                    &format!("{route} failed with {}", status.as_u16()),
                    if status.is_server_error() {
                        sentry::Level::Error
                    } else {
                        sentry::Level::Warning
                    },
                )
            },
        );
    }
    response
}

/// Where to listen, and what to say about it.
///
/// Off-loopback requires a paired device. The check is here rather than in the settings UI because
/// this is the last point before a socket exists: a hand-edited `state.json`, a stale file copied
/// between machines, or a future caller that forgets to ask all arrive here, and all of them must
/// end up on 127.0.0.1 rather than on the network.
fn bind_address() -> (Ipv4Addr, String) {
    let mode = crate::prefs::Prefs::load().bind.unwrap_or_default();
    if mode.is_empty() || mode == "loopback" {
        return (Ipv4Addr::LOCALHOST, String::new());
    }
    if !crate::pair::any_paired() {
        println!("\n  Not listening beyond this machine: no device is paired yet.");
        return (Ipv4Addr::LOCALHOST, String::new());
    }
    match mode.as_str() {
        "lan" => (
            Ipv4Addr::UNSPECIFIED,
            "  ·  reachable on this network".into(),
        ),
        "tailscale" => match crate::pair::tailscale_ip() {
            Some(ip) => (ip, format!("  ·  reachable at {ip} over Tailscale")),
            None => {
                println!("\n  Tailscale is not up, so Keel is listening on this machine only.");
                (Ipv4Addr::LOCALHOST, String::new())
            }
        },
        _ => (Ipv4Addr::LOCALHOST, String::new()),
    }
}

/// Walking a repository is filesystem work, not async work.
///
/// Every handler here used to do its work directly on the executor. A `git clone` of a real
/// repository takes seconds to minutes, and for all of it Keel served nothing at all — reported as
/// the screen freezing after cloning a project. The same was true, less dramatically, of walking a
/// large tree or running the scanner.
async fn api_tree(Checkout(repo): Checkout) -> Json<Vec<crate::tree::Node>> {
    Json(blocking(move || crate::tree::tree(&repo), Vec::new()).await)
}

#[derive(serde::Deserialize)]
struct ImportersQuery {
    file: String,
}

async fn api_importers(
    Checkout(repo): Checkout,
    Query(query): Query<ImportersQuery>,
) -> Json<Vec<String>> {
    Json(
        blocking(
            move || crate::imports::importers_of(&repo, &query.file),
            Vec::new(),
        )
        .await,
    )
}

/// Run blocking work off the executor, so one slow call cannot stall the whole server.
///
/// The caller supplies what to return if the task panics, rather than the helper requiring
/// `Default` — a scan report has no meaningful empty value, and inventing one to satisfy a
/// signature is the wrong way round.
pub(crate) async fn blocking<T, F>(work: F, fallback: T) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await.unwrap_or(fallback)
}

async fn api_raw(
    Checkout(repo): Checkout,
    Query(query): Query<crate::tree::FileQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Whatever the person clicked on: a screenshot, a PDF, a video the agent put in the tree.
    // Reading it is a blocking read of an arbitrary number of bytes.
    let read = blocking(
        move || crate::tree::read_raw(&repo, &query.path),
        Err("could not read that file".to_string()),
    )
    .await;
    match read {
        Ok((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct TailQuery {
    id: String,
    /// The directory the session was launched from, when not the repository.
    cwd: Option<String>,
    /// Where the last read stopped. `0` for the whole transcript.
    #[serde(default)]
    from: u64,
}

/// Follow one session's transcript, live.
///
/// This is what makes a session running somewhere else — a terminal, another editor — visible
/// here. Claude Code appends every record to `~/.claude/projects/<key>/<id>.jsonl` as it goes,
/// whoever started it, so the file is already the live feed; nothing was reading it as one.
///
/// Read-only, and deliberately so: two processes driving one conversation is a claim problem, and
/// `AppState::claim` is about lanes and working trees rather than sessions.
///
/// The events are the **same `msg` events `/api/chat` emits**, because the records are the same
/// shape the app's decoder already reads. That is the whole reason this is cheap: replaying a
/// session and following one become one path, and it is the path that has always drawn a live
/// turn — so a reopened conversation gets its reasoning, its tool arguments and its raw lines
/// back, none of which the two-endpoint replay ever carried.
///
/// Polled rather than watched. `notify` would be a dependency and a platform-specific one, and
/// what it would buy over a `stat` every 400 ms is latency nobody can see.
async fn api_session_tail(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Query(query): Query<TailQuery>,
) -> impl axum::response::IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};

    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    let dir = if query.cwd.is_some() {
        state.session_dir(query.cwd.as_deref())
    } else {
        repo
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(256);
    let store = state.repo();

    // Facts as they are emitted about this session, from whoever is driving it: a lane in this
    // window or another, or the daemon itself. Ends when the poll below ends — `stop` is dropped
    // with it — or when the window goes.
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    {
        let tx = tx.clone();
        let session = query.id.clone();
        tokio::spawn(async move {
            let mut facts = crate::turns::subscribe(&session);
            loop {
                let emitted = tokio::select! {
                    _ = &mut stopped => return,
                    e = facts.recv() => match e {
                        Ok(e) => e,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    },
                };
                let Ok(json) = serde_json::to_string(&*emitted) else {
                    continue;
                };
                if tx
                    .send(Ok(Event::default().event("fact").data(json)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
    }

    let watched = state.clone();
    tokio::spawn(async move {
        let _stop = stop;
        let mut from = query.from;
        // The first read is the catch-up; every one after it is the few hundred bytes that were
        // appended.
        let mut first = true;
        // What Keel knew about these turns, keyed by the record that opened each. Sent right
        // after that record on the first pass, so a replayed turn carries its files, gate, commit
        // and cost exactly where a live one would have.
        let known: std::collections::HashMap<String, crate::turns::Record> = {
            let (store, id) = (store.clone(), query.id.clone());
            blocking(
                move || {
                    crate::turns::read(&store, &id)
                        .into_iter()
                        .map(|r| (r.turn.clone(), r))
                        .collect()
                },
                std::collections::HashMap::new(),
            )
            .await
        };
        // Woken by the file rather than by a clock. kqueue on the transcript and on Claude
        // Code's pid files; the sleep below is the fallback for a watcher that could not be made.
        let (wake_tx, mut wake) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = watch_session(&dir, &home, &query.id, wake_tx);
        let mut follower: Option<crate::turns::Follower> = None;
        // The turn in flight when the window arrives: the last prompt of the replay with no
        // turn-end after it. A follower that starts at catch-up would otherwise never see that
        // turn open, and a window relaunched mid-turn — every `make dev` — lost the turn's
        // files, gate and commit for good.
        let mut inflight: Option<(String, String, Option<String>)> = None;
        let mut alive = keel_workspace::status(&home, &query.id).is_some();
        let mut last_bytes = std::time::Instant::now();
        let mut said_ended = false;
        loop {
            if tx.is_closed() {
                break;
            }
            let (d, h, sid) = (dir.clone(), home.clone(), query.id.clone());
            // Every read is off the executor, and bounded: the first is the tail of the file,
            // not the file, and none takes more than `MAX_READ` in one go.
            let read: Option<(Vec<String>, u64, u64)> = if first && from == 0 {
                blocking(
                    move || keel_workspace::tail_last(&d, &h, &sid, keel_workspace::MAX_READ),
                    None,
                )
                .await
            } else {
                blocking(
                    move || keel_workspace::tail(&d, &h, &sid, from).map(|(l, n)| (l, n, 0)),
                    None,
                )
                .await
            };
            let Some((lines, next, started_at)) = read else {
                let _ = tx
                    .send(Ok(Event::default()
                        .event("fatal")
                        .data("that session's transcript is not on this machine")))
                    .await;
                break;
            };

            // A very long history sent whole is a wall of records the app has to decode on its
            // main actor before it can draw anything. The tail is what a person opening a
            // conversation is looking for, so the head is dropped and the app is told.
            let (lines, dropped) = if first && lines.len() > MAX_REPLAY {
                let dropped = lines.len() - MAX_REPLAY;
                (lines[dropped..].to_vec(), dropped)
            } else {
                (lines, 0)
            };
            if first && (dropped > 0 || started_at > 0) {
                let _ = tx
                    .send(Ok(Event::default().event("truncated").data(
                        serde_json::json!({ "records": dropped, "bytes": started_at }).to_string(),
                    )))
                    .await;
            }
            let mut wrote = false;
            for line in lines {
                wrote = true;
                let kind = keel_workspace::kind_of(&line);
                let opener = if first && !known.is_empty() {
                    keel_workspace::opener_of(&line)
                } else {
                    None
                };
                // A task notification is Claude Code's note to itself. It is read here, as a
                // job finishing, and never drawn: drawn, it was a blue bubble attributed to the
                // person that split the turn.
                if !matches!(kind, keel_workspace::Kind::JobDone { .. })
                    && tx
                        .send(Ok(Event::default().event("msg").data(line)))
                        .await
                        .is_err()
                {
                    break; // The window closed, or the lane opened something else.
                }
                if let Some(record) = opener.and_then(|uuid| known.get(&uuid)) {
                    for fact in record.facts() {
                        let Ok(json) = serde_json::to_string(&fact) else {
                            continue;
                        };
                        if tx
                            .send(Ok(Event::default().event("fact").data(json)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                if follower.is_none() {
                    match &kind {
                        keel_workspace::Kind::Opener {
                            uuid, prompt, cwd, ..
                        } => inflight = Some((uuid.clone(), prompt.clone(), cwd.clone())),
                        keel_workspace::Kind::TurnEnd => inflight = None,
                        _ => {}
                    }
                }
                if let Some(f) = &mut follower {
                    match kind {
                        keel_workspace::Kind::Opener {
                            uuid, prompt, cwd, ..
                        } => f.opened(uuid, prompt, cwd).await,
                        keel_workspace::Kind::TurnEnd => f.turn_ended(),
                        keel_workspace::Kind::JobStarted { id, command } => {
                            f.job(crate::turns::Fact::JobStarted { id, command }).await
                        }
                        keel_workspace::Kind::JobDone { id } => {
                            f.job(crate::turns::Fact::JobDone { id }).await
                        }
                        keel_workspace::Kind::Other => {}
                    }
                    f.wrote();
                }
            }
            if tx.is_closed() {
                break;
            }
            if wrote {
                last_bytes = std::time::Instant::now();
                said_ended = false;
            }
            if first {
                first = false;
                // "Everything that already happened has been sent." Turns before this are a
                // replay; turns after it are the session running now.
                // The offset, not an empty string — axum writes no `data:` line at all for an
                // empty payload, which makes the frame a name with nothing under it and leaves
                // every client's parser to decide whether that is an event. Somewhere useful to
                // resume from is better than nothing to say.
                if tx
                    .send(Ok(Event::default()
                        .event("caught-up")
                        .data(next.to_string())))
                    .await
                    .is_err()
                {
                    break;
                }
                // From here, a session nobody in Keel is driving gets the lifecycle a lane's
                // turn gets. One a lane is driving is that lane's chat task's business.
                if !watched.owns_session(&query.id) {
                    watched.attach_follower(&query.id);
                    let mut f = crate::turns::Follower::new(
                        watched.clone(),
                        store.clone(),
                        home.clone(),
                        query.id.clone(),
                    );
                    // Busy, with a prompt nothing has closed: that turn is running now, and it
                    // is this follower's from here. The photograph is late — the turn may have
                    // written already — which beats no record of the turn at all.
                    if let Some((uuid, prompt, cwd)) = inflight.take()
                        && matches!(
                            keel_workspace::status(&home, &query.id),
                            Some(keel_workspace::Status::Busy | keel_workspace::Status::Unknown)
                        )
                    {
                        f.opened(uuid, prompt, cwd).await;
                    }
                    follower = Some(f);
                }
            }
            from = next;
            if let Some(f) = &mut follower {
                f.tick().await;
            }
            // The process went away and the file went quiet: the session is over, and the app
            // is told so a turn cut short by ⌃C in the terminal does not stay "running".
            let now_alive = keel_workspace::status(&home, &query.id).is_some();
            if alive
                && !now_alive
                && !said_ended
                && last_bytes.elapsed() >= std::time::Duration::from_secs(5)
            {
                if let Some(f) = &mut follower {
                    f.close().await;
                }
                let _ = tx
                    .send(Ok(Event::default().event("ended").data("gone")))
                    .await;
                said_ended = true;
            }
            alive = now_alive;
            // The stream stays open for a session that is not being written: waiting on the
            // watcher costs nothing, and a session resumed in a terminal later is picked up here.
            let fallback = std::time::Duration::from_secs(if alive { 2 } else { 5 });
            tokio::select! {
                _ = tx.closed() => break,
                _ = wake.recv() => {}
                _ = tokio::time::sleep(fallback) => {}
            }
        }
        if let Some(f) = &mut follower {
            f.close().await;
            watched.detach_follower(&query.id);
        }
    });

    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// Records sent when catching up on a conversation that was already long.
const MAX_REPLAY: usize = 1_500;

/// Watch a session's transcript and Claude Code's pid files, waking `wake` on any change.
///
/// kqueue, per file: instant, and never a recursive walk. `None` when a watcher cannot be made,
/// in which case the poll's fallback sleep is the whole of it.
fn watch_session(
    dir: &Utf8Path,
    home: &Utf8Path,
    id: &str,
    wake: tokio::sync::mpsc::Sender<()>,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let mut watcher =
        notify::recommended_watcher(move |_: Result<notify::Event, notify::Error>| {
            // A full slot means a wake is already pending, which is the same thing.
            let _ = wake.try_send(());
        })
        .ok()?;
    if let Some(path) = keel_workspace::transcript_path(dir, home, id) {
        let _ = watcher.watch(path.as_std_path(), notify::RecursiveMode::NonRecursive);
    }
    let _ = watcher.watch(
        home.join("sessions").as_std_path(),
        notify::RecursiveMode::NonRecursive,
    );
    Some(watcher)
}

async fn api_git_status(Checkout(repo): Checkout) -> Json<crate::repo::GitStatus> {
    Json(blocking(move || crate::repo::git_status(&repo), Default::default()).await)
}

async fn api_git_diff(
    Checkout(repo): Checkout,
    Query(query): Query<crate::tree::FileQuery>,
) -> Json<crate::repo::DiffResponse> {
    Json(
        blocking(
            move || crate::repo::git_diff(&repo, &query.path),
            Default::default(),
        )
        .await,
    )
}

#[derive(serde::Deserialize)]
struct GitActRequest {
    action: String,
    path: String,
    /// For `discard-hunk`: which `@@` block, counting from zero.
    hunk: Option<usize>,
}

/// Stage, unstage or discard one file, from the Changes panel.
/// `git init`, for a project that is not one yet.
#[derive(serde::Deserialize)]
struct LogQuery {
    #[serde(default = "twenty")]
    n: usize,
}
fn twenty() -> usize {
    20
}

async fn api_git_log(
    Checkout(repo): Checkout,
    Query(q): Query<LogQuery>,
) -> Json<Vec<crate::repo::Commit>> {
    Json(
        blocking(
            move || crate::repo::git_log(&repo, q.n.min(100)),
            Vec::new(),
        )
        .await,
    )
}

async fn api_git_branches(Checkout(repo): Checkout) -> Json<crate::repo::Branches> {
    Json(
        blocking(
            move || crate::repo::git_branches(&repo),
            crate::repo::Branches {
                current: None,
                local: Vec::new(),
                remote: Vec::new(),
                remotes: Vec::new(),
                staged: 0,
                unstaged: 0,
            },
        )
        .await,
    )
}

#[derive(serde::Deserialize)]
struct BranchBody {
    action: String,
    name: String,
}

async fn api_git_branch(
    Checkout(repo): Checkout,
    Json(b): Json<BranchBody>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_branch_act(&repo, &b.action, &b.name),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(serde::Deserialize)]
struct RemoteBody {
    action: String,
    url: Option<String>,
}

async fn api_git_remote(
    Checkout(repo): Checkout,
    Json(b): Json<RemoteBody>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_remote_act(&repo, &b.action, b.url.as_deref()),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_discard_all(
    Checkout(repo): Checkout,
) -> Result<Json<(u32, u32)>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_discard_all(&repo),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(serde::Deserialize)]
struct StageAllBody {
    stage: bool,
}

async fn api_git_stage_all(
    Checkout(repo): Checkout,
    Json(b): Json<StageAllBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_stage_all(&repo, b.stage),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_commit_staged(
    Checkout(repo): Checkout,
    Json(b): Json<crate::worktree::CommitBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_commit_staged(&repo, &b.message),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(serde::Deserialize)]
struct ShaQuery {
    sha: String,
}

async fn api_git_commit_diff(
    Checkout(repo): Checkout,
    Query(q): Query<ShaQuery>,
) -> Result<Json<Vec<crate::repo::DiffResponse>>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_commit_diff(&repo, &q.sha),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_push(
    Checkout(repo): Checkout,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_push(&repo),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_uncommit(
    Checkout(repo): Checkout,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_uncommit(&repo),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_init(
    Checkout(repo): Checkout,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_init(&repo),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_act(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Json(req): Json<GitActRequest>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    // A lane's checkout does not hold every file the panel can show — a session edit may have
    // been made in the project itself, or in a nested repository — and "no such file" for a
    // path the person is looking at is the least useful error there is. The project root is
    // the second place to look, and `resolve` still refuses anything outside it.
    let root = state.repo();
    blocking(
        move || {
            let first = crate::repo::git_act(&repo, &req.action, &req.path, req.hunk);
            match first {
                // Staging and unstaging only. `discard` and `discard-hunk` throw work away, and
                // looking somewhere else for a file to throw away is not a helpful second guess —
                // it is a different tree than the one the person is looking at.
                Err(e)
                    if e.contains("no such file")
                        && root != repo
                        && matches!(req.action.as_str(), "stage" | "unstage") =>
                {
                    crate::repo::git_act(&root, &req.action, &req.path, req.hunk)
                }
                other => other,
            }
        },
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(serde::Deserialize)]
struct GitIgnoreBody {
    path: String,
}

async fn api_git_ignore(
    Checkout(repo): Checkout,
    Json(b): Json<GitIgnoreBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::repo::git_ignore_path(&repo, &b.path),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(serde::Deserialize)]
struct IgnoreBody {
    id: String,
    ignored: bool,
    #[serde(default)]
    why: String,
}

async fn api_ignore(
    State(state): State<Arc<AppState>>,
    Json(b): Json<IgnoreBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    // A small file, but a read-modify-write of one all the same, and the rule is the rule: no
    // filesystem on the executor. The next one to be added here will be small too.
    let repo = state.repo();
    blocking(
        move || crate::ignored::set(&repo, &b.id, b.ignored, &b.why),
        Err("could not record that".to_string()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))
}

/// The session list alone, for the History panel to poll while it is open.
///
/// `/api/state` carries it too, but that one runs a full scan of the repository, which is
/// seconds on a large one — too much to pay every few seconds for a list of titles. This is the
/// one `stat` per transcript the summary cache already makes cheap, and it is what keeps
/// "● running" honest: `live` is a fact about the transcript's mtime, so a panel that never asks
/// again shows a session running for as long as it stays open.
async fn api_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<keel_workspace::Session>> {
    if !state.project_open() {
        return Json(Vec::new());
    }
    let repo = state.repo();
    let sessions = blocking(
        move || {
            let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
            keel_workspace::discover_sessions(&repo, &home)
        },
        Vec::new(),
    )
    .await;
    Json(sessions)
}

async fn api_state(State(state): State<Arc<AppState>>) -> Json<StateResponse> {
    let repo = state.repo();
    let prefs = crate::prefs::Prefs::load();

    // Nothing open means nothing to scan or inventory. Skipping both is not only faster — running
    // them would report on an empty scratch directory as though it were the user's work.
    if !state.project_open() {
        return Json(StateResponse {
            repo: String::new(),
            project_open: false,
            onboarded: prefs.onboarded,
            scan: keel_scanner::Report::new(Vec::new()),
            workspace: keel_workspace::Workspace::default(),
            policy: crate::policy::Policy::default(),
        });
    }

    // Re-read on every request. The developer is editing this repository in another window, and a
    // cached view of a tree that has moved on is worse than a slightly slower one.
    //
    // Off the executor, because it is a full scan and a walk of every Claude Code session in the
    // project — seconds on a large repository, during which nothing else Keel serves could answer.
    let scanning = repo.clone();
    let scan = blocking(
        move || {
            let report = keel_scanner::RepoContext::load(&scanning)
                .map(|ctx| keel_scanner::scan(&ctx))
                .unwrap_or_else(|_| keel_scanner::Report::new(Vec::new()));
            // What the team set aside leaves the panel; the score and `keel scan` are untouched.
            crate::ignored::apply(&scanning, report)
        },
        keel_scanner::Report::new(Vec::new()),
    )
    .await;

    let discovering = repo.clone();
    let mut workspace = blocking(
        move || match keel_workspace::claude_home() {
            Some(home) => keel_workspace::Workspace::discover(&discovering, &home),
            None => keel_workspace::Workspace::discover(&discovering, "/nonexistent".into()),
        },
        keel_workspace::Workspace::default(),
    )
    .await;
    // A session someone renamed in Keel keeps that name, without Claude Code's transcript being
    // touched to achieve it.
    crate::names::apply(&mut workspace.sessions);

    Json(StateResponse {
        repo: repo.to_string(),
        project_open: true,
        onboarded: prefs.onboarded,
        scan,
        workspace,
        policy: crate::policy::Policy::load(&repo),
    })
}

#[cfg(test)]
mod tests {
    /// Every route handler here keeps its blocking work off the executor.
    ///
    /// This is the rule with the worst failure mode of any in the file, because breaking it fails
    /// no other test: axum's executor has a small worker pool, so one handler that shells out to
    /// git or reads a transcript on it delays *every other request the window has in flight* —
    /// the approval poll and the chat stream included. What that looks like is a window that is
    /// intermittently slow for reasons nobody can reproduce, which is the exact complaint this
    /// codebase is trying to stop hearing.
    ///
    /// Read off the source rather than the behaviour, because there is no way to observe "did not
    /// occupy a worker" from a test. A handler that genuinely touches nothing outside memory can
    /// say so with `// no-blocking:` and a reason.
    #[test]
    fn every_handler_keeps_blocking_work_off_the_executor() {
        // Every module with a handler in it, not only this one: the rule with the worst failure
        // mode in the daemon was kept for `serve.rs` alone, and `dev::status` walked a checkout's
        // `package.json`s on the executor for its whole life.
        let sources: [(&str, &str); 10] = [
            ("serve.rs", include_str!("serve.rs")),
            ("agent.rs", include_str!("agent.rs")),
            ("approve.rs", include_str!("approve.rs")),
            ("dev.rs", include_str!("dev.rs")),
            ("monitor.rs", include_str!("monitor.rs")),
            ("turns.rs", include_str!("turns.rs")),
            ("worktree.rs", include_str!("worktree.rs")),
            ("snapshot.rs", include_str!("snapshot.rs")),
            ("verify.rs", include_str!("verify.rs")),
            ("permissions.rs", include_str!("permissions.rs")),
        ];
        let mut offenders = Vec::new();
        for (file, source) in sources {
            let body = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
            for part in body.split("async fn ").skip(1) {
                let name = part
                    .split(['(', '<', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                // A handler is a function axum routes to: `api_*` here, and every `pub async fn`
                // in the modules that register their own. The private helpers between them are
                // the handlers' own business.
                let handler =
                    name.starts_with("api_") || source.contains(&format!("pub async fn {name}"));
                if !handler {
                    continue;
                }
                // Its own body only: up to the next `fn` at column zero, so a well-behaved
                // handler cannot vouch for the one after it.
                let own = part.split("\n}\n").next().unwrap_or(part);
                if own.contains("blocking(")
                    || own.contains("spawn_blocking(")
                    || own.contains("off_thread(")
                    || own.contains("// no-blocking:")
                    || own.contains("tokio::spawn(")
                {
                    continue;
                }
                offenders.push(format!("{file}::{name}"));
            }
        }
        assert!(
            offenders.is_empty(),
            "these handlers do their work on the executor: {offenders:?}. \
             Wrap it in `blocking(…)`, or say `// no-blocking: <reason>` if it truly only \
             touches memory."
        );
    }

    use super::*;
    use std::os::unix::process::CommandExt;

    fn temporary_checkout() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let checkout = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        (dir, checkout)
    }

    /// A child in its own process group, so `interrupt` has something a signal can reach.
    fn spawn_sleeper() -> std::process::Child {
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        command.process_group(0);
        command.spawn().expect("could not spawn `sleep`")
    }

    /// The one this file exists for. Non-negotiable #6 said "Stop sends SIGINT" and nothing sent
    /// any signal at all — the daemon noticed a hung-up client only when the *next* output line
    /// arrived, so a quiet turn kept running after Stop. A budget that cannot fail reads as proof,
    /// so this signals a real process and waits for it to actually die.
    #[test]
    fn interrupt_stops_the_process_it_recorded() {
        let state = AppState::empty();
        let (_dir, checkout) = temporary_checkout();
        let mut child = spawn_sleeper();
        let token = state.claim("lane-a", &checkout, true).unwrap();
        state.started("lane-a", token, child.id());

        assert!(state.interrupt("lane-a"), "nothing was signalled");

        let status = child.wait().expect("could not wait for the child");
        assert!(
            !status.success(),
            "`sleep` finished normally — it was not interrupted"
        );
    }

    #[test]
    fn stop_during_preparation_is_acknowledged() {
        let state = AppState::empty();
        let (_dir, path) = temporary_checkout();
        let checkout = path.as_path();
        let token = state.claim("preparing", checkout, true).unwrap();

        assert!(
            state.interrupt("preparing"),
            "Stop was lost because the reserved turn had not spawned its agent yet"
        );
        assert!(state.claim("replacement", checkout, true).is_err());
        state.release("preparing", token);
        assert!(state.claim("replacement", checkout, true).is_ok());
    }

    #[test]
    fn a_stop_before_pid_registration_still_interrupts_the_child() {
        let state = AppState::empty();
        let (_dir, checkout) = temporary_checkout();
        let token = state.claim("preparing", &checkout, true).unwrap();
        assert!(state.interrupt("preparing"));
        assert!(state.cancelled("preparing", token));

        // Stop can land between the last cancellation checkpoint and registering the PID.
        let mut child = spawn_sleeper();
        state.started("preparing", token, child.id());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let exited = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            exited.is_some_and(|status| !status.success()),
            "the newly registered child ignored the Stop recorded during preparation"
        );
    }

    #[test]
    fn finishing_keeps_the_checkout_claim_but_forgets_the_process() {
        let state = AppState::empty();
        let (_dir, path) = temporary_checkout();
        let checkout = path.as_path();
        let token = state.claim("finishing", checkout, true).unwrap();
        state.started("finishing", token, 5678);
        state.finished("finishing", token);

        assert!(state.writer_in(checkout));
        assert!(state.claim("replacement", checkout, true).is_err());
        assert_eq!(state.running.locked().get("finishing").unwrap().pid, None);
        assert!(!state.interrupt("finishing"));

        state.release("finishing", token);
        let replacement = state.claim("finishing", checkout, true).unwrap();
        state.started("finishing", replacement, 6789);
        state.finished("finishing", token);
        assert_eq!(
            state.running.locked().get("finishing").unwrap().pid,
            Some(6789),
            "a late completion must not clear a newer turn's process"
        );
        assert!(!state.cancelled("finishing", replacement));
    }

    /// A question belongs to one conversation, and so does a Stop. Two lanes are two turns, and
    /// stopping one while the other is mid-build would be the same class of bug as the approval
    /// queue that drained every window's questions.
    #[test]
    fn interrupt_leaves_other_conversations_alone() {
        let state = AppState::empty();
        let (_mine_dir, mine_checkout) = temporary_checkout();
        let (_their_dir, their_checkout) = temporary_checkout();
        let mut mine = spawn_sleeper();
        let mut theirs = spawn_sleeper();
        let a = state.claim("mine", &mine_checkout, true).unwrap();
        let b = state.claim("theirs", &their_checkout, true).unwrap();
        state.started("mine", a, mine.id());
        state.started("theirs", b, theirs.id());

        state.interrupt("mine");
        mine.wait()
            .expect("could not wait for the interrupted child");

        assert!(
            theirs
                .try_wait()
                .expect("could not poll the other child")
                .is_none(),
            "the other conversation's turn was stopped too"
        );
        let _ = theirs.kill();
        let _ = theirs.wait();
    }

    /// Pressing Stop on a turn that already finished is not an error, and must not signal whatever
    /// process happens to hold that pid next.
    #[test]
    fn interrupt_with_nothing_running_signals_nothing() {
        let state = AppState::empty();
        assert!(!state.interrupt("lane-a"));

        let (_dir, checkout) = temporary_checkout();
        let mut child = spawn_sleeper();
        let token = state.claim("lane-a", &checkout, true).unwrap();
        state.started("lane-a", token, child.id());
        state.release("lane-a", token);
        assert!(!state.interrupt("lane-a"));
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A turn that ends after the next one has started must not release the new turn's slot, or
    /// the second Stop finds nothing and the agent runs on.
    #[test]
    fn a_late_ending_turn_does_not_forget_the_new_one() {
        let state = AppState::empty();
        let (_dir, path) = temporary_checkout();
        let lane = path.as_path();
        let first = state.claim("lane-a", lane, true).unwrap();
        state.release("lane-a", first);
        let second = state.claim("lane-a", lane, true).unwrap();
        state.started("lane-a", second, 5678);

        state.release("lane-a", first);
        assert_eq!(
            state.running.locked().get("lane-a").and_then(|t| t.pid),
            Some(5678),
            "the finished turn released the running turn's slot"
        );
    }

    /// Two `claude -p` in one lane is two agents on one checkout with one of them invisible to
    /// Stop: `running` held a single pid per lane and the second registration simply overwrote
    /// the first. The guard lived in `SessionModel.start`, which is one window's view.
    #[test]
    fn one_lane_takes_one_turn() {
        let state = AppState::empty();
        let (_dir, path) = temporary_checkout();
        let lane = path.as_path();
        let token = state.claim("lane-a", lane, true).unwrap();
        assert!(
            state.claim("lane-a", lane, true).is_err(),
            "a second turn started in a lane that already had one"
        );

        state.release("lane-a", token);
        assert!(
            state.claim("lane-a", lane, true).is_ok(),
            "the lane never became usable again"
        );
    }

    /// The multi-agent pillar's load-bearing constraint, kept where every window can be seen.
    ///
    /// Both of the things that end a turn are tree-wide — the auto-commit is `git add -A`, a
    /// rewind restores the whole tree — so two lanes writing one checkout means whichever
    /// finishes first commits the other's half-written files under the wrong prompt. Tearing a
    /// lane into its own window is a gesture this app offers, and it puts the two lanes in
    /// different arrays, so the Swift check cannot see the pair it exists to refuse.
    #[test]
    fn one_working_tree_takes_one_writer() {
        let state = AppState::empty();
        let (_dir, path) = temporary_checkout();
        let shared = path.as_path();
        let isolated = shared.join(".keel/worktrees/x");
        std::fs::create_dir_all(&isolated).unwrap();
        let held = state.claim("window-one", shared, true).unwrap();

        assert!(
            state.claim("window-two", shared, true).is_err(),
            "two lanes are writing one working tree"
        );
        // A lane for reading and planning beside one that is editing is what a shared lane is
        // for, and it writes nothing.
        assert!(
            state.claim("reader", shared, false).is_ok(),
            "a plan turn was refused beside a writing one"
        );
        // Its own checkout is the whole point of a lane having one.
        assert!(
            state.claim("isolated", &isolated, true).is_ok(),
            "a lane with its own checkout was refused"
        );
        assert!(
            state.writer_in(shared),
            "the auto-commit cannot see the writer"
        );

        state.release("window-one", held);
        assert!(!state.writer_in(shared));
        assert!(
            state.claim("window-two", shared, true).is_ok(),
            "the tree was never handed back"
        );
    }

    #[test]
    fn independent_daemons_cannot_claim_the_same_checkout_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = Utf8Path::from_path(dir.path()).unwrap();
        let first = AppState::new(checkout.to_owned());
        let second = AppState::new(checkout.to_owned());
        let token = first.claim("first-window", checkout, true).unwrap();

        assert!(
            second.claim("detached-window", checkout, true).is_err(),
            "a separate daemon acquired a checkout that another daemon is writing"
        );
        assert!(second.claim("planning", checkout, false).is_ok());
        first.finished("first-window", token);
        assert!(second.claim("detached-window", checkout, true).is_err());
        first.release("first-window", token);
        assert!(second.claim("detached-window", checkout, true).is_ok());
    }

    #[test]
    fn checkout_aliases_share_a_lock_but_linked_worktrees_are_independent() {
        let (_dir, folder) = temporary_checkout();
        let project = folder.join("project");
        let nested = project.join("src");
        let isolated = project.join(".keel/worktrees/feature");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir(&nested).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(isolated.join(".git"), "gitdir: unused-test-fixture\n").unwrap();
        let alias = folder.join("project-alias");
        std::os::unix::fs::symlink(&project, &alias).unwrap();

        let first = AppState::new(project.clone());
        let second = AppState::new(project.clone());
        first.claim("root", &project, true).unwrap();
        for (n, checkout) in [&nested, &folder, &alias].into_iter().enumerate() {
            assert!(
                second.claim("second", checkout, true).is_err(),
                "{checkout} bypassed the root checkout's writer lock"
            );
            assert!(second.claim(&format!("plan-{n}"), checkout, false).is_ok());
            assert!(first.writer_in_other(checkout, "another-session"));
        }
        assert!(second.claim("isolated", &isolated, true).is_ok());
    }

    #[test]
    fn writer_claim_child_process() {
        use std::io::Write;
        let Some(checkout) = std::env::var_os("KEEL_TEST_CLAIM_CHECKOUT") else {
            return;
        };
        let checkout = Utf8PathBuf::from_path_buf(checkout.into()).unwrap();
        let state = AppState::new(checkout.clone());
        state.claim("child", &checkout, true).unwrap();
        println!("keel-writer-claimed");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[tokio::test]
    async fn a_writer_lock_excludes_another_process_and_recovers_after_exit() {
        use tokio::io::AsyncBufReadExt;
        let (_dir, checkout) = temporary_checkout();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "serve::tests::writer_claim_child_process",
                "--nocapture",
            ])
            .env("KEEL_TEST_CLAIM_CHECKOUT", &checkout)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if line == "keel-writer-claimed" {
                    return true;
                }
            }
            false
        })
        .await;
        assert!(
            matches!(ready, Ok(true)),
            "the helper did not acquire its lock"
        );

        let state = AppState::new(checkout.clone());
        let blocked = state.claim("parent", &checkout, true).is_err();
        let planning = state.claim("planning", &checkout, false).is_ok();
        // Abrupt termination proves recovery does not depend on a Rust destructor running.
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        assert!(blocked, "a different process acquired the held checkout");
        assert!(planning, "a writer blocked a read-only plan");
        assert!(state.claim("parent", &checkout, true).is_ok());
    }

    /// Every way out of the turn task gives the lane back. A lane left claimed can take no
    /// further turn, and nothing on screen says what is holding it.
    #[test]
    fn a_turn_that_ends_any_way_at_all_gives_the_lane_back() {
        let state = Arc::new(AppState::empty());
        let (_dir, path) = temporary_checkout();
        let lane = path.as_path();
        let token = state.claim("lane-a", lane, true).unwrap();
        {
            let _held = Held::new(state.clone(), "lane-a".into(), token);
            assert!(state.claim("lane-a", lane, true).is_err());
        }
        assert!(
            state.claim("lane-a", lane, true).is_ok(),
            "the guard did not release the lane"
        );
    }
}
