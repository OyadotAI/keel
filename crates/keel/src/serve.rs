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

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::header,
    routing::get,
};
use camino::Utf8PathBuf;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

pub struct AppState {
    repo: std::sync::RwLock<Utf8PathBuf>,
    /// Whether `repo` is a project someone chose, or the empty stand-in used before one is open.
    open: std::sync::atomic::AtomicBool,
    /// The port this Keel is serving on. The approval hook is spawned by `claude`, in a separate
    /// process, and this is how it finds its way back.
    port: std::sync::atomic::AtomicU16,
}

impl AppState {
    pub fn new(repo: Utf8PathBuf) -> Self {
        Self {
            repo: std::sync::RwLock::new(repo),
            open: std::sync::atomic::AtomicBool::new(true),
            port: std::sync::atomic::AtomicU16::new(7777),
        }
    }

    /// Launched with nothing open — from the Dock, on a first run, or with a remembered project
    /// that has since moved.
    pub fn empty() -> Self {
        Self {
            repo: std::sync::RwLock::new(crate::prefs::no_project()),
            open: std::sync::atomic::AtomicBool::new(false),
            port: std::sync::atomic::AtomicU16::new(7777),
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
        self.repo.read().expect("repo lock poisoned").clone()
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
                    Err(format!("lane {name} has no checkout"))
                }
            }
        }
    }

    /// The directory a session was launched from, when it is not the repository: a parent
    /// (at most two up, never home) or a subdirectory. Anything else is the repository.
    pub fn session_dir(&self, cwd: Option<&str>) -> Utf8PathBuf {
        let repo = self.repo();
        let Some(cwd) = cwd.filter(|c| !c.is_empty()).map(Utf8PathBuf::from) else {
            return repo;
        };
        let Ok(cwd) = cwd.canonicalize_utf8() else {
            return repo;
        };
        let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
        let allowed = keel_workspace::session_dirs(&repo, &home)
            .into_iter()
            .any(|(d, scope)| scope != "below" && d == cwd)
            || cwd.starts_with(&repo);
        if allowed { cwd } else { repo }
    }

    pub fn set_repo(&self, path: Utf8PathBuf) {
        crate::prefs::Prefs::remember(&path);
        *self.repo.write().expect("repo lock poisoned") = path;
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
}

pub async fn run(repo: Utf8PathBuf, port: u16, open_browser: bool) -> Result<()> {
    let launch = if open_browser {
        Launch::Tab
    } else {
        Launch::None
    };
    serve(AppState::new(repo), port, launch).await
}

