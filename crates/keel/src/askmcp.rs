//! The one MCP tool Keel serves to the chat: `ask_user`.
//!
//! Headless Claude Code (`claude -p`) does not offer `AskUserQuestion` — there is nobody at a
//! terminal to answer — so an agent driven by Keel could never ask a question with options,
//! and guessed instead. This is a streamable-HTTP MCP server with a single tool that queues
//! the question into the same card the permission hook uses, blocks until the person answers,
//! and returns the chosen option as the tool's result. The card is unchanged: the input is
//! shaped exactly like `AskUserQuestion`'s.

use crate::approve::{Pending, WAIT, queue, waiters};
use crate::lock::Locked;
use crate::serve::AppState;
use axum::{Json, extract::Query, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const TOOL: &str = "ask_user";
pub const SERVER: &str = "keel";

/// The `--mcp-config` argument for one Claude turn: this daemon, this lane.
pub fn config(port: u16, lane: Option<&str>) -> String {
    json!({ "mcpServers": { SERVER: { "type": "http", "url": url(port, lane) } } }).to_string()
}

#[derive(Deserialize)]
pub struct McpQuery {
    #[serde(default)]
    pub lane: Option<String>,
}

/// The decision Claude's hook makes before a browser action, made here for an agent with no hook
/// in front of its calls. Measured on codex 0.163: `codex exec` called `browser_read` with no
/// approval at all, so without this a Codex lane would have clicked and typed in the Browser tab
/// unasked. It is the same path the hook takes (`approve::ask`), so the card, the trust and the
/// rules are the same ones — click, type and a key always ask — and as a caller nothing checks
/// after (`MCP_GATE`), a `defer` is never read as yes.
async fn gated(
    state: &Arc<AppState>,
    q: &McpQuery,
    name: &str,
    args: &Value,
) -> Result<(), String> {
    let lane = q.lane.clone().unwrap_or_default();
    let tool = format!("mcp__{SERVER}__{name}");
    // A Keel hook in front of the agent — Claude's, the SDK bridge's, a chat `agy`'s — already
    // decided this exact call; it is not asked twice.
    if crate::approve::take_hook_yes(&lane, &tool, args) {
        return Ok(());
    }
    let hook = crate::approve::HookInput {
        tool_name: tool,
        tool_input: args.clone(),
        tool_use_id: {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            format!(
                "mcp-{}",
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        },
        // The lane's conversation, when its turn is known, so "allow for this session" has one.
        session_id: state.lane_session(&lane).unwrap_or_default(),
        lane,
        cwd: String::new(),
        provider: crate::approve::MCP_GATE.into(),
    };
    match crate::approve::ask(State(state.clone()), Json(hook)).await {
        Ok(Json(d)) if d.decision == "allow" => Ok(()),
        Ok(Json(d)) => Err(d.reason),
        Err((_, why)) => Err(why),
    }
}

/// The address of this server for one lane. The lane is escaped: it lands in a URL, in a TOML
/// string on Codex's command line and in a JSON file, and a `&` or a `"` in it would add a
/// parameter or end the string. It carries nothing that waives a question: what a hook already
/// decided is known from the hook's own call (`approve::take_hook_yes`).
pub fn url(port: u16, lane: Option<&str>) -> String {
    match lane {
        Some(l) => format!("http://127.0.0.1:{port}/mcp?lane={}", escaped(l)),
        None => format!("http://127.0.0.1:{port}/mcp"),
    }
}

/// Percent-encoding of everything but the unreserved characters.
fn escaped(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Codex's built-in browser features, switched off for a Keel run.
const CODEX_BROWSERS: &[&str] = &[
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "in_app_browser",
    "browser_annotation_api",
    // Not browsers by name, but they drive the person's desktop — their signed-in Chrome with it.
    "computer_use",
    "in_app_local_automation",
];

/// How long `codex mcp list` may take. Measured at 10 ms; a Codex that hangs here must not
/// hold a turn.
const CODEX_LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The `-c` overrides that give a Codex run Keel's server and take the other browsers away, for
/// that run alone and without touching the person's config. Blocking: it asks Codex what it has.
///
/// Three things measured on 0.163 shape it. Codex refuses an MCP call it would have to approve
/// itself ("requires approval, but approval policy is never"), so Keel's server is `approve` —
/// Keel's own card is the question (`gated`). Its tool timeout is 60 s, under a card's four
/// minutes, so it is raised past them, or an answer would land in nothing. And `enabled=false`
/// for a server that is not configured fails the whole CLI, so only browsers Codex has are named.
/// A Codex that cannot list its own servers is not started: the browsers it has would be live.
pub fn codex_args(
    cwd: &camino::Utf8Path,
    port: u16,
    lane: Option<&str>,
) -> Result<Vec<String>, String> {
    Ok(codex_args_for(&codex_servers(cwd)?, port, lane))
}

/// [`codex_args`] for the servers Codex said it has.
fn codex_args_for(configured: &[String], port: u16, lane: Option<&str>) -> Vec<String> {
    let secs = tool_timeout().parse::<u64>().unwrap_or(300_000) / 1000;
    let mut args = vec![
        "-c".to_string(),
        format!("mcp_servers.{SERVER}.url=\"{}\"", url(port, lane)),
        "-c".to_string(),
        format!("mcp_servers.{SERVER}.default_tools_approval_mode=\"approve\""),
        "-c".to_string(),
        format!("mcp_servers.{SERVER}.tool_timeout_sec={secs}"),
    ];
    // Codex's own browsers too: `browser_use_external` and `browser_use_full_cdp_access` drive a
    // Chrome over CDP — the person's, signed in — which is what a Keel lane is never given.
    // Listed by `codex features list` on 0.163, and switching them off keeps the CLI starting.
    for feature in CODEX_BROWSERS {
        args.extend(["-c".to_string(), format!("features.{feature}=false")]);
    }
    // The same rule Claude's hook applies, not a list: any server whose name says it drives a
    // browser, however it is spelled.
    for name in configured {
        if name != SERVER && crate::approve::drives_another_browser(&format!("mcp__{name}__tool")) {
            args.extend([
                "-c".to_string(),
                format!("mcp_servers.{name}.enabled=false"),
            ]);
        }
    }
    args
}

/// The MCP servers Codex has configured for `cwd`, by name: its own `codex mcp list --json`.
fn codex_servers(cwd: &camino::Utf8Path) -> Result<Vec<String>, String> {
    const CANNOT: &str = "Keel could not ask Codex which MCP servers it has, so it cannot take the \
                          other browsers away from this turn; the turn did not start";
    let mut command = std::process::Command::new(crate::permissions::program("codex"));
    command
        .args(["mcp", "list", "--json"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let child = command.spawn().map_err(|e| format!("{CANNOT} ({e})"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = match rx.recv_timeout(CODEX_LIST_WAIT) {
        Ok(Ok(out)) if out.status.success() => out,
        _ => return Err(CANNOT.into()),
    };
    let listed: Vec<Value> = serde_json::from_slice(&out.stdout).map_err(|_| CANNOT.to_string())?;
    Ok(listed
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect())
}

/// A folder of Keel's own, per lane, holding `.agents/mcp_config.json` with this
/// server: `agy` loads MCP servers from a folder passed with `--add-dir` (measured on 1.4.4), and
/// the lane is in the address, so one folder cannot serve two lanes. Named by a hash of the lane,
/// so no two lanes can share one — stripping odd characters made `a.b` and `ab` the same folder,
/// and one lane's clicks would have landed in the other's tab. Never the repository's `.agents/`.
pub fn agy_dir(port: u16, lane: Option<&str>) -> Result<camino::Utf8PathBuf, String> {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(lane.unwrap_or("").as_bytes());
    let name: String = hash.iter().take(12).map(|b| format!("{b:02x}")).collect();
    let dir = crate::permissions::permissions_dir()?
        .with_file_name("antigravity-mcp")
        .join(name);
    let file = dir.join(".agents").join("mcp_config.json");
    let body = serde_json::to_vec_pretty(&json!({
        "mcpServers": { SERVER: { "serverUrl": url(port, lane) } }
    }))
    .map_err(|e| e.to_string())?;
    for p in [dir.clone(), dir.join(".agents"), file.clone()] {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{p} is a link, so Keel will not write through it"));
        }
    }
    if std::fs::read(&file).ok().as_deref() != Some(body.as_slice()) {
        crate::writes::replace(file.as_std_path(), &body)?;
    }
    Ok(dir)
}

fn tool_schema() -> Value {
    json!({
        "name": TOOL,
        "description": "Ask the person a question with options and wait for their answer. Use it when two readings of the request would lead to materially different work, or when a decision is theirs to make (a name, a provider, a trade-off). One call may carry several questions. The answer comes back as text: the chosen option label(s), or free text if they typed their own.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The question, ending in a question mark." },
                            "header": { "type": "string", "description": "A short label, at most 12 characters." },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": { "type": "object", "properties": { "label": { "type": "string" }, "description": { "type": "string" } }, "required": ["label"] }
                            },
                            "multiSelect": { "type": "boolean" }
                        },
                        "required": ["question", "options"]
                    }
                }
            },
            "required": ["questions"]
        }
    })
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}
fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `MCP_TOOL_TIMEOUT` for every `claude` Keel starts: past `WAIT`, so the person has the whole
/// four minutes a question card promises. Claude Code abandoned `ask_user` at 60 s otherwise. A
/// longer value the person set themselves is kept.
pub fn tool_timeout() -> String {
    let ours = (WAIT.as_millis() + 60_000) as u64;
    let theirs = std::env::var("MCP_TOOL_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    ours.max(theirs).to_string()
}

/// Queue the questions for the lane and wait for the answer. Public so a test can drive it
/// without HTTP.
pub async fn ask(lane: Option<&str>, args: &Value) -> Result<String, String> {
    // no-blocking: an in-memory queue and a oneshot the person answers; nothing blocks.
    let questions = args
        .get("questions")
        .and_then(|q| q.as_array())
        .ok_or("questions required")?;
    if questions.is_empty() {
        return Err("at least one question".into());
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "ask-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let pending = Pending {
        id: id.clone(),
        lane: lane.unwrap_or_default().to_string(),
        tool: "AskUserQuestion".into(),
        command: String::new(),
        rules: Vec::new(),
        input: json!({ "questions": questions }),
        session_id: lane.map(|l| format!("lane:{l}")).unwrap_or_default(),
        reason: String::new(),
        subagent: false,
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters()
        .locked()
        .insert(id.clone(), (tx, lane.unwrap_or_default().to_string()));
    queue().locked().push(pending);
    crate::events::emit(
        "pending",
        None,
        serde_json::json!({ "lane": lane.unwrap_or_default() }),
    );
    // Claude Code gives up on a tool call before `WAIT` does — measured, 60.1 s, twice in one
    // session — and its request is dropped here mid-wait. The card stayed on screen for the rest
    // of the four minutes and took the person's answer into nothing: they answered three times
    // and the agent was told nobody had. So the card goes when the asker does.
    let _withdrawn = crate::approve::Withdraw(id.clone());
    match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(decision)) => Ok(decision
            .reason
            .split_once("do not ask again.\n\n")
            .map(|(_, a)| a.to_string())
            .unwrap_or(decision.reason)),
        // `_withdrawn` takes the card down and tells the window.
        _ => Err("nobody answered within the time allowed; proceed with your best judgement and say what you assumed".into()),
    }
}

