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

use axum::{
    Json,
    extract::{Query, State},
};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::serve::AppState;

/// Rules approved only for as long as this Keel process lives, keyed by the conversation that
/// approved them.
///
/// Keyed, because there is more than one window now. This was a single set, so "allow once, this
/// session" in one window silently widened what another window's agent could run — a permission
/// nobody granted, applied to a conversation nobody was watching. Project rules stay shared,
/// because those *are* per-project by design.
///
/// Never cleaned up when a session ends. Bounded by sessions opened in one run of Keel, each a
/// handful of short strings.
// ponytail: grows for the process lifetime; evict by session if a long-running Keel ever cares.
fn session_rules() -> &'static Mutex<BTreeMap<String, BTreeSet<String>>> {
    static RULES: OnceLock<Mutex<BTreeMap<String, BTreeSet<String>>>> = OnceLock::new();
    RULES.get_or_init(Default::default)
}

fn store_path(repo: &Utf8Path) -> camino::Utf8PathBuf {
    repo.join(".keel").join("permissions.json")
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    allow: BTreeSet<String>,
    /// The project has been trusted: the agent runs commands here without asking each time.
    ///
    /// One decision instead of one per program. Approving `docker`, then `wc`, then `grep`, then
    /// `sed` on a repository somebody already owns is not a safety property — it is a toll, and a
    /// toll people pay by reaching for `--dangerously-skip-permissions`, which turns it off
    /// everywhere and for good. Scoped to one project, revocable, and never the default is a
    /// better trade than a guardrail people route around.
    #[serde(default)]
    trusted: bool,
}

fn read(repo: &Utf8Path) -> Stored {
    std::fs::read_to_string(store_path(repo))
        .ok()
        .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
        .unwrap_or_default()
}

fn load(repo: &Utf8Path) -> BTreeSet<String> {
    read(repo).allow.into_iter().filter(|r| sane(r)).collect()
}

/// Whether a stored rule is one that could ever have been meant.
///
/// Filtered on read rather than migrated, so a file written by the version that shredded scripts
/// into rules heals itself the first time it is used, without Keel rewriting somebody's config
/// behind their back. What it removes is `Bash(assert *)`, `Bash(def *)`, `Bash(} *)` and the
/// hundred and ninety others that came from treating each line of a heredoc as a command.
fn sane(rule: &str) -> bool {
    if !valid_rule(rule) {
        return false;
    }
    let Some(inner) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) else {
        // A bare tool name — `Edit`, `Write`, `Bash`. Nothing to check.
        return true;
    };
    let program = inner.trim_end_matches(" *").trim();

    // Shape alone cannot separate `Payments` and `PYEOF` — both lifted from the body of a heredoc —
    // from a real binary, because they are shaped exactly like one. What separates them is that a
    // real one is on the PATH. A rule for a tool that is not installed is doing nothing anyway, and
    // gets asked for again the first time it is genuinely used.
    //
    // `git status` and friends carry an argument, so the first word is what is checked.
    let binary = program.split_whitespace().next().unwrap_or_default();
    !binary.is_empty() && on_path(binary)
}

/// Whether a name resolves to an executable on the PATH.
///
/// Reads the directories rather than spawning the program: this runs over every stored rule, and
/// two hundred process spawns to answer a question about filenames would be its own problem.
fn on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':').any(|dir| {
        !dir.is_empty() && {
            let candidate = std::path::Path::new(dir).join(name);
            candidate.is_file()
        }
    })
}

/// Whether this project has been trusted. Never true unless somebody said so.
pub fn trusted(repo: &Utf8Path) -> bool {
    read(repo).trusted
}

