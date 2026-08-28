//! Connection management: GitHub, Cloudflare, and switching repositories.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use keel_providers::{cloudflare, credentials, github};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

type ApiResult<T> = Result<Json<T>, (StatusCode, String)>;

fn bad(e: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.into())
}

#[derive(Serialize)]
pub struct Connections {
    pub claude: bool,
    pub github: Option<github::Account>,
    /// True when the GitHub credential came from the `gh` CLI rather than one Keel stores.
    pub github_via_gh: bool,
    pub cloudflare: Option<Vec<cloudflare::Account>>,
}

pub async fn status() -> Json<Connections> {
    let stored = credentials::load(credentials::Kind::GitHub).is_some();
    Json(Connections {
        claude: which_claude(),
        github: github::current().await,
        github_via_gh: !stored && credentials::github_from_gh_cli().is_some(),
        cloudflare: cloudflare::current().await,
    })
}

/// Whether the `claude` binary Keel drives is actually installed.
pub fn which_claude() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[derive(Deserialize)]
pub struct TokenBody {
    pub token: String,
}

/// Verify a GitHub token before storing it.
///
/// Verification first, always: storing an unverified token means the failure surfaces later, in the
/// middle of some other operation, with nothing pointing at the credential as the cause.
pub async fn connect_github(Json(body): Json<TokenBody>) -> ApiResult<github::Account> {
    let account = github::verify(body.token.trim()).await.map_err(bad)?;
    credentials::store(credentials::Kind::GitHub, body.token.trim()).map_err(bad)?;
    Ok(Json(account))
}

pub async fn connect_cloudflare(Json(body): Json<TokenBody>) -> ApiResult<Vec<cloudflare::Account>> {
    let accounts = cloudflare::verify(body.token.trim()).await.map_err(bad)?;
    credentials::store(credentials::Kind::Cloudflare, body.token.trim()).map_err(bad)?;
    Ok(Json(accounts))
}

#[derive(Deserialize)]
pub struct ProviderBody {
    pub provider: String,
}

pub async fn disconnect(Json(body): Json<ProviderBody>) -> ApiResult<bool> {
    let kind = match body.provider.as_str() {
        "github" => credentials::Kind::GitHub,
        "cloudflare" => credentials::Kind::Cloudflare,
        other => return Err(bad(format!("unknown provider `{other}`"))),
    };
    credentials::clear(kind).map_err(bad)?;
    Ok(Json(true))
}

pub async fn repos() -> ApiResult<Vec<github::Repo>> {
    let token = credentials::github_token()
        .ok_or_else(|| bad("connect GitHub first"))?;
    github::repos(&token).await.map(Json).map_err(bad)
}

#[derive(Deserialize)]
pub struct CloneBody {
    pub clone_url: String,
    pub name: String,
    /// Where to put it. Defaults to the parent of the repository currently open, which is almost
    /// always where the user keeps their other projects.
    pub parent: Option<String>,
}

#[derive(Serialize)]
pub struct OpenedRepo {
    pub path: String,
}

pub async fn clone(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CloneBody>,
) -> ApiResult<OpenedRepo> {
    let parent = match &body.parent {
        Some(p) => Utf8PathBuf::from(p),
        None => state
            .repo()
            .parent()
            .map(ToOwned::to_owned)
            .ok_or_else(|| bad("no parent directory to clone into"))?,
    };

    let path = github::clone(&body.clone_url, &parent, &body.name).map_err(bad)?;
    state.set_repo(path.clone());
    Ok(Json(OpenedRepo {
        path: path.to_string(),
    }))
}

#[derive(Deserialize)]
pub struct OpenBody {
    pub path: String,
}

/// Point Keel at a different local directory.
pub async fn open_repo(
    State(state): State<Arc<AppState>>,
    Json(body): Json<OpenBody>,
) -> ApiResult<OpenedRepo> {
    let path = Utf8PathBuf::from(shellexpand(&body.path));
    let path = path
        .canonicalize_utf8()
        .map_err(|_| bad(format!("no such directory: {path}")))?;
    if !path.is_dir() {
        return Err(bad("that path is not a directory"));
    }
    state.set_repo(path.clone());
    Ok(Json(OpenedRepo {
        path: path.to_string(),
    }))
}

/// Expand a leading `~`, which is what people type.
fn shellexpand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}{rest}"),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_a_leading_tilde() {
        unsafe { std::env::set_var("HOME", "/Users/x") };
        assert_eq!(shellexpand("~/Dev/repo"), "/Users/x/Dev/repo");
        assert_eq!(shellexpand("/abs/path"), "/abs/path");
        assert_eq!(shellexpand("relative"), "relative");
    }
}