/// Streamable HTTP: one POST per JSON-RPC message, JSON back. Notifications get 202.
pub async fn post(
    State(state): State<Arc<AppState>>,
    Query(q): Query<McpQuery>,
    Json(msg): Json<Value>,
) -> (
    axum::http::StatusCode,
    [(&'static str, &'static str); 1],
    Json<Value>,
) {
    // no-blocking: JSON-RPC over the queue in memory; nothing blocks.
    let hdr = [("Mcp-Session-Id", "keel")];
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if method.starts_with("notifications/") {
        return (axum::http::StatusCode::ACCEPTED, hdr, Json(Value::Null));
    }
    let out = match method {
        "initialize" => rpc_ok(
            id,
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": SERVER, "version": env!("CARGO_PKG_VERSION") },
                "instructions": "ask_user shows the person a question with options in Keel and returns their answer. There is no AskUserQuestion here; this is it."
            }),
        ),
        "ping" => rpc_ok(id, json!({})),
        "tools/list" => {
            let mut tools = vec![tool_schema()];
            tools.extend(crate::browser::schemas());
            rpc_ok(id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(json!({}));
            let answered = if crate::browser::is_tool(name) {
                Some(match gated(&state, &q, name, &args).await {
                    Ok(()) => crate::browser::call(q.lane.as_deref(), name, &args).await,
                    Err(why) => Err(why),
                })
            } else if name == TOOL {
                Some(ask(q.lane.as_deref(), &args).await)
            } else {
                None
            };
            if let Some(answered) = answered {
                match answered {
                    Ok(answer) => rpc_ok(
                        id,
                        json!({ "content": [{ "type": "text", "text": answer }], "isError": false }),
                    ),
                    Err(e) => rpc_ok(
                        id,
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                    ),
                }
            } else {
                rpc_err(id, -32602, "unknown tool")
            }
        }
        _ => rpc_err(id, -32601, "method not found"),
    };
    (axum::http::StatusCode::OK, hdr, Json(out))
}

