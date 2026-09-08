//! Creating a subagent.
//!
//! A subagent is a Markdown file with YAML frontmatter under `.claude/agents/`, so this writes one
//! and opens it. The form exists because the frontmatter has required keys with meaning attached —
//! `description` is what the main agent reads to decide whether to delegate at all — and a file
//! created from a blank template gets those wrong quietly.
//!
//! There is deliberately no equivalent for hooks. A hook is a shell command Claude Code runs on
//! your behalf when a tool fires; a repository that ships one achieves code execution the moment
//! anybody opens it, which is why `keel-harness::trust` quarantines them and why the scanner rates
//! a repo-supplied `.claude/settings.json` Critical. Keel is not going to grow a button that
//! writes one.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

#[derive(Deserialize)]
pub struct NewAgent {
    pub name: String,
    /// When the main agent should delegate to this one. Read by the model, not by a person.
    pub description: String,
    /// Comma-separated tool names. Empty means "inherit everything the main agent has".
    #[serde(default)]
    pub tools: String,
    /// The agent's own system prompt.
    #[serde(default)]
    pub prompt: String,
    /// `project` writes `.claude/agents`; `user` writes `~/.claude/agents`.
    #[serde(default)]
    pub scope: String,
}

#[derive(Serialize)]
pub struct Created {
    /// Repository-relative when it is in the project, absolute when it is in the user's home.
    pub path: String,
}

fn bad(m: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, m.to_string())
}

/// A name that is safe as a filename and valid as the frontmatter's `name`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Quote a value for YAML.
///
/// A description is a sentence, and sentences contain colons. Unquoted, `Use this: for reviews`
/// parses as a nested mapping and the file loads with no description at all — which reads to the
/// main agent as an agent it has no reason to ever call.
fn yaml(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewAgent>,
) -> Result<Json<Created>, (StatusCode, String)> {
    if !valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens — the name becomes a filename.",
        ));
    }
    let description = req.description.trim();
    if description.is_empty() {
        return Err(bad(
            "A description is required: it is the only thing the main agent reads when deciding \
             whether to delegate.",
        ));
    }

    let dir: Utf8PathBuf = if req.scope == "user" {
        let home = std::env::var("HOME").map_err(|_| bad("no home directory"))?;
        Utf8PathBuf::from(home).join(".claude/agents")
    } else {
        state.repo().join(".claude/agents")
    };
    std::fs::create_dir_all(&dir).map_err(bad)?;

    let path = dir.join(format!("{}.md", req.name));
    if path.exists() {
        return Err(bad(format!("{}.md already exists", req.name)));
    }

    let mut front = format!(
        "---\nname: {}\ndescription: {}\n",
        req.name,
        yaml(description)
    );
    let tools: Vec<&str> = req
        .tools
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if !tools.is_empty() {
        front.push_str(&format!("tools: {}\n", tools.join(", ")));
    }
    front.push_str("---\n\n");

    let body = if req.prompt.trim().is_empty() {
        // A placeholder that says what belongs here rather than an empty file. An agent whose
        // prompt is blank inherits nothing useful and behaves like a worse copy of the main one.
        "Describe what this agent does, what it should read before acting, and what it should \
         return. Be specific about the shape of its answer — a subagent's reply is consumed by \
         another model, not by a person.\n"
            .to_string()
    } else {
        format!("{}\n", req.prompt.trim())
    };

    std::fs::write(&path, front + &body).map_err(bad)?;

    let repo = state.repo();
    let shown = match repo.canonicalize_utf8() {
        Ok(root) => path
            .strip_prefix(&root)
            .map(|p| p.to_string())
            .unwrap_or_else(|_| path.to_string()),
        Err(_) => path.to_string(),
    };
    Ok(Json(Created { path: shown }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_has_to_work_as_a_filename() {
        assert!(valid_name("code-reviewer"));
        assert!(!valid_name("Code Reviewer"));
        assert!(!valid_name("../escape"));
        assert!(!valid_name("-leading"));
        assert!(!valid_name(""));
    }

    /// A description is a sentence and sentences contain colons. Unquoted, YAML reads
    /// `Use this: for reviews` as a nested mapping and the description is lost — and an agent with
    /// no description is one the main agent has no reason to ever call.
    #[test]
    fn a_description_survives_its_own_punctuation() {
        assert_eq!(yaml("plain"), "\"plain\"");
        assert_eq!(
            yaml("Use this: after any change"),
            "\"Use this: after any change\""
        );
        assert_eq!(yaml("says \"hi\""), "\"says \\\"hi\\\"\"");
    }
}
