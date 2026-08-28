//! The local IDE server.
//!
//! Binds to loopback only. Keel reads the developer's repositories, their Claude Code sessions and
//! (later) their cloud credentials; none of that should be reachable from another machine, so the
//! bind address is not configurable.

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::header,
    response::Html,
    routing::get,
};
use camino::Utf8PathBuf;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// The single page, compiled into the binary so `keel` stays one file with no assets to lose.
const INDEX: &str = include_str!("../../../ui/index.html");

/// Monaco, vendored and compiled in for the same reason.
///
/// The editor is the one part of an IDE that cannot be approximated: a textarea behind a
/// highlighted div gives you no multi-cursor, no column selection, no folding, no real find and
/// replace, and no undo worth the name. Monaco is the engine VS Code itself runs on and ships a
/// prebuilt bundle needing no build step. The heavy language services (TypeScript, CSS, HTML
/// workers, ~7.5MB) are deliberately excluded — they add intellisense on top of editing, and
/// editing is what was missing.
#[derive(rust_embed::Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/vendor/"]
struct Vendor;

pub struct AppState {
    repo: std::sync::RwLock<Utf8PathBuf>,
    /// Whether `repo` is a project someone chose, or the empty stand-in used before one is open.
    open: std::sync::atomic::AtomicBool,
}

impl AppState {
    pub fn new(repo: Utf8PathBuf) -> Self {
        Self {
            repo: std::sync::RwLock::new(repo),
            open: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Launched with nothing open — from the Dock, on a first run, or with a remembered project
    /// that has since moved.
    pub fn empty() -> Self {
        Self {
            repo: std::sync::RwLock::new(crate::prefs::no_project()),
            open: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn project_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
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
    let launch = if open_browser { Launch::Tab } else { Launch::None };
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
    let launch = if open_browser { Launch::Window } else { Launch::None };
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
        .route("/", get(index))
        .route("/vendor/{*path}", get(vendor))
        .route("/api/state", get(api_state))
        .route("/api/tree", get(api_tree))
        .route("/api/file", get(api_file))
        .route("/api/file/original", get(api_original))
        .route("/api/session", get(api_session))
        .route("/api/raw", get(api_raw))
        .route("/api/chat", get(crate::api::chat))
        .route("/api/git/status", get(api_git_status))
        .route("/api/git/diff", get(api_git_diff))
        .route("/api/save", axum::routing::post(api_save))
        .route("/api/permissions", get(crate::permissions::list))
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
        .route("/api/claude", get(crate::clitools::claude_status))
        .route("/api/cli", get(crate::clitools::status))
        .route("/api/cli/install", get(crate::clitools::install))
        .route("/api/cli/login", get(crate::clitools::login))
        .route(
            "/api/github/clone",
            axum::routing::post(crate::connect::clone),
        )
        .route("/api/open", axum::routing::post(crate::connect::open_repo))
        .route("/api/browse", get(crate::connect::browse))
        .route("/api/browse-files", get(crate::connect::browse_files))
        .route(
            "/api/project/new",
            axum::routing::post(crate::project::create),
        )
        .with_state(state);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr} (is another keel already running?)"))?;

    println!("\n  Keel — {url}\n  Ctrl-C to stop\n");
    open_ui(&url, launch);

    axum::serve(listener, app).await.context("serving")?;
    Ok(())
}

async fn api_tree(State(state): State<Arc<AppState>>) -> Json<Vec<crate::api::Node>> {
    Json(crate::api::tree(&state.repo()))
}

async fn api_file(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::api::FileQuery>,
) -> Result<Json<crate::api::FileResponse>, (axum::http::StatusCode, String)> {
    crate::api::read_file(&state.repo(), &query.path)
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

async fn api_original(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::api::FileQuery>,
) -> Result<Json<crate::api::FileResponse>, (axum::http::StatusCode, String)> {
    crate::api::read_original(&state.repo(), &query.path)
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
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

async fn api_git_status(State(state): State<Arc<AppState>>) -> Json<crate::api::GitStatus> {
    Json(crate::api::git_status(&state.repo()))
}

async fn api_git_diff(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::api::FileQuery>,
) -> Json<crate::api::DiffResponse> {
    Json(crate::api::git_diff(&state.repo(), &query.path))
}

async fn api_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<crate::api::SaveRequest>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    crate::api::write_file(&state.repo(), &req)
        .map(|_| Json(true))
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

/// Serve a vendored asset straight from the binary.
async fn vendor(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match Vendor::get(&path) {
        Some(file) => {
            let mime = mime_for(&path);
            // These are content-addressed by filename, so they can be cached hard.
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                file.data,
            )
                .into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("ttf") => "font/ttf",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

async fn index() -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(INDEX),
    )
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
    let scan = keel_scanner::RepoContext::load(&repo)
        .map(|ctx| keel_scanner::scan(&ctx))
        .unwrap_or_else(|_| keel_scanner::Report::new(Vec::new()));

    let workspace = match keel_workspace::claude_home() {
        Some(home) => keel_workspace::Workspace::discover(&repo, &home),
        None => keel_workspace::Workspace::discover(&repo, "/nonexistent".into()),
    };

    Json(StateResponse {
        repo: repo.to_string(),
        project_open: true,
        onboarded: prefs.onboarded,
        scan,
        workspace,
    })
}

#[cfg(test)]
mod ui_tests {
    /// The UI is one HTML file with one large inline script, and a syntax error anywhere in it
    /// kills the entire page — no sidebar, no status rail, nothing in the console until you look
    /// for it. That happened once, from a patch that duplicated a block and so declared the same
    /// `let` twice. Nothing caught it but a screenshot.
    ///
    /// Skipped when node is not installed rather than failed: this asserts something about the
    /// UI, and refusing to build on a machine without a JavaScript runtime would be a worse trade
    /// than missing the check there.
    #[test]
    fn the_ui_script_parses() {
        let html = include_str!("../../../ui/index.html");
        let script = html
            .rsplit_once("<script>")
            .and_then(|(_, tail)| tail.split_once("</script>"))
            .map(|(body, _)| body)
            .expect("the UI has an inline script");
        assert!(script.len() > 10_000, "found the wrong script block");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.js");
        std::fs::write(&path, script).unwrap();

        let Ok(out) = std::process::Command::new("node")
            .arg("--check")
            .arg(&path)
            .output()
        else {
            eprintln!("node not installed — skipping the UI syntax check");
            return;
        };

        assert!(
            out.status.success(),
            "ui/index.html has a JavaScript syntax error, which blanks the whole page:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
