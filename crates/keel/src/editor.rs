//! Explicit human edits: bounded, checkout-scoped, conflict-checked and written atomically.
use crate::{
    serve::{AppState, Checkout},
    writes::Failure,
};
use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use serde::Deserialize;
use std::{io::Write, sync::Arc};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
pub struct Save {
    pub path: String,
    pub text: String,
    /// The last complete text read by the editor, never a timestamp with coarse precision.
    pub original: String,
}

pub async fn save(
    Checkout(repo): Checkout,
    State(state): State<Arc<AppState>>,
    Json(request): Json<Save>,
) -> Result<Json<crate::tree::FileContent>, Failure> {
    crate::serve::in_blocking(move || {
        let _held = crate::writes::hold(&state, &repo, "editor save")?;
        save_file(&repo, &request)?;
        Ok(Json(crate::tree::FileContent {
            text: Some(request.text),
            note: None,
        }))
    })
    .await
}

fn bad(message: impl ToString) -> Failure {
    (StatusCode::BAD_REQUEST, message.to_string())
}

/// Writes never follow links or reach git's own state, even when reads can preview that path.
fn writable_path(root: &Utf8Path, requested: &str) -> Result<Utf8PathBuf, Failure> {
    let relative = Utf8Path::new(requested);
    if relative.is_absolute()
        || relative.components().any(|part| {
            !matches!(part, camino::Utf8Component::Normal(_))
                || matches!(part.as_str(), ".git" | ".keel")
        })
    {
        return Err(bad("Choose a regular file inside this checkout."));
    }
    let root = root.real().map_err(bad)?;
    crate::writes::no_link_under(&root, requested).map_err(bad)?;
    let path = root.join(relative);
    let meta = std::fs::symlink_metadata(&path).map_err(bad)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(bad(
            "Only regular files can be edited; symbolic links are read-only.",
        ));
    }
    Ok(path)
}

fn save_file(root: &Utf8Path, request: &Save) -> Result<(), Failure> {
    if request.text.len() > MAX_BYTES
        || request.original.len() > MAX_BYTES
        || request.text.contains('\0')
    {
        return Err(bad("The editor saves UTF-8 text files up to 1 MiB."));
    }
    let path = writable_path(root, &request.path)?;
    let original = crate::tree::read_file(root, &request.path)
        .map_err(bad)?
        .text;
    if original.as_deref() != Some(request.original.as_str()) {
        return Err((StatusCode::CONFLICT, "This file changed on disk. Your edits are kept. Review the latest version before saving.".into()));
    }
    replace(&path, request.text.as_bytes()).map_err(bad)
}

/// Keep executable bits and permissions; a new temporary file must not turn a script into data.
fn replace(path: &Utf8Path, text: &[u8]) -> Result<(), String> {
    let permissions = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Missing parent directory")?)
            .map_err(|e| e.to_string())?;
    temporary.write_all(text).map_err(|e| e.to_string())?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        std::fs::write(root.join("code.ts"), "original\n").unwrap();
        (dir, root)
    }
    fn request() -> Save {
        Save {
            path: "code.ts".into(),
            text: "edited\n".into(),
            original: "original\n".into(),
        }
    }
    #[test]
    fn saves_the_selected_checkout_and_preserves_permissions() {
        let (_dir, root) = fixture();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("code.ts"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        save_file(&root, &request()).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("code.ts")).unwrap(),
            "edited\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.join("code.ts"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
    }
    #[test]
    fn refuses_conflicts_without_overwriting_other_edits() {
        let (_dir, root) = fixture();
        std::fs::write(root.join("code.ts"), "another editor\n").unwrap();
        assert_eq!(
            save_file(&root, &request()).unwrap_err().0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            std::fs::read_to_string(root.join("code.ts")).unwrap(),
            "another editor\n"
        );
    }
    #[test]
    fn refuses_traversal_git_state_binary_and_oversized_saves() {
        let (_dir, root) = fixture();
        for path in [
            "../code.ts",
            "/etc/passwd",
            ".git/config",
            ".keel/turns/private.json",
            "missing.ts",
            ".",
        ] {
            assert!(
                save_file(
                    &root,
                    &Save {
                        path: path.into(),
                        ..request()
                    }
                )
                .is_err(),
                "{path}"
            );
        }
        for text in ["\0".into(), "x".repeat(MAX_BYTES + 1)] {
            assert!(save_file(&root, &Save { text, ..request() }).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_in_any_component() {
        let (_dir, root) = fixture();
        std::os::unix::fs::symlink(root.join("code.ts"), root.join("link.ts")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("linked")).unwrap();
        for path in ["link.ts", "linked/code.ts"] {
            assert!(
                save_file(
                    &root,
                    &Save {
                        path: path.into(),
                        ..request()
                    }
                )
                .is_err()
            );
        }
    }
    #[tokio::test]
    async fn saving_waits_for_an_agent_writer() {
        let (_dir, root) = fixture();
        let state = Arc::new(AppState::empty());
        let held = crate::writes::hold(&state, &root, "another writer").unwrap();
        let error = save(
            Checkout(root.clone()),
            State(state.clone()),
            Json(request()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        drop(held);
        let _ = save(Checkout(root.clone()), State(state), Json(request()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("code.ts")).unwrap(),
            "edited\n"
        );
    }
}
