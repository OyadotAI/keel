//! File operations the tree's context menu performs.
//!
//! Every path goes through [`crate::tree::resolve`], so an operation can only reach inside the
//! repository or the user's Claude config — the same boundary reads and writes already respect.
//! A traversal bug here is worse than one in a read: it deletes.
//!
//! Deletion goes to the system trash rather than unlinking. An IDE that permanently destroys a
//! file on a menu click has to be right every time; one that moves it to the trash only has to be
//! recoverable, and the user already knows where to look.

use axum::{Json, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

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
    /// The entry's own name, typed by the user. Required only for a directory that holds a git
    /// repository — see [`delete`].
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Serialize)]
pub struct Stat {
    /// `file` or `dir`.
    pub kind: String,
    /// How many entries are inside, counted to a cap. `None` for a file.
    pub entries: Option<usize>,
    /// Whether more remained beyond the cap — the count is a floor, not a total.
    pub more: bool,
    /// The directory is, or contains, a git repository.
    pub repository: bool,
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
        return Err(bad(
            "a name cannot contain a path separator — move the file instead",
        ));
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
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(req): Json<CreateRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    valid_name(&req.name)?;

    // The parent must already exist and be inside the boundary; the new entry is then a name within
    // it, which is why the name is validated separately and never resolved.
    let parent = if req.parent.is_empty() || req.parent == "." {
        repo.canonicalize_utf8().map_err(bad)?
    } else {
        crate::tree::resolve_dir(&repo, &req.parent).map_err(bad)?
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
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(req): Json<RenameRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    valid_name(&req.name)?;
    let from = crate::tree::resolve(&repo, &req.path).map_err(bad)?;

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
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(req): Json<PathRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    let target = crate::tree::resolve(&repo, &req.path).map_err(bad)?;

    // Deleting the repository from inside the IDE that has it open is never what was meant.
    if Some(&target) == repo.canonicalize_utf8().ok().as_ref() {
        return Err(bad("that is the repository itself"));
    }

    // The open folder is not always a project — it is often the folder projects live in, and then
    // every project inside it is one confirmation away from the trash. A directory holding a git
    // repository is somebody's work with its own history, so it costs a typed name. This is
    // enforced here and not only in the dialog: the check has to hold for anything that can reach
    // the endpoint.
    if target.is_dir() {
        let (_, nested, _) = survey(&target);
        if nested || target.join(".git").exists() {
            let name = target.file_name().unwrap_or_default();
            if req.confirm.as_deref() != Some(name) {
                return Err(bad(format!(
                    "{name} contains a git repository — type its name to confirm"
                )));
            }
        }
    }

    trash(target.as_std_path()).map_err(bad)?;
    Ok(Json(PathResponse {
        path: req.path.clone(),
    }))
}

/// Count entries under a directory, and notice whether a git repository is in there.
///
/// Bounded rather than exhaustive: the number exists to tell someone that "delete" means two
/// thousand files and not two, and past a few thousand the distinction stops mattering while the
/// walk starts costing real time.
fn survey(root: &Utf8Path) -> (usize, bool, bool) {
    const CAP: usize = 2_000;
    let mut count = 0usize;
    let mut repo = false;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            count += 1;
            let name = entry.file_name();
            if name == ".git" {
                repo = true;
            }
            if count >= CAP {
                return (count, repo, true);
            }
            // Symlinks are counted but never followed: a link into $HOME would turn a count into a
            // walk of the whole disk, and a link out of the tree is not this directory's content.
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && let Ok(p) = Utf8PathBuf::from_path_buf(entry.path())
            {
                stack.push(p);
            }
        }
    }
    (count, repo, false)
}

/// Report what deleting a path would actually destroy.
pub async fn stat(
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(req): Json<PathRequest>,
) -> Result<Json<Stat>, (StatusCode, String)> {
    let target = crate::tree::resolve(&repo, &req.path).map_err(bad)?;

    if !target.is_dir() {
        return Ok(Json(Stat {
            kind: "file".into(),
            entries: None,
            more: false,
            repository: false,
        }));
    }

    let (entries, repository, more) = survey(&target);
    Ok(Json(Stat {
        kind: "dir".into(),
        entries: Some(entries),
        more,
        repository: repository || target.join(".git").exists(),
    }))
}

