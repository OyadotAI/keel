//! Connection management: GitHub, Cloudflare, and switching repositories.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
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

pub async fn connect_cloudflare(
    Json(body): Json<TokenBody>,
) -> ApiResult<Vec<cloudflare::Account>> {
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
    let token = credentials::github_token().ok_or_else(|| bad("connect GitHub first"))?;
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
        // `~/Dev` is what a person types; expanding it here means every caller gets it right
        // rather than each one remembering to.
        Some(p) => Utf8PathBuf::from(match p.strip_prefix('~') {
            Some(rest) => std::env::var("HOME")
                .map(|h| format!("{h}{rest}"))
                .unwrap_or_else(|_| p.clone()),
            None => p.clone(),
        }),
        None => state
            .repo()
            .parent()
            .map(ToOwned::to_owned)
            .ok_or_else(|| bad("no parent directory to clone into"))?,
    };

    // Off the executor: this is the one call in Keel that can take minutes, and running it here
    // stalled every other request for the whole clone — the UI simply stopped responding.
    let (url, name) = (body.clone_url.clone(), body.name.clone());
    let path = tokio::task::spawn_blocking(move || github::clone(&url, &parent, &name))
        .await
        .map_err(|e| bad(e.to_string()))?
        .map_err(bad)?;
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

#[derive(Deserialize)]
pub struct BrowseQuery {
    /// Directory to list. Defaults to the user's home.
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// True when the directory is itself a git repository, which is what you are usually looking for.
    pub repo: bool,
}

#[derive(Serialize)]
pub struct Listing {
    pub path: String,
    /// `None` at the top of the browsable area.
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    /// True when this directory can be opened as a project.
    pub is_repo: bool,
}

/// List directories, for the folder picker.
///
/// Bounded to the user's home. Keel binds to loopback, but any page in the browser can reach a
/// loopback server, so an unbounded filesystem enumerator would be a real disclosure. Home covers
/// essentially every project location while keeping the rest of the disk out of reach.
pub async fn browse(Query(q): Query<BrowseQuery>) -> ApiResult<Listing> {
    let home = std::env::var("HOME").map_err(|_| bad("no home directory"))?;
    let home = Utf8PathBuf::from(home)
        .canonicalize_utf8()
        .map_err(|_| bad("home directory is unreadable"))?;

    let requested = q
        .path
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(|p| Utf8PathBuf::from(shellexpand(p)))
        .unwrap_or_else(|| home.clone());

    let path = requested
        .canonicalize_utf8()
        .map_err(|_| bad(format!("no such directory: {requested}")))?;
    if !path.starts_with(&home) {
        return Err(bad("Keel browses inside your home directory only."));
    }
    if !path.is_dir() {
        return Err(bad("that path is not a directory"));
    }

    let mut entries: Vec<Entry> = std::fs::read_dir(&path)
        .map_err(|e| bad(e.to_string()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| {
            // Hidden directories are noise in a project picker, and node_modules is worse.
            let name = p.file_name().unwrap_or("");
            !name.starts_with('.') && name != "node_modules" && name != "target"
        })
        .map(|p| Entry {
            name: p.file_name().unwrap_or("").to_string(),
            repo: p.join(".git").exists(),
            path: p.to_string(),
        })
        .collect();
    entries.sort_by_key(|e| e.name.to_lowercase());

    Ok(Json(Listing {
        parent: (path != home).then(|| {
            path.parent()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| home.clone())
                .to_string()
        }),
        is_repo: path.join(".git").exists(),
        path: path.to_string(),
        entries,
    }))
}

#[derive(Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
}

/// List the files in one directory, for the skill file strip.
///
/// Allowed inside the repository or the user's Claude home, the same boundary the file reader uses —
/// a skill lives in one or the other and nothing else needs listing.
pub async fn browse_files(
    State(state): State<Arc<AppState>>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Vec<FileEntry>> {
    let requested = q.path.unwrap_or_default();
    let dir = Utf8PathBuf::from(shellexpand(&requested))
        .canonicalize_utf8()
        .map_err(|_| bad("no such directory"))?;

    let repo = state.repo();
    let mut allowed: Vec<Utf8PathBuf> = repo.canonicalize_utf8().into_iter().collect();
    if let Some(home) = keel_workspace::claude_home()
        && let Ok(c) = home.canonicalize_utf8()
    {
        allowed.push(c);
    }
    if !allowed.iter().any(|r| dir.starts_with(r)) {
        return Err(bad(
            "that directory is outside the repository and your Claude config",
        ));
    }

    let mut files: Vec<FileEntry> = std::fs::read_dir(&dir)
        .map_err(|e| bad(e.to_string()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .map(|p| FileEntry {
            name: p.file_name().unwrap_or("").to_string(),
            path: p.to_string(),
        })
        .collect();
    // The manifest first, then everything else alphabetically.
    files.sort_by(|a, b| {
        (a.name != "SKILL.md", a.name.to_lowercase())
            .cmp(&(b.name != "SKILL.md", b.name.to_lowercase()))
    });
    Ok(Json(files))
}

/// Expand a leading `~`, which is what people type.
fn shellexpand(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) => expand_under(&home, path),
        Err(_) => path.to_string(),
    }
}

/// The half that can be tested, which is the half that has the rule in it.
///
/// Separated because the test for it used to be `set_var("HOME", "/Users/x")`, and an environment
/// variable is process-global while `cargo test` is a thread pool. It hijacked `HOME` for whatever
/// else happened to be running: on Linux the `trash` crate resolves `$HOME/.local/share/Trash`, so
/// `discard_all_…` tried to write into `/Users/x` and failed with `PermissionDenied`. It passed on
/// macOS, where the trash does not consult `HOME` — so the race was invisible on the machine the
/// gate runs on and only ever showed up in CI, and only when scheduling happened to line up.
///
/// Rust 2024 made `set_var` `unsafe` for exactly this. The fix is not a lock around it; it is not
/// needing it.
fn expand_under(home: &str, path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_a_leading_tilde() {
        assert_eq!(expand_under("/Users/x", "~/Dev/repo"), "/Users/x/Dev/repo");
        assert_eq!(expand_under("/Users/x", "~"), "/Users/x");
        assert_eq!(expand_under("/Users/x", "/abs/path"), "/abs/path");
        assert_eq!(expand_under("/Users/x", "relative"), "relative");
    }
}
