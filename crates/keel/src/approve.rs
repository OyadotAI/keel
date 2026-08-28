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

use axum::{Json, extract::State, http::StatusCode};
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
}

#[derive(Deserialize)]
pub struct Answer {
    pub id: String,
    /// `allow` or `deny`.
    pub decision: String,
    /// Rules to remember, so the same command is not asked about twice.
    #[serde(default)]
    pub rules: Vec<String>,
    /// `project` or `session`.
    #[serde(default)]
    pub scope: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Decision {
    pub decision: String,
    pub reason: String,
}

type Waiters = Mutex<HashMap<String, tokio::sync::oneshot::Sender<Decision>>>;

fn waiters() -> &'static Waiters {
    static W: OnceLock<Waiters> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// Requests the UI has not yet been shown, so a page that reloads mid-question still sees it.
fn queue() -> &'static Mutex<Vec<Pending>> {
    static Q: OnceLock<Mutex<Vec<Pending>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

/// A command that stands in front of the real one and would launch it.
///
/// Never the rule itself: approving `sudo` approves everything it can start.
const WRAPPERS: &[&str] = &["sudo", "doas", "env", "nohup", "time", "xargs", "command", "exec"];

fn is_assignment(token: &str) -> bool {
    token.contains('=')
        && token
            .chars()
            .take_while(|c| *c != '=')
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        return Some(name);
    }
}

/// Every rule a call needs.
///
/// Claude Code approves a compound command part by part: `bun install && make check` needs both
/// `bun` and `make`. Deriving one rule from the first token leaves the second half refused and the
/// approval looking broken, which is a bug this project has already had once.
pub fn rules_for(tool: &str, input: &serde_json::Value) -> Vec<String> {
    if tool != "Bash" {
        return vec![tool.to_string()];
    }
    let command = input
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or_default();

    let mut out: Vec<String> = Vec::new();
    for segment in command.split(['\n', ';']).flat_map(|s| s.split("&&")) {
        for part in segment.split("||").flat_map(|s| s.split('|')) {
            let Some(program) = program_of(part) else {
                continue;
            };
            let rule = format!("Bash({program} *)");
            if !out.contains(&rule) {
                out.push(rule);
            }
        }
    }
    out
}

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

/// Ask the person, and wait for them.
pub async fn ask(
    State(state): State<Arc<AppState>>,
    Json(hook): Json<HookInput>,
) -> Result<Json<Decision>, (StatusCode, String)> {
    let rules = rules_for(&hook.tool_name, &hook.tool_input);

    if already_allowed(&rules, &crate::permissions::effective(&state.repo())) {
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

/// What the UI is waiting to show. Polled rather than pushed: the chat already holds an SSE stream
/// per turn, and a second long-lived connection for one message at a time is not worth its
/// reconnection logic.
pub async fn poll() -> Json<Vec<Pending>> {
    Json(std::mem::take(&mut *queue().lock().expect("queue lock")))
}

/// The person answered.
pub async fn answer(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Answer>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let allow = body.decision == "allow";

    if allow {
        // Remembered before the agent is released, so the retry that follows does not ask again.
        for rule in &body.rules {
            let _ = crate::permissions::remember(&state.repo(), rule, &body.scope);
        }
    }

    let decision = Decision {
        decision: if allow { "allow" } else { "deny" }.into(),
        reason: if allow {
            "Approved in Keel.".into()
        } else {
            "Not approved. Say what you needed and stop; do not substitute another command.".into()
        },
    };

    match waiters().lock().expect("waiters lock").remove(&body.id) {
        Some(tx) => {
            let _ = tx.send(decision);
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        // Already timed out, or answered twice. Not an error worth showing anyone.
        None => Ok(Json(serde_json::json!({ "ok": false, "reason": "no longer waiting" }))),
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
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;

    let request = format!(
        "POST /api/approve/ask HTTP/1.0\r\nHost: 127.0.0.1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(request.as_bytes()).await.ok()?;

    // The server holds this open while the person decides, so the read deadline has to exceed the
    // wait it enforces — otherwise the answer arrives after this side has already given up.
    let mut raw = String::new();
    tokio::time::timeout(WAIT + std::time::Duration::from_secs(20), sock.read_to_string(&mut raw))
        .await
        .ok()?
        .ok()?;

    let payload = raw.split("\r\n\r\n").nth(1)?;
    serde_json::from_str::<Decision>(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            &rules_for("Bash", &serde_json::json!({ "command": "bun install && make check" })),
            &allowed
        ));
        // One half missing is still a question.
        assert!(!already_allowed(
            &rules_for("Bash", &serde_json::json!({ "command": "bun install && docker ps" })),
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
}
