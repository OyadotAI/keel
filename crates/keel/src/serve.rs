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

    pub fn set_repo(&self, path: Utf8PathBuf) {
        crate::prefs::Prefs::remember(&path);
        *self.repo.write().expect("repo lock poisoned") = path;
        self.open.store(true, std::sync::atomic::Ordering::Relaxed);
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
async fn api_tree(State(state): State<Arc<AppState>>) -> Json<Vec<crate::api::Node>> {
    let repo = state.repo();
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
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::api::FileQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match crate::api::read_raw(&state.repo(), &query.path) {
        Ok((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct SessionQuery {
    id: String,
}

/// Read one session's transcript, for the session switcher in the agent panel.
async fn api_session(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionQuery>,
) -> Json<Vec<keel_workspace::Turn>> {
    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    Json(keel_workspace::transcript(&state.repo(), &home, &query.id))
}

/// What one session changed and ran. Explicit, on a click — see `keel_workspace::session_work`.
async fn api_session_work(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionQuery>,
) -> Json<keel_workspace::SessionWork> {
    let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
    Json(keel_workspace::session_work(
        &state.repo(),
        &home,
        &query.id,
    ))
}

async fn api_git_status(State(state): State<Arc<AppState>>) -> Json<crate::api::GitStatus> {
    let repo = state.repo();
    Json(blocking(move || crate::api::git_status(&repo), Default::default()).await)
}

async fn api_git_diff(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::api::FileQuery>,
) -> Json<crate::api::DiffResponse> {
    let repo = state.repo();
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
}

/// Stage, unstage or discard one file, from the Changes panel.
/// `git init`, for a project that is not one yet.
async fn api_git_init(
    State(state): State<Arc<AppState>>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    let repo = state.repo();
    blocking(move || crate::api::git_init(&repo), Err("timed out".into()))
        .await
        .map(|()| Json(true))
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_git_act(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GitActRequest>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    let repo = state.repo();
    blocking(
        move || crate::api::git_act(&repo, &req.action, &req.path),
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