pub async fn get() -> axum::http::StatusCode {
    // no-blocking: returns a status and touches nothing.
    axum::http::StatusCode::METHOD_NOT_ALLOWED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_question_is_queued_for_the_lane_and_the_answer_comes_back_as_text() {
        let _guard = crate::approve::tests::lock().await;
        let args = json!({ "questions": [{ "question": "Red or blue?", "header": "Colour", "options": [{ "label": "Red" }, { "label": "Blue" }] }] });
        let asking = tokio::spawn(async move { ask(Some("lane-1"), &args).await });
        // The UI polls, sees it under its lane, answers.
        let mut found = None;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let q = queue().lock().unwrap();
            if let Some(p) = q.iter().find(|p| p.session_id == "lane:lane-1") {
                found = Some(p.id.clone());
                break;
            }
        }
        let id = found.expect("queued");
        assert!(
            queue()
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.id == id && p.tool == "AskUserQuestion")
        );
        let tx = waiters().lock().unwrap().remove(&id).unwrap();
        queue().lock().unwrap().retain(|p| p.id != id);
        tx.0.send(crate::approve::answered("Blue")).unwrap();
        assert_eq!(asking.await.unwrap().unwrap(), "Blue");
        assert!(config(7777, Some("lane-1")).contains("/mcp?lane=lane-1"));
    }

    /// Claude Code abandoned the call at 60 s; the card stayed and took three answers into
    /// nothing. When the asker goes, the question goes with it.
    #[tokio::test]
    async fn a_question_whose_asker_gave_up_leaves_the_screen() {
        let _guard = crate::approve::tests::lock().await;
        let args = json!({ "questions": [{ "question": "Ship it?", "header": "Ship", "options": [{ "label": "Yes" }, { "label": "No" }] }] });
        let asking = tokio::spawn(async move { ask(Some("lane-gone"), &args).await });
        let mut queued = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if queue()
                .locked()
                .iter()
                .any(|p| p.session_id == "lane:lane-gone")
            {
                queued = true;
                break;
            }
        }
        assert!(queued, "queued");
        // The request is dropped mid-wait, as axum drops it when Claude Code hangs up.
        asking.abort();
        let _ = asking.await;
        assert!(
            !queue()
                .locked()
                .iter()
                .any(|p| p.session_id == "lane:lane-gone"),
            "the card outlived the call that asked it"
        );
        assert!(
            waiters().locked().is_empty()
                || !waiters().locked().values().any(|(_, l)| l == "lane-gone")
        );
    }
}

