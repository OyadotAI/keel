//! Opaque document and node capabilities survive neither navigation nor a replacement native view.
use super::constants::{MAX_DEPTH, MAX_ELEMENTS, OWNER_POLL_DELAY, OWNER_POLLS};
use super::{super::locked, channel};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
/// A reference binds the native WKFrameInfo and its pre-page isolated document generation.
#[derive(Clone, Deserialize)]
pub struct Frame {
    /// Host-issued capability, never an address or frame index.
    pub reference: String,
    /// Isolated generation checked by every renderer operation.
    pub document: String,
    /// Native WebKit main-frame flag.
    pub main: bool,
    /// Observed metadata, never selection authority.
    pub url: String,
}
/// Read numbers retain their exact native document and private node capability.
#[derive(Clone)]
pub struct Element {
    /// Exact owning document.
    pub frame: Frame,
    /// Isolated node capability, invalid after replacement.
    pub local: u32,
}
#[derive(Default)]
struct State {
    next: u32,
    elements: HashMap<u32, Element>,
    owners: HashMap<String, (Frame, String)>,
}
fn states() -> &'static Mutex<HashMap<String, State>> {
    static STATE: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
/// Native registration in the private WK world is the only source of a frame reference.
pub async fn inventory(view: &tauri::Webview) -> Result<Vec<Frame>, String> {
    serde_json::from_value(channel::call(view, "frames", "", "", false).await?)
        .map_err(|_| "Invalid native frame inventory.".into())
}
/// A URL, frame name or old lane reference cannot silently select another document.
pub async fn resolve(view: &tauri::Webview, reference: Option<&str>) -> Result<Frame, String> {
    inventory(view)
        .await?
        .into_iter()
        .find(|f| reference.map(|r| f.reference == r).unwrap_or(f.main))
        .ok_or("Frame changed or closed. Read again with frames:true.".into())
}
/// The isolated runtime checks its own document generation atomically with every operation.
pub async fn run(
    view: &tauri::Webview,
    frame: &Frame,
    kind: &str,
    params: Value,
) -> Result<Value, String> {
    let request = json!({"id":"native","document":frame.document,"kind":kind,"params":params});
    let source = format!("return JSON.stringify(await globalThis.__keelDebug.execute({request}));");
    let answer = channel::call(view, "run", &frame.reference, &source, false).await?;
    if answer["ok"] != true || answer["document"] != frame.document {
        return Err(answer["error"]
            .as_str()
            .unwrap_or("Frame changed or closed.")
            .into());
    }
    Ok(answer["data"].clone())
}
/// All published IDs remain monotonically unique until this native view is closed.
pub fn begin_read(view: &tauri::Webview) {
    let mut states = locked(states());
    states
        .entry(view.label().into())
        .or_default()
        .elements
        .clear();
}
/// Numbers bind exact isolated nodes, never selectors copied from page attributes.
pub fn publish(view: &tauri::Webview, frame: &Frame, local: u32) -> Result<u32, String> {
    let mut states = locked(states());
    let state = states.entry(view.label().into()).or_default();
    if state.elements.len() >= MAX_ELEMENTS {
        return Err("Element capacity exceeded.".into());
    }
    state.next = state
        .next
        .checked_add(1)
        .ok_or("Element reference capacity exceeded.")?;
    let id = state.next;
    state.elements.insert(
        id,
        Element {
            frame: frame.clone(),
            local,
        },
    );
    Ok(id)
}
/// The private map is cleared before each read and is scoped to one native view.
pub fn target(view: &tauri::Webview, id: u32) -> Result<Element, String> {
    locked(states())
        .get(view.label())
        .and_then(|s| s.elements.get(&id))
        .cloned()
        .ok_or("Element number is stale. Read the page again.".into())
}
/// Native frame ancestry in public WK is recovered through a trusted, one-use WindowProxy challenge.
pub async fn owner(
    view: &tauri::Webview,
    child: &Frame,
) -> Result<Option<(Frame, String)>, String> {
    if child.main {
        return Ok(None);
    }
    let cached = locked(states())
        .get(view.label())
        .and_then(|s| s.owners.get(&child.reference))
        .cloned();
    if let Some((parent, node)) = cached {
        run(view, &parent, "metadata", json!({})).await?;
        run(view, child, "metadata", json!({})).await?;
        return Ok(Some((parent, node)));
    }
    discover_owner(view, child).await.map(Some)
}
async fn discover_owner(view: &tauri::Webview, child: &Frame) -> Result<(Frame, String), String> {
    let nonce = crate::browser::hex(crate::browser::store_key(&format!(
        "{}:{}:{}",
        view.label(),
        child.reference,
        unique_nonce()
    )));
    let candidates: Vec<_> = inventory(view)
        .await?
        .into_iter()
        .filter(|f| f.reference != child.reference)
        .collect();
    let mut armed = Vec::new();
    for frame in &candidates {
        if run(view, frame, "arm", json!({"nonce":nonce}))
            .await
            .is_ok()
        {
            armed.push(frame.clone());
        }
    }
    let result = discover_armed(view, child, &nonce, &armed).await;
    for frame in &armed {
        let _ = run(view, frame, "cancel_owner", json!({"nonce":nonce})).await;
    }
    let result = result?;
    locked(states())
        .entry(view.label().into())
        .or_default()
        .owners
        .insert(child.reference.clone(), result.clone());
    Ok(result)
}
fn unique_nonce() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS randomness for frame association");
    crate::browser::hex(bytes)
}
async fn discover_armed(
    view: &tauri::Webview,
    child: &Frame,
    nonce: &str,
    parents: &[Frame],
) -> Result<(Frame, String), String> {
    run(view, child, "challenge", json!({"nonce":nonce})).await?;
    for _ in 0..OWNER_POLLS {
        for parent in parents {
            if let Ok(Value::String(node)) =
                run(view, parent, "owner_peek", json!({"nonce":nonce})).await
            {
                run(view, child, "metadata", json!({})).await?;
                return Ok((parent.clone(), node));
            }
        }
        tokio::time::sleep(OWNER_POLL_DELAY).await;
    }
    Err("The iframe has no verified live parent association.".into())
}
/// Every projection checks the exact connected owner and its native document generation.
pub async fn top_point(
    view: &tauri::Webview,
    frame: &Frame,
    mut point: Value,
    geometry: bool,
    scroll: bool,
) -> Result<Value, String> {
    let mut current = frame.clone();
    for _ in 0..MAX_DEPTH {
        let Some((parent, node)) = owner(view, &current).await? else {
            return Ok(point);
        };
        point = run(
            view,
            &parent,
            "project",
            json!({"owner":node,"x":point["x"],"y":point["y"],"geometry":geometry,"scroll":scroll}),
        )
        .await?;
        run(view, &current, "metadata", json!({})).await?;
        current = parent;
    }
    Err("Frame nesting exceeds 16 levels.".into())
}
/// Explicit close retires both native objects and Rust capabilities; late callbacks cannot republish.
pub fn forget(view: &tauri::Webview) {
    locked(states()).remove(view.label());
    channel::forget(view);
}
