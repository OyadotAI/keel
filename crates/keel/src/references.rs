//! Checkout-bound file references are validated before a receipt is accepted or an agent starts.
use crate::{agent::ChatQuery, serve::AppState};
use axum::{Json, extract::State, http::StatusCode};
use keel_workspace::Real;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_REFERENCES: usize = 32;

/// Native chat references identify an existing file, never an upload or an arbitrary host path.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileReference {
    pub project: String,
    pub wt: Option<String>,
    pub path: String,
}

/// The destination checkout is independent of the source, so cross-checkout drops can be refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    wt: Option<String>,
    reference: FileReference,
}

fn validate(state: &AppState, wt: Option<&str>, reference: &FileReference) -> Result<(), String> {
    let project = camino::Utf8Path::new(&reference.project)
        .real()
        .map_err(|_| "The source project is no longer available.")?;
    let expected = state
        .repo()
        .real()
        .map_err(|_| "The project checkout is no longer available.")?;
    if reference.wt.as_deref() != wt || project != expected {
        return Err("This file belongs to another checkout.".into());
    }
    crate::tree::validate_reference(&state.checkout(wt)?, &reference.path)
}

/// Validate metadata only; large or binary files remain valid references without being read.
pub async fn check(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Check>,
) -> Result<Json<Value>, (StatusCode, String)> {
    tokio::task::spawn_blocking(move || validate(&state, body.wt.as_deref(), &body.reference))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"ok":true})))
}

/// Recheck at submission so a stale draft cannot silently point at a removed or foreign file.
pub fn validate_query(state: &AppState, query: &ChatQuery) -> Result<(), String> {
    if query.references.len() > MAX_REFERENCES {
        return Err("Too many file references.".into());
    }
    for reference in &query.references {
        validate(state, query.wt.as_deref(), reference)?;
    }
    Ok(())
}

/// Encode relative paths once for every provider while keeping the visible user prompt unchanged.
pub fn provider_prompt(
    state: &AppState,
    query: &ChatQuery,
    cwd: &camino::Utf8Path,
) -> Result<String, String> {
    validate_query(state, query)?;
    if query.references.is_empty() {
        return Ok(query.prompt.clone());
    }
    if cwd.real().map_err(|e| e.to_string())?
        != state
            .checkout(query.wt.as_deref())?
            .real()
            .map_err(|e| e.to_string())?
    {
        return Err("File references require this lane’s checkout.".into());
    }
    let paths: Vec<&str> = query.references.iter().map(|r| r.path.as_str()).collect();
    let context = serde_json::to_string(&paths).map_err(|e| e.to_string())?;
    Ok(format!(
        "{}\n\nReferenced files in this checkout (JSON paths):\n{context}",
        query.prompt
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(root: &str, path: &str) -> ChatQuery {
        serde_json::from_value(
            json!({"prompt":"Change this", "references":[{"project":root,"path":path}]}),
        )
        .unwrap()
    }
    #[test]
    fn references_preserve_unicode_spaces_and_the_visible_prompt_for_every_provider() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::write(root.join("日本語 file.rs"), "test").unwrap();
        let state = AppState::new(root.clone());
        for provider in ["claude", "codex", "antigravity"] {
            let mut q = query(root.as_str(), "日本語 file.rs");
            q.provider = Some(provider.into());
            let sent = provider_prompt(&state, &q, &root).unwrap();
            assert!(sent.contains("[\"日本語 file.rs\"]"));
            assert_eq!(q.prompt, "Change this");
        }
    }
    #[test]
    fn stale_or_foreign_checkout_references_cannot_be_submitted() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::write(root.join("file.rs"), "test").unwrap();
        let state = AppState::new(root.clone());
        let mut q = query(root.as_str(), "file.rs");
        q.references[0].wt = Some("other".into());
        assert!(
            validate_query(&state, &q)
                .unwrap_err()
                .contains("another checkout")
        );
        q.references[0].wt = None;
        std::fs::remove_file(root.join("file.rs")).unwrap();
        assert!(validate_query(&state, &q).is_err());
    }
}
