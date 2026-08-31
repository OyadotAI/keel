//! The tools Keel serves to the chat: `ask_user`, and `submit_plan` in plan mode.
//!
//! Headless Claude Code (`claude -p`) offers neither `AskUserQuestion` nor `ExitPlanMode` — there
//! is nobody at a terminal to answer either one — so an agent driven by Keel could not ask a
//! question with options, and could not put a finished plan in front of anybody. It guessed at the
//! first and wrote the second into its reply or into a file. This is a streamable-HTTP MCP server
//! whose tools queue into the same card the permission hook uses, block until the person answers,
//! and return the answer as the tool's result. The question card is unchanged: its input is shaped
//! exactly like `AskUserQuestion`'s.

use crate::approve::{Pending, WAIT, queue, waiters};
use crate::lock::Locked;
use crate::serve::AppState;
use axum::{Json, extract::Query, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const TOOL: &str = "ask_user";
pub const PLAN: &str = "submit_plan";
pub const SERVER: &str = "keel";

/// What the Approve button sends back as the answer to a plan.
///
/// A sentinel rather than prose, so every word the agent reads is written here beside the rest of
/// what Keel tells it, and not in a Swift view.
pub const APPROVED: &str = "keel:plan-approved";

/// The `--mcp-config` argument for one turn: this daemon, this lane, this mode.
pub fn config(port: u16, lane: Option<&str>, mode: Option<&str>) -> String {
    let mut query = Vec::new();
    if let Some(lane) = lane {
        query.push(format!("lane={lane}"));
    }
    if let Some(mode) = mode {
        query.push(format!("mode={mode}"));
    }
    let url = match query.is_empty() {
        true => format!("http://127.0.0.1:{port}/mcp"),
        false => format!("http://127.0.0.1:{port}/mcp?{}", query.join("&")),
    };
    json!({ "mcpServers": { SERVER: { "type": "http", "url": url } } }).to_string()
}

#[derive(Deserialize)]
pub struct McpQuery {
    #[serde(default)]
    pub lane: Option<String>,
    /// The turn's permission mode. `submit_plan` is offered only in `plan`.
    #[serde(default)]
    pub mode: Option<String>,
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

/// Presenting a finished plan and waiting on the person.
///
/// `ExitPlanMode` does not exist in headless `claude -p` — verified against 2.1.251, whose tool
/// list in `--permission-mode plan` has no plan tool at all. So a planning turn had nowhere to put
/// its plan except its own reply, or a file: reported verbatim from a real turn, *"ExitPlanMode
/// isn't available in this session, so here's the plan for approval — it's written to
/// ~/.claude/plans/…"*. The plan was the turn's whole output and the one thing the person had to
/// decide on, and it arrived as prose with nothing to click.
fn plan_schema() -> Value {
    json!({
        "name": PLAN,
        "description": "Present your finished plan to the person and wait for their decision. This session has no ExitPlanMode; this is it. Call it when the plan is ready, instead of writing the plan to a file or leaving it in your reply. Approving starts the build as its own turn, so do not implement anything yourself.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "plan": { "type": "string", "description": "The plan, as Markdown: what changes, in which files, and how it is verified." }
            },
            "required": ["plan"]
        }
    })
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}
fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Queue one card for the lane and block until the person answers it.
///
/// Shared by both tools because both are the same event: the turn has stopped and is waiting on
/// somebody. What differs is the card the UI draws and what the answer means, not the mechanism.
async fn wait(
    lane: Option<&str>,
    tool: &str,
    input: Value,
    gave_up: &str,
) -> Result<String, String> {
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
        tool: tool.into(),
        command: String::new(),
        rules: Vec::new(),
        input,
        session_id: lane.map(|l| format!("lane:{l}")).unwrap_or_default(),
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters().locked().insert(id.clone(), tx);
    queue().locked().push(pending);
    match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(decision)) => Ok(decision
            .reason
            .split_once("do not ask again.\n\n")
            .map(|(_, a)| a.to_string())
            .unwrap_or(decision.reason)),
        _ => {
            waiters().locked().remove(&id);
            queue().locked().retain(|p| p.id != id);
            Err(gave_up.into())
        }
    }
}

/// Queue the questions for the lane and wait for the answer. Public so a test can drive it
/// without HTTP.
pub async fn ask(lane: Option<&str>, args: &Value) -> Result<String, String> {
    let questions = args
        .get("questions")
        .and_then(|q| q.as_array())
        .ok_or("questions required")?;
    if questions.is_empty() {
        return Err("at least one question".into());
    }
    wait(
        lane,
        "AskUserQuestion",
        json!({ "questions": questions }),
        "nobody answered within the time allowed; proceed with your best judgement and say what you assumed",
    )
    .await
}

