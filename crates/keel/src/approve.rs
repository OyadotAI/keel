//! Making the agent wait.
//!
//! Before this, an approval was a message sent after the fact. A command outside the allowlist came
//! back refused, the turn carried on without it, and the person's click added a rule and asked the
//! agent to try again — by which point it had often already worked around the gap or finished
//! saying it could not do the thing. The agent never waited, because headless `claude -p` has
//! nobody to wait for.
//!
//! It does now. Claude Code runs a `PreToolUse` hook before every matching tool call and blocks on
//! it, honouring the `permissionDecision` it prints. So Keel supplies its own hook, pointing at
//! this binary: the hook asks the running Keel, Keel asks the person, and the agent is genuinely
//! stopped at the point of the question rather than told about it afterwards.
//!
//! Measured before building on it: a hook with `timeout: 300` held a turn for 65 seconds and its
//! decision was still applied. Empty output defers to the normal permission check rather than
//! failing.
//!
//! # Failing open, on purpose
//!
//! Every error path here exits 0 and prints nothing. Keel not running, a dropped socket, a person
//! who walked away — all of them fall through to Claude Code's own permission logic, which is the
//! allowlist Keel already wrote into `--settings`. The alternative is an agent wedged behind a
//! question nobody is going to answer, and a guardrail that can hang the product is a guardrail
//! people turn off.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::lock::Locked;
use crate::serve::AppState;

/// How long the agent may be held. Claude Code's own hook timeout is set alongside this and must
/// be the larger of the two, or it gives up first and the answer arrives too late to matter.
pub const WAIT: std::time::Duration = std::time::Duration::from_secs(240);

/// What Claude Code sends a `PreToolUse` hook on stdin.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct HookInput {
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub session_id: String,
    /// The window that started this turn. Claude Code does not send it — Keel puts it on the
    /// hook's command line — and it is the only id that exists on a lane's first turn.
    #[serde(default)]
    pub lane: String,
    /// The checkout the turn runs in, from the same command line. A monitored command has to run
    /// where the agent would have run it.
    #[serde(default)]
    pub cwd: String,
    /// `antigravity` for an `agy` lane, whose hook is the only gate: nothing sits underneath a
    /// `defer`, so the branches that lean on Claude's own checks are strict for it. Empty from
    /// Claude's hook, and from any hook older than the field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

#[derive(Serialize, Clone)]
pub struct Pending {
    pub id: String,
    /// The window this belongs to, when Keel knew it — the hook is told on its command line.
    #[serde(default)]
    pub lane: String,
    pub tool: String,
    /// The command, for a Bash call. Empty for anything else.
    pub command: String,
    /// The rules that would let this through, derived the same way the UI used to derive them.
    pub rules: Vec<String>,
    /// The tool's whole input. For `AskUserQuestion` this is the questions and their options,
    /// which the card renders; for everything else the UI reads `command` and ignores this.
    #[serde(default)]
    pub input: serde_json::Value,
    /// The conversation that provoked the question.
    ///
    /// Claude Code has always sent this and it was always thrown away, which was survivable while
    /// exactly one window existed. With two, a question has to find the conversation it belongs to
    /// or it surfaces in the wrong one. Empty when the hook did not say, and an empty one is shown
    /// to whoever asks — the same failing-open this whole module does.
    pub session_id: String,
    /// Why Keel is asking, when it is not the usual "this is not allowed yet".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Asked by a subagent the lane's turn started, whose own steps the turn never shows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subagent: bool,
}

#[derive(Deserialize)]
pub struct Answer {
    pub id: String,
    /// `allow` or `deny`.
    pub decision: String,
    /// The conversation being answered, so a `session`-scoped rule lands on it and not on every
    /// other window's agent.
    #[serde(default)]
    pub session: Option<String>,
    /// Rules to remember, so the same command is not asked about twice.
    #[serde(default)]
    pub rules: Vec<String>,
    /// `project`, `session`, or `trust` — the last meaning "stop asking about this project".
    #[serde(default)]
    pub scope: String,
    /// For a question rather than a permission: what the person answered, already rendered as
    /// text. Delivered to the agent as the tool's result.
    #[serde(default)]
    pub answer: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Decision {
    pub decision: String,
    pub reason: String,
}

/// The hook process blocked on each question, with the lane it belongs to — the lane is what
/// files the answer against the turn as a fact.
type Waiters = Mutex<HashMap<String, (tokio::sync::oneshot::Sender<Decision>, String)>>;

pub fn waiters() -> &'static Waiters {
    static W: OnceLock<Waiters> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// Calls the person approved, by tool-use id. One call can pass two gates — the hook, and then
/// Claude Code asking again for an `ask` rule or a path it protects — and the second must not put
/// the same question back on screen. Bounded: only the latest few are ever looked up.
fn approved() -> &'static Mutex<std::collections::VecDeque<String>> {
    static A: OnceLock<Mutex<std::collections::VecDeque<String>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

/// `permissions.ask` from the person's settings — in a test, the test's own, never the machine's.
fn person_asks() -> Vec<String> {
    #[cfg(test)]
    return tests::ASKS.locked().clone();
    #[cfg(not(test))]
    keel_workspace::claude_home()
        .map(|h| crate::denies::read_ask(&h))
        .unwrap_or_default()
}

fn note_approved(id: &str) {
    let mut ids = approved().locked();
    ids.push_back(id.to_string());
    while ids.len() > 256 {
        ids.pop_front();
    }
}

/// Withdraws a question however its wait ends. The hook process is killed by Stop, or by Claude
/// Code's own hook timeout, and axum then drops the waiting future — and only the timeout branch
/// used to clean up, so the card stayed on screen for the life of the daemon and answering it
/// reported success to nobody.
struct Withdraw(String);

impl Drop for Withdraw {
    fn drop(&mut self) {
        let gone = waiters().locked().remove(&self.0).is_some();
        let before = {
            let mut q = queue().locked();
            let n = q.len();
            q.retain(|p| p.id != self.0);
            n != q.len()
        };
        if gone || before {
            // The window reads the queue again and the card goes.
            crate::events::emit("pending", None, serde_json::json!({ "withdrawn": self.0 }));
        }
    }
}

/// Requests the UI has not yet been shown, so a page that reloads mid-question still sees it.
pub fn queue() -> &'static Mutex<Vec<Pending>> {
    static Q: OnceLock<Mutex<Vec<Pending>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

/// A command that stands in front of the real one and would launch it.
///
/// Never the rule itself: approving `sudo` approves everything it can start.
pub(crate) const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "time", "xargs", "command", "exec", "nice",
];

/// A builtin whose arguments are not a command.
///
/// Stepping over `cd` the way a wrapper is stepped over reads its *path* as the program, so
/// `cd frontend && bun install` asks to approve `frontend`. It needs no permission of its own, so
/// the whole segment is skipped.
const BUILTINS: &[&str] = &[
    "cd", "pushd", "popd", "export", "source", ".", "set", "umask", "alias",
];

/// Shell keywords that open a construct whose next word is *not* a program.
///
/// `for f in a b c` names a variable, not a command; `done` and `fi` close a block and name
/// nothing. A segment starting with one of these contributes no rule at all.
///
/// This is the same bug as the heredoc one the comment on `rules_for` describes, arriving from a
/// different direction. A loop over ten files produced `Bash(for *)`, `Bash(do *)`, `Bash(cat *)`
/// and `Bash(done *)`, and approving it wrote all four into the project's allowlist — where
/// `Bash(do *)` matches nothing ever and `Bash(for *)` matches every command that starts with the
/// word "for". An allowlist full of rules that mean nothing is an allowlist nobody can read.
const KEYWORDS_WITH_NO_PROGRAM: &[&str] = &[
    "for", "done", "fi", "esac", "in", "case", "select", "function", "{", "}", "[[", "]]",
];

/// Shell keywords the *next* word follows as the program: `do cat x` is a request to run `cat`.
const KEYWORDS_BEFORE_A_PROGRAM: &[&str] = &["do", "then", "else", "elif", "if", "while", "until"];

