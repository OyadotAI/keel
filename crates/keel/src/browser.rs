//! The lane's browser tab: tools the agent calls, carried to the window that draws the tab.
//!
//! When an agent in a Keel lane needs a website, it used to reach for the person's own browser —
//! Oya's MCP drove their active window, took the tab they were reading, opened its own in front
//! of them and closed them again when the run ended. So Keel has a browser of its own, a webview
//! in the lane's workspace, and the agent drives it with `browser_*` tools served beside
//! `ask_user` on Keel's MCP server. The person's own browser servers are withheld from Keel's
//! turns (`permissions::settings_json`).
//!
//! The webview lives in the app; the tool call arrives here. A call becomes an op on the event
//! bus, the window that has the lane runs it against the lane's webview, and posts the result back
//! to `/api/browser/result`. Every wait has an end: no window at all fails at once, an op nobody
//! answers fails at its timeout, and a call Claude Code abandons takes its waiter with it.
//!
//! What it cannot do, said to the agent in the tool descriptions rather than discovered: no
//! screenshots (neither WKWebView through Tauri nor WebView2 offers a snapshot on desktop), the top
//! frame only, synthetic input only, and none of the person's logins.

use crate::lock::Locked;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// What the window gets back to answer. Opening waits for a page load; everything else is a
/// script against the page that is already there.
const OPEN_WAIT: Duration = Duration::from_secs(45);
const OTHER_WAIT: Duration = Duration::from_secs(20);
/// The most text a page result carries to the agent, head and tail kept.
const RESULT_CAP: usize = 64 * 1024;
/// The most a `browser_type` writes into a field.
const TEXT_CAP: usize = 10 * 1024;

/// One thing to do in the lane's tab. The window's `browser.ts` reads exactly these shapes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Open {
        url: String,
    },
    Read,
    Click {
        target: u32,
    },
    Type {
        target: u32,
        text: String,
        submit: bool,
    },
    Key {
        key: String,
    },
    Scroll {
        direction: String,
    },
    Back,
}

/// Where the tab may go: the web. Never a file, a script URL, or Keel's own page — the same rule
/// the app enforces on every navigation the page makes by itself.
pub fn web_only(url: &str) -> Result<String, String> {
    let url = url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("Only http and https addresses open in the Browser tab.")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err("Only http and https addresses open in the Browser tab.".into());
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err(format!("{url} has no host."));
    }
    if matches!(
        host.as_str(),
        "tauri.localhost" | "ipc.localhost" | "asset.localhost"
    ) {
        return Err("That address is Keel's own page, not a website.".into());
    }
    Ok(url.to_string())
}

/// The op a tool call asks for, or why its arguments are not one.
pub fn op(name: &str, args: &Value) -> Result<Op, String> {
    let target = || {
        args["element"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("`element` is the number `browser_read` gave the element.".to_string())
    };
    Ok(match name {
        "browser_open" => Op::Open {
            url: web_only(args["url"].as_str().unwrap_or(""))?,
        },
        "browser_read" => Op::Read,
        "browser_click" => Op::Click { target: target()? },
        "browser_type" => {
            let text = args["text"].as_str().unwrap_or("");
            if text.len() > TEXT_CAP {
                return Err(format!("`text` is at most {TEXT_CAP} bytes."));
            }
            Op::Type {
                target: target()?,
                text: text.to_string(),
                submit: args["submit"].as_bool().unwrap_or(false),
            }
        }
        "browser_key" => {
            let key = args["key"].as_str().unwrap_or("").trim();
            if !matches!(
                key,
                "Enter"
                    | "Escape"
                    | "Tab"
                    | "ArrowUp"
                    | "ArrowDown"
                    | "ArrowLeft"
                    | "ArrowRight"
                    | "Backspace"
                    | "Space"
            ) {
                return Err(
                    "`key` is one of Enter, Escape, Tab, Space, Backspace and the arrows.".into(),
                );
            }
            Op::Key {
                key: key.to_string(),
            }
        }
        "browser_scroll" => match args["direction"].as_str().unwrap_or("down") {
            d @ ("up" | "down") => Op::Scroll {
                direction: d.to_string(),
            },
            _ => return Err("`direction` is up or down.".into()),
        },
        "browser_back" => Op::Back,
        _ => return Err(format!("unknown tool {name}")),
    })
}

pub fn is_tool(name: &str) -> bool {
    matches!(
        name,
        "browser_open"
            | "browser_read"
            | "browser_click"
            | "browser_type"
            | "browser_key"
            | "browser_scroll"
            | "browser_back"
    )
}

type Answer = Result<String, String>;

fn waiters() -> &'static Mutex<HashMap<String, tokio::sync::oneshot::Sender<Answer>>> {
    static W: OnceLock<Mutex<HashMap<String, tokio::sync::oneshot::Sender<Answer>>>> =
        OnceLock::new();
    W.get_or_init(Default::default)
}

