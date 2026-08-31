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

type Waiters = Mutex<HashMap<String, tokio::sync::oneshot::Sender<Decision>>>;

pub fn waiters() -> &'static Waiters {
    static W: OnceLock<Waiters> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// Requests the UI has not yet been shown, so a page that reloads mid-question still sees it.
pub fn queue() -> &'static Mutex<Vec<Pending>> {
    static Q: OnceLock<Mutex<Vec<Pending>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

/// A command that stands in front of the real one and would launch it.
///
/// Never the rule itself: approving `sudo` approves everything it can start.
const WRAPPERS: &[&str] = &[
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
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // The file may not exist yet; its nearest existing ancestor decides.
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        let Some(parent) = probe.parent() else {
            return false;
        };
        probe = parent.to_path_buf();
    }
    canon(&probe).starts_with(canon(repo.as_std_path()))
}

/// Ask the person, and wait for them.
pub async fn ask(
    State(state): State<Arc<AppState>>,
    Json(hook): Json<HookInput>,
) -> Result<Json<Decision>, (StatusCode, String)> {
    let repo = state.repo();

    // A command the agent backgrounds is killed when the turn ends — measured, and the reason
    // `monitor.rs` exists. So it never runs under the agent at all: the person is asked whether
    // Keel should watch it, Keel runs it if they say yes, and either way the call is refused so
    // the agent does not end up with a duplicate shell that is about to die.
    //
    // Above the trust check on purpose. Trust means "stop asking whether it may run things", and
    // "should this keep running after the turn" is not that question — the same reasoning that
    // keeps `AskUserQuestion` out of it.
    if is_background(&hook.tool_name, &hook.tool_input) {
        return Ok(Json(monitor_request(&state, &hook).await));
    }

    // A question is not a permission. Trust means "stop asking whether it may run things", and
    // an answer to "which of these two designs" is not covered by that — so it is never
    // short-circuited, on any project.
    let is_question = is_question(&hook.tool_name);

    // An edit that leaves the repository is asked about whatever else is true, because every
    // other reason to stay quiet is scoped to *this* project and this is not in it.
    //
    // These two checks used to be the other way round, so trust — "stop asking about commands in
    // this repository" — silently covered writing to `/tmp`, to the home directory, and to
    // somebody else's checkout. `edit_is_inside`'s own comment has always said those are a
    // question; the trust check above it meant they never were.
    let leaves_the_project = is_edit(&hook.tool_name) && !edit_is_inside(&repo, &hook.tool_input);

    if is_edit(&hook.tool_name) && !leaves_the_project {
        // Inside the repository is what `acceptEdits` already covers; nothing to ask.
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    // One decision, already made. Nothing is queued and nobody is asked.
    if !is_question && !leaves_the_project && crate::permissions::trusted(&repo) {
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    let rules = rules_for(&hook.tool_name, &hook.tool_input);
    let session = (!hook.session_id.is_empty()).then_some(hook.session_id.as_str());

    if !is_question
        && !leaves_the_project
        && already_allowed(&rules, &crate::permissions::effective(&repo, session))
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
        input: hook.tool_input.clone(),
        session_id: hook.session_id.clone(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters().locked().insert(id.clone(), tx);
    queue().locked().push(pending);

    // A person who walked away must not leave the agent wedged. Timing out falls back to the
    // allowlist, which will refuse it — the same outcome as before, reached without hanging.
    match tokio::time::timeout(WAIT, rx).await {
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
            Ok(Json(Decision {
                decision: "defer".into(),
                reason: "nobody answered".into(),
            }))
        }
    }
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

/// Ask whether Keel should monitor it, and either start it or say why it did not.
///
/// The answer is always a `deny`, and the reason is the whole message: `deny` is the only hook
/// verdict that reaches the agent as text it can act on, and here there is genuinely something to
/// say — "I am watching this for you, end your turn" is not a refusal even though it travels as
/// one.
async fn monitor_request(state: &Arc<AppState>, hook: &HookInput) -> Decision {
    let command = hook
        .tool_input
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();

    let id = if hook.tool_use_id.is_empty() {
        format!("{:?}", std::time::Instant::now())
    } else {
        hook.tool_use_id.clone()
    };
    let pending = Pending {
        id: id.clone(),
        lane: hook.lane.clone(),
        // Not "Bash": the card this draws asks a different question, with different answers.
        tool: MONITOR.into(),
        command: shown(&command),
        // Nothing to remember. "Monitor this" is a decision about one command, not a rule about
        // a program — writing it into the allowlist would silently background the next one too.
        rules: Vec::new(),
        input: hook.tool_input.clone(),
        session_id: hook.session_id.clone(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters().locked().insert(id.clone(), tx);
    queue().locked().push(pending);

    let answered = match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(d)) => Some(d.decision == "allow"),
        // Nobody answered, or nobody was there — which is not the same as being told no, and used
        // to arrive as the same sentence.
        _ => {
            waiters().locked().remove(&id);
            queue().locked().retain(|p| p.id != id);
            None
        }
    };

    if answered != Some(true) {
        return Decision {
            decision: "deny".into(),
            reason: not_monitored(&state.repo(), &command, answered.is_none()),
        };
    }

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
                    "Keel agreed to watch this and then could not start it ({e}). {}",
                    not_monitored(&state.repo(), &command, false)
                ),
            }
        }
    }
}

