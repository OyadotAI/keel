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
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::sync::{Arc, Mutex, OnceLock};

use crate::lock::Locked;
use crate::serve::AppState;

/// Rules approved only for as long as this Keel process lives, keyed by the conversation that
/// approved them.
///
/// Keyed, because there is more than one window now. This was a single set, so "allow once, this
/// session" in one window silently widened what another window's agent could run — a permission
/// nobody granted, applied to a conversation nobody was watching. Project rules stay shared,
/// because those *are* per-project by design.
///
/// Never cleaned up when a session ends; capped instead, at `MAX_SESSION_RULES` conversations,
/// dropping the first key past the cap. Each is a handful of short strings.
const MAX_SESSION_RULES: usize = 200;
const MAX_STORE_BYTES: usize = 256 * 1024;

type SessionRules = BTreeMap<(String, String), BTreeSet<String>>;

fn session_rules() -> &'static Mutex<SessionRules> {
    static RULES: OnceLock<Mutex<SessionRules>> = OnceLock::new();
    RULES.get_or_init(Default::default)
}

/// Authorization belongs to this Mac, never to files supplied by a clone or edited by an agent.
/// Test binaries use an isolated directory without changing process-global HOME.
fn permissions_dir() -> Result<Utf8PathBuf, String> {
    #[cfg(test)]
    {
        static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        Ok(Utf8PathBuf::from_path_buf(
            DIR.get_or_init(|| tempfile::tempdir().expect("permission test storage"))
                .path()
                .to_path_buf(),
        )
        .expect("UTF-8 test directory")
        .join("permissions"))
    }
    #[cfg(not(test))]
    {
        // An explicit daemon configuration, also used to isolate subprocess integration tests.
        // Never read this location from project configuration.
        std::env::var("KEEL_PERMISSIONS_DIR")
            .ok()
            .map(Utf8PathBuf::from)
            .or_else(|| crate::prefs::dir().map(|d| d.join("permissions")))
            .filter(|p| p.is_absolute())
            .ok_or_else(|| "No absolute local permissions directory is available.".into())
    }
}

fn project_identity(repo: &Utf8Path) -> Result<(Utf8PathBuf, String), String> {
    let canonical = repo.canonicalize_utf8().map_err(|e| e.to_string())?;
    let metadata = std::fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !metadata.is_dir() {
        return Err("Permissions require an existing project directory.".into());
    }
    // A new clone at the same path must not inherit the deleted directory's authorization.
    let identity = format!("{}\0{}\0{}", canonical, metadata.dev(), metadata.ino());
    Ok((
        canonical,
        format!("{:x}", Sha256::digest(identity.as_bytes())),
    ))
}

fn store_path(repo: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let (project, key) = project_identity(repo)?;
    let dir = permissions_dir()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| e.to_string())?;
    let metadata = std::fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(
            "Local permission storage must be a private, user-owned directory, not a symlink."
                .into(),
        );
    }
    let canonical = dir.canonicalize_utf8().map_err(|e| e.to_string())?;
    if canonical.starts_with(&project) {
        return Err(
            "Permission storage cannot be inside the open project. Open a narrower project folder."
                .into(),
        );
    }
    Ok(canonical.join(format!("{key}.json")))
}

fn open_store(repo: &Utf8Path, write: bool) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .create(write)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(store_path(repo)?)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(
            "Local permission records must be private, user-owned files without links.".into(),
        );
    }
    // Separate daemons can update the same project. Hold the lock across read-modify-write;
    // readers also lock so they never observe a partially written grant.
    if write {
        file.lock()
    } else {
        file.lock_shared()
    }
    .map_err(|e| e.to_string())?;
    Ok(file)
}

