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
#[derive(serde::Deserialize, Clone, Copy)]
pub struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    /// The page's own viewport height, in the same units: with the main webview's real height it
    /// gives how far down WebKit drew the page inside its own view (see `place`).
    #[serde(default)]
    viewport: Option<f64>,
}

impl Bounds {
    pub(crate) fn rect(&self) -> tauri::Rect {
        tauri::Rect {
            position: LogicalPosition::new(self.x, self.y).into(),
            size: LogicalSize::new(self.width.max(1.0), self.height.max(1.0)).into(),
        }
    }

}

/// How far a child webview lands from where it is asked to go, learned on the first placement and
/// kept: one window, one offset. Tauri's own sizes cannot give it — on macOS it reported the window's
/// outer and inner heights as equal (868 pt) while a title bar was there, and wry turns `y` into
/// AppKit's bottom-left origin against a parent view that is taller than the page: every child
/// webview landed ~32 pt too high, the Browser tab over its own status row, the preview over the
/// toolbar. So it is measured on the native views, not computed.
static CORRECTION: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// Put a child webview exactly over `bounds`, in the page's logical pixels.
pub(crate) async fn place(view: &tauri::Webview, bounds: &Bounds) -> Result<(), String> {
    let at = |dy: f64| Bounds { y: bounds.y + dy, ..*bounds }.rect();
    let dy = *CORRECTION.lock().unwrap_or_else(|e| e.into_inner());
    view.set_bounds(at(dy)).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        if let Some((top, page_height)) = measured_top(view).await {
            // The page's coordinate 0 is not the main webview's top edge: WebKit insets its content
            // under the title bar, so the page draws `inset` points lower than its view begins.
            // Measured as the view's real height less the page's own viewport height.
            let inset = bounds.viewport.map_or(0.0, |v| (page_height - v).max(0.0));
            // Where it landed against where the page wanted it; `dy` was already applied.
            let error = top - (bounds.y + inset);
            if error.abs() > 0.5 {
                let fixed = dy - error;
                *CORRECTION.lock().unwrap_or_else(|e| e.into_inner()) = fixed;
                view.set_bounds(at(fixed)).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Where the view's top edge really is, in points below the top of the main page's webview — the
/// origin the page measured `bounds` from. Not the window's content view: the main webview need
/// not start at its top, and measured against it a misplaced child read as placed.
#[cfg(target_os = "macos")]
async fn measured_top(view: &tauri::Webview) -> Option<(f64, f64)> {
    use tauri::Manager;
    let main = view.app_handle().get_webview("main")?;
    let (child, _) = edge(view).await?;
    let (page, page_height) = edge(&main).await?;
    Some((child - page, page_height))
}

/// A webview's top edge, in points below the top of its window's content view, and its height.
#[cfg(target_os = "macos")]
async fn edge(view: &tauri::Webview) -> Option<(f64, f64)> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    // Runs on the main thread, after the `set_bounds` queued before it.
    view.with_webview(move |platform| {
        let _ = tx.send(top_of(platform.inner()));
    })
    .ok()?;
    tokio::time::timeout(std::time::Duration::from_millis(250), rx).await.ok()?.ok()?
}

#[cfg(target_os = "macos")]
fn top_of(webview: *mut std::ffi::c_void) -> Option<(f64, f64)> {
    use objc2_app_kit::NSView;
    if webview.is_null() {
        return None;
    }
    // Safety: wry's WKWebView, an NSView, handed to us on the main thread by `with_webview`.
    let view: &NSView = unsafe { &*(webview as *const NSView) };
    let content = view.window()?.contentView()?;
    // Both in the window's base coordinates, origin at the bottom left.
    let c = content.convertRect_toView(content.bounds(), None);
    let v = view.convertRect_toView(view.bounds(), None);
    Some(((c.origin.y + c.size.height) - (v.origin.y + v.size.height), v.size.height))
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

/// Where a child webview may go once it is open: the web, and never Keel's own page. The dev
/// server's page runs the project's dependencies, and `location = "tauri://localhost/"` (or
/// `http://tauri.localhost/` on Windows) would put it at the app's origin inside the main window —
/// where, before capabilities were scoped by webview, it could call `open_project` and read back
/// the daemon's port and token. A link the page follows to another site is still a page.
pub(crate) fn web_only(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && !matches!(
            url.host_str(),
            Some("tauri.localhost" | "ipc.localhost" | "asset.localhost")
        )
}

/// Show the preview at `url` over `bounds`, creating it the first time.
#[tauri::command]
pub async fn preview_show(app: tauri::AppHandle, url: String, bounds: Bounds) -> Result<(), String> {
    let url = local(&url)?;
    let (view, existing) = show_or_create(&app, url, &bounds)?;
    place(&view, &bounds).await?;
    if existing {
        view.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The preview's webview, navigated to `url` or created there, and whether it already existed. A
/// function of its own so the lock is released before `place` awaits: a held std guard cannot
/// cross an await, and the placement does not need it.
fn show_or_create(app: &tauri::AppHandle, url: Url, bounds: &Bounds) -> Result<(tauri::Webview, bool), String> {
    // Held for the whole of this: two shows at once both found no webview and both created one.
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(view) = app.get_webview(LABEL) {
        // The URL Keel last loaded, not `view.url()`: wry unwraps WebKit's URL, which is nil
        // until the first load starts, and a second show in that window crashed the app.
        if shown.as_ref().map(|u| u.origin() != url.origin()).unwrap_or(true) {
            view.navigate(url.clone()).map_err(|e| e.to_string())?;
            *shown = Some(url);
        }
        return Ok((view, true));
    }
    let window = app.get_window("main").ok_or("The main window is gone.")?;
    *shown = Some(url.clone());
    let rect = bounds.rect();
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::External(url))
        .initialization_script_for_all_frames(PICKER)
        // On macOS this decides every frame, not only the top one, and a dev page's iframes are
        // often `about:blank`, `about:srcdoc`, `data:` or `blob:` — none of which is Keel's
        // origin. Windows asks for the top frame only; there the capability scoping is the wall.
        .on_navigation(|to| web_only(to) || matches!(to.scheme(), "about" | "blob" | "data"))
        // Nothing the page offers lands in ~/Downloads unasked.
        .on_download(|_, _| false);
    let view = window.add_child(builder, rect.position, rect.size).map_err(|e| e.to_string())?;
    Ok((view, false))
}

/// Move the preview with the panel: a resize, a drag of the divider.
#[tauri::command]
pub async fn preview_bounds(app: tauri::AppHandle, bounds: Bounds) -> Result<(), String> {
    // Set the complete rectangle in one operation. Separate position/size calls
    // read native bounds back between updates; on macOS that can reflect the Y
    // coordinate and move the child over the toolbar.
    let Some(view) = app.get_webview(LABEL) else { return Ok(()) };
    place(&view, &bounds).await
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

    /// The top frame of a child webview never navigates to Keel's own page, whichever spelling the
    /// platform uses. (A subframe on Windows is not asked about; the capabilities are the wall there.)
    #[test]
    fn a_child_webview_stays_on_the_web() {
        let go = |u: &str| web_only(&u.parse().unwrap());
        for ok in ["http://localhost:3000/a", "https://127.0.0.1:5173/", "https://example.com/x"] {
            assert!(go(ok), "refused {ok}");
        }
        for no in [
            "tauri://localhost/",
            "http://tauri.localhost/",
            "https://tauri.localhost/index.html",
            "http://ipc.localhost/open_project",
            "asset://localhost/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
        ] {
            assert!(!go(no), "allowed {no}");
        }
    }

    /// A capability granted by window reaches every child webview in it: the preview, which runs
    /// the project's dependencies, inherited `open_project` and the daemon's token that way. Each
    /// file is read from disk, so a new one is covered without a list to keep.
    #[test]
    fn no_capability_reaches_a_child_webview_by_its_window() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut read = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let cap: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            read += 1;
            assert!(cap.get("windows").is_none(), "{} grants by window; grant by webview label", path.display());
            let webviews = cap["webviews"].as_array().expect("a capability names its webviews");
            assert!(!webviews.is_empty(), "{} names no webview", path.display());
        }
        assert!(read >= 2, "read {read} capability files from {}", dir.display());
    }
}