fn is_assignment(token: &str) -> bool {
    token.contains('=')
        && token
            .chars()
            .take_while(|c| *c != '=')
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether a token could be the name of a program.
///
/// A binary name is letters, digits and a little punctuation. Requiring that is what stops a
/// heredoc body turning into permission rules: `assert`, `def`, `print(f`, `}` and `1"))` all
/// arrive looking like the first word of a command, and only some of them are even close.
fn looks_like_a_program(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// The program a single shell segment actually invokes.
///
/// Leading environment assignments and wrappers are stepped over rather than treated as the
/// program — `sudo rm -rf /` is a request to approve `rm`, and the first version of this returned
/// nothing at all for it, which reads as "no rule needed".
fn program_of(segment: &str) -> Option<String> {
    let mut tokens = segment.split_whitespace();
    loop {
        let token = tokens.next()?;
        if is_assignment(token) {
            continue;
        }
        let name = token
            .rsplit('/')
            .next()
            .unwrap_or(token)
            .trim_matches(['"', '\''])
            .to_string();
        if name.is_empty() {
            continue;
        }
        if WRAPPERS.contains(&name.as_str()) || KEYWORDS_BEFORE_A_PROGRAM.contains(&name.as_str()) {
            continue;
        }
        if BUILTINS.contains(&name.as_str()) || KEYWORDS_WITH_NO_PROGRAM.contains(&name.as_str()) {
            return None;
        }
        return looks_like_a_program(&name).then_some(name);
    }
}

/// Every rule a call needs.
///
/// Claude Code approves a compound command part by part: `bun install && make check` needs both
/// `bun` and `make`. Deriving one rule from the first token leaves the second half refused and the
/// approval looking broken, which is a bug this project has already had once.
pub fn rules_for(tool: &str, input: &serde_json::Value) -> Vec<String> {
    if is_edit(tool) {
        // The directory, not the file: approving `/tmp/report.md` and then being asked about
        // `/tmp/report2.md` is the toll again.
        let path = input
            .get("file_path")
            .and_then(|p| p.as_str())
            .unwrap_or_default();
        let dir = std::path::Path::new(path)
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        return vec![format!("{tool}({dir}/*)")];
    }
    // A server's tools come as a set — a browser is thirty of them — so "always" is the server,
    // `mcp__<server>`, which Claude Code's allowlist reads the same way.
    // Only when the name has one reading: in `mcp__a__b__c` the server could be `a` or `a__b`, so
    // that card offers the exact tool rather than a server the person never saw.
    if let Some((server, rest)) = tool
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        && !rest.contains("__")
    {
        return vec![format!("mcp__{server}")];
    }
    if tool != "Bash" {
        return vec![tool.to_string()];
    }
    let command = input
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or_default();

    // Not split on newlines. A tool call's `command` is one shell invocation, and a newline inside
    // it is nearly always a heredoc or an inline script — not a second command to get approved. It
    // used to split on them, so `python3 - <<'PY' … PY` became one fake command per line of
    // Python, and clicking "always allow" stored a rule for each: `Bash(assert *)`, `Bash(def *)`,
    // `Bash(the *)`. Two hundred rules in one real project, most of them meaningless, which is an
    // allowlist that has stopped meaning anything.
    let mut out: Vec<String> = Vec::new();
    for segment in command.split(';').flat_map(|s| s.split("&&")) {
        for part in segment.split("||").flat_map(|s| s.split('|')) {
            let Some(program) = program_of(part) else {
                continue;
            };
            let rule = format!("Bash({program} *)");
            if !out.contains(&rule) {
                out.push(rule);
            }
            // A command that needs five different programs approved is one to think about as a
            // whole rather than to shred into rules.
            if out.len() >= MAX_RULES {
                return out;
            }
        }
    }
    out
}

/// How many programs one command may contribute.
const MAX_RULES: usize = 4;

/// Whether the allowlist already covers this call, in which case nobody is asked.
///
/// Deliberately conservative: it mirrors Claude Code's prefix rules closely enough to stay quiet on
/// the common path, and anything it is unsure about becomes a question. Being wrong in this
/// direction costs a click; being wrong in the other costs a command nobody approved.
pub fn already_allowed(rules: &[String], allowed: &[String]) -> bool {
    !rules.is_empty()
        && rules.iter().all(|rule| {
            allowed.iter().any(|a| {
                a == rule
                    || a == "Bash"
                    // `mcp__srv` covers `mcp__srv__tool`, and not `mcp__srv2__tool`.
                    || a.strip_prefix("mcp__").is_some_and(|server| !server.contains("__"))
                        && rule
                            .strip_prefix(a.as_str())
                            .is_some_and(|r| r.starts_with("__"))
                    || a.strip_suffix(" *)")
                        .zip(rule.strip_suffix(" *)"))
                        .is_some_and(|(a, r)| a == r)
            })
        })
}

pub fn is_edit(tool: &str) -> bool {
    matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit")
}

/// An edit inside the repository (or one of its lane checkouts) is what `acceptEdits` already
/// covers; the hook has nothing to ask. Anything else — `/tmp`, the home directory, another
/// project — is a question.
pub fn edit_is_inside(repo: &camino::Utf8Path, input: &serde_json::Value) -> bool {
    let Some(path) = input.get("file_path").and_then(|p| p.as_str()) else {
        return false;
    };
    let path = std::path::Path::new(path);
    // `repo/new/../../x` climbs out once a writer makes `new`, while its nearest existing
    // ancestor is still `repo`. Nothing honest needs `..` in a path it writes.
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    let canon =
        |p: &std::path::Path| keel_workspace::real_std(p).unwrap_or_else(|_| p.to_path_buf());
    // The file may not exist yet; its nearest existing ancestor decides. Asked of the link itself:
    // `exists()` follows it, so `repo/link -> /elsewhere/new` read as missing, the walk went up
    // to `repo`, and the write created a file outside it. A link that leads nowhere is a question.
    let mut probe = path.to_path_buf();
    loop {
        match std::fs::symlink_metadata(&probe) {
            Ok(m) if m.file_type().is_symlink() && !probe.exists() => return false,
            Ok(_) => break,
            Err(_) => {
                let Some(parent) = probe.parent() else {
                    return false;
                };
                probe = parent.to_path_buf();
            }
        }
    }
    canon(&probe).starts_with(canon(repo.as_std_path()))
}

/// The provider a chat lane's SDK bridge sends from `canUseTool`. Claude has already looked for a
/// rule and found none, so there is nothing underneath a `defer` but a refusal — and the lane's
/// allowlist was frozen when its runtime started, so a rule approved since, or trust granted
/// since, is only known here. Its `Allow once` card used to be the whole gate: no rule, no
/// "always", asked again on every call to every MCP tool, for the life of the lane.
pub const SDK: &str = "claude-sdk";
/// A Codex lane's command approval: its sandbox refused and nothing else decides after this.
pub const CODEX: &str = "codex";

/// Whether this caller takes the answer as final: nothing checks the call after Keel does. Such a
/// caller only reaches Keel because its own gate already declined, so `allow` is said only where
/// Keel can vouch for the whole call — never on a weaker reading than the gate that declined.
fn is_final(provider: &str) -> bool {
    provider == SDK || provider == CODEX
}

/// Ask the person, and wait for them.
pub async fn ask(
    state: State<Arc<AppState>>,
    Json(hook): Json<HookInput>,
) -> Result<Json<Decision>, (StatusCode, String)> {
    // no-blocking: forwards to `decide`, whose disk reads are in `blocking(…)`.
    let last = is_final(&hook.provider);
    let Json(mut decision) = decide(state, hook).await?;
    // For this caller `decide` defers only on trust or a rule it read whole; nobody answering is
    // a `deny` (`unanswered`).
    if last && decision.decision == "defer" {
        decision.decision = "allow".into();
    }
    Ok(Json(decision))
}

async fn decide(
    State(state): State<Arc<AppState>>,
    hook: HookInput,
) -> Result<Json<Decision>, (StatusCode, String)> {
    let repo = state.repo();
    let strict = hook.provider == "antigravity";
    let sdk = is_final(&hook.provider);
    if strict {
        crate::antigravity::note_asked(&hook.tool_use_id);
    }

    // A background lifetime is not authorization to execute. Only hand the command to the
    // monitor after a local trust decision or an explicit approval of this invocation.
    let background = is_background(&hook.tool_name, &hook.tool_input);
    // The monitor is Claude's. An agy command detached by hand would die with the turn unseen.
    if strict && background {
        return Ok(Json(Decision {
            decision: "deny".into(),
            reason: crate::antigravity::BACKGROUND.into(),
        }));
    }

    // A plan is not a permission either, so it sits before
    // the trust check. Trust means "stop asking whether it may run things"; whether to build this
    // plan is not that question, and a trusted project that silently swallowed the plan write
    // would leave a planning turn with no way to show its work at all.
    if let Some(plan) = plan_being_written(&hook.tool_name, &hook.tool_input) {
        return Ok(Json(plan_request(&hook, plan).await));
    }

    // A question is not a permission. Trust means "stop asking whether it may run things", and
    // an answer to "which of these two designs" is not covered by that — so it is never
    // short-circuited, on any project.
    let is_question = is_question(&hook.tool_name);

    // The license meter. Every effectful step passes this hook, so this is the one place it is
    // counted; a question is not a step. Before trust: trust is about permission, this is not.
    // The SDK asks about a hooked tool only after the hook already counted it.
    let counted = sdk
        && crate::permissions::HOOKED_TOOLS
            .split('|')
            .any(|t| t == hook.tool_name);
    if !is_question
        && !counted
        && let Err(reason) = crate::serve::blocking(crate::license::step, Ok(())).await
    {
        return Ok(Json(Decision {
            decision: "deny".into(),
            reason,
        }));
    }

    // An edit that leaves the repository is asked about whatever else is true, because every
    // other reason to stay quiet is scoped to *this* project and this is not in it.
    //
    // These two checks used to be the other way round, so trust — "stop asking about commands in
    // this repository" — silently covered writing to `/tmp`, to the home directory, and to
    // somebody else's checkout. `edit_is_inside`'s own comment has always said those are a
    // question; the trust check above it meant they never were.
    // Canonical paths and machine-local permission records are read on every
    // hooked tool call. Off the executor together, once.
    // An agy subagent asks under its own conversation, which the window never sees: its card is
    // filed under the lane's turn, and "this session" rules are the turn's — or it would be
    // asked again on every call, for ever.
    let parent = strict
        .then(|| state.lane_session(&hook.lane))
        .flatten()
        .filter(|s| !hook.session_id.is_empty() && *s != hook.session_id);
    let subagent = parent.is_some();
    let mut hook = hook;
    if let Some(parent) = parent {
        hook.session_id = parent;
    }
    let session = (!hook.session_id.is_empty()).then_some(hook.session_id.as_str());
    // Already answered by the person at the hook: the gate asking again is not a second question.
    if sdk && !hook.tool_use_id.is_empty() && approved().locked().contains(&hook.tool_use_id) {
        return Ok(Json(Decision {
            decision: "allow".into(),
            reason: String::new(),
        }));
    }
    let (leaves_the_project, trusted, effective, denies, asks) = {
        let (repo, tool, input, session) = (
            repo.clone(),
            hook.tool_name.clone(),
            hook.tool_input.clone(),
            session.map(str::to_string),
        );
        crate::serve::blocking(
            move || {
                (
                    is_edit(&tool) && !edit_is_inside(&repo, &input),
                    crate::permissions::trusted(&repo),
                    crate::permissions::effective(&repo, session.as_deref()),
                    // For agy only: Claude Code reads its own deny rules after a `defer`.
                    if strict {
                        keel_workspace::claude_home()
                            .map(|h| crate::denies::read(&h))
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    // Every provider: trust and Keel's rules answer before Claude reads these.
                    person_asks(),
                )
            },
            // Closed: a read that failed is not evidence the edit stays in the repository.
            (true, false, Default::default(), Vec::new(), Vec::new()),
        )
        .await
    };
    // A deny rule beats trust, the allowlist and a card, as it does in Claude Code.
    let unsure = match crate::denies::check(&denies, &hook.tool_name, &hook.tool_input) {
        crate::denies::Verdict::Denied(rule) => {
            return Ok(Json(Decision {
                decision: "deny".into(),
                reason: crate::antigravity::denied(&rule),
            }));
        }
        crate::denies::Verdict::Unsure => true,
        crate::denies::Verdict::Clear => false,
    };
    // An `ask` rule of the person's beats trust and Keel's own rules: it is always a card.
    let asked = match crate::denies::check(&asks, &hook.tool_name, &hook.tool_input) {
        crate::denies::Verdict::Clear => None,
        crate::denies::Verdict::Denied(rule) => Some(rule),
        crate::denies::Verdict::Unsure => Some(String::new()),
    };
    let unsure_of_denies = unsure;
    let unsure = unsure || asked.is_some();

    // One decision, already made. Nothing is queued and nobody is asked.
    //
    // An edit inside a trusted project is `allow`ed: a terminal lane's `claude` runs in its own
    // default mode and asked about every edit itself, trusted or not. Commands and fetches
    // `defer` to Claude Code's allowlist, which trust fills (`settings_json`), so the person's
    // own `deny` rules and Claude Code's shell checks still apply to them. The cost: a terminal
    // session started before the project was trusted keeps its older allowlist for commands
    // until it is restarted. A chat turn is a fresh `claude` and always has the current one.
    // Trust covers MCP tools too: it is "stop asking whether it may do things here", and a
    // browser server is thirty tools that asked one card at a time on a trusted project. An MCP
    // call is `allow`ed rather than deferred, because no allowlist rule names every server.
    let mcp = hook.tool_name.starts_with("mcp__");
    // A final caller asks about `Read` outside the workspace, a path its gate protects, or an `ask`
    // rule of the person's — none of which trust is about. Only what trust covers is answered.
    let trust_covers =
        !sdk || mcp || ["Bash", "WebFetch", "WebSearch"].contains(&hook.tool_name.as_str());
    if !is_question && !leaves_the_project && trusted && !unsure && trust_covers {
        if background {
            return Ok(Json(monitor_request(&state, &hook).await));
        }
        return Ok(Json(Decision {
            decision: if is_edit(&hook.tool_name) || mcp {
                "allow"
            } else {
                "defer"
            }
            .into(),
            reason: String::new(),
        }));
    }

    // Not for a final caller: its gate already declined this edit (plan mode, a protected path).
    if is_edit(&hook.tool_name) && !leaves_the_project && !unsure && !sdk {
        // Inside the repository is what `acceptEdits` already covers; nothing to ask.
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    let rules = rules_for(&hook.tool_name, &hook.tool_input);

    if background && !unsure && effective.iter().any(|rule| rule == "Bash") {
        return Ok(Json(monitor_request(&state, &hook).await));
    }
    // Foreground commands still pass through Claude's own shell permission checker. Monitored
    // commands do not, so our lossy display-rule parser cannot authorize their shell syntax.
    // For agy nothing checks the shell afterwards, so the lossy parser may only wave through a
    // command with no syntax it cannot see into: `ls $(rm -rf ~)` reads as `Bash(ls *)`.
    // An empty rule list means `rules_for` saw only builtins (`source`, `.`, `export`), which
    // need no rule for Claude because Claude's checker reads them; for agy that is a card.
    // Only a command has syntax to see past; an MCP call or a fetch is its name.
    // A final caller's own checker already split the command and declined it, so it is as strict.
    let parsed_whole = !(strict || sdk)
        || hook.tool_name != "Bash"
        || !rules.is_empty()
            && plain_shell(
                hook.tool_input
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default(),
            );
    if !background
        && !is_question
        && !leaves_the_project
        && !unsure
        && parsed_whole
        // An MCP card offers the server; whether this call is covered is asked of its own name,
        // so a rule for one tool still counts and does not cover its siblings.
        && if mcp {
            already_allowed(std::slice::from_ref(&hook.tool_name), &effective)
        } else {
            already_allowed(&rules, &effective)
        }
    {
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    let id = if hook.tool_use_id.is_empty() {
        format!("{:?}", std::time::Instant::now())
    } else {
        hook.tool_use_id.clone()
    };

    let pending = Pending {
        id: id.clone(),
        lane: hook.lane.clone(),
        tool: hook.tool_name.clone(),
        command: shown(
            hook.tool_input
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or_default(),
        ),
        rules,
        input: if hook.tool_name.starts_with("mcp__") {
            crate::turns::bounded_input(&hook.tool_input)
        } else {
            hook.tool_input.clone()
        },
        session_id: hook.session_id.clone(),
        reason: if let Some(rule) = asked.as_deref().filter(|r| !r.is_empty()) {
            format!("Asked because ~/.claude/settings.json asks before {rule}.")
        } else if asked.is_some() {
            "Asked because ~/.claude/settings.json asks before some commands, and Keel can't \
             tell whether this is one."
                .into()
        } else if unsure_of_denies {
            "Asked because ~/.claude/settings.json denies some commands, and Keel can't check \
             this one against them."
                .into()
        } else {
            String::new()
        },
        subagent,
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters()
        .locked()
        .insert(id.clone(), (tx, hook.lane.clone()));
    queue().locked().push(pending);
    // The window is told a question is waiting, rather than asking every 700 ms whether one is.
    crate::events::emit(
        "pending",
        None,
        serde_json::json!({ "lane": hook.lane, "session": hook.session_id }),
    );
    // On the record, so a reopened turn shows what it asked — the card itself still comes from
    // the poll.
    crate::turns::emit_for_lane(
        &state,
        &hook.lane,
        crate::turns::Fact::ApprovalAsked {
            id: id.clone(),
            tool: hook.tool_name.clone(),
            command: shown(
                hook.tool_input
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default(),
            ),
            rules: rules_for(&hook.tool_name, &hook.tool_input),
            input: crate::turns::bounded_input(&hook.tool_input),
        },
    );

    let _withdraw = Withdraw(id.clone());
    // A person who walked away must not leave the agent wedged. Timing out falls back to the
    // allowlist, which will refuse it — the same outcome as before, reached without hanging.
    match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(decision)) if background && decision.decision == "allow" => {
            Ok(Json(monitor_request(&state, &hook).await))
        }
        Ok(Ok(decision)) => Ok(Json(decision)),
        _ => {
            waiters().locked().remove(&id);
            queue().locked().retain(|p| p.id != id);
            // A question nobody answered in four minutes is the signature of "stuck on
            // thinking" — the tool name goes to Sentry, never the command.
            sentry::with_scope(
                |scope| {
                    scope.set_tag("tool", &hook.tool_name);
                    scope.set_tag("question", is_question.to_string());
                },
                || {
                    sentry::capture_message(
                        "approval timed out: nobody answered",
                        sentry::Level::Warning,
                    )
                },
            );
            Ok(Json(unanswered(strict || sdk, background).unwrap_or_else(
                || {
                    Decision {
                        decision: if background { "deny" } else { "defer" }.into(),
                        // For a background command the refusal is the agent's next prompt, and "nobody
                        // answered" on its own sent it to detach the command by hand.
                        reason: if background {
                            unanswered_background(
                                &state.repo(),
                                hook.tool_input
                                    .get("command")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or_default(),
                            )
                        } else {
                            "nobody answered".into()
                        },
                    }
                },
            )))
        }
    }
}

/// What an unanswered question becomes when nothing would refuse it afterwards: a refusal that
/// says nobody answered, where Claude's `defer` hands it to an allowlist that refuses it.
fn unanswered(strict: bool, background: bool) -> Option<Decision> {
    (strict && !background).then(|| Decision {
        decision: "deny".into(),
        reason: crate::antigravity::UNANSWERED.into(),
    })
}

/// A command the allowlist may wave through when nothing checks the shell after it: one program
/// and its arguments, every one a literal word. `rules_for` reads a command with a heuristic, and
/// a heuristic is not a parser — quoting, escapes, assignments, globs, operators and expansions
/// all make what it names differ from what a shell runs. So none of them are allowed here; a
/// command with any of them is a card, which costs a click and nothing else.
pub fn plain_shell(command: &str) -> bool {
    let words: Vec<&str> = command.split(' ').filter(|w| !w.is_empty()).collect();
    !words.is_empty()
        && words.iter().all(|w| {
            w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:@,+%".contains(c))
        })
        && !matches!(
            words[0],
            "eval" | "exec" | "env" | "command" | "builtin" | "xargs"
        )
        && !BUILTINS.contains(&words[0])
}

/// A `Bash` call the agent wants to leave running behind it.
///
/// Two ways to ask for that, and only one of them is a parameter. The other is the shell, and it
/// is the one the agent reaches for the moment the first is refused: `nohup … &`, `setsid`,
/// `disown`, a bare trailing `&`. Reported by a person watching a turn say *"my background launch
/// was refused. Starting it detached:"* — which is a reasonable move against the refusal it had
/// just been given, and which walked straight past the whole of `monitor.rs`. A command detached
/// in the text is spawned by `claude`, inside `claude`'s process group, and is killed or orphaned
/// when the turn ends; nothing delivers its output to anyone.
///
/// So both spellings ask the same question.
pub fn is_background(tool: &str, input: &serde_json::Value) -> bool {
    if tool != "Bash" {
        return false;
    }
    if input
        .get("run_in_background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    input
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(detaches)
}

/// Whether a shell command puts something behind it.
///
/// Conservative on purpose: a false positive costs one card the person answers "no" to, and a
/// false negative costs an invisible process nobody can stop. Quoted text is dropped first so
/// `echo "a & b"` is not a detach, and `&&`, `&>` and `>&` are excluded because none of them
/// background anything.
fn detaches(command: &str) -> bool {
    let mut bare = String::with_capacity(command.len());
    let (mut single, mut double) = (false, false);
    for c in command.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ if single || double => {}
            _ => bare.push(c),
        }
    }

    for word in bare.split_whitespace() {
        if matches!(word, "nohup" | "setsid" | "disown") {
            return true;
        }
    }

    let b = bare.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'&' {
            continue;
        }
        let before = i.checked_sub(1).map(|j| b[j]);
        let after = b.get(i + 1).copied();
        // `&&` (either side of it), `&>` and `>&` are redirection and control flow, not a job.
        if before == Some(b'&')
            || after == Some(b'&')
            || after == Some(b'>')
            || before == Some(b'>')
        {
            continue;
        }
        return true;
    }
    false
}

/// Take an authorized command off the turn and run it in the daemon.
///
/// It used to be a question, and the question was the wrong shape. "Should this keep running
/// after the turn?" is not a permission — the agent is not asking to be *allowed* to run
/// something, `Bash` may already be allowed and the project may be trusted; it is asking for the
/// only thing a turn cannot give it, which is to outlive the turn. There is no second answer
/// worth having: "no" means the agent runs a dev server in the foreground until Claude Code's own
/// `Bash` timeout kills it, having produced nothing, and a four-minute wait for a card nobody was
/// at the keyboard for means the same thing after a four-minute stall. Both were dressed up as a
/// decision the person was making.
///
/// Called only after authorization by `ask`. The Monitors panel owns the approved job's output
/// and Stop button; visibility is not a substitute for permission to execute it.
///
/// The answer is still a `deny`, and the reason is the whole message: `deny` is the only hook
/// verdict that reaches the agent as text it can act on, and here there is genuinely something to
/// say — "I am watching this for you, end your turn" is not a refusal even though it travels as
/// one. The agent's own call must not also run: a duplicate shell that dies at teardown helps
/// nobody.
/// The command without its own way of leaving: the monitor is what keeps it running past the
/// turn now, so `nohup`, `setsid`, a trailing `&` or `disown`, and the `>/dev/null` that went with
/// them are taken off. Run verbatim, `sleep 30 &` detached a second time inside the job — the job
/// "finished" at once with no output, and the real process ran on with nothing able to stop it.
fn attached(command: &str) -> String {
    let mut c = command.trim().to_string();
    loop {
        let before = c.clone();
        for tail in [
            "disown",
            "2>&1 >/dev/null",
            ">/dev/null 2>&1",
            "> /dev/null 2>&1",
            "&>/dev/null",
            "&> /dev/null",
        ] {
            if let Some(rest) = c.strip_suffix(tail) {
                c = rest.trim_end().to_string();
            }
        }
        for sep in [';', '&'] {
            // A lone trailing `&` or `;` — never the second half of `&&`.
            if c.ends_with(sep) && !c.ends_with("&&") {
                c.pop();
                c = c.trim_end().to_string();
            }
        }
        for head in ["nohup ", "setsid -f ", "setsid "] {
            if let Some(rest) = c.strip_prefix(head) {
                c = rest.trim_start().to_string();
            }
        }
        if c == before {
            return c;
        }
    }
}

async fn monitor_request(state: &Arc<AppState>, hook: &HookInput) -> Decision {
    let command = attached(
        hook.tool_input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or_default(),
    );

    // The lane's checkout, so a monitored `make check` sees the lane's work and not the project's.
    let dir = if hook.cwd.is_empty() {
        state.repo()
    } else {
        camino::Utf8PathBuf::from(&hook.cwd)
    };
    match crate::monitor::start(&hook.lane, &command, &dir) {
        Ok(job) => Decision {
            decision: "deny".into(),
            reason: format!(
                "Keel is running this for you as background job `{job}`, outside this turn. It \
                 survives past the end of the turn and its output will be delivered to you as a \
                 new message when it finishes — so do not wait for it, do not poll it, and do not \
                 start it again. Say that you are watching it and end your turn."
            ),
        },
        Err(e) => {
            sentry::capture_message("could not start a background job", sentry::Level::Error);
            Decision {
                decision: "deny".into(),
                reason: format!(
                    "Keel took this off your turn to run it itself and then could not ({e}). {}",
                    not_monitored(&state.repo(), &command)
                ),
            }
        }
    }
}

/// The name a finished plan travels under: the tool Claude Code would have used, if it had one.
pub const PLAN: &str = "ExitPlanMode";

/// What the Approve button answers a plan with.
///
/// A sentinel rather than prose, so every word the agent reads is written here with the rest of
/// what Keel tells it, and not in a Swift view. Matched by `Wire.Pending.planApproved`.
pub const APPROVED: &str = "keel:plan-approved";

/// The plan a plan-mode turn is writing, if this write is that.
///
/// There is no `ExitPlanMode` in headless `claude -p` — its plan-mode tool list has no plan tool
/// at all, verified against 2.1.251 — and an MCP tool cannot stand in for one, because plan mode
/// refuses every MCP call outright: `Cannot call mcp__keel__ask_user while in plan mode`, arriving
/// as `toolDenialKind: "user-rejected"`. That is Claude Code, not Keel, and nothing on our command
/// line changes it. It is also why `ask_user` has never worked in a planning turn.
///
/// What does work is the channel Claude Code itself drives. Every plan turn opens with a
/// `plan_mode` attachment naming a `planFilePath` under the Claude home, the turn is told to write
/// its plan there, and that write is permitted where everything else is refused. So the plan
/// arrives as a `Write` — a tool Keel already hooks — and this is where it is recognised.
///
/// `Write` alone, deliberately: both answers deny the write, so the file is never created and a
/// revised plan is another `Write` rather than an `Edit`.
fn plan_being_written(tool: &str, input: &serde_json::Value) -> Option<String> {
    if tool != "Write" {
        return None;
    }
    let path = input.get("file_path").and_then(|p| p.as_str())?;
    let home = keel_workspace::claude_home()?;
    if !std::path::Path::new(path).starts_with(home.join("plans").as_std_path()) {
        return None;
    }
    let plan = input.get("content").and_then(|c| c.as_str())?.trim();
    (!plan.is_empty()).then(|| plan.to_string())
}

/// Show the plan and hold the turn on it.
///
/// Both answers deny the write, and neither is a refusal. Approving cannot mean "carry on": this
/// turn runs under `--permission-mode plan` and cannot write a file whatever it is told, so the
/// build is a second turn in `acceptEdits` that Keel sends, resumed on the same conversation —
/// which is where the plan is. Keel already holds the plan's text, so the file it denies is one
/// nobody needed.
async fn plan_request(hook: &HookInput, plan: String) -> Decision {
    let id = if hook.tool_use_id.is_empty() {
        format!("{:?}", std::time::Instant::now())
    } else {
        hook.tool_use_id.clone()
    };
    let pending = Pending {
        id: id.clone(),
        lane: hook.lane.clone(),
        // Not "Write": the card this draws is the plan, and its answers are not permissions.
        tool: PLAN.into(),
        command: String::new(),
        // Nothing to remember. "Build this plan" is a decision about one plan, and a rule
        // allowing writes to the plans directory would answer the next one silently.
        rules: Vec::new(),
        input: serde_json::json!({ "plan": plan }),
        session_id: hook.session_id.clone(),
        reason: String::new(),
        subagent: false,
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters()
        .locked()
        .insert(id.clone(), (tx, hook.lane.clone()));
    queue().locked().push(pending);
    crate::events::emit(
        "pending",
        None,
        serde_json::json!({ "lane": hook.lane, "session": hook.session_id }),
    );

    let _withdraw = Withdraw(id.clone());
    let answer = match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(d)) => d
            .reason
            .split_once("do not ask again.\n\n")
            .map(|(_, a)| a.to_string())
            .unwrap_or(d.reason),
        // Nobody there, which is not the same as being told no. The plan is the whole of what the
        // turn did, so the one thing that must not happen is losing it.
        _ => {
            waiters().locked().remove(&id);
            queue().locked().retain(|p| p.id != id);
            return Decision {
                decision: "deny".into(),
                reason: "Nobody answered within the time allowed, so Keel did not show this plan. \
                         End your turn with the plan in your reply so it is not lost."
                    .into(),
            };
        }
    };

    Decision {
        decision: "deny".into(),
        reason: if answer.trim() == APPROVED {
            "The person approved the plan in Keel, which already has its text — this write is \
             refused because the file is not needed, not because anything went wrong. Keel is \
             starting the build itself, as a new turn in edit mode on this same conversation, so \
             do not implement anything now. End your turn with one line saying the plan is \
             approved."
                .into()
        } else {
            format!(
                "The person has not approved the plan. Revise it and write it again; do not start \
                 building.\n\nWhat they said:\n{answer}"
            )
        },
    }
}

/// What to say when Keel is not going to watch it.
///
/// The sentence this replaces was one sentence for two different situations, and its advice —
/// "run it in the foreground instead" — is impossible for the single most common thing anyone
/// backgrounds. A dev server never exits, so foregrounding it means Claude Code's own `Bash`
/// timeout kills it a minute or two in, having produced nothing. An agent told to do that will
/// correctly decide not to, and the only move left is to detach it by hand: reported verbatim as
/// *"my background launch was refused. Starting it detached:"*. That is not the model being
/// careless in the IDE; it is the model routing around advice it was right to reject, into the
/// one behaviour this whole subsystem exists to prevent.
///
/// So: say which of the two happened, name the thing that actually works, and close the door the
/// agent would otherwise find on its own.
/// A background command nobody was at the keyboard to approve: which of the two cases this is,
/// and the same way forward `not_monitored` gives.
fn unanswered_background(repo: &camino::Utf8Path, command: &str) -> String {
    let rest = not_monitored(repo, command);
    let advice = rest.split_once("\n\n").map_or("", |(_, a)| a);
    format!(
        "Not started: it needed the person's approval to run in the background, and nobody \
         answered in four minutes. Nobody said no.\n\n{advice}"
    )
}

fn not_monitored(repo: &camino::Utf8Path, command: &str) -> String {
    let mut out = String::from(
        "Not started: Keel tried to run this as a background job and could not. This is a fault \
         on Keel's side, not a decision anybody made about the command.",
    );

    // A dev server is what this refusal is nearly always about, and Keel has one. Saying so here
    // rather than only in the system prompt matters, because here is where the agent is looking.
    if let Some(dev) = crate::dev::detect(repo)
        && looks_like(&dev.command, command)
    {
        out.push_str(
            "\n\nThis looks like the project's dev server, which Keel runs itself: the person \
             starts it from the Preview tab and the preview follows whatever URL it announces. \
             Ask them to start it rather than starting one of your own — a second server on the \
             same port fails, and one they cannot see is worse.",
        );
    } else {
        out.push_str(
            "\n\nIf it finishes on its own, run it in the foreground and report what it said. If \
             it does not — a server, a watcher, a tail — there is nothing useful you can do with \
             it in this turn; say so and ask the person how they want it run.",
        );
    }

    out.push_str(
        "\n\nDo not detach it instead. `nohup`, `setsid`, `disown` and a trailing `&` all reach \
         the same question and get the same answer, and a shell that escapes it is killed when \
         this turn ends or left running with nothing on the machine that knows what it is.",
    );
    out
}

/// Whether a command is the project's dev server, allowing for the ways it gets spelled.
fn looks_like(dev: &str, command: &str) -> bool {
    let tail = |s: &str| {
        s.split_whitespace()
            .last()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    command.contains(dev) || (!dev.is_empty() && tail(dev) == tail(command))
}

#[derive(Deserialize)]
pub struct PollQuery {
    /// Only take questions belonging to this conversation. Absent means only the questions
    /// that belong to no conversation — never another window's.
    #[serde(default)]
    pub session: Option<String>,
    /// Take everything regardless of session: for a CLI or a debugger, never a window.
    #[serde(default)]
    pub all: bool,
    /// The window's own id. The reliable half of the match: a lane knows this before it knows
    /// its session, and a first-turn question belonged to nobody the poll could name.
    #[serde(default)]
    pub lane: Option<String>,
}

/// What the UI is waiting to show. Polled rather than pushed: the chat already holds an SSE stream
/// per turn, and a second long-lived connection for one message at a time is not worth its
/// reconnection logic.
///
/// Scoped by session, because there is more than one window now. This used to `mem::take` the whole
/// queue, so whichever caller polled first swallowed every pending question — including the ones
/// belonging to another window, which then waited out the full four minutes and failed open. A
/// caller with no session of its own, such as the CLI, passes nothing and still gets everything.
pub async fn poll(Query(q): Query<PollQuery>) -> Json<Vec<Pending>> {
    // no-blocking: the queue is memory.
    let mut queue = queue().locked();

    if q.all {
        return Json(std::mem::take(&mut *queue));
    }
    // A window on its first turn does not know its session yet, and used to take the whole
    // queue on the strength of that — including a question meant for a lane that could see it.
    // It takes only what belongs to nobody; the rest waits for the window that owns it.
    let session = q.session.filter(|s| !s.is_empty());
    let lane = q.lane.clone().filter(|l| !l.is_empty());
    let (mine, theirs): (Vec<Pending>, Vec<Pending>) = queue.drain(..).partition(|p| {
        // The lane is the reliable half: the window knows its own id before it knows the
        // session's, and a first-turn question used to match nothing and time out.
        (!p.lane.is_empty() && lane.as_deref() == Some(p.lane.as_str()))
            || (p.lane.is_empty()
                && (p.session_id.is_empty() || session.as_deref() == Some(p.session_id.as_str())))
            || (!p.session_id.is_empty() && session.as_deref() == Some(p.session_id.as_str()))
    });
    *queue = theirs;
    Json(mine)
}

/// The command, at a length a card can draw.
///
/// This is the one view that must appear instantly: the turn has stopped and is waiting on it. A
/// command carrying a large argument — a heredoc writing a file, a long commit message — was put
/// on the card whole, into a `Text` with `fixedSize` and no line limit, and CoreText measured
/// every character on the main thread. Two seconds of that is an App Hang, and it happened on the
/// one surface that must never stall.
///
/// The full command still runs; this is only what is shown. Approving a command you cannot read
/// the end of is no worse than the alternative, which is an approval you cannot see at all.
fn shown(command: &str) -> String {
    const LIMIT: usize = 4000;
    if command.chars().count() <= LIMIT {
        return command.to_string();
    }
    let head: String = command.chars().take(LIMIT).collect();
    format!(
        "{head}\n\n[… {} more characters, not shown]",
        command.chars().count() - LIMIT
    )
}

/// Whether a tool is a question to the person rather than a request to do something.
pub fn is_question(tool: &str) -> bool {
    tool == "AskUserQuestion"
}

/// What the agent is told when the person answers a question.
///
/// A `deny` whose reason is the answer. There is no hook verb for "run this tool with this
/// result", but a denial's reason is delivered to the model as the tool's result, which is the
/// same thing from where it sits — and the alternative, letting the tool run, means the CLI's own
/// sixty-second wait for a terminal that is not there.
pub fn answered(answer: &str) -> Decision {
    Decision {
        decision: "deny".into(),
        reason: format!(
            "The user answered your question in Keel. Continue with this answer; do not ask \
             again.\n\n{answer}"
        ),
    }
}

/// The person answered.
pub async fn answer(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Answer>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let allow = body.decision == "allow";

    if allow {
        // Remembered before the agent is released, so the call that follows does not ask again.
        // Machine-local permission storage is filesystem work: off the executor.
        let (repo, scope, rules, session) = (
            state.repo(),
            body.scope.clone(),
            body.rules.clone(),
            body.session.clone(),
        );
        crate::serve::blocking(
            move || {
                if scope == "trust" {
                    if let Err(why) = crate::permissions::set_trusted(&repo, true) {
                        tracing::warn!("could not remember project trust: {why}");
                        sentry::capture_message(
                            "project trust could not be remembered",
                            sentry::Level::Error,
                        );
                    }
                    return;
                }
                for rule in &rules {
                    // Reported rather than dropped. "Allow once, this session" needs a
                    // conversation to belong to, and when it had none the `Err` went into a
                    // `let _ =` — the call was allowed, the rule was never stored, and the next
                    // identical call asked again. Silent, and indistinguishable from the feature
                    // not existing.
                    if let Err(why) =
                        crate::permissions::remember(&repo, rule, &scope, session.as_deref())
                    {
                        tracing::warn!("could not remember {rule:?} for {scope}: {why}");
                        sentry::capture_message(
                            "an approval rule could not be remembered",
                            sentry::Level::Error,
                        );
                    }
                }
            },
            (),
        )
        .await;
    }

    let decision = if !body.answer.is_empty() {
        answered(&body.answer)
    } else {
        Decision {
            decision: if allow { "allow" } else { "deny" }.into(),
            reason: if allow {
                "Approved in Keel.".into()
            } else {
                "Not approved. Say what you needed and stop; do not substitute another command."
                    .into()
            },
        }
    };

    match waiters().locked().remove(&body.id) {
        Some((tx, lane)) => {
            if decision.decision == "allow" {
                note_approved(&body.id);
            }
            crate::turns::emit_for_lane(
                &state,
                &lane,
                crate::turns::Fact::ApprovalAnswered {
                    id: body.id.clone(),
                    decision: decision.decision.clone(),
                    answer: (!body.answer.is_empty()).then(|| body.answer.clone()),
                },
            );
            let _ = tx.send(decision);
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        // Already timed out, or answered twice. Not an error worth showing anyone.
        None => Ok(Json(
            serde_json::json!({ "ok": false, "reason": "no longer waiting" }),
        )),
    }
}

/// Ask the Keel on this port, from inside the hook process.
///
/// Written by hand rather than with an HTTP client: this runs on every Bash call the agent makes,
/// so it is a process spawn plus one loopback request, and a dependency for that would be the
/// larger cost. `None` on any failure, which the caller turns into silence.
pub async fn request(port: u16, hook: &HookInput) -> Option<Decision> {
    // no-blocking: the hook process's own client, not a handler.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let body = serde_json::to_string(hook).ok()?;
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;

    let request = format!(
        "POST /api/approve/ask HTTP/1.0\r\nHost: 127.0.0.1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(request.as_bytes()).await.ok()?;

    // The server holds this open while the person decides, so the read deadline has to exceed the
    // wait it enforces — otherwise the answer arrives after this side has already given up.
    let mut raw = String::new();
    tokio::time::timeout(
        WAIT + std::time::Duration::from_secs(20),
        sock.read_to_string(&mut raw),
    )
    .await
    .ok()?
    .ok()?;

    let payload = raw.split("\r\n\r\n").nth(1)?;
    serde_json::from_str::<Decision>(payload).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The allowlist's parser cannot see inside substitutions, so for agy it does not get the
    /// last word on them.
    #[test]
    fn only_a_plain_command_is_waved_through_for_antigravity() {
        assert!(plain_shell("ls -la src/"));
        assert!(plain_shell("git status"));
        assert!(!plain_shell("ls -la && git status"));
        assert!(!plain_shell("ls | sh"));
        assert!(!plain_shell("FOO=1 rm x"));
        assert!(!plain_shell("\"rm\" -rf x"));
        assert!(!plain_shell("\\rm x"));
        assert!(!plain_shell("ls *"));
        assert!(!plain_shell("xargs rm"));
        assert!(!plain_shell("source ./x.sh"));
        assert!(!plain_shell(". ./x.sh"));
        assert!(!plain_shell("ls\trm"));
        assert!(!plain_shell(""));
        assert!(!plain_shell("ls $(rm -rf ~)"));
        assert!(!plain_shell("ls `rm x`"));
        assert!(!plain_shell("echo hi > ~/.zshrc"));
        assert!(!plain_shell("ls\nrm -rf x"));
        assert!(!plain_shell("ls; eval \"$X\""));
        let rules = rules_for("Bash", &serde_json::json!({"command": "ls $(rm -rf x)"}));
        assert!(
            already_allowed(&rules, &["Bash(ls *)".to_string()]),
            "the parser this guards against stopped being lossy; revisit plain_shell"
        );
    }

    /// Nobody answering an agy question is a refusal; for Claude it still defers.
    #[test]
    fn an_unanswered_antigravity_call_is_refused() {
        assert_eq!(unanswered(true, false).unwrap().decision, "deny");
        assert!(unanswered(false, false).is_none());
        assert!(unanswered(true, true).is_none());
    }

    /// The SDK asks only when Claude found no rule, so nobody answering is a refusal there too.
    #[test]
    fn an_unanswered_sdk_call_is_refused() {
        assert_eq!(unanswered(true, false).unwrap().decision, "deny");
    }

    /// One "always" covers a server's tools, and only that server's.
    #[test]
    fn an_mcp_rule_is_its_server() {
        assert_eq!(
            rules_for("mcp__oya-browser__open_tab", &serde_json::json!({})),
            vec!["mcp__oya-browser".to_string()]
        );
        let allowed = vec!["mcp__oya-browser".to_string()];
        let call = |t: &str| already_allowed(&[t.to_string()], &allowed);
        assert!(call("mcp__oya-browser__open_tab"));
        assert!(call("mcp__oya-browser"));
        assert!(!call("mcp__oya-browser2__open_tab"));
        assert!(!call("mcp__other__open_tab"));
        // A rule for one tool still counts for that tool, and not for its siblings.
        let one = vec!["mcp__srv__a".to_string()];
        assert!(already_allowed(&["mcp__srv__a".to_string()], &one));
        assert!(!already_allowed(&["mcp__srv__ab".to_string()], &one));
        // A name with two readings offers the exact tool, and a rule that is not a bare server
        // covers nothing but itself.
        assert_eq!(
            rules_for("mcp__a__b__c", &serde_json::json!({})),
            vec!["mcp__a__b__c".to_string()]
        );
        assert!(!already_allowed(&["mcp__srv__a__x".to_string()], &one));
    }
    /// A final caller (the SDK bridge, a Codex lane) only reaches Keel after its own gate declined,
    /// so `allow` is said only where Keel can vouch for the whole call.
    #[tokio::test]
    async fn a_final_caller_is_allowed_only_what_keel_can_vouch_for() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let hook = |provider: &str, tool: &str, input: serde_json::Value| HookInput {
            tool_name: tool.into(),
            tool_input: input,
            tool_use_id: format!("final-{provider}-{tool}"),
            session_id: "final".into(),
            lane: "final".into(),
            cwd: root.to_string(),
            provider: provider.into(),
        };
        let decided = |h: HookInput| {
            let state = state.clone();
            async move {
                let call = tokio::spawn(ask(State(state), Json(h)));
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                if call.is_finished() {
                    Some(call.await.unwrap().unwrap().0.decision)
                } else {
                    call.abort();
                    let rules = queue()
                        .locked()
                        .iter()
                        .find(|p| p.lane == "final")
                        .map(|p| p.rules.clone());
                    queue().locked().retain(|p| p.lane != "final");
                    assert!(rules.is_some(), "waiting means a card");
                    None
                }
            }
        };

        // Untrusted, with a rule for git: a command the lossy parser cannot see all of is a card,
        // not the rule's `allow` — the final caller's own checker already declined it.
        crate::permissions::remember(&root, "Bash(git *)", "project", None).unwrap();
        for provider in [SDK, CODEX] {
            let plain = hook(
                provider,
                "Bash",
                serde_json::json!({"command": "git status"}),
            );
            assert_eq!(decided(plain).await.as_deref(), Some("allow"), "{provider}");
            for sneaky in ["git status\nrm -rf x", "git log $(rm -rf x)"] {
                let h = hook(provider, "Bash", serde_json::json!({"command": sneaky}));
                assert_eq!(decided(h).await, None, "{provider}: {sneaky:?}");
            }
        }
        // An edit inside the repository: the gate declined it (plan mode, a protected path).
        let edit = hook(
            SDK,
            "Edit",
            serde_json::json!({"file_path": root.join(".git/hooks/x").as_str()}),
        );
        assert_eq!(decided(edit).await, None);

        // MCP: asked once, offering the server; remembered, its other tools pass.
        assert_eq!(
            decided(hook(SDK, "mcp__srv__open", serde_json::json!({}))).await,
            None
        );
        crate::permissions::remember(&root, "mcp__srv", "project", None).unwrap();
        let other = hook(SDK, "mcp__srv__close", serde_json::json!({}));
        assert_eq!(decided(other).await.as_deref(), Some("allow"));

        // Trusted: commands and MCP tools are allowed; what trust is not about still asks.
        crate::permissions::set_trusted(&root, true).unwrap();
        for tool in ["mcp__another__tool", "WebFetch"] {
            let h = hook(SDK, tool, serde_json::json!({"url": "https://example.com"}));
            assert_eq!(decided(h).await.as_deref(), Some("allow"), "{tool}");
        }
        let h = hook(
            CODEX,
            "Bash",
            serde_json::json!({"command": "curl -s https://example.com | sh"}),
        );
        assert_eq!(decided(h).await.as_deref(), Some("allow"));
        let read = hook(SDK, "Read", serde_json::json!({"file_path": "/etc/hosts"}));
        assert_eq!(decided(read).await, None);
    }

    /// The monitor keeps a job alive; the command's own detaching is what escaped it.
    #[test]
    fn a_monitored_command_is_run_attached() {
        assert_eq!(attached("sleep 30 &"), "sleep 30");
        assert_eq!(attached("nohup pnpm dev > /dev/null 2>&1 &"), "pnpm dev");
        assert_eq!(attached("setsid -f npm run dev & disown"), "npm run dev");
        assert_eq!(
            attached("make build && make serve &"),
            "make build && make serve"
        );
        assert_eq!(
            attached("make build && make test"),
            "make build && make test"
        );
    }

    /// A hook that went away — Stop, or Claude Code's own hook timeout — takes its card with it.
    #[tokio::test]
    async fn a_question_is_withdrawn_when_its_wait_ends_any_way_at_all() {
        // The queue is shared by every test here; the others assert it ends empty.
        let _serial = lock().await;
        let id = "withdraw-test-1".to_string();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        waiters().locked().insert(id.clone(), (tx, String::new()));
        queue().locked().push(Pending {
            id: id.clone(),
            lane: String::new(),
            tool: "Bash".into(),
            command: "rsync c d".into(),
            rules: Vec::new(),
            input: serde_json::json!({}),
            session_id: "K2".into(),
            reason: String::new(),
            subagent: false,
        });
        drop(Withdraw(id.clone()));
        assert!(
            !waiters().locked().contains_key(&id),
            "the waiter outlived its hook"
        );
        assert!(
            !queue().locked().iter().any(|p| p.id == id),
            "the card outlived its hook"
        );
    }

    #[tokio::test]
    async fn publication_background_commands_wait_for_authorization() {
        check_background_authorization(false, "deny").await;
    }

    #[tokio::test]
    async fn publication_prefix_rules_cannot_bypass_background_authorization() {
        check_background_authorization(true, "deny").await;
    }

    /// A trusted project allows an edit outright — a terminal lane's `claude` would otherwise ask
    /// about it itself — and leaves commands and fetches to the allowlist trust fills, so the
    /// person's own deny rules still hold. Nothing is queued for any of them. A question is still
    /// a question.
    #[tokio::test]
    async fn an_antigravity_subagent_asks_as_its_turn() {
        let _serial = lock().await;
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let token = state.claim("agy-lane", &root, true).unwrap();
        state.bound("agy-lane", token, "parent");
        let hook = |tool: &str, input: serde_json::Value, id: &str| HookInput {
            tool_name: tool.into(),
            tool_input: input,
            tool_use_id: id.into(),
            session_id: "child".into(),
            lane: "agy-lane".into(),
            cwd: root.to_string(),
            provider: "antigravity".into(),
        };
        // A session rule the window granted the turn covers its subagent, and an MCP call is
        // matched by its name like Claude's — neither is asked about again.
        crate::permissions::remember(&root, "mcp__qa__echo", "session", Some("parent")).unwrap();
        let d = ask(
            State(state.clone()),
            Json(hook(
                "mcp__qa__echo",
                serde_json::json!({"text": "hi"}),
                "agy-sub-1",
            )),
        )
        .await
        .unwrap();
        assert_eq!(d.decision, "defer");
        // Anything else it asks is filed under the turn's session and says it is a subagent's.
        let pending = tokio::spawn(ask(
            State(state.clone()),
            Json(hook(
                "Bash",
                serde_json::json!({"command": "cargo build"}),
                "agy-sub-2",
            )),
        ));
        let card = loop {
            if let Some(p) = queue()
                .locked()
                .iter()
                .find(|p| p.id == "agy-sub-2")
                .cloned()
            {
                break p;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(card.session_id, "parent");
        assert!(card.subagent);
        let (tx, _) = waiters().locked().remove("agy-sub-2").unwrap();
        let _ = tx.send(Decision {
            decision: "deny".into(),
            reason: "no".into(),
        });
        assert_eq!(pending.await.unwrap().unwrap().decision, "deny");
        queue().locked().retain(|p| p.id != "agy-sub-2");
    }

    #[tokio::test]
    async fn a_trusted_project_asks_nothing_but_questions() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        crate::permissions::set_trusted(&root, true).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let hook = |tool: &str, input: serde_json::Value| HookInput {
            tool_name: tool.into(),
            tool_input: input,
            tool_use_id: format!("trusted-{tool}"),
            session_id: "trusted".into(),
            lane: "trusted".into(),
            cwd: root.to_string(),
            provider: String::new(),
        };
        for (tool, input, expected) in [
            (
                "Bash",
                serde_json::json!({"command": "cargo test"}),
                "defer",
            ),
            (
                "Edit",
                serde_json::json!({"file_path": root.join("a.rs").as_str()}),
                "allow",
            ),
            (
                "WebFetch",
                serde_json::json!({"url": "https://example.com"}),
                "defer",
            ),
        ] {
            let d = ask(State(state.clone()), Json(hook(tool, input)))
                .await
                .unwrap();
            assert_eq!(d.decision, expected, "{tool}");
        }
        let allow: serde_json::Value = serde_json::from_str(&crate::permissions::settings_json(
            &root, 1, None, None, &root,
        ))
        .unwrap();
        for tool in ["Bash", "WebFetch"] {
            assert!(
                allow["permissions"]["allow"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r == tool),
                "trust must put {tool} on the allowlist it defers to"
            );
        }
        let question = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ask(
                State(state.clone()),
                Json(hook("AskUserQuestion", serde_json::json!({}))),
            ),
        )
        .await;
        assert!(question.is_err(), "a question waits for the person");
        queue().locked().retain(|p| p.lane != "trusted");
    }

    #[tokio::test]
    async fn publication_explicit_approval_starts_one_monitored_job() {
        check_background_authorization(false, "allow").await;
    }

    async fn check_background_authorization(prefix: bool, response: &str) {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        if prefix {
            crate::permissions::remember(&root, "Bash(echo *)", "project", None).unwrap();
        }
        let id = format!("background-{}", root.file_name().unwrap());
        let hook = HookInput {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command":"echo permission-test", "run_in_background":true}),
            tool_use_id: id.clone(),
            session_id: id.clone(),
            lane: id.clone(),
            cwd: root.to_string(),
            provider: String::new(),
        };
        let task_state = state.clone();
        let task = tokio::spawn(async move { ask(State(task_state), Json(hook)).await });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !task.is_finished()
            && !queue().locked().iter().any(|p| p.id == id)
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            queue().locked().iter().any(|p| p.id == id),
            "a background flag must not bypass approval"
        );
        let _ = answer(
            State(state),
            Json(Answer {
                id: id.clone(),
                decision: response.into(),
                session: None,
                rules: vec![],
                scope: "once".into(),
                answer: String::new(),
            }),
        )
        .await
        .unwrap();
        let decision = task.await.unwrap().unwrap().0;
        assert_eq!(decision.decision, "deny");
        assert_eq!(
            decision.reason.contains("background job"),
            response == "allow"
        );
        queue().locked().retain(|pending| pending.id != id);
    }

    /// Detaching in the shell is the same request as `run_in_background`, and reaches the same
    /// question.
    ///
    /// From a real turn: *"my background launch was refused. Starting it detached:"*. The flag
    /// was the only thing checked, so the second attempt walked past `monitor.rs` entirely and
    /// became a process `claude` spawned, in `claude`'s group, killed or orphaned at turn end.
    #[test]
    fn a_command_detached_in_the_shell_is_a_background_command() {
        let bash = |c: &str| serde_json::json!({ "command": c });
        for command in [
            "pnpm dev &",
            "nohup pnpm dev &",
            "nohup python -m http.server",
            "setsid ./run.sh",
            "npm start & disown",
            "cargo watch -x test &",
        ] {
            assert!(
                is_background("Bash", &bash(command)),
                "not caught: {command}"
            );
        }
    }

    /// And the things that merely contain an ampersand are not. A false positive is one card
    /// nobody asked for, and `&&` is in half the commands an agent writes.
    #[test]
    fn ordinary_commands_are_not_detaching() {
        let bash = |c: &str| serde_json::json!({ "command": c });
        for command in [
            "make check && make build",
            "cargo test 2>&1 | tail",
            "grep -r 'a & b' .",
            "ls -la",
            "npm run build &> out.log",
        ] {
            assert!(
                !is_background("Bash", &bash(command)),
                "false positive: {command}"
            );
        }
    }

    #[test]
    fn the_flag_is_still_the_ordinary_way_to_ask() {
        assert!(is_background(
            "Bash",
            &serde_json::json!({ "command": "make check", "run_in_background": true })
        ));
        assert!(!is_background(
            "Edit",
            &serde_json::json!({ "command": "pnpm dev &" })
        ));
    }

    /// A job that could not start is Keel's fault, and says so.
    ///
    /// This message used to have two other openings — "the person said no" and "nobody answered
    /// within four minutes" — because monitoring was a question. It is not one any more: a
    /// command that wants to outlive the turn is taken off the turn and listed in Monitors, where
    /// it can be watched and stopped. The only way not to be running now is a failure to spawn,
    /// and an agent told that must not read it as a decision it should route around.
    #[test]
    fn a_job_that_could_not_start_reads_as_a_fault_and_not_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8Path::from_path(dir.path()).unwrap();
        let text = not_monitored(repo, "pnpm dev");
        assert!(text.contains("could not"), "{text}");
        assert!(
            !text.contains("said no") && !text.contains("nobody answered"),
            "nobody decided anything: {text}"
        );
    }

    /// And it does not leave the door open that the agent walked through.
    #[test]
    fn the_refusal_closes_the_door_it_used_to_leave_open() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8Path::from_path(dir.path()).unwrap();
        let text = not_monitored(repo, "pnpm dev");
        assert!(text.contains("Do not detach it"), "{text}");
        assert!(text.contains("nohup"), "name the spellings: {text}");
    }

    /// When the command is the project's own dev server, say the thing that works instead of
    /// "run it in the foreground", which for a server is advice no agent should take.
    #[test]
    fn a_dev_server_is_pointed_at_the_one_keel_runs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            repo.join("package.json"),
            r#"{"name":"x","scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();
        let detected = crate::dev::detect(&repo).expect("a dev script is a dev server");

        let text = not_monitored(&repo, &detected.command);
        assert!(text.contains("Preview tab"), "{text}");
        let late = unanswered_background(&repo, &detected.command);
        assert!(
            late.contains("nobody answered") && late.contains("Preview tab"),
            "{late}"
        );
        assert!(!not_monitored(&repo, "gh run watch 123").contains("Preview tab"));
    }

    /// The queue is a process-wide static, and the tests that touch it run in parallel.
    pub(crate) async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        LOCK.lock().await
    }

    pub(super) static ASKS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// A person's `ask` rule is a card whatever else would allow the call — trust, a broader
    /// saved rule — and once they approve it at the hook, the gate asking again gets that answer.
    #[tokio::test]
    async fn the_persons_ask_rules_are_always_a_card() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        crate::permissions::remember(&root, "Bash(git *)", "project", None).unwrap();
        crate::permissions::set_trusted(&root, true).unwrap();
        *ASKS.locked() = vec!["Bash(git push --keel-test *)".into()];
        let state = Arc::new(AppState::new(root.clone()));
        let hook = |provider: &str, command: &str, id: &str| HookInput {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": command}),
            tool_use_id: id.into(),
            session_id: "asks".into(),
            lane: "asks".into(),
            cwd: root.to_string(),
            provider: provider.into(),
        };
        // Not asked about: trust answers.
        let d = ask(State(state.clone()), Json(hook(SDK, "git status", "a1")))
            .await
            .unwrap();
        assert_eq!(d.decision, "allow");
        // Asked about: a card, from the hook, naming the rule.
        let waiting = tokio::spawn(ask(
            State(state.clone()),
            Json(hook("", "git push --keel-test origin", "a2")),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!waiting.is_finished(), "an ask rule beats trust");
        let reason = queue()
            .locked()
            .iter()
            .find(|p| p.id == "a2")
            .map(|p| p.reason.clone())
            .unwrap();
        assert!(reason.contains("Bash(git push --keel-test *)"), "{reason}");
        // Approved there; the gate asking again for the same call is not a second card.
        let _ = answer(
            State(state.clone()),
            Json(Answer {
                id: "a2".into(),
                decision: "allow".into(),
                session: None,
                rules: Vec::new(),
                scope: "once".into(),
                answer: String::new(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(waiting.await.unwrap().unwrap().decision, "allow");
        let again = ask(
            State(state.clone()),
            Json(hook(SDK, "git push --keel-test origin", "a2")),
        )
        .await
        .unwrap();
        assert_eq!(again.decision, "allow");
        ASKS.locked().clear();
    }

    /// A link inside the repository that leads out of it, or a path that climbs out once a
    /// directory is made, is not an edit inside the repository.
    #[cfg(unix)]
    #[test]
    fn an_edit_through_a_link_or_a_climb_is_a_question() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(repo.path()).unwrap();
        let at = |p: std::path::PathBuf| serde_json::json!({"file_path": p.to_str().unwrap()});
        // Dangling: the target does not exist yet, which is exactly when the write creates it.
        std::os::unix::fs::symlink(outside.path().join("new"), repo.path().join("pm.md")).unwrap();
        assert!(!edit_is_inside(root, &at(repo.path().join("pm.md"))));
        // Live, to a file outside.
        std::fs::write(outside.path().join("there"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("there"), repo.path().join("live")).unwrap();
        assert!(!edit_is_inside(root, &at(repo.path().join("live"))));
        let climb = repo.path().join("new/../../x");
        assert!(!edit_is_inside(root, &at(climb)));
        // An ordinary new file in a new folder is still inside.
        assert!(edit_is_inside(root, &at(repo.path().join("src/new.rs"))));
    }

    #[test]
    fn edits_inside_the_repository_are_not_questions_and_outside_ones_name_the_directory() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let inside = serde_json::json!({ "file_path": repo.join("src/new/file.ts").as_str() });
        let outside = serde_json::json!({ "file_path": "/tmp/keel-test/report.md" });
        assert!(edit_is_inside(&repo, &inside));
        assert!(!edit_is_inside(&repo, &outside));
        assert_eq!(
            rules_for("Write", &outside),
            vec!["Write(/tmp/keel-test/*)"]
        );
    }

    /// Trust stops at the edge of the project it was granted for.
    ///
    /// "Trust this project" is scoped to one repository and stored in its own
    /// machine-local permission storage — that scoping is the reason it is safe to offer. But the
    /// trust check ran *before* the "does this edit leave the repository" check, so on a trusted
    /// project an edit to `/tmp`, to the home directory, or to somebody else's checkout was
    /// deferred without a question. `edit_is_inside`'s own comment had always said those are a
    /// question; the ordering above it meant they never were.
    #[test]
    fn an_edit_outside_the_project_is_not_covered_by_trusting_it() {
        let repo = camino::Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        let inside = serde_json::json!({ "file_path": repo.join("src/approve.rs").to_string() });
        assert!(
            edit_is_inside(&repo, &inside),
            "an edit to the project's own source read as leaving it"
        );

        for stray in ["/tmp/keel-test/x.rs", "/etc/hosts"] {
            let outside = serde_json::json!({ "file_path": stray });
            assert!(
                !edit_is_inside(&repo, &outside),
                "{stray} read as inside the project, so trust would cover it"
            );
        }
    }

    /// The approval card is the one view that must appear instantly — the turn has stopped and is
    /// waiting on it — and a command carrying a heredoc was drawn whole, into a `Text` with
    /// `fixedSize` and no line limit. CoreText measured every character on the main thread and the
    /// app hung. Reported from a real machine, at 0.2.46.
    #[test]
    fn a_huge_command_is_cut_down_before_it_reaches_a_card() {
        let ordinary = "git commit -m 'a normal message'";
        assert_eq!(shown(ordinary), ordinary, "nothing normal is touched");

        let huge = "cat <<'EOF'\n".to_string() + &"x".repeat(200_000) + "\nEOF";
        let out = shown(&huge);
        assert!(
            out.chars().count() < 4_100,
            "still {} chars",
            out.chars().count()
        );
        assert!(
            out.contains("more characters, not shown"),
            "it says it was cut"
        );
        assert!(
            out.starts_with("cat <<'EOF'"),
            "the beginning is what identifies it"
        );
    }

    /// A loop is one thing to approve, not one rule per keyword in it.
    ///
    /// Reported from the running app: a `for f in …; do echo; cat -n; done` over ten files put
    /// "Allow Bash(for *) Bash(do *) Bash(cat *) Bash(done *)" on a button, and approving it would
    /// have written all four. `Bash(do *)` matches nothing ever; `Bash(for *)` matches every
    /// command beginning with the word "for".
    #[test]
    fn a_shell_loop_does_not_become_a_rule_per_keyword() {
        let input = serde_json::json!({
            "command": "cd /tmp/x && for f in a.java b.java; do echo \"=== $f ===\"; cat -n \"$f\"; done"
        });
        let rules = rules_for("Bash", &input);
        for junk in [
            "Bash(for *)",
            "Bash(do *)",
            "Bash(done *)",
            "Bash(f *)",
            "Bash(in *)",
        ] {
            assert!(
                !rules.contains(&junk.to_string()),
                "{junk} is not a program: {rules:?}"
            );
        }
        assert!(
            rules.contains(&"Bash(cat *)".to_string()),
            "the real command is gone: {rules:?}"
        );
        assert!(
            rules.contains(&"Bash(echo *)".to_string()),
            "the real command is gone: {rules:?}"
        );
    }

    /// The other shapes, so the keyword lists do not swallow a real command with them.
    #[test]
    fn a_keyword_before_a_program_still_names_the_program() {
        let cases = [
            ("if make check; then echo ok; fi", "Bash(make *)"),
            ("while read line; do wc -l; done", "Bash(wc *)"),
        ];
        for (command, expected) in cases {
            let rules = rules_for("Bash", &serde_json::json!({ "command": command }));
            assert!(
                rules.contains(&expected.to_string()),
                "`{command}` lost its program: {rules:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_first_turn_question_reaches_the_lane_that_provoked_it() {
        use super::*;
        let _guard = lock().await;
        let q = queue();
        q.lock().unwrap().clear();
        // The hook knows the Claude session; the window does not yet, and polls by lane only.
        q.lock().unwrap().push(Pending {
            id: "1".into(),
            lane: "LANE-A".into(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: "s-new".into(),
            reason: String::new(),
            subagent: false,
        });
        let other = poll(Query(PollQuery {
            session: None,
            all: false,
            lane: Some("LANE-B".into()),
        }))
        .await;
        assert!(other.0.is_empty(), "another window's question stays queued");
        let mine = poll(Query(PollQuery {
            session: None,
            all: false,
            lane: Some("LANE-A".into()),
        }))
        .await;
        assert_eq!(
            mine.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["1"]
        );
    }

    #[tokio::test]
    async fn a_window_without_a_session_never_takes_another_windows_question() {
        use super::*;
        let _guard = lock().await;
        let q = queue();
        q.lock().unwrap().clear();
        q.lock().unwrap().push(Pending {
            id: "1".into(),
            lane: String::new(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: "s-other".into(),
            reason: String::new(),
            subagent: false,
        });
        q.lock().unwrap().push(Pending {
            id: "2".into(),
            lane: String::new(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: String::new(),
            reason: String::new(),
            subagent: false,
        });
        let got = poll(Query(PollQuery {
            session: None,
            all: false,
            lane: None,
        }))
        .await;
        assert_eq!(
            got.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["2"]
        );
        let got = poll(Query(PollQuery {
            session: Some("s-other".into()),
            all: false,
            lane: None,
        }))
        .await;
        assert_eq!(
            got.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["1"]
        );
        assert!(q.lock().unwrap().is_empty());
    }

    fn pending(id: &str, session: &str) -> Pending {
        Pending {
            id: id.into(),
            lane: String::new(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec!["Bash(ls *)".into()],
            input: serde_json::Value::Null,
            session_id: session.into(),
            reason: String::new(),
            subagent: false,
        }
    }

    /// A question reaches the window that provoked it, and no other.
    ///
    /// One test rather than three, because the queue is a process-global and parallel tests over it
    /// would interfere with each other rather than with the bug.
    ///
    /// Before this, `poll` took the whole queue. Two windows meant the first one to poll swallowed
    /// every pending question, and the window actually waiting on one sat there until the hook's
    /// four-minute timeout failed it open — a refusal the person never saw and never agreed to.
    #[tokio::test]
    async fn a_question_goes_to_the_window_that_asked_it() {
        // The queue is a process-wide static and these tests run in parallel. Without this the
        // test drains a queue another test is filling and fails about once in three runs — which
        // is worse than no test, because the failure is about the harness and reads as the
        // product.
        let _guard = lock().await;
        {
            let mut q = queue().lock().expect("queue lock");
            q.clear();
            q.push(pending("a1", "session-a"));
            q.push(pending("b1", "session-b"));
            // The hook did not say which conversation this came from.
            q.push(pending("orphan", ""));
        }

        let mine = poll(Query(PollQuery {
            session: Some("session-a".into()),
            all: false,
            lane: None,
        }))
        .await;
        let got: Vec<&str> = mine.0.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            got,
            vec!["a1", "orphan"],
            "a poll takes its own questions, and an unattributed one rather than stranding it"
        );

        let theirs = poll(Query(PollQuery {
            session: Some("session-b".into()),
            all: false,
            lane: None,
        }))
        .await;
        assert_eq!(
            theirs.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["b1"],
            "the other window's question survived the first poll"
        );

        assert!(
            queue().lock().expect("queue lock").is_empty(),
            "nothing is left queued once both windows have polled"
        );
    }

    /// A question reaches the person even on a trusted project, and the answer reaches the agent.
    ///
    /// The plan a planning turn writes is a plan, and a write anywhere else is not.
    ///
    /// This is the whole seam. Plan mode refuses every MCP tool, so the plan write is the only
    /// channel out of a planning turn — if the detector stops matching, a plan silently becomes a
    /// permission card asking whether the agent may write a file in the Claude home, which is the
    /// question nobody can answer usefully.
    #[test]
    fn a_plan_write_is_recognised_and_an_ordinary_write_is_not() {
        let home = keel_workspace::claude_home().expect("a claude home");
        let plan = serde_json::json!({
            "file_path": home.join("plans").join("some-slug.md"),
            "content": "# Plan\n1. Do the thing",
        });
        assert_eq!(
            plan_being_written("Write", &plan).as_deref(),
            Some("# Plan\n1. Do the thing")
        );
        // The tool matters: only `Write` creates one, because both answers deny the write and a
        // revision therefore arrives as another `Write`.
        assert!(plan_being_written("Edit", &plan).is_none());
        // A write to the Claude home that is not a plan is an ordinary out-of-project edit.
        let elsewhere = serde_json::json!({
            "file_path": home.join("settings.json"),
            "content": "{}",
        });
        assert!(plan_being_written("Write", &elsewhere).is_none());
        // An empty plan is not a plan; it would draw an empty card the turn then waits on.
        let empty = serde_json::json!({
            "file_path": home.join("plans").join("some-slug.md"),
            "content": "   ",
        });
        assert!(plan_being_written("Write", &empty).is_none());
    }

    /// Approving stops the planning turn, and both answers refuse the write.
    ///
    /// The stop is the part worth pinning. A turn told "approved, go ahead" is running under
    /// `--permission-mode plan` and cannot write whatever it is told, so it would read as success
    /// and produce nothing.
    #[tokio::test]
    async fn approving_a_plan_stops_the_turn_and_refusing_sends_the_note_back() {
        let _guard = lock().await;
        for (answer, expected) in [
            (APPROVED, "do not implement anything now"),
            ("Fold the export instead", "has not approved"),
        ] {
            let hook = HookInput {
                tool_name: "Write".into(),
                tool_input: serde_json::json!({}),
                tool_use_id: format!("plan-{answer}"),
                session_id: "s1".into(),
                lane: "l1".into(),
                cwd: String::new(),
                provider: String::new(),
            };
            let asking = tokio::spawn(async move { plan_request(&hook, "# Plan".into()).await });
            let mut found = None;
            for _ in 0..50 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if let Some(p) = queue().locked().iter().find(|p| p.tool == PLAN) {
                    assert_eq!(p.input["plan"], "# Plan");
                    found = Some(p.id.clone());
                    break;
                }
            }
            let id = found.expect("queued");
            let (tx, _) = waiters().locked().remove(&id).unwrap();
            queue().locked().retain(|p| p.id != id);
            tx.send(answered(answer)).unwrap();
            let decision = asking.await.unwrap();
            // Never an allow: the file is not needed either way, and letting the write through on
            // approval would leave the turn believing it is still planning.
            assert_eq!(decision.decision, "deny");
            assert!(decision.reason.contains(expected), "{}", decision.reason);
        }
    }

    /// Trust short-circuits permissions, and `AskUserQuestion` matched the same hook — so on a
    /// trusted project the CLI's own sixty-second timeout ran instead, and the agent "continued
    /// without an answer" to a question nobody saw.
    #[test]
    fn a_question_is_never_a_permission() {
        assert!(is_question("AskUserQuestion"));
        assert!(!is_question("Bash"));
        let d = answered("Which database?: Postgres");
        assert_eq!(d.decision, "deny", "delivered as the tool result");
        assert!(d.reason.contains("Postgres"));
        assert!(d.reason.contains("do not ask again"));
    }

    #[test]
    fn a_compound_command_needs_every_program_in_it() {
        let input = serde_json::json!({ "command": "bun install && make check" });
        assert_eq!(
            rules_for("Bash", &input),
            vec!["Bash(bun *)".to_string(), "Bash(make *)".to_string()]
        );
    }

    /// Approving the wrapper approves everything it can launch, so it is stepped over — not
    /// dropped. The first version dropped the whole segment, so `sudo rm -rf /` derived no rules
    /// at all, which `already_allowed` would then have to read as "nothing to check".
    #[test]
    fn a_wrapper_is_stepped_over_not_dropped() {
        for (command, want) in [
            ("sudo rm -rf /tmp/x", "Bash(rm *)"),
            ("env FOO=1 node app.js", "Bash(node *)"),
            ("FOO=1 BAR=2 node app.js", "Bash(node *)"),
            ("time sudo make install", "Bash(make *)"),
            ("/usr/local/bin/wrangler deploy", "Bash(wrangler *)"),
            // `cd` takes a path, not a command: stepping over it the way a wrapper is stepped
            // over would ask to approve `frontend`.
            ("cd frontend && bun install", "Bash(bun *)"),
        ] {
            assert_eq!(
                rules_for("Bash", &serde_json::json!({ "command": command })),
                vec![want.to_string()],
                "{command}"
            );
        }
    }

    #[test]
    fn a_pipeline_needs_both_sides() {
        let input = serde_json::json!({ "command": "cat a.txt | grep x" });
        assert_eq!(
            rules_for("Bash", &input),
            vec!["Bash(cat *)".to_string(), "Bash(grep *)".to_string()]
        );
    }

    /// The common path is a command already approved, and it must not cost a question.
    #[test]
    fn an_allowed_command_asks_nobody() {
        let allowed = vec!["Bash(make *)".to_string(), "Bash(bun *)".to_string()];
        assert!(already_allowed(
            &rules_for("Bash", &serde_json::json!({ "command": "make check" })),
            &allowed
        ));
        assert!(already_allowed(
            &rules_for(
                "Bash",
                &serde_json::json!({ "command": "bun install && make check" })
            ),
            &allowed
        ));
        // One half missing is still a question.
        assert!(!already_allowed(
            &rules_for(
                "Bash",
                &serde_json::json!({ "command": "bun install && docker ps" })
            ),
            &allowed
        ));
        assert!(!already_allowed(
            &rules_for("Bash", &serde_json::json!({ "command": "docker ps" })),
            &allowed
        ));
    }

    /// A command that derives no rule at all — an empty string — must not read as "allowed".
    #[test]
    fn nothing_to_check_is_not_the_same_as_allowed() {
        assert!(!already_allowed(&[], &["Bash(make *)".to_string()]));
    }

    /// A heredoc is one command, not one command per line.
    ///
    /// Splitting on newlines turned an inline Python script into a rule for every line of it, and
    /// clicking "always allow" stored them: two hundred rules in one real project, including
    /// `Bash(assert *)`, `Bash(def *)` and `Bash(the *)`.
    #[test]
    fn a_script_does_not_become_one_rule_per_line() {
        let command = "python3 - <<'PY'\n\
                       import json\n\
                       assert 1 == 1\n\
                       def go():\n\
                       print(\"ok\")\n\
                       PY";
        let rules = rules_for("Bash", &serde_json::json!({ "command": command }));
        assert_eq!(rules, vec!["Bash(python3 *)".to_string()], "got {rules:?}");
    }

    /// And a token that is not shaped like a binary never becomes one.
    #[test]
    fn only_things_that_look_like_programs_become_rules() {
        for junk in ["} && x", "1\")) && y", "# comment && z", "-flag && w"] {
            let rules = rules_for("Bash", &serde_json::json!({ "command": junk }));
            assert!(
                rules
                    .iter()
                    .all(|r| !r.contains('}') && !r.contains('#') && !r.contains('"')),
                "{junk} produced {rules:?}"
            );
        }
    }

    /// A command needing five different programs is one to consider whole.
    #[test]
    fn rules_are_capped() {
        let command = "a && b && c && d && e && f && g";
        let rules = rules_for("Bash", &serde_json::json!({ "command": command }));
        assert!(rules.len() <= MAX_RULES, "got {rules:?}");
    }
}
