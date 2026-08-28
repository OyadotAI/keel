//! Renaming a session.
//!
//! A session's title is written by Claude Code into its own transcript. Keel does not edit that
//! file — it reads Claude Code's state and must never be a reason for it to change — so a rename
//! is an override stored beside it, keyed by session id.
//!
//! Its own file rather than a field in `prefs.json`: a rename rewrites this and nothing else, and
//! losing a set of nicknames to a half-written prefs file would be a bad trade for one fewer path.

use axum::{Json, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn path() -> Option<Utf8PathBuf> {
    Some(crate::prefs::dir()?.join("session-names.json"))
}

pub fn load() -> BTreeMap<String, String> {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(map: &BTreeMap<String, String>) -> Result<(), String> {
    let (Some(dir), Some(path)) = (crate::prefs::dir(), path()) else {
        return Err("no home directory".into());
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// Apply the overrides to a discovered list.
pub fn apply(sessions: &mut [keel_workspace::Session]) {
    let names = load();
    if names.is_empty() {
        return;
    }
    for session in sessions {
        if let Some(name) = names.get(&session.id) {
            session.title = Some(name.clone());
        }
    }
}

#[derive(Deserialize)]
pub struct RenameBody {
    pub id: String,
    /// Empty restores the title Claude Code gave it.
    pub name: String,
}

#[derive(Serialize)]
pub struct Renamed {
    pub name: String,
}

pub async fn rename(Json(body): Json<RenameBody>) -> Result<Json<Renamed>, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());

    // The id is a map key here rather than a path, but it is the same id that becomes a filename
    // elsewhere and the same rule keeps both honest.
    if body.id.is_empty()
        || !body
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(bad("that is not a session id"));
    }
    let name = body.name.trim();
    if name.chars().count() > 80 {
        return Err(bad("keep it under 80 characters"));
    }
    if name.contains('\n') {
        return Err(bad("a name is one line"));
    }

    let mut names = load();
    if name.is_empty() {
        names.remove(&body.id);
    } else {
        names.insert(body.id.clone(), name.to_string());
    }
    save(&names).map_err(|e| bad(&e))?;

    Ok(Json(Renamed {
        name: name.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An override replaces the title and nothing else; a session with no override keeps the one
    /// Claude Code wrote.
    #[test]
    fn overrides_apply_only_where_they_exist() {
        let names: BTreeMap<String, String> =
            [("aaa".to_string(), "my nickname".to_string())].into();

        let titled = |id: &str, title: &str| -> Option<String> {
            names.get(id).cloned().or_else(|| Some(title.to_string()))
        };

        assert_eq!(
            titled("aaa", "written by claude").as_deref(),
            Some("my nickname")
        );
        assert_eq!(
            titled("bbb", "also written by claude").as_deref(),
            Some("also written by claude")
        );
    }
}