/// A waiter that goes with its call: answered, timed out, or abandoned by Claude Code — axum drops
/// the future, and a late result then finds nothing and is said to be late, not a failure.
struct Waiting(String);

impl Drop for Waiting {
    fn drop(&mut self) {
        waiters().locked().remove(&self.0);
    }
}

/// With no window there is no tab, and waiting for one would be waiting for nobody.
fn some_window(open: usize) -> Result<(), String> {
    if open == 0 {
        return Err(
            "No Keel window is open, so there is no Browser tab to drive. Say so and continue without it."
                .into(),
        );
    }
    Ok(())
}

/// Run one tool call in the lane's tab and return what the agent reads.
pub async fn call(lane: Option<&str>, name: &str, args: &Value) -> Answer {
    // no-blocking: an event out and a oneshot back; the work is the window's.
    let lane = lane
        .filter(|l| !l.is_empty())
        .ok_or("The Browser tab belongs to a lane, and this call came from none.")?;
    let op = op(name, args)?;
    some_window(crate::events::windows())?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "b-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters().locked().insert(id.clone(), tx);
    let _waiting = Waiting(id.clone());
    let wait = if matches!(op, Op::Open { .. }) {
        OPEN_WAIT
    } else {
        OTHER_WAIT
    };
    crate::events::emit("browser", None, json!({ "id": id, "lane": lane, "op": op }));
    match tokio::time::timeout(wait, rx).await {
        Ok(Ok(answer)) => answer,
        _ => Err(format!(
            "The Browser tab did not answer within {} s. The page may still be loading; read it again, or say what you could not do.",
            wait.as_secs()
        )),
    }
}

/// What a window posts back: the op's text, or why it could not be done.
#[derive(Deserialize)]
pub struct Result_ {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub text: String,
}

/// Answer a waiting call. `false` for an id nobody is waiting on — a result that arrived after its
/// call gave up — which is ordinary, so it is a 200 and not a failure report.
pub async fn api_result(Json(r): Json<Result_>) -> Json<bool> {
    // no-blocking: hands a string to a oneshot in memory.
    let Some(tx) = waiters().locked().remove(&r.id) else {
        return Json(false);
    };
    let text = capped(r.text);
    Json(tx.send(if r.ok { Ok(text) } else { Err(text) }).is_ok())
}

