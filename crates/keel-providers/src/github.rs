//! GitHub: identity, repository listing, and cloning.

use crate::credentials;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

const API: &str = "https://api.github.com";
const UA: &str = "keel";

/// The authenticated account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// A repository the user can open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub description: Option<String>,
    pub clone_url: String,
    pub default_branch: Option<String>,
    pub updated_at: Option<String>,
    pub language: Option<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .expect("building an HTTP client with no custom TLS config cannot fail")
}

/// Verify a token and return who it belongs to.
pub async fn verify(token: &str) -> Result<Account, String> {
    let response = client()
        .get(format!("{API}/user"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("that token was rejected — check it has not expired".into());
    }
    if !response.status().is_success() {
        return Err(format!("GitHub returned {}", response.status()));
    }

    response
        .json::<Account>()
        .await
        .map_err(|e| format!("unexpected response from GitHub: {e}"))
}

/// Repositories the user can push to, most recently updated first.
///
/// Sorted by update time rather than name because the repository someone wants to open is
/// overwhelmingly one they touched recently.
pub async fn repos(token: &str) -> Result<Vec<Repo>, String> {
    let response = client()
        .get(format!("{API}/user/repos"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .query(&[
            ("sort", "updated"),
            ("per_page", "100"),
            ("affiliation", "owner,collaborator"),
        ])
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("GitHub returned {}", response.status()));
    }

    response
        .json::<Vec<Repo>>()
        .await
        .map_err(|e| format!("unexpected response from GitHub: {e}"))
}

/// Clone `url` into `parent/name`, returning the new path.
///
/// Shells out to `git` rather than linking a git library: the user's own git already has their
/// credential helper, SSH agent, proxy settings and hooks configured, and reimplementing that is
/// how a tool ends up unable to clone from a private host.
pub fn clone(url: &str, parent: &Utf8Path, name: &str) -> Result<camino::Utf8PathBuf, String> {
    let target = parent.join(name);
    if target.exists() {
        return Err(format!("{target} already exists"));
    }

    let out = std::process::Command::new("git")
        .current_dir(parent)
        .args(["clone", "--depth", "1", url, name])
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(target)
}

/// The currently connected account, if any.
pub async fn current() -> Option<Account> {
    let token = credentials::github_token()?;
    verify(&token).await.ok()
}
