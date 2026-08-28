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
}

impl AppState {
    pub fn new(repo: Utf8PathBuf) -> Self {
        Self {
            repo: std::sync::RwLock::new(repo),
        }
    }

    /// The repository currently open. Cloned rather than borrowed so no handler holds the lock
    /// across an await point.
    pub fn repo(&self) -> Utf8PathBuf {
        self.repo.read().expect("repo lock poisoned").clone()
    }

    pub fn set_repo(&self, path: Utf8PathBuf) {
        *self.repo.write().expect("repo lock poisoned") = path;
    }
}

/// Everything the UI needs, in one request.
///
/// A single round trip rather than four keeps the first paint honest: the page never renders a
/// half-populated view where one pane has loaded and another has not.
#[derive(Serialize)]
struct StateResponse {
    repo: String,
    scan: keel_scanner::Report,
    workspace: keel_workspace::Workspace,
}

pub async fn run(repo: Utf8PathBuf, port: u16, open_browser: bool) -> Result<()> {
    let state = Arc::new(AppState::new(repo));

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

    let url = format!("http://127.0.0.1:{port}");
    println!("\n  Keel — {url}\n  Ctrl-C to stop\n");

    if open_browser {
        // Failing to open a browser is not a reason to refuse to serve.
        let _ = open::that_detached(&url);
    }

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
        scan,
        workspace,
    })
}
