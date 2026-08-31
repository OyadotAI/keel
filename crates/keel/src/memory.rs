//! `# something` in the composer, the way `#` works in the terminal.
//!
//! A line the agent reads every turn, appended to the project's own CLAUDE.md under one heading.
//! Not a hidden store — it is a file in the repository, reviewed like any other line of it. This
//! is all that survived `review.rs`, which was the staff-engineer repo review: a different product
//! from the one this is.

use crate::serve::{AppState, Checkout};
use axum::{Json, extract::State};
use camino::Utf8Path;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct MemoryBody {
    pub text: String,
}

/// `# something` in the composer, the way `#` works in the terminal: a line the agent reads
/// every turn, appended to the project's own CLAUDE.md under one heading. Not a hidden store —
/// it is a file in the repository, reviewed like any other line of it.
pub async fn api_memory(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Json(b): Json<MemoryBody>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let text = b.text.trim().trim_start_matches('#').trim();
    if text.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "nothing to remember".into(),
        ));
    }
    remember(&repo, text)
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))
}

const MEMORY_HEADING: &str = "## Project memory";

pub fn remember(repo: &Utf8Path, text: &str) -> Result<String, String> {
    let path = repo.join("CLAUDE.md");
    let line = format!("- {}\n", text.replace('\n', " ").trim());
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let next = match existing.find(MEMORY_HEADING) {
        // Under the heading, at the end of its section, so the newest note is the last line
        // of it rather than the first line of whatever follows.
        Some(at) => {
            let after = at + MEMORY_HEADING.len();
            let end = existing[after..]
                .find("\n## ")
                .map(|i| after + i + 1)
                .unwrap_or(existing.len());
            let mut out = String::with_capacity(existing.len() + line.len());
            out.push_str(existing[..end].trim_end());
            out.push('\n');
            out.push_str(&line);
            if end < existing.len() {
                out.push('\n');
                out.push_str(&existing[end..]);
            }
            out
        }
        None => {
            let name = repo.file_name().unwrap_or("project");
            let head = if existing.trim().is_empty() {
                format!("# {name}\n")
            } else {
                existing
            };
            format!(
                "{}\n{MEMORY_HEADING}\n\nWhat the team told Keel to remember here. Read every turn; edit or delete a line like any other.\n\n{line}",
                head.trim_end()
            )
        }
    };
    std::fs::write(&path, next).map_err(|e| e.to_string())?;
    Ok("CLAUDE.md".into())
}