/// Start with whatever was open last, or with nothing.
///
/// This is how the application is launched from the Dock, where there is no working directory to
/// infer a project from — the Finder hands a process `/` and it would be a strange thing to open.
pub async fn run_app(port: u16, open_browser: bool) -> Result<()> {
    let state = match crate::prefs::Prefs::load().resume() {
        Some(project) => AppState::new(project),
        None => AppState::empty(),
    };
    let launch = if open_browser {
        Launch::Window
    } else {
        Launch::None
    };
    serve(state, port, launch).await
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

/// Chromium-based browsers can open a page as a window with no tab strip, address bar or
/// bookmarks. It costs nothing and it is the difference between Keel looking like an application
/// and looking like a bookmark someone opened.
const APP_MODE_BROWSERS: &[&str] = &[
    "/Applications/Google Chrome.app",
    "/Applications/Brave Browser.app",
    "/Applications/Microsoft Edge.app",
    "/Applications/Chromium.app",
];

fn open_ui(url: &str, launch: Launch) {
    match launch {
        Launch::None => {}
        // Failing to open a browser is never a reason to refuse to serve — the URL is printed.
        Launch::Tab => {
            let _ = open::that_detached(url);
        }
        Launch::Window => {
            if let Some(app) = APP_MODE_BROWSERS
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                && std::process::Command::new("open")
                    .args(["-na", app, "--args", &format!("--app={url}")])
                    .spawn()
                    .is_ok()
            {
                return;
            }
            let _ = open::that_detached(url);
        }
    }
}

/// How the window is opened.
#[derive(Clone, Copy, PartialEq)]
pub enum Launch {
    /// Nothing. `--no-open`, or a headless run.
    None,
    /// A tab in the default browser — what you want when you typed `keel serve` in a terminal.
    Tab,
    /// A chromeless window, so a Dock launch does not look like a bookmark. Falls back to a tab
    /// when no Chromium-based browser is installed.
    Window,
}

async fn serve(state: AppState, port: u16, launch: Launch) -> Result<()> {
    state.port.store(port, std::sync::atomic::Ordering::Relaxed);
    let url = format!("http://127.0.0.1:{port}");

    // Launching a second time from the Dock must raise the window that is already open, not fail
    // with "address in use" behind an icon that then does nothing.
    if already_running(port) {
        println!("\n  Keel is already running — {url}\n");
        open_ui(&url, launch);
        return Ok(());
    }

    let state = Arc::new(state);

    let app = Router::new()
        .route("/api/state", get(api_state))
        .route("/api/tree", get(api_tree))
        .route("/api/session", get(api_session))
        .route("/api/session/work", get(api_session_work))
        .route(
            "/api/session/rename",
            axum::routing::post(crate::names::rename),
        )
        .route("/api/raw", get(api_raw))
        .route("/api/chat", get(crate::api::chat))
        .route(
            "/api/attach",
            axum::routing::post(crate::api::attach)
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
            "/api/git/snapshot",
            axum::routing::post(crate::snapshot::take),
        )
        .route(
            "/api/git/restore",
            axum::routing::post(crate::snapshot::put_back),
        )
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
        .with_state(state);

    let (ip, reach) = bind_address();
    let addr = SocketAddr::from((ip, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr} (is another keel already running?)"))?;

    println!("\n  Keel — {url}{reach}\n  Ctrl-C to stop\n");
    open_ui(&url, launch);

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
async fn api_tree(Checkout(repo): Checkout) -> Json<Vec<crate::api::Node>> {
    Json(blocking(move || crate::api::tree(&repo), Vec::new()).await)
}

/// Run blocking work off the executor, so one slow call cannot stall the whole server.
///
/// The caller supplies what to return if the task panics, rather than the helper requiring
/// `Default` — a scan report has no meaningful empty value, and inventing one to satisfy a
/// signature is the wrong way round.
async fn blocking<T, F>(work: F, fallback: T) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await.unwrap_or(fallback)
}

async fn api_raw(
    Checkout(repo): Checkout,
    Query(query): Query<crate::api::FileQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match crate::api::read_raw(&repo, &query.path) {
        Ok((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct SessionQuery {
    id: String,
    /// The directory the session was launched from, when not the repository.
    cwd: Option<String>,
}

/// Read one session's transcript, for the session switcher in the agent panel.
async fn api_session(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Query(query): Query<SessionQuery>,
) -> Json<Vec<keel_workspace::Turn>> {
    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    let dir = if query.cwd.is_some() {
        state.session_dir(query.cwd.as_deref())
    } else {
        repo
    };
    Json(keel_workspace::transcript(&dir, &home, &query.id))
}

/// What one session changed and ran. Explicit, on a click — see `keel_workspace::session_work`.
async fn api_session_work(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Query(query): Query<SessionQuery>,
) -> Json<keel_workspace::SessionWork> {
    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    let dir = if query.cwd.is_some() {
        state.session_dir(query.cwd.as_deref())
    } else {
        repo
    };
    Json(keel_workspace::session_work(&dir, &home, &query.id))
}

async fn api_git_status(Checkout(repo): Checkout) -> Json<crate::api::GitStatus> {
    Json(blocking(move || crate::api::git_status(&repo), Default::default()).await)
}

async fn api_git_diff(
    Checkout(repo): Checkout,
    Query(query): Query<crate::api::FileQuery>,
) -> Json<crate::api::DiffResponse> {
    Json(
        blocking(
            move || crate::api::git_diff(&repo, &query.path),
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
) -> Json<Vec<crate::api::Commit>> {
    Json(blocking(move || crate::api::git_log(&repo, q.n.min(100)), Vec::new()).await)
}

async fn api_git_branches(Checkout(repo): Checkout) -> Json<crate::api::Branches> {
    Json(
        blocking(
            move || crate::api::git_branches(&repo),
            crate::api::Branches {
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
        move || crate::api::git_branch_act(&repo, &b.action, &b.name),
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
        move || crate::api::git_remote_act(&repo, &b.action, b.url.as_deref()),
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
        move || crate::api::git_discard_all(&repo),
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
        move || crate::api::git_stage_all(&repo, b.stage),
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
        move || crate::api::git_commit_staged(&repo, &b.message),
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
) -> Result<Json<Vec<crate::api::DiffResponse>>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::api::git_commit_diff(&repo, &q.sha),
        Err("timed out".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_push(
    Checkout(repo): Checkout,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    blocking(move || crate::api::git_push(&repo), Err("timed out".into()))
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_uncommit(
    Checkout(repo): Checkout,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::api::git_uncommit(&repo),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_init(
    Checkout(repo): Checkout,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(move || crate::api::git_init(&repo), Err("timed out".into()))
        .await
        .map(|()| Json(true))
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_act(
    Checkout(repo): Checkout,
    Json(req): Json<GitActRequest>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    blocking(
        move || crate::api::git_act(&repo, &req.action, &req.path, req.hunk),
        Err("timed out".into()),
    )
    .await
    .map(|()| Json(true))
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
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
            keel_scanner::RepoContext::load(&scanning)
                .map(|ctx| keel_scanner::scan(&ctx))
                .unwrap_or_else(|_| keel_scanner::Report::new(Vec::new()))
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
    })
}
