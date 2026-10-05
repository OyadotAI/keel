//! The design preview: the project's dev server in a child webview laid over the side panel, with
//! the picker (`picker.js`) installed in every frame it loads.
//!
//! A child webview rather than an iframe in the page, because a page cannot script a frame from
//! another origin and a host-installed script does not have to. This is the same reason the Swift
//! app used a `WKUserScript` with `forMainFrameOnly: false`, and the reason Keel does not proxy the
//! dev server: a proxy that rewrites HTML breaks HMR.
//!
//! The dev server's page runs the project's dependencies, so what it may call is the one thing it
//! needs: `preview_msg`, which forwards what the picker saw to the main window and does nothing
//! else. `capabilities/preview.json` grants it, and `build.rs` lists the app's commands so no
//! other command is reachable from a remote page.

use serde_json::Value;
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewBuilder, WebviewUrl};

const LABEL: &str = "preview";
const PICKER: &str = include_str!("picker.js");
static SHOWN: std::sync::Mutex<Option<Url>> = std::sync::Mutex::new(None);

/// Where the panel's preview area is, in the window's logical pixels.
#[derive(serde::Deserialize)]
pub struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Only the dev server on this machine: the child webview may reach `preview_msg`, and nothing
/// that is not a local address should be loaded with even that.
fn local(url: &str) -> Result<Url, String> {
    let url: Url = url.parse().map_err(|_| format!("Not a URL: {url}"))?;
    let host = url.host_str().unwrap_or_default();
    if !matches!(url.scheme(), "http" | "https") || !matches!(host, "localhost" | "127.0.0.1") {
        return Err("The preview shows a dev server on this machine only.".into());
    }
    Ok(url)
}

/// Show the preview at `url` over `bounds`, creating it the first time.
#[tauri::command]
pub async fn preview_show(app: tauri::AppHandle, url: String, bounds: Bounds) -> Result<(), String> {
    let url = local(&url)?;
    let (pos, size) = (LogicalPosition::new(bounds.x, bounds.y), LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)));
    // Held for the whole call: two shows at once both found no webview and both created one.
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(view) = app.get_webview(LABEL) {
        // The URL Keel last loaded, not `view.url()`: wry unwraps WebKit's URL, which is nil
        // until the first load starts, and a second show in that window crashed the app.
        if shown.as_ref().map(|u| u.origin() != url.origin()).unwrap_or(true) {
            view.navigate(url.clone()).map_err(|e| e.to_string())?;
            *shown = Some(url);
        }
        view.set_position(pos).map_err(|e| e.to_string())?;
        view.set_size(size).map_err(|e| e.to_string())?;
        return view.show().map_err(|e| e.to_string());
    }
    let window = app.get_window("main").ok_or("The main window is gone.")?;
    *shown = Some(url.clone());
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::External(url)).initialization_script_for_all_frames(PICKER);
    window.add_child(builder, pos, size).map_err(|e| e.to_string())?;
    Ok(())
}

/// Move the preview with the panel: a resize, a drag of the divider.
#[tauri::command]
pub fn preview_bounds(app: tauri::AppHandle, bounds: Bounds) -> Result<(), String> {
    // no work beyond two window-server calls; cheap enough for a ResizeObserver
    let Some(view) = app.get_webview(LABEL) else { return Ok(()) };
    view.set_position(LogicalPosition::new(bounds.x, bounds.y)).map_err(|e| e.to_string())?;
    view.set_size(LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0))).map_err(|e| e.to_string())
}

