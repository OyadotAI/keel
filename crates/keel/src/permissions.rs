//! Which commands the agent may run, and who decides.
//!
//! `--permission-mode acceptEdits` lets the agent write files but not run arbitrary shell commands,
//! and headless `claude -p` has nobody to ask — so a denied command just fails and the agent
//! correctly reports that it could not proceed. This CLI has no `--permission-prompt-tool`, so a
//! live prompt is not available either.
//!
//! What is available is `--settings`, which carries `permissions.allow` rules. So Keel owns the
//! allowlist: a denial surfaces in the UI, the user approves it there, and the rule is passed on
//! every subsequent run. Approval stays a human act; it just happens in the IDE instead of a
//! terminal prompt.

use axum::{Json, extract::State};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};

use crate::serve::AppState;

/// Rules approved only for as long as this Keel process lives.
fn session_rules() -> &'static Mutex<BTreeSet<String>> {
    static RULES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    RULES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn store_path(repo: &Utf8Path) -> camino::Utf8PathBuf {
    repo.join(".keel").join("permissions.json")
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    allow: BTreeSet<String>,
}

fn load(repo: &Utf8Path) -> BTreeSet<String> {
    std::fs::read_to_string(store_path(repo))
        .ok()
        .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
        .map(|s| s.allow)
        .unwrap_or_default()
}

fn save(repo: &Utf8Path, allow: &BTreeSet<String>) -> Result<(), String> {
    let path = store_path(repo);
    std::fs::create_dir_all(path.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&Stored {
        allow: allow.clone(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// Rules the project already declares by its own toolchain.
///
/// Seeding these is not Keel deciding on the user's behalf: running the build and test commands the
/// project itself defines is the reason the agent is here. Anything beyond them still needs asking.
pub fn defaults(repo: &Utf8Path) -> Vec<String> {
    let mut out = vec![
        "Bash(git status *)".into(),
        "Bash(git diff *)".into(),
        "Bash(git log *)".into(),
    ];
    if repo.join("package.json").exists() {
        let bun = repo.join("bun.lock").exists() || repo.join("bun.lockb").exists();
        out.push(if bun {
            "Bash(bun *)".into()
        } else {
            "Bash(npm *)".into()
        });
        out.push("Bash(npx *)".into());
    }
    if repo.join("Cargo.toml").exists() {
        out.push("Bash(cargo *)".into());
    }
    if repo.join("Makefile").exists() {
        out.push("Bash(make *)".into());
    }
    out
}

/// Every rule that should be passed to the CLI for this repository.
pub fn effective(repo: &Utf8Path) -> Vec<String> {
    let mut all: BTreeSet<String> = load(repo);
    all.extend(session_rules().lock().expect("rules lock").iter().cloned());
    all.into_iter().collect()
}

/// The `--settings` payload carrying those rules.
pub fn settings_json(repo: &Utf8Path) -> String {
    serde_json::json!({ "permissions": { "allow": effective(repo) } }).to_string()
}

#[derive(Serialize)]
pub struct PermissionsView {
    pub project: Vec<String>,
    pub session: Vec<String>,
    /// Rules the project's own toolchain justifies, not yet approved.
    pub suggested: Vec<String>,
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<PermissionsView> {
    let repo = state.repo();
    let project = load(&repo);
    let session: Vec<String> = session_rules()
        .lock()
        .expect("rules lock")
        .iter()
        .cloned()
        .collect();
    let suggested = defaults(&repo)
        .into_iter()
        .filter(|d| !project.contains(d) && !session.contains(d))
        .collect();

    Json(PermissionsView {
        project: project.into_iter().collect(),
        session,
        suggested,
    })
}

#[derive(Deserialize)]
pub struct RuleBody {
    pub rule: String,
    /// `project` persists to `.keel/permissions.json`; `session` lasts until Keel restarts.
    pub scope: String,
}

/// A rule must look like a tool pattern, not a shell fragment.
///
/// The rule text reaches Claude Code's permission matcher, not a shell, but a malformed rule that
/// silently matches nothing is worse than a refusal — it would look approved and keep failing.
fn valid_rule(rule: &str) -> bool {
    !rule.is_empty()
        && rule.len() <= 200
        && !rule.contains('\n')
        && rule.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

pub async fn add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RuleBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    let bad = |m: &str| (axum::http::StatusCode::BAD_REQUEST, m.to_string());
    if !valid_rule(&body.rule) {
        return Err(bad(
            "That does not look like a permission rule, e.g. `Bash(bun *)`.",
        ));
    }

    match body.scope.as_str() {
        "session" => {
            session_rules()
                .lock()
                .expect("rules lock")
                .insert(body.rule);
        }
        "project" => {
            let repo = state.repo();
            let mut rules = load(&repo);
            rules.insert(body.rule);
            save(&repo, &rules).map_err(|e| bad(&e))?;
        }
        _ => return Err(bad("scope must be `project` or `session`")),
    }
    Ok(Json(true))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RuleBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    if body.scope == "session" {
        session_rules()
            .lock()
            .expect("rules lock")
            .remove(&body.rule);
    } else {
        let repo = state.repo();
        let mut rules = load(&repo);
        rules.remove(&body.rule);
        save(&repo, &rules).map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;
    }
    Ok(Json(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    fn repo(files: &[&str]) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        for f in files {
            std::fs::write(root.join(f), "").expect("write");
        }
        (dir, root)
    }

    #[test]
    fn defaults_follow_the_projects_own_toolchain() {
        let (_d, root) = repo(&["package.json", "bun.lock"]);
        let d = defaults(&root);
        assert!(d.contains(&"Bash(bun *)".to_string()));
        assert!(!d.contains(&"Bash(cargo *)".to_string()));

        let (_d2, rust) = repo(&["Cargo.toml", "Makefile"]);
        let d = defaults(&rust);
        assert!(d.contains(&"Bash(cargo *)".to_string()));
        assert!(d.contains(&"Bash(make *)".to_string()));
        assert!(!d.contains(&"Bash(bun *)".to_string()));
    }

    #[test]
    fn npm_is_chosen_when_there_is_no_bun_lockfile() {
        let (_d, root) = repo(&["package.json"]);
        assert!(defaults(&root).contains(&"Bash(npm *)".to_string()));
    }

    #[test]
    fn project_rules_round_trip() {
        let (_d, root) = repo(&[]);
        let mut rules = BTreeSet::new();
        rules.insert("Bash(make *)".to_string());
        save(&root, &rules).expect("save");
        assert!(load(&root).contains("Bash(make *)"));
    }

    #[test]
    fn malformed_rules_are_refused() {
        assert!(valid_rule("Bash(bun *)"));
        assert!(valid_rule("Edit"));
        assert!(!valid_rule("bun install")); // a shell command, not a rule
        assert!(!valid_rule(""));
        assert!(!valid_rule("Bash(x)\nBash(y)"));
    }

    #[test]
    fn settings_payload_is_shaped_the_way_the_cli_expects() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root)).expect("valid json");
        assert!(json["permissions"]["allow"].is_array());
    }
}
