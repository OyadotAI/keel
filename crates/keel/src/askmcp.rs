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

/// The `--mcp-config` argument for one turn: this daemon, this lane.
pub fn config(port: u16, lane: Option<&str>) -> String {
    let url = match lane {
        Some(l) => format!("http://127.0.0.1:{port}/mcp?lane={l}"),
        None => format!("http://127.0.0.1:{port}/mcp"),
    };
    json!({ "mcpServers": { SERVER: { "type": "http", "url": url } } }).to_string()
}

#[derive(Deserialize)]
pub struct McpQuery {
    #[serde(default)]
    pub lane: Option<String>,
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
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters()
        .lock()
        .expect("waiters lock")
        .insert(id.clone(), tx);
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
            Err("nobody answered within the time allowed; proceed with your best judgement and say what you assumed".into())
        }
    }
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
                "instructions": "ask_user shows the person a question with options in Keel and returns their answer. There is no AskUserQuestion here; this is it."
            }),
        ),
        "ping" => rpc_ok(id, json!({})),
        "tools/list" => rpc_ok(id, json!({ "tools": [tool_schema()] })),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(json!({}));
            if name != TOOL {
                rpc_err(id, -32602, "unknown tool")
            } else {
                match ask(q.lane.as_deref(), &args).await {
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
        assert!(config(7777, Some("lane-1")).contains("/mcp?lane=lane-1"));
    }
}
