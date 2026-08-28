//! File operations the tree's context menu performs.
//!
//! Every path goes through [`crate::api::resolve`], so an operation can only reach inside the
//! repository or the user's Claude config — the same boundary reads and writes already respect.
//! A traversal bug here is worse than one in a read: it deletes.
//!
//! Deletion goes to the system trash rather than unlinking. An IDE that permanently destroys a
//! file on a menu click has to be right every time; one that moves it to the trash only has to be
//! recoverable, and the user already knows where to look.

use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

#[derive(Deserialize)]
pub struct CreateRequest {
    /// Directory the new entry goes in, relative to the repository.
    pub parent: String,
    pub name: String,
    /// `file` or `dir`.
    pub kind: String,
}

#[derive(Deserialize)]
pub struct PathRequest {
    pub path: String,
}

#[derive(Deserialize)]
pub struct RenameRequest {
    pub path: String,
    /// The new basename. Renaming is not moving — a name with a separator in it is rejected rather
    /// than quietly relocating the file somewhere the user did not look at.
    pub name: String,
}

#[derive(Serialize)]
pub struct PathResponse {
    /// Repository-relative, because that is what the tree and the editor's tabs are keyed by.
    pub path: String,
}

fn bad(m: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, m.to_string())
}

/// A basename that cannot escape the directory it was meant for.
fn valid_name(name: &str) -> Result<(), (StatusCode, String)> {
    if name.is_empty() || name.len() > 255 {
        return Err(bad("a name is required"));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(bad("a name cannot contain a path separator — move the file instead"));
    }
    if name == "." || name == ".." {
        return Err(bad("that is not a name"));
    }
    Ok(())
}

/// Express a path back to the UI the way the tree keys it.
fn relative(repo: &Utf8Path, path: &Utf8Path) -> String {
    match repo.canonicalize_utf8() {
        Ok(root) => path
            .strip_prefix(&root)
            .map(|p| p.to_string())
            .unwrap_or_else(|_| path.to_string()),
        Err(_) => path.to_string(),
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    valid_name(&req.name)?;
    let repo = state.repo();

    // The parent must already exist and be inside the boundary; the new entry is then a name within
    // it, which is why the name is validated separately and never resolved.
    let parent = if req.parent.is_empty() || req.parent == "." {
        repo.canonicalize_utf8().map_err(bad)?
    } else {
        crate::api::resolve_dir(&repo, &req.parent).map_err(bad)?
    };

    let target = parent.join(&req.name);
    if target.exists() {
        return Err(bad(format!("{} already exists", req.name)));
    }

    match req.kind.as_str() {
        "dir" => std::fs::create_dir(&target).map_err(bad)?,
        _ => std::fs::write(&target, "").map_err(bad)?,
    }

    Ok(Json(PathResponse {
        path: relative(&repo, &target),
    }))
}

pub async fn rename(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    valid_name(&req.name)?;
    let repo = state.repo();
    let from = crate::api::resolve(&repo, &req.path).map_err(bad)?;

    if Some(&from) == repo.canonicalize_utf8().ok().as_ref() {
        return Err(bad("that is the repository itself"));
    }

    let parent: Utf8PathBuf = from.parent().ok_or_else(|| bad("invalid path"))?.to_owned();
    let to = parent.join(&req.name);
    if to.exists() {
        return Err(bad(format!("{} already exists", req.name)));
    }

    std::fs::rename(&from, &to).map_err(bad)?;
    Ok(Json(PathResponse {
        path: relative(&repo, &to),
    }))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PathRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    let repo = state.repo();
    let target = crate::api::resolve(&repo, &req.path).map_err(bad)?;

    // Deleting the repository from inside the IDE that has it open is never what was meant.
    if Some(&target) == repo.canonicalize_utf8().ok().as_ref() {
        return Err(bad("that is the repository itself"));
    }

    to_trash(target.as_std_path()).map_err(bad)?;
    Ok(Json(PathResponse { path: req.path }))
}

/// Move a path to the system trash.
///
/// On macOS the default route is AppleScript to Finder, which gives the file a working "Put Back"
/// — and fails outright until the user has granted this process automation access to Finder, a
/// prompt nobody expects from an IDE deleting a file. So Finder is tried first for the better
/// outcome, and `NSFileManager` catches the case where that permission is missing. The file lands
/// in the trash either way; only "Put Back" is lost on the fallback.
fn to_trash(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        if trash::delete(path).is_ok() {
            return Ok(());
        }
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        return ctx
            .delete(path)
            .map_err(|e| format!("could not move to trash: {e}"));
    }
    #[cfg(not(target_os = "macos"))]
    trash::delete(path).map_err(|e| format!("could not move to trash: {e}"))
}

/// Show a path in the platform's file manager.
pub async fn reveal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PathRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    let repo = state.repo();
    let target = crate::api::resolve(&repo, &req.path).map_err(bad)?;

    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open")
        .arg("-R")
        .arg(target.as_str())
        .spawn();
    #[cfg(target_os = "linux")]
    let spawned = std::process::Command::new("xdg-open")
        .arg(target.parent().unwrap_or(&target).as_str())
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let spawned: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("unsupported platform"));

    spawned.map_err(|e| bad(format!("could not open the file manager: {e}")))?;
    Ok(Json(PathResponse { path: req.path }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_carry_a_path() {
        assert!(valid_name("index.ts").is_ok());
        assert!(valid_name(".gitignore").is_ok());
        assert!(valid_name("../escape").is_err());
        assert!(valid_name("nested/file").is_err());
        assert!(valid_name("..").is_err());
        assert!(valid_name("").is_err());
    }
}
