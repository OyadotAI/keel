//! Read-derived capabilities drive page inspection and genuine native pointer dispatch.
use super::constants::{
    BUTTON_PAIR_EVENT, MAX_DEPTH, MAX_EXPRESSION, MAX_JSON, MOVE_EVENT, POINT_TOLERANCE,
    POINTER_POLL_DELAY, POINTER_POLLS, URL_BYTES,
};
use super::{
    channel,
    frames::{self, Frame},
    read,
};
use serde_json::{Value, json};
/// Page-world code is parsed without invocation, fenced against navigation and serialized outside its scope.
pub async fn evaluate(view: &tauri::Webview, op: &Value) -> Result<Value, String> {
    let frame = frames::resolve(view, op["frame"].as_str()).await?;
    let expression = op["expression"].as_str().ok_or("Missing expression.")?;
    if expression.len() > MAX_EXPRESSION {
        return Err("Expression exceeds 32 KiB.".into());
    }
    let body = format!("return(\n{expression}\n);");
    channel::parse(&body)?;
    frames::run(view, &frame, "metadata", json!({})).await?;
    let nonce = channel::call(
        view,
        "run",
        &frame.reference,
        "this.__keelDebugSerialize(this.__keelDebugMainDocument)",
        true,
    )
    .await?;
    frames::run(view, &frame, "metadata", json!({})).await?;
    let source = format!(
        "(()=>{{if(this.__keelDebugMainDocument!=={nonce})throw 'Frame changed or closed';try{{const value=(function(){{{body}}})();return '{{\"ok\":true,\"value\":'+this.__keelDebugSerialize(value)+'}}';}}catch(error){{return this.__keelDebugFailure(error);}}}})()"
    );
    let result = channel::call(view, "run", &frame.reference, &source, true).await?;
    if result["ok"] != true {
        return Err(format!(
            "{}. The expression ran.",
            result["error"]
                .as_str()
                .unwrap_or("JavaScript execution failed")
        ));
    }
    let value = result["value"].clone();
    if value.to_string().len() > MAX_JSON {
        return Err("JSON result exceeded 64 KiB. The expression ran.".into());
    }
    Ok(json!({"frame":frame.reference,"url":read::prefix(&frame.url,URL_BYTES),"value":value}))
}
/// The element returned by each document's actual hit test selects the next verified frame owner.
pub async fn hit_test(view: &tauri::Webview, op: &Value) -> Result<Value, String> {
    let mut frame = frames::resolve(view, None).await?;
    let mut point = json!({"x":op["x"],"y":op["y"]});
    let mut path = Vec::new();
    for _ in 0..MAX_DEPTH {
        path.push(json!({"ref":frame.reference,"url":read::prefix(&frame.url,URL_BYTES)}));
        let hit = frames::run(view, &frame, "hit", point.clone()).await?;
        let Some(owner) = hit["owner"].as_str() else {
            return finish_hit(view, &frame, hit, &path, &op["x"], &op["y"], None).await;
        };
        match child_for_owner(view, &frame, owner).await {
            Ok(child) => {
                point = frames::run(
                    view,
                    &frame,
                    "project",
                    json!({"owner":owner,"inverse":true,"x":point["x"],"y":point["y"]}),
                )
                .await?;
                frame = child;
            }
            Err(why) => {
                return finish_hit(view, &frame, hit, &path, &op["x"], &op["y"], Some(why)).await;
            }
        }
    }
    Err("Frame nesting exceeds 16 levels.".into())
}
async fn child_for_owner(
    view: &tauri::Webview,
    parent: &Frame,
    node: &str,
) -> Result<Frame, String> {
    for child in frames::inventory(view).await? {
        if child.main {
            continue;
        }
        if let Ok(Some((owner, id))) = frames::owner(view, &child).await {
            if owner.reference == parent.reference && id == node {
                return Ok(child);
            }
        }
    }
    Err("The iframe has no verified live document association.".into())
}
async fn finish_hit(
    view: &tauri::Webview,
    frame: &Frame,
    mut hit: Value,
    path: &[Value],
    x: &Value,
    y: &Value,
    reason: Option<String>,
) -> Result<Value, String> {
    let bounds = hit["element"]["bounds"].clone();
    if hit["element"].is_object() {
        hit["element"]["localBounds"] = bounds.clone();
    }
    let top = project_bounds(view, frame, &bounds).await;
    let partial = reason.is_some() || top.is_err();
    if hit["element"].is_object() {
        hit["element"]["bounds"] = top.unwrap_or(Value::Null);
    }
    Ok(
        json!({"point":{"x":x,"y":y},"frame":frame.reference,"url":read::prefix(&frame.url,URL_BYTES),"framePath":path,"element":hit["element"],"partial":partial,"reason":reason}),
    )
}
async fn project_bounds(view: &tauri::Webview, frame: &Frame, b: &Value) -> Result<Value, String> {
    let (x, y, w, h) = (
        b["x"].as_f64().ok_or("No element bounds.")?,
        b["y"].as_f64().ok_or("No element bounds.")?,
        b["width"].as_f64().ok_or("No width.")?,
        b["height"].as_f64().ok_or("No height.")?,
    );
    let a = frames::top_point(view, frame, json!({"x":x,"y":y}), true, false).await?;
    let z = frames::top_point(view, frame, json!({"x":x+w,"y":y+h}), true, false).await?;
    Ok(
        json!({"x":a["x"],"y":a["y"],"width":z["x"].as_f64().unwrap()-a["x"].as_f64().unwrap(),"height":z["y"].as_f64().unwrap()-a["y"].as_f64().unwrap()}),
    )
}
/// Native movement must reach the exact frame/node before any button is pressed.
pub async fn click(
    view: &tauri::Webview,
    id: u32,
) -> Result<super::super::capture::Output, String> {
    if let Some(output) = super::input_unavailable(view).await? {
        return Ok(output);
    }
    let target = frames::target(view, id)?;
    let mut sent = Vec::new();
    let result = click_target(view, &target, &mut sent).await;
    let _ = frames::run(view, &target.frame, "release_pointer", json!({})).await;
    let data = match &result {
        Ok(point) => {
            json!({"input":"native_mouse","point":point,"frame":target.frame.reference,"dispatched":sent,"outcome":"dispatched"})
        }
        Err(_) => json!({"dispatched":sent}),
    };
    let output = match result {
        Ok(_) => super::super::capture::Output {
            text: data.to_string(),
            data: Some(data),
            ..Default::default()
        },
        Err(why) => super::super::capture::Output {
            text: format!(
                "{why} Completed native events: {}. Inspect before retrying.",
                sent.join(", ")
            ),
            data: Some(data),
            ok: Some(false),
            outcome: Some(
                if sent.is_empty() {
                    "failed"
                } else {
                    "partial_input"
                }
                .into(),
            ),
            ..Default::default()
        },
    };
    Ok(output)
}