fn write(repo: &Utf8Path, stored: &Stored) -> Result<(), String> {
    let path = store_path(repo);
    std::fs::create_dir_all(path.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(stored).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

fn save(repo: &Utf8Path, allow: &BTreeSet<String>) -> Result<(), String> {
    let mut stored = read(repo);
    stored.allow = allow.clone();
    write(repo, &stored)
}

pub fn set_trusted(repo: &Utf8Path, on: bool) -> Result<(), String> {
    let mut stored = read(repo);
    stored.trusted = on;
    write(repo, &stored)
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
        // Configuring the workspace from the chat, through the same CLI Keel's own MCP panel
        // shells out to. Subcommands only: bare `claude` would let the agent start another agent,
        // which is not workspace configuration and is nobody's idea of a default.
        "Bash(claude mcp *)".into(),
        "Bash(claude plugin *)".into(),
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

/// Every rule that should be passed to the CLI for this repository and conversation.
///
/// `None` means a conversation that does not exist yet — the first turn, before `claude` has
/// generated an id. It has no session rules by definition, and anything approved during it is
/// carried by the hook's own answer rather than by these settings.
pub fn effective(repo: &Utf8Path, session: Option<&str>) -> Vec<String> {
    let mut all: BTreeSet<String> = load(repo);
    if let Some(session) = session
        && let Some(rules) = session_rules().lock().expect("rules lock").get(session)
    {
        all.extend(rules.iter().cloned());
    }
    // The project's own build and test commands, applied rather than merely suggested.
    //
    // Running what a repository declares about itself is the reason the agent is here, and asking
    // permission to run `make check` in a project whose Makefile defines `check` is a question
    // with one answer. Every approval spent on those is one not spent on the command that actually
    // deserved a look.
    all.extend(defaults(repo));
    all.into_iter().collect()
}

/// Tools whose refusal becomes a question rather than an error.
///
/// A Claude Code matcher is a regular expression over the tool name, so this is one alternation.
/// `AskUserQuestion` is in the list for a different reason than the others: it is not a permission
/// but a question, and headless `-p` has nobody to answer it — the CLI waits sixty seconds and
/// carries on without an answer. Hooking it makes the agent wait for a real one.
/// Edits are hooked too, but only an edit outside the repository ever becomes a question — the
/// hook waves the rest through. Headless Claude Code cannot prompt for `/tmp/x`, so without this
/// the write was refused and the agent worked around it inside the repository instead.
pub const HOOKED_TOOLS: &str = "Bash|WebSearch|WebFetch|AskUserQuestion|Write|Edit|MultiEdit";

/// The `--settings` payload carrying those rules.
/// The settings Keel hands `claude`: the allowlist, and the hook that makes the agent wait.
///
/// Note whose hook this is. Keel quarantines a *repository's* `.claude/settings.json` because a
/// hook there is a shell command that runs on the machine of whoever opens the repo. This one is
/// Keel's own, points at Keel's own binary, and is passed on the command line rather than read
/// from the working tree — the thing that made repo hooks dangerous is exactly the thing this
/// does not do.
pub fn settings_json(
    repo: &Utf8Path,
    port: u16,
    session: Option<&str>,
    lane: Option<&str>,
) -> String {
    let mut allow = effective(repo, session);
    if trusted(repo) {
        // Every tool the hook covers, not just `Bash`.
        //
        // A bare tool name allows every use of it. This matters more than it looks: on a trusted
        // project the hook answers `defer` immediately, which means "fall through to this
        // allowlist" — so a tool the hook covers but the allowlist omits is refused *and* never
        // asked about. `Bash` alone made trusting a project the one state in which `WebFetch`
        // could not be approved at all, by anyone, ever.
        for tool in HOOKED_TOOLS.split('|') {
            allow.push(tool.to_string());
        }
    }
    // Keel's own question tool is never a permission question.
    allow.push(format!(
        "mcp__{}__{}",
        crate::askmcp::SERVER,
        crate::askmcp::TOOL
    ));
    let mut settings = serde_json::json!({ "permissions": { "allow": allow } });

    // Without an executable there is nothing to point the hook at, and a hook that cannot run
    // would stall every tool call until Claude Code's own timeout. Better to ship no hook and fall
    // back to the behaviour that at least does not hang.
    if let Ok(exe) = std::env::current_exe() {
        settings["hooks"] = serde_json::json!({
            // The tools that can be refused and that a person can meaningfully widen.
            //
            // `Bash` was the whole list, which meant a `WebSearch` denial never reached anyone:
            // the agent was refused, said so in its reply, and there was no question to answer
            // anywhere — the exact failure this hook exists to prevent, for every tool but one.
            //
            // Reads and edits stay out. Edits are covered by `acceptEdits`, and asking about every
            // `Read` turns the loop into a clicking exercise.
            "PreToolUse": [{
                "matcher": HOOKED_TOOLS,
                "hooks": [{
                    "type": "command",
                    "command": format!("{} approve --port {port} --lane {}", exe.display(), lane.unwrap_or("")),
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
    /// The whole project is trusted, so none of the above is being consulted.
    pub trusted: bool,
}

#[derive(Deserialize)]
pub struct TrustBody {
    pub trusted: bool,
}

/// Grant or withdraw trust for the open project.
pub async fn trust(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TrustBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    set_trusted(&state.repo(), body.trusted)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;
    Ok(Json(body.trusted))
}

#[derive(Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    pub session: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionQuery>,
) -> Json<PermissionsView> {
    let repo = state.repo();
    let project = load(&repo);
    // Only this window's one-time rules. Showing another conversation's would suggest they applied
    // here, which is exactly the confusion the keying removed.
    let session: Vec<String> = q
        .session
        .as_deref()
        .and_then(|s| {
            session_rules()
                .lock()
                .expect("rules lock")
                .get(s)
                .map(|r| r.iter().cloned().collect())
        })
        .unwrap_or_default();
    // These are applied now, not suggested — the panel shows them so it is visible *why* the
    // agent can run `make` without ever having asked, rather than leaving that unexplained.
    let suggested = defaults(&repo);

    Json(PermissionsView {
        trusted: trusted(&repo),
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
    /// Which conversation a `session` rule belongs to. Ignored for `project`.
    #[serde(default)]
    pub session: Option<String>,
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
pub fn remember(
    repo: &Utf8Path,
    rule: &str,
    scope: &str,
    session: Option<&str>,
) -> Result<(), String> {
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
            // A one-time rule with no conversation to belong to would be a rule that applies
            // everywhere and expires nowhere — the bug this keying exists to remove.
            let Some(session) = session else {
                return Err("A session rule needs the conversation it belongs to.".into());
            };
            session_rules()
                .lock()
                .expect("rules lock")
                .entry(session.to_string())
                .or_default()
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
    remember(
        &state.repo(),
        &body.rule,
        &body.scope,
        body.session.as_deref(),
    )
    .map_err(|e| bad(&e))?;
    Ok(Json(true))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RuleBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    if body.scope == "session" {
        if let Some(session) = body.session.as_deref()
            && let Some(rules) = session_rules().lock().expect("rules lock").get_mut(session)
        {
            rules.remove(&body.rule);
        }
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

    /// "Allow once, this session" means this conversation, not every window's.
    ///
    /// The session set used to be a single global, so approving a command once in one window
    /// silently let another window's agent run it too — a permission the person never granted for
    /// that conversation, applied where they were not looking.
    #[test]
    fn a_one_time_rule_does_not_leak_into_another_conversation() {
        let (_dir, root) = repo(&[]);

        // `sh` and `ls`, not `docker` and `bun`: `remember` refuses a rule for a program that is
        // not on the PATH, and CI's runner has neither. The test is about conversations, not
        // about which tools a machine happens to have.
        remember(&root, "Bash(sh *)", "session", Some("session-a")).expect("remembered");

        assert!(
            effective(&root, Some("session-a")).contains(&"Bash(sh *)".to_string()),
            "the conversation that approved it can run it"
        );
        assert!(
            !effective(&root, Some("session-b")).contains(&"Bash(sh *)".to_string()),
            "another conversation still has to ask"
        );
        assert!(
            !effective(&root, None).contains(&"Bash(sh *)".to_string()),
            "and so does a conversation that does not exist yet"
        );

        // A project rule is shared on purpose: it is a decision about the repository, not about
        // one conversation in it.
        remember(&root, "Bash(ls *)", "project", None).expect("remembered");
        assert!(
            effective(&root, Some("session-b")).contains(&"Bash(ls *)".to_string()),
            "a project rule still reaches every conversation"
        );

        // A one-time rule with nowhere to belong is refused rather than made global.
        assert!(remember(&root, "Bash(rm *)", "session", None).is_err());
    }

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

    /// Managing MCP servers and plugins is something the person can do in a terminal in one
    /// command, and asking to approve `claude mcp list` is a question with one answer.
    #[test]
    fn the_workspace_cli_needs_no_approval() {
        let (_d, root) = repo(&[]);
        let d = defaults(&root);
        assert!(d.contains(&"Bash(claude mcp *)".to_string()));
        assert!(d.contains(&"Bash(claude plugin *)".to_string()));
        // Bare `claude` would be an agent spawning agents, not configuration.
        assert!(!d.contains(&"Bash(claude *)".to_string()));
        // Every default is well-formed, and none is subject to the PATH check: defaults are
        // Keel's own, appended by `effective` after stored rules are filtered, so a machine
        // without `claude` on it still passes the rule through rather than dropping it silently.
        assert!(
            d.iter().all(|r| valid_rule(r)),
            "a default that is not a rule"
        );
        for rule in &d {
            assert!(
                effective(&root, None).contains(rule),
                "{rule} did not reach the CLI"
            );
        }
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

    /// A file written by the version that shredded scripts into rules heals when it is read.
    #[test]
    fn nonsense_rules_are_dropped_on_read() {
        let (_d, root) = repo(&["Cargo.toml"]);
        std::fs::create_dir_all(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/permissions.json"),
            r#"{"allow":["Bash(git status *)","Bash(assert *)","Bash(} *)","Bash(1\")) *)",
                         "Bash(# *)","Bash(-flag *)","Edit","Bash(PYEOF *)","Bash(Payments *)"]}"#,
        )
        .unwrap();

        let kept = load(&root);
        assert!(
            kept.contains("Bash(git status *)"),
            "git is real and on the PATH"
        );
        assert!(kept.contains("Edit"), "a bare tool name is not a program");

        // Malformed, and — the harder half — shaped exactly like a binary but not one.
        for junk in [
            "Bash(} *)",
            "Bash(# *)",
            "Bash(-flag *)",
            "Bash(assert *)",
            "Bash(PYEOF *)",
            "Bash(Payments *)",
        ] {
            assert!(!kept.contains(junk), "{junk} survived");
        }
    }

    /// The project's own commands are allowed, not merely offered. Asking permission to run
    /// `make check` in a repository whose Makefile defines `check` is a question with one answer.
    #[test]
    fn the_projects_own_commands_need_no_approval() {
        let (_d, root) = repo(&["Cargo.toml", "Makefile"]);
        let effective = effective(&root, None);
        for rule in ["Bash(cargo *)", "Bash(make *)", "Bash(git status *)"] {
            assert!(
                effective.contains(&rule.to_string()),
                "{rule} is declared by this project and still asks"
            );
        }
    }

    /// Trust is a standing grant to run anything in one project, so the only acceptable default is
    /// off — including for a store written before the field existed, and for a file that will not
    /// parse at all.
    #[test]
    fn trust_is_never_on_by_accident() {
        let (_d, root) = repo(&["Cargo.toml"]);
        assert!(!trusted(&root), "a project nobody has trusted");

        // A store from before the field existed.
        std::fs::create_dir_all(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/permissions.json"),
            r#"{"allow":["Bash(make *)"]}"#,
        )
        .unwrap();
        assert!(!trusted(&root), "an older store is not trusted");
        assert!(load(&root).contains("Bash(make *)"), "and keeps its rules");

        // Unparseable.
        std::fs::write(root.join(".keel/permissions.json"), "{ not json").unwrap();
        assert!(!trusted(&root), "an unreadable store is not trusted");
    }

    /// Granting trust must not quietly discard the rules already approved, or withdrawing it drops
    /// someone back to being asked about everything they had already answered.
    #[test]
    fn trust_and_the_allowlist_are_independent() {
        let (_d, root) = repo(&["Cargo.toml"]);
        remember(&root, "Bash(docker *)", "project", None).unwrap();
        set_trusted(&root, true).unwrap();

        assert!(trusted(&root));
        assert!(load(&root).contains("Bash(docker *)"));

        set_trusted(&root, false).unwrap();
        assert!(!trusted(&root));
        assert!(
            load(&root).contains("Bash(docker *)"),
            "rules survive withdrawal"
        );
    }

    /// A trusted project passes a blanket rule too, so the hook path and the plain permission path
    /// cannot disagree about what is allowed.
    /// Trusting a project must not be the one state in which a tool cannot be used.
    ///
    /// The hook defers on a trusted project, so the allowlist is the only thing deciding — and it
    /// listed `Bash` alone. `WebFetch` was therefore refused with no question raised and no rule
    /// that could be added, which is worse than not trusting at all.
    #[test]
    fn trust_allows_every_tool_the_hook_would_have_asked_about() {
        let (_d, root) = repo(&["Cargo.toml"]);
        set_trusted(&root, true).unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None)).unwrap();
        let allow = json["permissions"]["allow"].as_array().unwrap();

        for tool in HOOKED_TOOLS.split('|') {
            assert!(
                allow.iter().any(|r| r == tool),
                "a trusted project cannot use {tool}: the hook defers and the allowlist omits it"
            );
        }
    }

    #[test]
    fn a_trusted_project_says_so_in_its_settings() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let before: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None)).unwrap();
        let allow = before["permissions"]["allow"].as_array().unwrap();
        assert!(!allow.iter().any(|r| r == "Bash"));

        set_trusted(&root, true).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None)).unwrap();
        let allow = after["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|r| r == "Bash"));
    }

    #[test]
    fn settings_payload_is_shaped_the_way_the_cli_expects() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None)).expect("valid json");
        assert!(json["permissions"]["allow"].is_array());
    }

    /// The hook is what makes the agent wait, so its shape is not incidental: the wrong event
    /// name, matcher or key and Claude Code silently ignores it, the tool runs unapproved, and
    /// nothing anywhere says so.
    #[test]
    fn the_settings_carry_the_hook_that_blocks_the_agent() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7788, None, None)).expect("valid json");

        let hook = &json["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], HOOKED_TOOLS);
        // The regression this guards: `Bash` alone meant every other refusable tool failed
        // silently, with the agent reporting it could not proceed and no question anywhere.
        for tool in ["Bash", "WebSearch", "WebFetch"] {
            assert!(
                HOOKED_TOOLS.split('|').any(|t| t == tool),
                "{tool} can be refused but its refusal would never be asked about"
            );
        }

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