#[cfg(test)]
mod browser_gate_tests {
    use super::*;

    /// Codex gets Keel's server — Codex's own approval off for it, its timeout past a card's wait —
    /// and loses every browser it has by the rule Claude's hook uses, and nothing else.
    #[test]
    fn a_codex_run_gets_keels_browser_and_loses_the_ones_it_has() {
        let configured = [
            "oya-browser".to_string(),
            "chrome-devtools".to_string(),
            "github".to_string(),
        ];
        let args = codex_args_for(&configured, 7777, Some("lane-1"));
        assert_eq!(
            args[..2],
            [
                "-c",
                "mcp_servers.keel.url=\"http://127.0.0.1:7777/mcp?lane=lane-1\""
            ]
        );
        assert!(
            args.contains(&"mcp_servers.keel.default_tools_approval_mode=\"approve\"".to_string())
        );
        let secs: u64 = args
            .iter()
            .find_map(|a| a.strip_prefix("mcp_servers.keel.tool_timeout_sec="))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            secs > crate::approve::WAIT.as_secs() + 45,
            "past the card and a page load"
        );
        assert!(args.contains(&"mcp_servers.oya-browser.enabled=false".to_string()));
        assert!(args.contains(&"mcp_servers.chrome-devtools.enabled=false".to_string()));
        assert!(
            !args.iter().any(|a| a.contains("github")),
            "a server that is not a browser stays"
        );
        assert!(
            args.contains(&"features.browser_use_external=false".to_string()),
            "nor Codex's own"
        );
        assert!(args.contains(&"features.computer_use=false".to_string()));
    }

    /// The address names the lane and nothing that waives a question; a lane id cannot add a
    /// parameter.
    #[test]
    fn an_address_carries_the_lane_and_nothing_that_waives_a_question() {
        assert_eq!(url(1, Some("a")), "http://127.0.0.1:1/mcp?lane=a");
        assert!(!config(1, Some("a")).contains("hooked"));
        assert_eq!(
            url(1, Some("x&hooked=1\"")),
            "http://127.0.0.1:1/mcp?lane=x%26hooked%3D1%22"
        );
    }

    /// agy's folder is Keel's own, per lane, and no two lanes can share one.
    #[test]
    fn an_agy_lane_gets_its_own_folder_naming_keels_server() {
        let one = agy_dir(7777, Some("lane-t")).unwrap();
        let read = std::fs::read_to_string(one.join(".agents/mcp_config.json")).unwrap();
        assert!(read.contains("http://127.0.0.1:7777/mcp?lane=lane-t\""));
        assert_ne!(
            agy_dir(1, Some("a.b")).unwrap(),
            agy_dir(1, Some("ab")).unwrap()
        );
        assert!(!agy_dir(1, Some("../../x")).unwrap().as_str().contains(".."));
    }

    /// Every Browser tab call is decided once: by the hook in front of the agent when there is one
    /// — its yes to that exact call lets it through — and by Claude's rules here when there is
    /// not: a click always asks, reading follows trust. Anything claiming a yes it was not given
    /// is asked.
    #[tokio::test]
    async fn a_browser_action_is_asked_once_by_whoever_is_in_front() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let q = McpQuery {
            lane: Some("gate-test".into()),
        };
        let asked = |name: &'static str, args: Value| {
            let (state, q) = (
                state.clone(),
                McpQuery {
                    lane: q.lane.clone(),
                },
            );
            async move {
                let call = tokio::spawn(async move { gated(&state, &q, name, &args).await });
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                let finished = call.is_finished();
                call.abort();
                let card = crate::approve::queue()
                    .locked()
                    .iter()
                    .any(|p| p.lane == "gate-test" && p.tool == format!("mcp__keel__{name}"));
                crate::approve::queue()
                    .locked()
                    .retain(|p| p.lane != "gate-test");
                (finished, card)
            }
        };
        assert_eq!(
            asked("browser_read", json!({})).await,
            (false, true),
            "untrusted: reading asks"
        );
        crate::permissions::set_trusted(&root, true).unwrap();
        assert_eq!(
            asked("browser_read", json!({})).await,
            (true, false),
            "trusted: reading needs no card"
        );
        assert_eq!(
            asked("browser_click", json!({"element": 1})).await,
            (false, true),
            "a click always asks"
        );

        // Claude's hook in front answered this exact click (its own rules decide after Keel): it
        // passes, once; another click, or the same one again, is asked.
        let hook = crate::approve::HookInput {
            tool_name: "mcp__keel__browser_click".into(),
            tool_input: json!({"element": 2}),
            tool_use_id: "t1".into(),
            session_id: String::new(),
            lane: "gate-test".into(),
            cwd: String::new(),
            provider: String::new(),
        };
        let said = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::approve::ask(State(state.clone()), Json(hook)),
        )
        .await
        .expect("a trusted project's hook answers at once")
        .unwrap();
        assert_eq!(
            said.0.decision, "defer",
            "Claude Code's own rules decide a click after Keel"
        );
        assert_eq!(
            asked("browser_click", json!({"element": 2})).await,
            (true, false),
            "the hook answered"
        );
        assert_eq!(
            asked("browser_click", json!({"element": 2})).await,
            (false, true),
            "spent: asked again"
        );
        assert_eq!(
            asked("browser_click", json!({"element": 3})).await,
            (false, true),
            "another click is not that one"
        );
    }
}