async fn click_target(
    view: &tauri::Webview,
    target: &frames::Element,
    sent: &mut Vec<String>,
) -> Result<Value, String> {
    let point = target_point(view, target, true).await?;
    let (x, y) = xy(&point)?;
    super::super::capture::capture(view).await?;
    frames::run(
        view,
        &target.frame,
        "arm_pointer",
        json!({"element":target.local}),
    )
    .await?;
    native_events(view, target, x, y, MOVE_EVENT, sent).await?;
    let mut reached = false;
    for _ in 0..POINTER_POLLS {
        if frames::run(view, &target.frame, "pointer_reached", json!({})).await? == true {
            reached = true;
            break;
        }
        tokio::time::sleep(POINTER_POLL_DELAY).await;
        native_events(view, target, x, y, MOVE_EVENT, sent).await?;
    }
    if !reached {
        return Err(
            "Native pointer did not reach the intended frame. No button press was sent.".into(),
        );
    }
    let after = target_point(view, target, false).await?;
    let (ax, ay) = xy(&after)?;
    if (ax - x).abs() > POINT_TOLERANCE || (ay - y).abs() > POINT_TOLERANCE {
        return Err("Element moved before input. No button press was sent.".into());
    }
    frames::run(view, &target.frame, "metadata", json!({})).await?;
    // Both buttons run in one native UI turn; cancellation cannot leave a completed down without release.
    native_events(view, target, x, y, BUTTON_PAIR_EVENT, sent).await?;
    Ok(point)
}
async fn target_point(
    view: &tauri::Webview,
    target: &frames::Element,
    scroll: bool,
) -> Result<Value, String> {
    let local = frames::run(
        view,
        &target.frame,
        "point",
        json!({"element":target.local,"scroll":scroll}),
    )
    .await?;
    frames::top_point(view, &target.frame, local, false, scroll).await
}
fn xy(p: &Value) -> Result<(f64, f64), String> {
    Ok((
        p["x"].as_f64().ok_or("Missing x coordinate.")?,
        p["y"].as_f64().ok_or("Missing y coordinate.")?,
    ))
}
/// Typing reports its DOM value operation separately from native pointer events.
pub async fn type_text(view: &tauri::Webview, op: &Value) -> Result<Value, String> {
    let target = frames::target(view, op["target"].as_u64().unwrap_or(0) as u32)?;
    frames::run(
        view,
        &target.frame,
        "type",
        json!({"element":target.local,"text":op["text"],"submit":op["submit"]}),
    )
    .await?;
    Ok(json!({"typed":true,"frame":target.frame.reference,"input":"dom_value"}))
}

/// Native partial input is carried in structured JSON rather than reconstructed from error text.
async fn native_events(
    view: &tauri::Webview,
    target: &frames::Element,
    x: f64,
    y: f64,
    kind: i32,
    sent: &mut Vec<String>,
) -> Result<(), String> {
    let result = channel::mouse(view, &target.frame, x, y, kind).await?;
    for event in result["dispatched"].as_array().into_iter().flatten() {
        if let Some(event) = event.as_str() {
            sent.push(event.into());
        }
    }
    if result["ok"] != true {
        return Err(result["error"]
            .as_str()
            .unwrap_or("Native input failed.")
            .into());
    }
    Ok(())
}