/// Out of the way: the tab was left, or a menu or dialog is open over it. A native webview draws
/// above the page, so anything the page shows in that rectangle would otherwise be hidden by it.
#[tauri::command]
pub fn preview_hide(app: tauri::AppHandle) -> Result<(), String> {
    match app.get_webview(LABEL) {
        Some(view) => view.hide().map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Tell the picker something: arm, disarm, probe the pins, reload.
#[tauri::command]
pub fn preview_send(app: tauri::AppHandle, msg: Value) -> Result<(), String> {
    let Some(view) = app.get_webview(LABEL) else { return Err("The preview is not open.".into()) };
    // Serialised, never interpolated: the message is JSON the script parses, not code.
    let json = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    view.eval(format!("window.__keelPicker && window.__keelPicker({json})")).map_err(|e| e.to_string())
}

/// What the picker may say, and nothing else. The dev page can call `preview_msg` itself, so the
/// message is parsed into exactly these shapes, with every string capped, and anything else is
/// refused here rather than handed to the window — an unexpected field in a pick once reached
/// the pin list as-is, and a page that sent `verdict: "x"` could blank the whole window.
#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Msg {
    Pick { pin: Pick },
    Probe { results: Vec<Probe> },
    Armed { on: bool },
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    selector: String,
    exact: bool,
    tag: String,
    text: String,
    rect: Rect,
    sources: Vec<Source>,
    styles: std::collections::BTreeMap<String, String>,
    print: String,
    url: String,
    #[serde(default)]
    frame: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Source {
    kind: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    id: String,
    found: bool,
    #[serde(default)]
    print: Option<String>,
    #[serde(default)]
    rect: Option<Rect>,
}

const KINDS: &[&str] = &["inspector", "react", "svelte", "vue", "component", "testid"];

fn cap(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

impl Msg {
    /// Within bounds, or refused: a real pick is a few hundred bytes of each.
    fn bounded(mut self) -> Result<Self, String> {
        match &mut self {
            Msg::Pick { pin } => {
                for (s, max) in [(&mut pin.selector, 400), (&mut pin.tag, 40), (&mut pin.text, 200), (&mut pin.print, 16), (&mut pin.url, 400)] {
                    cap(s, max);
                }
                if let Some(f) = &mut pin.frame {
                    cap(f, 400);
                }
                pin.sources.truncate(5);
                if pin.sources.iter().any(|s| !KINDS.contains(&s.kind.as_str())) {
                    return Err("Unknown source kind.".into());
                }
                for s in &mut pin.sources {
                    for f in [&mut s.file, &mut s.name].into_iter().flatten() {
                        cap(f, 300);
                    }
                }
                if pin.styles.len() > 40 {
                    return Err("Too many styles.".into());
                }
                for v in pin.styles.values_mut() {
                    cap(v, 200);
                }
            }
            Msg::Probe { results } => {
                if results.len() > 50 {
                    return Err("Too many probe results.".into());
                }
            }
            Msg::Armed { .. } => {}
        }
        Ok(self)
    }
}

/// What the picker saw, from the preview's page, forwarded to the main window as `preview`.
/// Refused from any other webview: the main page has no reason to call it, and it must not be a
/// way for anything else to speak as the picker.
#[tauri::command]
pub fn preview_msg(app: tauri::AppHandle, webview: tauri::Webview, msg: Value) -> Result<(), String> {
    if webview.label() != LABEL {
        return Err("Only the preview may send picker messages.".into());
    }
    // A pick is a few KB; anything far past that is not the picker.
    if msg.to_string().len() > 256 * 1024 {
        return Err("Picker message too large.".into());
    }
    let msg: Msg = serde_json::from_value(msg).map_err(|_| "Not a picker message.".to_string())?;
    app.emit_to("main", "preview", msg.bounded()?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page can call preview_msg directly; only the picker's shapes get through.
    #[test]
    fn only_the_pickers_shapes_get_through() {
        let pick = serde_json::json!({"type":"pick","pin":{"selector":"a","exact":true,"tag":"a","text":"t",
            "rect":{"x":0,"y":0,"width":1,"height":1},"sources":[{"kind":"react","file":"src/a.tsx","line":3}],
            "styles":{},"print":"ab","url":"http://localhost:3000/"}});
        assert!(serde_json::from_value::<Msg>(pick.clone()).unwrap().bounded().is_ok());
        let mut forged = pick.clone();
        forged["pin"]["verdict"] = "x".into();
        assert!(serde_json::from_value::<Msg>(forged).is_err(), "a verdict is Keel's to give");
        let mut odd = pick;
        odd["pin"]["sources"] = serde_json::json!([{"kind":"curl|sh"}]);
        assert!(serde_json::from_value::<Msg>(odd).unwrap().bounded().is_err());
        assert!(serde_json::from_value::<Msg>(serde_json::json!({"type":"eval","js":"x"})).is_err());
    }
}