/// Show the plan and wait for the decision.
///
/// Approval cannot be "carry on": this turn is running under `--permission-mode plan` and cannot
/// write a file whatever it is told. So the agent is stopped here and Keel starts the build as a
/// second turn in `acceptEdits`, resumed on the same conversation — which is where the plan is.
pub async fn plan(lane: Option<&str>, args: &Value) -> Result<String, String> {
    let text = args
        .get("plan")
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("plan required")?;
    let answer = wait(
        lane,
        "ExitPlanMode",
        json!({ "plan": text }),
        "nobody answered within the time allowed; end your turn with the plan in your reply so it is not lost",
    )
    .await?;
    Ok(if answer.trim() == APPROVED {
        "The person approved the plan. Keel is starting the build itself, as a new turn in edit \
         mode on this same conversation, so do not implement anything now and do not call this \
         tool again. End your turn with one line saying the plan is approved."
            .into()
    } else {
        format!(
            "The person has not approved the plan. Revise it and call `{PLAN}` again; do not \
             start building.\n\nWhat they said:\n{answer}"
        )
    })
}

/// Streamable HTTP: one POST per JSON-RPC message, JSON back. Notifications get 202.
pub async fn post(
    State(_state): State<Arc<AppState>>,
    Query(q): Query<McpQuery>,
    Json(msg): Json<Value>,
) -> (
    axum::http::StatusCode,
    [(&'static str, &'static str); 1],
    Json<Value>,
) {
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
                "instructions": "ask_user shows the person a question with options in Keel and returns their answer; there is no AskUserQuestion here, this is it. submit_plan, in plan mode, shows them the plan with an Approve button; there is no ExitPlanMode here either."
            }),
        ),
        "ping" => rpc_ok(id, json!({})),
        "tools/list" => {
            let mut tools = vec![tool_schema()];
            // Offered only in the mode it belongs to. In `acceptEdits` a plan tool invites a turn
            // to stop and ask approval for a plan nobody asked for, and the person would have no
            // way to tell that from the turn having stalled.
            if q.mode.as_deref() == Some("plan") {
                tools.push(plan_schema());
            }
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
            let called = if name == TOOL {
                ask(q.lane.as_deref(), &args).await
            } else if name == PLAN {
                plan(q.lane.as_deref(), &args).await
            } else {
                return (
                    axum::http::StatusCode::OK,
                    hdr,
                    Json(rpc_err(id, -32602, "unknown tool")),
                );
            };
            match called {
                Ok(answer) => rpc_ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": answer }], "isError": false }),
                ),
                Err(e) => rpc_ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                ),
            }
        }
        _ => rpc_err(id, -32601, "method not found"),
    };
    (axum::http::StatusCode::OK, hdr, Json(out))
}

pub async fn get() -> axum::http::StatusCode {
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
        tx.send(crate::approve::answered("Blue")).unwrap();
        assert_eq!(asking.await.unwrap().unwrap(), "Blue");
        assert!(config(7777, Some("lane-1"), None).contains("/mcp?lane=lane-1"));
        assert!(config(7777, Some("lane-1"), Some("plan")).contains("lane=lane-1&mode=plan"));
    }

    /// A plan is queued as `ExitPlanMode`, and the two answers say different things.
    ///
    /// Approval is the one that matters: the turn holding the plan runs under
    /// `--permission-mode plan` and cannot write whatever it is told, so the agent must be told to
    /// stop rather than to carry on. An "approved, go ahead" here reads as success and produces
    /// nothing.
    #[tokio::test]
    async fn a_plan_is_queued_and_approving_it_stops_the_planning_turn() {
        let _guard = crate::approve::tests::lock().await;
        for (answer, expected) in [
            (APPROVED.to_string(), "do not implement anything now"),
            ("Use the other crate".into(), "has not approved"),
        ] {
            let args = json!({ "plan": "1. Do the thing\n2. Check it" });
            let asking = tokio::spawn(async move { plan(Some("lane-2"), &args).await });
            let mut found = None;
            for _ in 0..50 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if let Some(p) = queue().locked().iter().find(|p| p.tool == "ExitPlanMode") {
                    assert_eq!(p.input["plan"], "1. Do the thing\n2. Check it");
                    found = Some(p.id.clone());
                    break;
                }
            }
            let id = found.expect("queued");
            let tx = waiters().locked().remove(&id).unwrap();
            queue().locked().retain(|p| p.id != id);
            tx.send(crate::approve::answered(&answer)).unwrap();
            let result = asking.await.unwrap().unwrap();
            assert!(result.contains(expected), "{result}");
        }
    }

    /// The plan tool exists only in the mode that can use it.
    #[test]
    fn submit_plan_is_offered_in_plan_mode_alone() {
        assert_eq!(plan_schema()["name"], PLAN);
    }
}