/// The tool name a monitor request travels under, so the app can draw the right card.
pub const MONITOR: &str = "MonitorRequest";

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
fn not_monitored(repo: &camino::Utf8Path, command: &str, timed_out: bool) -> String {
    let mut out = String::from(if timed_out {
        "Not started: nobody answered the question about this within four minutes, so Keel did \
         not run it. Nobody said no — the person may simply have been away from the window."
    } else {
        "Not started: the person said no to Keel watching this."
    });

    // A dev server is what this refusal is nearly always about, and Keel has one. Saying so here
    // rather than only in the system prompt matters, because here is where the agent is looking.
    if let Some(dev) = crate::dev::detect(repo)
        && looks_like(&dev.command, command)
    {
        out.push_str(
            "\n\nThis looks like the project's dev server, which Keel runs itself: the person \
             starts it from the Designer tab and the preview follows whatever URL it announces. \
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
        if body.scope == "trust" {
            let _ = crate::permissions::set_trusted(&state.repo(), true);
        } else {
            for rule in &body.rules {
                let _ = crate::permissions::remember(
                    &state.repo(),
                    rule,
                    &body.scope,
                    body.session.as_deref(),
                );
            }
        }
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
        Some(tx) => {
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

    /// "Nobody answered" and "they said no" are different things and now say so.
    #[test]
    fn a_refusal_nobody_gave_is_not_reported_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8Path::from_path(dir.path()).unwrap();

        let timed_out = not_monitored(repo, "pnpm dev", true);
        assert!(timed_out.contains("nobody answered"), "{timed_out}");
        assert!(
            timed_out.contains("Nobody said no"),
            "a timeout must not read as a decision: {timed_out}"
        );
        assert!(not_monitored(repo, "pnpm dev", false).contains("the person said no"));
    }

    /// And neither of them leaves the door open that the agent walked through.
    #[test]
    fn the_refusal_closes_the_door_it_used_to_leave_open() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8Path::from_path(dir.path()).unwrap();
        for timed_out in [true, false] {
            let text = not_monitored(repo, "pnpm dev", timed_out);
            assert!(text.contains("Do not detach it"), "{text}");
            assert!(text.contains("nohup"), "name the spellings: {text}");
        }
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

        let text = not_monitored(&repo, &detected.command, false);
        assert!(text.contains("Designer tab"), "{text}");
        assert!(!not_monitored(&repo, "gh run watch 123", false).contains("Designer tab"));
    }

    /// The queue is a process-wide static, and the tests that touch it run in parallel.
    pub(crate) async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        LOCK.lock().await
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
    /// `.keel/permissions.json` — that scoping is the reason it is safe to offer at all. But the
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
        });
        q.lock().unwrap().push(Pending {
            id: "2".into(),
            lane: String::new(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: String::new(),
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