fn read_store(file: &mut std::fs::File) -> Stored {
    // Do not parse a valid prefix of an oversized/corrupt record as an authorization.
    if file
        .metadata()
        .map_or(true, |m| m.len() > MAX_STORE_BYTES as u64)
    {
        return Stored::default();
    }
    let mut body = String::new();
    file.take(MAX_STORE_BYTES as u64)
        .read_to_string(&mut body)
        .ok()
        .and_then(|_| serde_json::from_str(&body).ok())
        .unwrap_or_default()
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
    open_store(repo, false)
        .ok()
        .map(|mut file| read_store(&mut file))
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
        // A bare tool name — `Edit`, `Write`, `Bash`, or an `mcp__…` tool. Nothing to check.
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

fn update(repo: &Utf8Path, change: impl FnOnce(&mut Stored)) -> Result<(), String> {
    let mut file = open_store(repo, true)?;
    let mut stored = read_store(&mut file);
    change(&mut stored);
    let body = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?;
    if body.len() > MAX_STORE_BYTES {
        return Err(
            "Too many saved permission rules. Remove unused rules before adding more.".into(),
        );
    }
    file.rewind()
        .and_then(|_| file.set_len(0))
        .and_then(|_| file.write_all(&body))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
fn save(repo: &Utf8Path, allow: &BTreeSet<String>) -> Result<(), String> {
    update(repo, |stored| stored.allow = allow.clone())
}

pub fn set_trusted(repo: &Utf8Path, on: bool) -> Result<(), String> {
    update(repo, |stored| stored.trusted = on)
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
        && let Ok((_, project)) = project_identity(repo)
        && let Some(rules) = session_rules()
            .locked()
            .get(&(project, session.to_string()))
    {
        all.extend(rules.iter().cloned());
    }
    // Build files and installer commands are suggestions, not user authorization. A Makefile,
    // package script, cargo build script or plugin installer can execute arbitrary code.
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
/// One shell word, whatever is in it.
///
/// Claude Code runs a hook command through a shell, so an unquoted path with a space in it
/// arrives as two arguments — and `clap`'s usage error exits 2, which a `PreToolUse` hook uses to
/// mean *block this call*. Every command on a machine whose checkout lives under "My Projects"
/// would have been refused, with a usage message as the reason.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn settings_json(
    repo: &Utf8Path,
    port: u16,
    session: Option<&str>,
    lane: Option<&str>,
    // Where the turn runs — the lane's checkout, which is not the project root.
    cwd: &Utf8Path,
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
                    "command": format!(
                        "{} approve --port {port} --lane {} --cwd {}",
                        sh_quote(&exe.display().to_string()),
                        // Quoted even though a lane id is a uuid: `--lane` with no lane rendered
                        // as two spaces, the shell collapsed them, and `--cwd` became the *value*
                        // of `--lane`. Same exit 2, same "every call blocked".
                        sh_quote(lane.unwrap_or("")),
                        sh_quote(cwd.as_str()),
                    ),
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
    let repo = state.repo();
    crate::serve::blocking(move || set_trusted(&repo, body.trusted), Ok(()))
        .await
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
    // Two files and a walk, off the executor: this runs on every panel and every turn end.
    let (project, suggested, is_trusted, identity) = {
        let repo = repo.clone();
        crate::serve::blocking(
            move || {
                (
                    load(&repo),
                    defaults(&repo),
                    trusted(&repo),
                    project_identity(&repo).ok().map(|(_, key)| key),
                )
            },
            (BTreeSet::new(), Vec::new(), false, None),
        )
        .await
    };
    // Only this window's one-time rules. Showing another conversation's would suggest they applied
    // here, which is exactly the confusion the keying removed.
    let session: Vec<String> = q
        .session
        .as_deref()
        .and_then(|s| {
            session_rules()
                .locked()
                .get(&(identity?, s.to_string()))
                .map(|r| r.iter().cloned().collect())
        })
        .unwrap_or_default();
    // Suggestions are shown separately and do not enter the effective allowlist.
    Json(PermissionsView {
        trusted: is_trusted,
        project: project.into_iter().collect(),
        session,
        suggested,
    })
}

#[derive(Deserialize)]
pub struct RuleBody {
    pub rule: String,
    /// `project` persists in machine-local storage; `session` lasts until Keel restarts.
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
        // An MCP tool rule is the one legitimate lowercase shape — `mcp__server` allows a whole
        // connector, `mcp__server__tool` one of its tools. Keel writes one itself for its own ask
        // server, so requiring an uppercase first letter meant a rule Keel emits was a rule Keel
        // refused to store: a hand-added `mcp__…` was dropped on read, silently, and the connector
        // stayed refused however many times you restarted.
        && (rule.starts_with("mcp__") || rule.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
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
        // The held hook receives this approval directly. Do not turn one invocation into a
        // reusable rule, and do not require init to have supplied a conversation ID yet.
        "once" => Ok(()),
        // The default is the narrower one. A rule that outlives the session is a decision worth
        // asking for, not one to fall into by leaving a field blank.
        "project" => update(repo, |stored| {
            stored.allow.insert(rule.to_string());
        }),
        _ => {
            // A one-time rule with no conversation to belong to would be a rule that applies
            // everywhere and expires nowhere — the bug this keying exists to remove.
            let Some(session) = session else {
                return Err("A session rule needs the conversation it belongs to.".into());
            };
            let key = (project_identity(repo)?.1, session.to_string());
            let mut all = session_rules().locked();
            // Capped: the first conversation past the cap loses its one-time rules, which is a
            // question asked again, not a permission granted.
            if all.len() >= MAX_SESSION_RULES
                && !all.contains_key(&key)
                && let Some(first) = all.keys().next().cloned()
            {
                all.remove(&first);
            }
            all.entry(key).or_default().insert(rule.to_string());
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
    let repo = state.repo();
    crate::serve::blocking(
        move || remember(&repo, &body.rule, &body.scope, body.session.as_deref()),
        Ok(()),
    )
    .await
    .map_err(|e| bad(&e))?;
    Ok(Json(true))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RuleBody>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    if body.scope == "session" {
        let repo = state.repo();
        crate::serve::blocking(
            move || {
                if let Some(session) = body.session.as_deref()
                    && let Ok((_, project)) = project_identity(&repo)
                    && let Some(rules) = session_rules()
                        .locked()
                        .get_mut(&(project, session.to_string()))
                {
                    rules.remove(&body.rule);
                }
            },
            (),
        )
        .await;
    } else {
        let repo = state.repo();
        crate::serve::blocking(
            move || {
                update(&repo, |stored| {
                    stored.allow.remove(&body.rule);
                })
            },
            Ok(()),
        )
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;
    }
    Ok(Json(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    #[test]
    fn publication_repository_permissions_cannot_grant_trust_or_rules() {
        let (_dir, root) = repo(&[]);
        std::fs::create_dir(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/permissions.json"),
            r#"{"allow":["Bash","WebFetch"],"trusted":true}"#,
        )
        .unwrap();
        assert!(
            !trusted(&root),
            "repository content is not a local trust decision"
        );
        assert!(
            load(&root).is_empty(),
            "legacy grants must not migrate implicitly"
        );
        let settings: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None, &root)).unwrap();
        assert!(
            !settings["permissions"]["allow"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "Bash")
        );
    }

    #[test]
    fn publication_project_manifests_do_not_authorize_execution() {
        let (_dir, root) = repo(&["Makefile", "Cargo.toml", "package.json"]);
        let grants = effective(&root, None);
        for rule in defaults(&root) {
            assert!(
                !grants.contains(&rule),
                "a suggested command is not approved: {rule}"
            );
        }
    }

    #[test]
    fn publication_local_grants_survive_repository_edits_but_not_a_new_clone() {
        let (_dir, root) = repo(&[]);
        let (_other, clone) = repo(&[]);
        set_trusted(&root, true).unwrap();
        remember(&root, "Bash(sh *)", "project", None).unwrap();
        std::fs::create_dir(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/permissions.json"),
            r#"{"allow":["Bash"],"trusted":false}"#,
        )
        .unwrap();
        assert!(trusted(&root));
        assert!(!load(&root).contains("Bash"));
        assert!(load(&root).contains("Bash(sh *)"));
        assert!(
            !store_path(&root)
                .unwrap()
                .starts_with(root.canonicalize_utf8().unwrap())
        );
        std::fs::create_dir(clone.join(".keel")).unwrap();
        std::fs::copy(
            root.join(".keel/permissions.json"),
            clone.join(".keel/permissions.json"),
        )
        .unwrap();
        assert!(!trusted(&clone));
        assert!(load(&clone).is_empty());
        let moved = root.with_extension("old-checkout");
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(
            !trusted(&root),
            "a replacement checkout needs its own approval"
        );
        std::fs::remove_dir(&root).unwrap();
        std::fs::rename(moved, root).unwrap();
    }

    #[test]
    fn publication_symlink_aliases_share_only_their_canonical_project_grants() {
        let (dir, root) = repo(&[]);
        let target = root.join("project");
        let alias = root.join("alias");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        set_trusted(&target, true).unwrap();
        assert!(trusted(&alias));
        assert_eq!(store_path(&target).unwrap(), store_path(&alias).unwrap());
        assert!(!trusted(
            &Utf8PathBuf::from_path_buf(dir.path().join("missing")).unwrap()
        ));
    }

    #[test]
    fn publication_session_grants_do_not_cross_project_boundaries() {
        let (_a, first) = repo(&[]);
        let (_b, second) = repo(&[]);
        remember(&first, "Bash", "session", Some("same-session-id")).unwrap();
        assert!(effective(&first, Some("same-session-id")).contains(&"Bash".into()));
        assert!(!effective(&second, Some("same-session-id")).contains(&"Bash".into()));
    }

    #[test]
    fn publication_corrupt_or_linked_local_records_fail_closed() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, root) = repo(&[]);
        set_trusted(&root, true).unwrap();
        let path = store_path(&root).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::write(&path, "{broken").unwrap();
        assert!(!trusted(&root));
        let mut oversized = r#"{"allow":["Bash"],"trusted":true}"#.to_string();
        oversized.push_str(&" ".repeat(MAX_STORE_BYTES));
        oversized.push_str("not-json");
        std::fs::write(&path, oversized).unwrap();
        assert!(
            !trusted(&root),
            "a valid prefix is not a valid permission record"
        );
        set_trusted(&root, true).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!trusted(&root));
        assert!(set_trusted(&root, true).is_err());
        std::fs::remove_file(&path).unwrap();
        let attacker = root.join("grant.json");
        std::fs::write(&attacker, r#"{"allow":["Bash"],"trusted":true}"#).unwrap();
        std::os::unix::fs::symlink(&attacker, &path).unwrap();
        assert!(!trusted(&root));
        assert!(set_trusted(&root, true).is_err());
        assert!(std::fs::read_to_string(attacker).unwrap().contains("true"));
    }

    #[test]
    fn publication_concurrent_rule_and_trust_saves_preserve_every_decision() {
        let (_dir, root) = repo(&[]);
        std::thread::scope(|scope| {
            for n in 0..16 {
                let root = &root;
                scope.spawn(move || {
                    remember(root, &format!("mcp__test__tool{n}"), "project", None).unwrap()
                });
            }
            scope.spawn(|| set_trusted(&root, true).unwrap());
        });
        assert!(trusted(&root));
        assert_eq!(load(&root).len(), 16);
    }

    #[test]
    fn publication_permissions_cannot_be_kept_inside_the_project_being_authorized() {
        let dir = permissions_dir().unwrap();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        assert!(set_trusted(dir.parent().unwrap(), true).is_err());
        assert!(!trusted(dir.parent().unwrap()));
    }

    /// "Allow once, this session" means this conversation, not every window's.
    ///
    /// The session set used to be a single global, so approving a command once in one window
    /// silently let another window's agent run it too — a permission the person never granted for
    /// that conversation, applied where they were not looking.
    #[test]
    fn allowing_once_does_not_remember_a_session_or_project_rule() {
        let (_dir, root) = repo(&[]);
        remember(&root, "Bash(sh *)", "once", Some("allow-once-regression")).unwrap();
        assert!(
            !effective(&root, Some("allow-once-regression")).contains(&"Bash(sh *)".to_string())
        );
        assert!(!store_path(&root).unwrap().exists());
        // Approval can precede provider init, before a session ID exists.
        remember(&root, "Bash(sh *)", "once", None).unwrap();
    }

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

    /// MCP/plugin installation can execute external code. Offer it, but do not authorize it.
    #[test]
    fn the_workspace_cli_is_suggested_but_needs_approval() {
        let (_d, root) = repo(&[]);
        let d = defaults(&root);
        assert!(d.contains(&"Bash(claude mcp *)".to_string()));
        assert!(d.contains(&"Bash(claude plugin *)".to_string()));
        // Bare `claude` would be an agent spawning agents, not configuration.
        assert!(!d.contains(&"Bash(claude *)".to_string()));
        // Suggestions are well-formed even when the program has not been installed yet.
        assert!(
            d.iter().all(|r| valid_rule(r)),
            "a default that is not a rule"
        );
        for rule in &d {
            assert!(
                !effective(&root, None).contains(rule),
                "{rule} reached the CLI without approval"
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
        assert!(valid_rule("mcp__claude_ai_Figma"));
        assert!(valid_rule("mcp__claude_ai_Figma__get_metadata"));
        assert!(sane("mcp__claude_ai_Figma")); // survives the read filter, which is where it died
        assert!(!valid_rule(""));
        assert!(!valid_rule("Bash(x)\nBash(y)"));
    }

    /// A file written by the version that shredded scripts into rules heals when it is read.
    #[test]
    fn nonsense_rules_are_dropped_on_read() {
        let (_d, root) = repo(&["Cargo.toml"]);
        save(
            &root,
            &[
                "Bash(git status *)",
                "Bash(assert *)",
                "Bash(} *)",
                "Bash(1\")) *)",
                "Bash(# *)",
                "Bash(-flag *)",
                "Edit",
                "Bash(PYEOF *)",
                "Bash(Payments *)",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
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

    /// A project script becomes authorized only after an explicit local approval.
    #[test]
    fn project_commands_require_a_local_approval() {
        let (_d, root) = repo(&["Cargo.toml", "Makefile"]);
        let before = effective(&root, None);
        for rule in ["Bash(cargo *)", "Bash(make *)", "Bash(git status *)"] {
            assert!(!before.contains(&rule.to_string()));
            remember(&root, rule, "project", None).unwrap();
            assert!(effective(&root, None).contains(&rule.to_string()));
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
        assert!(
            load(&root).is_empty(),
            "legacy rules are not imported without approval"
        );

        // Unparseable.
        std::fs::write(root.join(".keel/permissions.json"), "{ not json").unwrap();
        assert!(!trusted(&root), "an unreadable store is not trusted");
    }

    /// Granting trust must not quietly discard the rules already approved, or withdrawing it drops
    /// someone back to being asked about everything they had already answered.
    #[test]
    fn trust_and_the_allowlist_are_independent() {
        let (_d, root) = repo(&["Cargo.toml"]);
        remember(&root, "Bash(sh *)", "project", None).unwrap();
        set_trusted(&root, true).unwrap();

        assert!(trusted(&root));
        assert!(load(&root).contains("Bash(sh *)"));

        set_trusted(&root, false).unwrap();
        assert!(!trusted(&root));
        assert!(
            load(&root).contains("Bash(sh *)"),
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
            serde_json::from_str(&settings_json(&root, 7777, None, None, &root)).unwrap();
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
            serde_json::from_str(&settings_json(&root, 7777, None, None, &root)).unwrap();
        let allow = before["permissions"]["allow"].as_array().unwrap();
        assert!(!allow.iter().any(|r| r == "Bash"));

        set_trusted(&root, true).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None, &root)).unwrap();
        let allow = after["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|r| r == "Bash"));
    }

    #[test]
    fn settings_payload_is_shaped_the_way_the_cli_expects() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None, &root))
                .expect("valid json");
        assert!(json["permissions"]["allow"].is_array());
    }

    /// The hook is what makes the agent wait, so its shape is not incidental: the wrong event
    /// name, matcher or key and Claude Code silently ignores it, the tool runs unapproved, and
    /// nothing anywhere says so.
    #[test]
    fn the_settings_carry_the_hook_that_blocks_the_agent() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7788, None, None, &root))
                .expect("valid json");

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

    /// A hook command is a shell line, and a checkout under "My Projects" made it two arguments.
    ///
    /// `clap`'s usage error exits 2, and 2 from a `PreToolUse` hook means *block the call* — so
    /// on that machine every command, edit and question would have come back refused, with a
    /// usage message as the reason.
    #[test]
    fn a_path_with_a_space_in_it_stays_one_shell_word() {
        let (_d, root) = repo(&["Cargo.toml"]);
        let checkout = root.join("My Projects/app");
        std::fs::create_dir_all(&checkout).expect("mkdir");

        let json: serde_json::Value =
            serde_json::from_str(&settings_json(&root, 7777, None, None, &checkout))
                .expect("valid json");
        let command = json["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("a command");
        assert!(
            command.contains(&format!("--cwd '{checkout}'")),
            "the checkout is not one shell word: {command}"
        );
        // And an absent lane is an empty word, not an absent one: `--lane  --cwd x` makes
        // `--cwd` the lane's value and the path an unexpected argument.
        assert!(
            command.contains("--lane ''"),
            "an absent lane swallowed the next flag: {command}"
        );
    }
}