/// At most `RESULT_CAP` bytes, the head and tail kept, on character boundaries.
fn capped(text: String) -> String {
    if text.len() <= RESULT_CAP {
        return text;
    }
    let half = RESULT_CAP / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n… {} bytes cut …\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

/// The tools, as `tools/list` offers them. Every one takes a `description`, which is what the
/// turn row shows for the call.
pub fn schemas() -> Vec<Value> {
    let description = json!({ "type": "string", "description": "What this step is for, in a few words. Shown to the person beside the call." });
    let element = json!({ "type": "integer", "description": "The element's number from the last browser_read." });
    let tool = |name: &str,
                text: &str,
                mut props: serde_json::Map<String, Value>,
                required: &[&str],
                read_only: bool| {
        props.insert("description".into(), description.clone());
        let mut req: Vec<&str> = vec!["description"];
        req.extend_from_slice(required);
        json!({
            "name": name,
            "description": text,
            "inputSchema": { "type": "object", "properties": props, "required": req },
            "annotations": { "readOnlyHint": read_only },
        })
    };
    let props = |pairs: &[(&str, Value)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<serde_json::Map<_, _>>()
    };
    vec![
        tool(
            "browser_open",
            "Open an http or https address in this lane's Browser tab inside Keel, which the person can see. Use it for any website; it is the only browser in this lane. It has none of the person's logins: if a page needs a sign-in, ask the person with ask_user to sign in in the Browser tab. Returns when the page has loaded.",
            props(&[("url", json!({ "type": "string" }))]),
            &["url"],
            false,
        ),
        tool(
            "browser_read",
            "Read the page in this lane's Browser tab: its title, address, visible text, and its interactive elements numbered as [n]. Text only: the tab cannot take screenshots, and only the top frame is read (not cross-origin iframes such as embedded payment or sign-in widgets).",
            serde_json::Map::new(),
            &[],
            true,
        ),
        tool(
            "browser_click",
            "Click element [n] from the last browser_read. Read again afterwards to see what changed.",
            props(&[("element", element.clone())]),
            &["element"],
            false,
        ),
        tool(
            "browser_type",
            "Type text into field [n] from the last browser_read, replacing what is there. `submit` presses Enter after, which submits a form. A `<select>` takes the option's text or value. The text is shown to the person with the call, like any tool's input: never type a secret they did not give you for this.",
            props(&[
                ("element", element.clone()),
                ("text", json!({ "type": "string" })),
                ("submit", json!({ "type": "boolean" })),
            ]),
            &["element", "text"],
            false,
        ),
        tool(
            "browser_key",
            "Press one key in the page: Enter, Escape, Tab, Space, Backspace, ArrowUp, ArrowDown, ArrowLeft or ArrowRight. Only the page's own key handlers see it, plus Enter in a text field (submits its form) and Enter or Space on a button: Tab does not move focus and Backspace does not delete, so click or type instead.",
            props(&[("key", json!({ "type": "string" }))]),
            &["key"],
            false,
        ),
        tool(
            "browser_scroll",
            "Scroll the page up or down by about a screen.",
            props(&[(
                "direction",
                json!({ "type": "string", "enum": ["up", "down"] }),
            )]),
            &[],
            false,
        ),
        tool(
            "browser_back",
            "Go back one page in the tab's history.",
            serde_json::Map::new(),
            &[],
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_addresses_are_opened() {
        for ok in [
            "http://localhost:3000/a",
            "https://example.com",
            "HTTPS://Example.com/x?y#z",
        ] {
            assert!(web_only(ok).is_ok(), "{ok}");
        }
        for no in [
            "file:///Users/me/.ssh/id_rsa",
            "javascript:alert(1)",
            "data:text/html,hi",
            "tauri://localhost/",
            "about:blank",
            "view-source:https://example.com",
            "http://tauri.localhost/",
            "https://user@ipc.localhost:443/x",
            "http:///nohost",
            "example.com",
        ] {
            assert!(web_only(no).is_err(), "{no}");
        }
    }

    #[test]
    fn a_call_s_arguments_are_checked_before_anything_is_sent() {
        assert_eq!(op("browser_read", &json!({})).unwrap(), Op::Read);
        assert!(op("browser_click", &json!({ "element": -1 })).is_err());
        assert!(op("browser_key", &json!({ "key": "rm -rf" })).is_err());
        assert!(
            op(
                "browser_type",
                &json!({ "element": 1, "text": "x".repeat(TEXT_CAP + 1) })
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(
                op(
                    "browser_type",
                    &json!({ "element": 4, "text": "hi", "submit": true })
                )
                .unwrap()
            )
            .unwrap(),
            json!({ "op": "type", "target": 4, "text": "hi", "submit": true })
        );
    }

    #[tokio::test]
    async fn no_window_fails_at_once_and_no_lane_is_refused() {
        assert!(call(None, "browser_read", &json!({})).await.is_err());
        assert!(some_window(0).unwrap_err().contains("No Keel window"));
        assert!(some_window(1).is_ok());
    }

    /// The window's answer comes back as the tool's result; a late one is not a failure.
    #[tokio::test]
    async fn a_call_reaches_the_window_and_its_answer_comes_back() {
        let _window = crate::events::tests_window();
        let mut bus = crate::events::subscribe();
        let asking = tokio::spawn(async { call(Some("lane-b"), "browser_read", &json!({})).await });
        let id = loop {
            let e = bus.recv().await.unwrap();
            if e.kind == "browser" && e.data["lane"] == "lane-b" {
                assert_eq!(e.data["op"], json!({ "op": "read" }));
                break e.data["id"].as_str().unwrap().to_string();
            }
        };
        let Json(taken) = api_result(Json(Result_ {
            id: id.clone(),
            ok: true,
            text: "Example Domain".into(),
        }))
        .await;
        assert!(taken);
        assert_eq!(asking.await.unwrap().unwrap(), "Example Domain");
        let Json(late) = api_result(Json(Result_ {
            id,
            ok: true,
            text: String::new(),
        }))
        .await;
        assert!(!late, "an answer to nobody is said to be late");
    }

    /// A call Claude Code abandons leaves no waiter behind.
    #[tokio::test]
    async fn a_dropped_call_leaves_no_waiter() {
        let _window = crate::events::tests_window();
        let mut bus = crate::events::subscribe();
        let asking = tokio::spawn(async { call(Some("lane-d"), "browser_read", &json!({})).await });
        let id = loop {
            let e = bus.recv().await.unwrap();
            if e.kind == "browser" && e.data["lane"] == "lane-d" {
                break e.data["id"].as_str().unwrap().to_string();
            }
        };
        assert!(waiters().locked().contains_key(&id));
        asking.abort();
        let _ = asking.await;
        assert!(
            !waiters().locked().contains_key(&id),
            "the waiter outlived its call"
        );
    }

    #[test]
    fn a_long_page_keeps_its_head_and_tail() {
        let text = format!("{}{}", "a".repeat(RESULT_CAP), "z".repeat(RESULT_CAP));
        let c = capped(text);
        assert!(c.len() < RESULT_CAP + 100);
        assert!(c.starts_with('a') && c.ends_with('z') && c.contains("bytes cut"));
    }
}
