//! The lane's Browser tab: a child webview the agent drives, one per lane.
//!
//! Not the preview's webview. The preview shows this machine's dev server and carries the picker
//! with its one command; this one loads any website, so it carries no picker and is granted no
//! command at all — no capability names a `browser-*` label, and `default.json` is scoped to the
//! main page's webview, so nothing it loads can reach the app. What it may do is bounded here:
//! the web only (`preview::web_only`), no downloads, no camera or microphone, no new windows.
//!
//! The agent's call reaches the daemon, the page hears it as a `browser` event and invokes
//! `browser_act` with the op, and the result goes back the same way. Scripts answer through
//! `eval_with_callback`, which serialises their return value at once — so `browser.js` returns a
//! string synchronously and every wait is here, each with its own ceiling shorter than the
//! daemon's, so the agent hears which thing did not happen rather than a generic timeout.

use crate::preview::{Bounds, web_only};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewBuilder, WebviewUrl};

const SCRIPT: &str = include_str!("browser.js");
/// Each webview is a WebContent process of a few hundred MB. Past this, the least recently used
/// one no lane is showing is closed; its lane's next call opens it again.
const MAX_LIVE: usize = 6;
const LOAD_WAIT: Duration = Duration::from_secs(30);
const SCRIPT_WAIT: Duration = Duration::from_secs(10);
/// The most a page's answer may be before it is refused rather than carried. The daemon cuts what
/// reaches the agent to 64 KB; this is the ceiling on what the app holds to get there.
const MAX_ANSWER: usize = 1024 * 1024;
/// Where a webview waits while no lane shows it: on the window, so it keeps running, but nowhere
/// the person can see.
const OFF: (f64, f64) = (-20_000.0, 0.0);

struct Live {
    label: String,
    used: Instant,
    shown: bool,
}

static LIVE: Mutex<Vec<Live>> = Mutex::new(Vec::new());
/// Per label: who is waiting for the page to finish loading.
static LOADS: Mutex<Option<HashMap<String, Vec<tokio::sync::oneshot::Sender<()>>>>> = Mutex::new(None);
/// Per label: one op at a time. Claude Code issues tool calls in parallel, and a click landing
/// between another call's read and its type is two agents in one page.
static TURNS: Mutex<Option<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The webview's label for a lane, or why the lane id is not one.
fn label(lane: &str) -> Result<String, String> {
    if lane.is_empty() || lane.len() > 64 || !lane.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Not a lane.".into());
    }
    Ok(format!("browser-{lane}"))
}

/// The project's own store of cookies and site data, so a sign-in is kept for the project and
/// for no other. FNV-1a over the path: stable across Rust versions, which `DefaultHasher` is not.
fn store_key(project: &str) -> [u8; 16] {
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for b in project.as_bytes() {
        hash ^= u128::from(*b);
        hash = hash.wrapping_mul(0x0000000001000000000000000000013b);
    }
    hash.to_be_bytes()
}

/// The Windows data directory's name; macOS takes the key itself.
#[cfg_attr(not(windows), allow(dead_code))]
fn hex(key: [u8; 16]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn page_loaded(label: &str) {
    if let Some(waiting) = locked(&LOADS).get_or_insert_with(HashMap::new).remove(label) {
        for tx in waiting {
            let _ = tx.send(());
        }
    }
}

fn await_load(label: &str) -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    locked(&LOADS).get_or_insert_with(HashMap::new).entry(label.to_string()).or_default().push(tx);
    rx
}

fn touch(label: &str, shown: Option<bool>) {
    let mut live = locked(&LIVE);
    match live.iter_mut().find(|l| l.label == label) {
        Some(l) => {
            l.used = Instant::now();
            if let Some(s) = shown {
                l.shown = s;
            }
        }
        None => live.push(Live { label: label.to_string(), used: Instant::now(), shown: shown.unwrap_or(false) }),
    }
}

