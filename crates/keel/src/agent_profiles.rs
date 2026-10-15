//! Keel-only display names and avatars for Claude subagents.
//!
//! Kept beside the project rather than in Claude's agent frontmatter: these are presentation
//! details and must not change how Claude loads or selects an agent.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

const LIMIT: usize = 1_400_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub avatar: Option<String>,
}

fn path(repo: &camino::Utf8Path) -> Utf8PathBuf {
    repo.join(".keel").join("subagent-profiles.json")
}

fn read(repo: &camino::Utf8Path) -> Vec<Profile> {
    std::fs::read(path(repo))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<Profile>> {
    let repo = state.repo();
    crate::serve::blocking(
        move || {
            let saved = read(&repo);
            let mut profiles: Vec<Profile> = Vec::new();
            let home = keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into());
            for agent in keel_workspace::discover_agents(&repo, &home) {
                if !profiles.iter().any(|p| p.name == agent.name) {
                    profiles.push(
                        saved
                            .iter()
                            .find(|p| p.name == agent.name)
                            .cloned()
                            .unwrap_or(Profile {
                                name: agent.name,
                                display_name: String::new(),
                                avatar: None,
                            }),
                    );
                }
            }
            profiles.sort_by(|a, b| a.name.cmp(&b.name));
            profiles
        },
        Vec::new(),
    )
    .await
    .into()
}

pub async fn save(
    State(state): State<Arc<AppState>>,
    Json(profile): Json<Profile>,
) -> Result<Json<Vec<Profile>>, (StatusCode, String)> {
    if profile.name.is_empty()
        || profile.name.len() > 64
        || !profile
            .name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err((StatusCode::BAD_REQUEST, "invalid subagent name".into()));
    }
    if profile.display_name.trim().len() > 80 {
        return Err((StatusCode::BAD_REQUEST, "display name is too long".into()));
    }
    if let Some(avatar) = &profile.avatar {
        let valid = avatar.len() <= LIMIT
            && [
                "data:image/png;base64,",
                "data:image/jpeg;base64,",
                "data:image/webp;base64,",
                "data:image/gif;base64,",
            ]
            .iter()
            .any(|p| avatar.starts_with(p));
        if !valid {
            return Err((
                StatusCode::BAD_REQUEST,
                "avatar must be a PNG, JPEG, WebP or GIF image under 1 MB".into(),
            ));
        }
    }
    let repo = state.repo();
    crate::serve::blocking(
        move || {
            let file = path(&repo);
            let mut profiles = read(&repo);
            if let Some(old) = profiles.iter_mut().find(|p| p.name == profile.name) {
                *old = profile;
            } else {
                profiles.push(profile);
            }
            profiles.sort_by(|a, b| a.name.cmp(&b.name));
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(
                file,
                serde_json::to_vec_pretty(&profiles).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>(profiles)
        },
        Err("could not save subagent profile".to_string()),
    )
    .await
    .map(Json)
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}