/// Move a path to the platform's Trash.
///
/// On macOS the default route is AppleScript to Finder, which gives the file a working "Put Back"
/// — and fails outright until the user has granted this process automation access to Finder, a
/// prompt nobody expects from an IDE deleting a file. So Finder is tried first for the better
/// outcome, and `NSFileManager` catches the case where that permission is missing. The file lands
/// in the trash either way; only "Put Back" is lost on the fallback.
///
/// Shared with the git panel: discarding an untracked file destroys the only copy there is, so it
/// goes where a deleted file goes rather than being unlinked.
pub fn trash(path: &std::path::Path) -> Result<(), String> {
    // A unit test that discards an untracked file in a scratch repository was putting a real
    // one-byte `new.txt` in the person's Trash on every `make check` — one per turn, renamed by
    // Finder as they piled up. The tests are about what git counts, not about the Trash, so
    // under test the file is unlinked instead.
    #[cfg(test)]
    {
        let done = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        done.map_err(|e| format!("could not remove: {e}"))
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        if trash::delete(path).is_ok() {
            return Ok(());
        }
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path)
            .map_err(|e| format!("could not move to trash: {e}"))
    }
    #[cfg(all(not(target_os = "macos"), not(test)))]
    trash::delete(path).map_err(|e| format!("could not move to trash: {e}"))
}

/// Show a path in the platform's file manager.
pub async fn reveal(
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(req): Json<PathRequest>,
) -> Result<Json<PathResponse>, (StatusCode, String)> {
    let target = crate::tree::resolve(&repo, &req.path).map_err(bad)?;

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

/// Open a URL in the real browser.
///
/// `window.open` does nothing inside a WKWebView unless the host implements the delegate that
/// creates a second web view, so every "open on GitHub" in the application silently did nothing.
/// It is also the wrong behaviour: a run page is GitHub's, and it wants a session and an extension
/// set that Keel's window does not have.
///
/// Lives here rather than in its own module because it is the same kind of thing as `reveal`:
/// Keel asking the host to do something it deliberately will not do itself.
pub async fn open_url(Json(body): Json<OpenUrl>) -> Result<Json<bool>, (StatusCode, String)> {
    // The handler hands a string to the system's URL opener, which will happily launch a `file://`
    // or a custom scheme registered by some other application. Two schemes is the whole allowance.
    if !(body.url.starts_with("https://") || body.url.starts_with("http://")) {
        return Err(bad("only http and https links can be opened"));
    }
    open::that_detached(&body.url).map_err(|e| bad(e.to_string()))?;
    Ok(Json(true))
}

#[derive(serde::Deserialize)]
pub struct OpenUrl {
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The open folder is frequently the folder projects live in rather than a project, and then
    /// every project under it is one click from the trash. This is the check that stops that, and
    /// it lives on the server so the dialog is not the only thing standing between the two.
    #[test]
    fn a_directory_holding_a_repository_needs_its_name_typed() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let plain = root.join("notes");
        std::fs::create_dir_all(plain.join("sub")).unwrap();
        std::fs::write(plain.join("sub/a.txt"), "x").unwrap();
        let (count, repo, _) = survey(&plain);
        assert_eq!(count, 2);
        assert!(!repo, "a plain folder is not a repository");

        let project = root.join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let (_, repo, _) = survey(&project);
        assert!(repo, "a folder holding .git is a repository");

        // And so is a folder that merely contains one, which is the case that lost a project.
        let workspace = root.join("workspace");
        std::fs::create_dir_all(workspace.join("app/.git")).unwrap();
        let (_, repo, _) = survey(&workspace);
        assert!(repo, "a folder containing a repository counts too");
    }

    #[test]
    fn the_survey_stops_rather_than_walking_a_whole_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for i in 0..2_100 {
            std::fs::write(root.join(format!("f{i}")), "").unwrap();
        }
        let (count, _, more) = survey(&root);
        assert!(
            more,
            "the count is reported as a floor once it hits the cap"
        );
        assert!(count <= 2_100);
    }

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