/// Make room for one more, closing the least recently used webview nobody is looking at.
fn make_room(app: &tauri::AppHandle) {
    let gone = {
        let mut live = locked(&LIVE);
        if live.len() < MAX_LIVE {
            return;
        }
        let Some(at) = live.iter().enumerate().filter(|(_, l)| !l.shown).min_by_key(|(_, l)| l.used).map(|(i, _)| i) else {
            return;
        };
        live.remove(at).label
    };
    if let Some(view) = app.get_webview(&gone) {
        let _ = view.close();
    }
    let lane = gone.trim_start_matches("browser-").to_string();
    let _ = app.emit_to("main", "browser-page", serde_json::json!({ "lane": lane, "closed": "room" }));
}

fn create(app: &tauri::AppHandle, label: &str, project: &str, url: Url) -> Result<(), String> {
    make_room(app);
    let window = app.get_window("main").ok_or("The main window is gone.")?;
    let lane = label.trim_start_matches("browser-").to_string();
    let handle = app.clone();
    let opener = app.clone();
    let named = label.to_string();
    #[allow(unused_mut)]
    let mut builder = WebviewBuilder::new(label, WebviewUrl::External(url))
        .on_navigation(|to| web_only(to) || matches!(to.scheme(), "about" | "blob" | "data"))
        .on_download(|_, _| false)
        .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
        // A link that opens a window opens here instead, if it is somewhere the tab may go.
        .on_new_window(move |to, _| {
            if web_only(&to)
                && let Some(view) = opener.get_webview(&named)
            {
                let _ = view.navigate(to);
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_page_load(move |view, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                page_loaded(view.label());
            }
            let _ = handle.emit_to(
                "main",
                "browser-page",
                serde_json::json!({
                    "lane": lane,
                    "url": payload.url().as_str(),
                    "loading": matches!(payload.event(), tauri::webview::PageLoadEvent::Started),
                }),
            );
        })
        .focused(false)
        // ponytail: macOS 14 and later only; on Windows a tab no lane is showing is throttled, and
        // each op waits briefly after acting. A WebView2 policy hook if that proves too slow.
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
    #[cfg(target_os = "macos")]
    {
        builder = builder.data_store_identifier(store_key(project));
    }
    #[cfg(windows)]
    {
        let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("browser").join(hex(store_key(project)));
        builder = builder.data_directory(dir);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = project;
    window
        .add_child(builder, LogicalPosition::new(OFF.0, OFF.1), LogicalSize::new(1280.0, 800.0))
        .map_err(|e| e.to_string())?;
    touch(label, Some(false));
    Ok(())
}

/// Run a script op and return its text, or why it could not be done.
async fn script(view: &tauri::Webview, op: &serde_json::Value) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = Mutex::new(Some(tx));
    view.eval_with_callback(format!("({SCRIPT})({op})"), move |raw| {
        if let Some(tx) = locked(&tx).take() {
            let _ = tx.send(raw);
        }
    })
    .map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(SCRIPT_WAIT, rx)
        .await
        .map_err(|_| "The page did not answer within 10 s; it may be busy or still loading. Read it again.".to_string())?
        .map_err(|_| "The page closed before it answered.".to_string())?;
    // The script runs in the page's own realm, so a page can hand back anything at all — hundreds
    // of MB through an overridden `JSON.stringify`. Nothing that size is carried to the agent.
    if raw.len() > MAX_ANSWER {
        return Err("The page's answer was too large to use.".into());
    }
    // The script returns a JSON string; the platform serialises that value once more on macOS.
    let inner = serde_json::from_str::<String>(&raw).unwrap_or(raw);
    let answer: serde_json::Value = serde_json::from_str(&inner).map_err(|_| "The page gave an answer Keel could not read.".to_string())?;
    let text = answer["text"].as_str().unwrap_or_default().to_string();
    if answer["ok"] == true { Ok(text) } else { Err(text) }
}

/// Do one op in the lane's tab: the agent's, or the person's own back, forward, reload or address.
#[tauri::command]
pub async fn browser_act(app: tauri::AppHandle, lane: String, project: String, op: serde_json::Value) -> Result<String, String> {
    let label = label(&lane)?;
    let turn = locked(&TURNS).get_or_insert_with(HashMap::new).entry(label.clone()).or_default().clone();
    let _turn = turn.lock().await;
    let kind = op["op"].as_str().unwrap_or_default();
    if kind == "open" {
        let url: Url = op["url"].as_str().unwrap_or_default().parse().map_err(|_| "Not an address.".to_string())?;
        if !web_only(&url) {
            return Err("Only http and https addresses open in the Browser tab.".into());
        }
        let loaded = await_load(&label);
        let made = match app.get_webview(&label) {
            Some(view) => {
                touch(&label, None);
                view.navigate(url.clone()).map_err(|e| e.to_string())?;
                false
            }
            None => {
                create(&app, &label, &project, url.clone())?;
                true
            }
        };
        let host = url.host_str().unwrap_or("The page").to_string();
        // No reason is given for a load that never finishes: the platform reports none.
        return match tokio::time::timeout(LOAD_WAIT, loaded).await {
            Ok(_) => Ok(format!("Opened {url}. Read the page to see it.")),
            Err(_) => {
                // A first page that never commits leaves WebKit queueing every script, unanswered,
                // for whatever page commits next. That tab is closed, so the next call says plainly
                // that there is no page, and the tab says what happened.
                if made {
                    let _ = browser_close(app.clone(), lane.clone());
                }
                let said = format!("{host} did not finish loading within 30 s.");
                let _ = app.emit_to("main", "browser-page", serde_json::json!({ "lane": lane, "error": said }));
                Err(said)
            }
        };
    }
    let view = app
        .get_webview(&label)
        .ok_or("This lane's Browser tab has no page yet. Open one with browser_open.")?;
    touch(&label, None);
    let navigates = matches!(kind, "back" | "forward" | "reload");
    let loaded = navigates.then(|| await_load(&label));
    let text = script(&view, &op).await?;
    if let Some(loaded) = loaded {
        // Going back may not load anything (a same-page history entry); waiting for it is bounded.
        let _ = tokio::time::timeout(Duration::from_secs(5), loaded).await;
    } else if matches!(kind, "click" | "type" | "key") {
        // Let the page react before the agent's next read sees it, including a page no lane is
        // showing, whose timers may run slowly.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(text)
}

/// Show the lane's tab over `bounds`, or put it away with `None`.
#[tauri::command]
pub async fn browser_place(app: tauri::AppHandle, lane: String, bounds: Option<Bounds>) -> Result<(), String> {
    let label = label(&lane)?;
    let Some(view) = app.get_webview(&label) else { return Ok(()) };
    match bounds {
        Some(b) => {
            crate::preview::place(&view, &b).await?;
            touch(&label, Some(true));
        }
        None => {
            view.set_position(LogicalPosition::new(OFF.0, OFF.1)).map_err(|e| e.to_string())?;
            touch(&label, Some(false));
        }
    }
    Ok(())
}

/// Close the lane's tab: the lane closed, or its project did. Its logins stay with the project.
#[tauri::command]
pub fn browser_close(app: tauri::AppHandle, lane: String) -> Result<(), String> {
    let label = label(&lane)?;
    locked(&LIVE).retain(|l| l.label != label);
    locked(&TURNS).get_or_insert_with(HashMap::new).remove(&label);
    page_loaded(&label);
    if let Some(view) = app.get_webview(&label) {
        view.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_id_is_a_label_and_nothing_else_is() {
        assert_eq!(label("lane-1_a").unwrap(), "browser-lane-1_a");
        for bad in ["", "a/b", "../x", "a b", &"x".repeat(65)] {
            assert!(label(bad).is_err(), "{bad}");
        }
    }

    /// One project, one store, every launch; another project, another store.
    #[test]
    fn a_project_s_logins_are_its_own() {
        assert_eq!(store_key("/Users/a/one"), store_key("/Users/a/one"));
        assert_ne!(store_key("/Users/a/one"), store_key("/Users/a/two"));
        assert_eq!(hex(store_key("/x")).len(), 32);
    }

    /// The script the page runs parses as a function and returns a string for every op it knows.
    #[test]
    fn the_page_script_is_one_function() {
        assert!(SCRIPT.trim_start().starts_with("//") && SCRIPT.contains("(op) =>"));
        for op in ["read", "click", "type", "key", "scroll", "back", "forward", "reload"] {
            assert!(SCRIPT.contains(&format!("case \"{op}\"")), "{op} is not handled in browser.js");
        }
        assert!(!SCRIPT.contains("op.text}"), "typed text is never echoed back");
    }
}
