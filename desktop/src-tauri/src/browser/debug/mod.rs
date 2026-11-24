//! Native adapters retain browser identity on both completed operations and inspection failures.
#[cfg(target_os = "macos")]
mod actions;
#[cfg(target_os = "macos")]
mod channel;
#[cfg(target_os = "macos")]
mod frames;
#[cfg(target_os = "macos")]
mod read;
use super::capture::Output;
#[cfg(target_os = "macos")]
mod constants;
use serde_json::{Value, json};
/// Scripts are installed before remote navigation; unsupported platforms reject diagnostics explicitly.
pub async fn install(view: &tauri::Webview) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return channel::install(view).await;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = view;
        Ok(())
    }
}
/// Closing invalidates document objects and node maps before a label can be reused.
pub fn forget(view: &tauri::Webview) {
    #[cfg(target_os = "macos")]
    frames::forget(view);
    #[cfg(not(target_os = "macos"))]
    let _ = view;
}
/// Browser-native metadata remains available when a page is frozen or has no scriptable document.
pub async fn identity(view: &tauri::Webview) -> Value {
    #[cfg(target_os = "macos")]
    {
        let mut value=channel::call(view,"identity","","",false).await.unwrap_or(json!({"contacted":true,"browser":"Inside Oya Factory","version":null,"engine":null,"profile":null,"dpr":null}));
        if let Ok(frame) = frames::resolve(view, None).await {
            if let Ok(metrics) = frames::run(view, &frame, "metadata", json!({})).await {
                value["dpr"] = metrics["dpr"].clone();
            }
        }
        return value;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = view;
        json!({"contacted":true,"browser":"Inside Oya Factory","version":null,"engine":null,"profile":null,"dpr":null})
    }
}
/// Viewport metadata uses the isolated document, so author scripts cannot forge host measurements.
#[cfg(target_os = "macos")]
pub(super) async fn metrics(view: &tauri::Webview) -> Result<Value, String> {
    let frame = frames::resolve(view, None).await?;
    frames::run(view, &frame, "metadata", json!({})).await
}

/// Entry admission prevents every page and native effect when this page uses snapshot or hidden presentation.
#[cfg(target_os = "macos")]
pub(super) async fn input_unavailable(view: &tauri::Webview) -> Result<Option<Output>, String> {
    let reason = super::viewport::input_reason(view.label());
    let reason = match reason {
        Some(reason) => Some(reason),
        None => {
            if channel::call(view, "presentation", "", "", false).await? == true {
                None
            } else {
                Some("parked")
            }
        }
    };
    Ok(reason.map(unavailable_output))
}
#[cfg(target_os = "macos")]
fn unavailable_output(reason: &str) -> Output {
    let fixed = reason == "fixed_snapshot";
    let text = if fixed {
        "Native mouse input is unavailable while this embedded page is shown as a fixed-size snapshot. No input was sent. Show this lane’s Browser tab, reset with browser_viewport({reset:true}), then run browser_read again and retry using the new element number. Reset changes the layout; this retry does not verify phone-width interaction. Fixed-size inspection and screenshots remain available."
    } else {
        "Native mouse input is unavailable while this embedded page is hidden. No input was sent. Show this lane’s Browser tab, run browser_read again, then retry using the new element number. No viewport reset is needed."
    };
    let recovery = if fixed {
        vec![
            "show_browser_tab",
            "reset_viewport",
            "reread",
            "retry_click",
        ]
    } else {
        vec!["show_browser_tab", "reread", "retry_click"]
    };
    Output {
        text: text.into(),
        ok: Some(false),
        outcome: Some("native_input_unavailable".into()),
        data: Some(
            json!({"code":"native_input_unavailable","reason":reason,"input":"native_mouse","dispatched":[],"recovery":recovery}),
        ),
        ..Default::default()
    }
}

/// Only this fixed set routes through the frame-aware adapter; no arbitrary native operation is exposed.
pub async fn perform(view: &tauri::Webview, op: &Value) -> Result<Output, String> {
    #[cfg(target_os = "macos")]
    {
        let result=tokio::time::timeout(constants::OPERATION_WAIT,dispatch(view,op)).await.map_err(|_|"Browser inspection deadline reached. An expression or partial input may have run; inspect before retrying.")??;
        return Ok(result);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (view, op);
        Err("Frame diagnostics and native clicks are unavailable on this platform. This lane did not switch browsers.".into())
    }
}
#[cfg(target_os = "macos")]
async fn dispatch(view: &tauri::Webview, op: &Value) -> Result<Output, String> {
    let data = match op["op"].as_str().unwrap_or("") {
        "read" | "read_frames" => {
            let (text, data) = read::read(view, op["op"] == "read_frames").await?;
            return Ok(Output {
                text,
                data: Some(data),
                ..Default::default()
            });
        }
        "eval" => actions::evaluate(view, op).await?,
        "hit_test" => actions::hit_test(view, op).await?,
        "click" => return actions::click(view, op["target"].as_u64().unwrap_or(0) as u32).await,
        "type" => actions::type_text(view, op).await?,
        _ => return Err("Unsupported native diagnostic operation.".into()),
    };
    Ok(Output {
        text: data.to_string(),
        data: Some(data),
        ..Default::default()
    })
}
