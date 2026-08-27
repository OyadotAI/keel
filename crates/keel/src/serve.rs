//! The local IDE server.
//!
//! Binds to loopback only. Keel reads the developer's repositories, their Claude Code sessions and
//! (later) their cloud credentials; none of that should be reachable from another machine, so the
//! bind address is not configurable.

use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, http::header, response::Html, routing::get};
use camino::Utf8PathBuf;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// The single page, compiled into the binary so `keel` stays one file with no assets to lose.
const INDEX: &str = include_str!("../../../ui/index.html");

struct AppState {
    repo: Utf8PathBuf,
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
    /// Cloud connection status. Explicitly reported rather than assumed, because an IDE that
    /// implies a deployment exists when none does is worse than one that says so.
    connections: Connections,
}

#[derive(Serialize)]
struct Connections {
    github: bool,
    cloudflare: bool,
    claude: bool,
}

pub async fn run(repo: Utf8PathBuf, port: u16, open_browser: bool) -> Result<()> {
    let state = Arc::new(AppState { repo });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/state", get(api_state))
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

async fn index() -> impl axum::response::IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Html(INDEX))
}

async fn api_state(State(state): State<Arc<AppState>>) -> Json<StateResponse> {
    let repo = state.repo.clone();

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
        connections: Connections {
            // Nothing is wired to a provider yet, and the UI says so rather than implying
            // a deployment that does not exist.
            github: false,
            cloudflare: false,
            claude: which_claude(),
        },
    })
}

/// Whether the `claude` binary Keel drives is actually installed.
fn which_claude() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}
