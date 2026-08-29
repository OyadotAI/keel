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
}

#[derive(Serialize, Clone)]
pub struct Pending {
    pub id: String,
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
        if WRAPPERS.contains(&name.as_str()) {
            continue;
        }
        if BUILTINS.contains(&name.as_str()) {
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

    // A question is not a permission. Trust means "stop asking whether it may run things", and
    // an answer to "which of these two designs" is not covered by that — so it is never
    // short-circuited, on any project.
    let is_question = is_question(&hook.tool_name);

    // One decision, already made. Nothing is queued and nobody is asked.
    if !is_question && crate::permissions::trusted(&repo) {
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    if is_edit(&hook.tool_name) && edit_is_inside(&repo, &hook.tool_input) {
        return Ok(Json(Decision {
            decision: "defer".into(),
            reason: String::new(),
        }));
    }

    let rules = rules_for(&hook.tool_name, &hook.tool_input);
    let session = (!hook.session_id.is_empty()).then_some(hook.session_id.as_str());

    if !is_question && already_allowed(&rules, &crate::permissions::effective(&repo, session)) {
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
        tool: hook.tool_name.clone(),
        command: hook
            .tool_input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
        rules,
        input: hook.tool_input.clone(),
        session_id: hook.session_id.clone(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters()
        .lock()
        .expect("waiters lock")
        .insert(id.clone(), tx);
    queue().lock().expect("queue lock").push(pending);

    // A person who walked away must not leave the agent wedged. Timing out falls back to the
    // allowlist, which will refuse it — the same outcome as before, reached without hanging.
    match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(decision)) => Ok(Json(decision)),
        _ => {
            waiters().lock().expect("waiters lock").remove(&id);
            queue().lock().expect("queue lock").retain(|p| p.id != id);
            Ok(Json(Decision {
                decision: "defer".into(),
                reason: "nobody answered".into(),
            }))
        }
    }
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
    let mut queue = queue().lock().expect("queue lock");

    if q.all {
        return Json(std::mem::take(&mut *queue));
    }
    // A window on its first turn does not know its session yet, and used to take the whole
    // queue on the strength of that — including a question meant for a lane that could see it.
    // It takes only what belongs to nobody; the rest waits for the window that owns it.
    let session = q.session.filter(|s| !s.is_empty());
    let (mine, theirs): (Vec<Pending>, Vec<Pending>) = queue.drain(..).partition(|p| {
        p.session_id.is_empty() || session.as_deref() == Some(p.session_id.as_str())
    });
    *queue = theirs;
    Json(mine)
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

    match waiters().lock().expect("waiters lock").remove(&body.id) {
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
mod tests {
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

    #[tokio::test]
    async fn a_window_without_a_session_never_takes_another_windows_question() {
        use super::*;
        let q = queue();
        q.lock().unwrap().clear();
        q.lock().unwrap().push(Pending {
            id: "1".into(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: "s-other".into(),
        });
        q.lock().unwrap().push(Pending {
            id: "2".into(),
            tool: "Bash".into(),
            command: "ls".into(),
            rules: vec![],
            input: serde_json::json!({}),
            session_id: String::new(),
        });
        let got = poll(Query(PollQuery {
            session: None,
            all: false,
        }))
        .await;
        assert_eq!(
            got.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["2"]
        );
        let got = poll(Query(PollQuery {
            session: Some("s-other".into()),
            all: false,
        }))
        .await;
        assert_eq!(
            got.0.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["1"]
        );
        assert!(q.lock().unwrap().is_empty());
    }

    use super::*;

    fn pending(id: &str, session: &str) -> Pending {
        Pending {
            id: id.into(),
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
