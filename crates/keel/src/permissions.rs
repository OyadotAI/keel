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
/// The settings Keel hands `claude`: the allowlist, and the hook that makes the agent wait.
///
/// Note whose hook this is. Keel quarantines a *repository's* `.claude/settings.json` because a
/// hook there is a shell command that runs on the machine of whoever opens the repo. This one is
/// Keel's own, points at Keel's own binary, and is passed on the command line rather than read
/// from the working tree — the thing that made repo hooks dangerous is exactly the thing this
/// does not do.
pub fn settings_json(repo: &Utf8Path, port: u16) -> String {
    let mut settings = serde_json::json!({ "permissions": { "allow": effective(repo) } });

    // Without an executable there is nothing to point the hook at, and a hook that cannot run
    // would stall every tool call until Claude Code's own timeout. Better to ship no hook and fall
    // back to the behaviour that at least does not hang.
    if let Ok(exe) = std::env::current_exe() {
        settings["hooks"] = serde_json::json!({
            // Bash only. Edits are already covered by `acceptEdits`, and asking about every Read
            // would turn the loop into a clicking exercise.
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": format!("{} approve --port {port}", exe.display()),
                    // Must exceed the wait Keel itself enforces, or Claude Code gives up first and
                    // the person's answer arrives too late to be applied.
                    "timeout": crate::approve::WAIT.as_secs() + 30,
                }]
            }]
        });
    }

    settings.to_string()
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

/// Store a rule. Shared by the HTTP handler and by an approval answered while the agent waits.
pub fn remember(repo: &Utf8Path, rule: &str, scope: &str) -> Result<(), String> {
    if !valid_rule(rule) {
        return Err("That does not look like a permission rule, e.g. `Bash(bun *)`.".into());
    }
    match scope {
        // The default is the narrower one. A rule that outlives the session is a decision worth
        // asking for, not one to fall into by leaving a field blank.
        "project" => {
            let mut rules = load(repo);
            rules.insert(rule.to_string());
            save(repo, &rules)
        }
        _ => {
            session_rules()
                .lock()
                .expect("rules lock")
                .insert(rule.to_string());
            Ok(())
        }
    }
}

pub async fn add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RuleBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    let bad = |m: &str| (axum::http::StatusCode::BAD_REQUEST, m.to_string());
    if !matches!(body.scope.as_str(), "project" | "session") {
        return Err(bad("scope must be `project` or `session`"));
    }
    remember(&state.repo(), &body.rule, &body.scope).map_err(|e| bad(&e))?;
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
            serde_json::from_str(&settings_json(&root, 7777)).expect("valid json");
        assert!(json["permissions"]["allow"].is_array());
    }

    /// The hook is what makes the agent wait, so its shape is not incidental: the wrong event
    /// name, matcher or key and Claude Code silently ignores it, the tool runs unapproved, and
    /// nothing anywhere says so.
    #[test]
    fn the_settings_carry_the_hook_that_blocks_the_agent() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7788)).expect("valid json");

        let hook = &json["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash");

        let entry = &hook["hooks"][0];
        assert_eq!(entry["type"], "command");
        let command = entry["command"].as_str().expect("a command");
        assert!(command.contains("approve --port 7788"), "got {command}");

        // Claude Code has to wait longer than Keel does, or it gives up first and the answer
        // arrives after the tool call has already been decided without it.
        let timeout = entry["timeout"].as_u64().expect("a timeout");
        assert!(
            timeout > crate::approve::WAIT.as_secs(),
            "the CLI's timeout ({timeout}s) must exceed Keel's wait ({}s)",
            crate::approve::WAIT.as_secs()
        );
    }
}
