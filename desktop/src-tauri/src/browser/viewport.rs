//! Lane-owned fixed dimensions stay independent of panel placement; snapshots fit without page zoom.
use super::{OFF, capture, locked, script};
use crate::preview::Bounds;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, sync::Mutex};
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager};

const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: f64 = 16_777_216.0;
const PREVIEW_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
static STATES: Mutex<Option<HashMap<String, State>>> = Mutex::new(None);

#[derive(Clone, Default)]
struct State {
    size: Option<(u32, u32)>,
    bounds: Option<Bounds>,
    shown: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Settings {
    width: Option<u32>,
    height: Option<u32>,
    device_scale_factor: Option<f64>,
    mobile: Option<bool>,
    #[serde(default)]
    reset: bool,
}

/// A hide request revokes native input before waiting for the active browser operation to settle.
pub(super) fn intend(label: &str, bounds: Option<Bounds>) {
    if let Some(state) = locked(&STATES)
        .as_mut()
        .and_then(|states| states.get_mut(label))
    {
        state.shown = bounds.is_some();
        if let Some(bounds) = bounds {
            state.bounds = Some(bounds);
        }
    }
}
/// Fixed-size snapshot mode takes precedence over an automatic view that is currently parked.
pub(super) fn input_reason(label: &str) -> Option<&'static str> {
    let state = state(label);
    if state.size.is_some() {
        Some("fixed_snapshot")
    } else if !state.shown {
        Some("parked")
    } else {
        None
    }
}

/// Panel changes remember their automatic bounds without resizing an overridden page.
pub async fn place(view: &tauri::Webview, bounds: Option<Bounds>) -> Result<(), String> {
    let size = {
        let mut states = locked(&STATES);
        let state = states
            .get_or_insert_with(HashMap::new)
            .entry(view.label().into())
            .or_default();
        if let Some(bounds) = bounds {
            state.bounds = Some(bounds);
        }
        state.shown = bounds.is_some();
        state.size
    };
    match (bounds, size) {
        (Some(bounds), None) => crate::preview::place(view, &bounds).await,
        _ => park(view).await,
    }
}

/// Build the native view at retained dimensions so even the first page script sees its viewport.
pub fn initial_size(label: &str) -> LogicalSize<f64> {
    let (width, height) = state(label)
        .size
        .map(|(width, height)| (f64::from(width), f64::from(height)))
        .unwrap_or(super::DEFAULT_SIZE);
    LogicalSize::new(width, height)
}

fn state(label: &str) -> State {
    locked(&STATES)
        .as_ref()
        .and_then(|states| states.get(label))
        .cloned()
        .unwrap_or_default()
}

/// Closing a lane retires its dimensions; a pending native callback cannot recreate state.
pub fn forget(label: &str) {
    if let Some(states) = locked(&STATES).as_mut() {
        states.remove(label);
    }
}

fn validate(value: &serde_json::Value) -> Result<Settings, String> {
    capture::supported()?;
    let settings: Settings =
        serde_json::from_value(value.clone()).map_err(|_| "Invalid viewport settings.")?;
    if settings.device_scale_factor.is_some() || settings.mobile.is_some() {
        return Err("This browser cannot apply: deviceScaleFactor, mobile. The viewport was not changed. Retry without those settings.".into());
    }
    if settings.reset {
        if settings.width.is_some() || settings.height.is_some() {
            return Err("reset:true cannot be combined with dimensions.".into());
        }
    } else if !matches!((settings.width, settings.height), (Some(w),Some(h)) if w > 0 && h > 0 && w <= MAX_DIMENSION && h <= MAX_DIMENSION)
    {
        return Err("width and height must be whole numbers from 1 to 4096 CSS pixels.".into());
    }
    Ok(settings)
}

/// Establish a truthful preview before committing new dimensions, restoring old bounds on failure.
pub async fn apply(
    app: &tauri::AppHandle,
    label: &str,
    value: &serde_json::Value,
) -> Result<capture::Output, String> {
    let settings = validate(value)?;
    let view = app
        .get_webview(label)
        .ok_or("No page is open in this lane. Open one with browser_open, then retry.")?;
    let old = state(label);
    let before = script(&view, &json!({"op":"viewport_info"})).await?;
    let measured: serde_json::Value =
        serde_json::from_str(&before).map_err(|_| "Could not measure this page's viewport.")?;
    let _dpr = measured["dpr"]
        .as_f64()
        .filter(|dpr| dpr.is_finite() && *dpr > 0.0)
        .ok_or("Could not measure this page's DPR.")?;
    let native_dpr = view.window().scale_factor().map_err(|e| e.to_string())?;
    let size = settings.width.zip(settings.height);
    if let Some((width, height)) = size {
        if f64::from(width) * f64::from(height) * native_dpr * native_dpr > MAX_PIXELS {
            return Err(
                "Viewport exceeds 16,777,216 physical pixels. Use a smaller viewport.".into(),
            );
        }
    }
    let result = resize(&view, size, old.bounds, old.shown).await;
    let result = match result {
        Ok(()) => establish(&view, size).await,
        Err(why) => Err(why),
    };
    let (text, image) = match result {
        Ok(result) => result,
        Err(why) => {
            if let Err(restore) = resize(&view, old.size, old.bounds, old.shown).await {
                return Err(format!(
                    "Could not set viewport: {why}. Restoring the previous viewport also failed: {restore}. Inspect the page before retrying."
                ));
            }
            return Err(format!(
                "Could not set viewport: {why}. Previous viewport restored."
            ));
        }
    };
    locked(&STATES)
        .get_or_insert_with(HashMap::new)
        .insert(label.into(), State { size, ..old });
    emit(app, label, image, None);
    Ok(capture::Output::text(format!(
        "Viewport {}: {text}. Applies to this lane until reset.",
        if settings.reset {
            "reset to automatic sizing"
        } else {
            "set"
        }
    )))
}

async fn resize(
    view: &tauri::Webview,
    size: Option<(u32, u32)>,
    bounds: Option<Bounds>,
    shown: bool,
) -> Result<(), String> {
    if let Some((width, height)) = size {
        view.set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        park(view).await
    } else if shown {
        if let Some(bounds) = bounds {
            crate::preview::place(view, &bounds).await?;
        }
        Ok(())
    } else {
        let size = bounds.map(|bounds| bounds.rect().size).unwrap_or_else(|| {
            LogicalSize::new(super::DEFAULT_SIZE.0, super::DEFAULT_SIZE.1).into()
        });
        view.set_bounds(tauri::Rect {
            position: LogicalPosition::new(OFF.0, OFF.1).into(),
            size,
        })
        .map_err(|e| e.to_string())
    }
}

async fn establish(
    view: &tauri::Webview,
    size: Option<(u32, u32)>,
) -> Result<(String, Option<capture::Image>), String> {
    let text = script(view, &json!({"op":"viewport_info"})).await?;
    let info: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "Could not measure the viewport.")?;
    if let Some((width, height)) = size {
        if info["width"] != width || info["height"] != height {
            return Err(format!(
                "The page measured {} × {}, expected {width} × {height}",
                info["width"], info["height"]
            ));
        }
    }
    script(view, &json!({"op":"clear_marks"})).await?;
    let image = if size.is_some() {
        Some(capture::capture(view).await?.0)
    } else {
        None
    };
    Ok((
        format!(
            "{} × {} CSS px. DPR: {}. Mobile mode: unsupported",
            info["width"], info["height"], info["dpr"]
        ),
        image,
    ))
}

/// A completed click stays successful if its later preview refresh fails; the UI says why.
pub async fn refresh(app: &tauri::AppHandle, label: &str) {
    if state(label).size.is_none() {
        return;
    }
    let Some(view) = app.get_webview(label) else {
        return;
    };
    match tokio::time::timeout(PREVIEW_WAIT, establish(&view, state(label).size)).await {
        Ok(Ok((_, image))) => emit(app, label, image, None),
        Ok(Err(why)) => emit(app, label, None, Some(why)),
        Err(_) => emit(
            app,
            label,
            None,
            Some("Preview capture timed out; the browser action completed.".into()),
        ),
    }
}

/// Keep parked content below native title-bar insets so CSS dimensions equal the requested viewport.
pub(super) async fn park(view: &tauri::Webview) -> Result<(), String> {
    view.set_position(LogicalPosition::new(OFF.0, OFF.1))
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        let top = parked_inset(view).await?;
        if top > 0.0 {
            view.set_position(LogicalPosition::new(OFF.0, OFF.1 + top))
                .map_err(|e| e.to_string())?;
            let _ = parked_inset(view).await?;
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
async fn parked_inset(view: &tauri::Webview) -> Result<f64, String> {
    use objc2_app_kit::NSView;
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |platform| {
        let native = unsafe { &*(platform.inner() as *const NSView) };
        native.layoutSubtreeIfNeeded();
        let top = native.safeAreaInsets().top;
        let _ = tx.send(top);
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(super::METADATA_WAIT, rx)
        .await
        .map_err(|_| "Native viewport inset measurement timed out.")?
        .map_err(|_| "The browser closed during viewport placement.".into())
}

fn emit(app: &tauri::AppHandle, label: &str, image: Option<capture::Image>, error: Option<String>) {
    let _ = app.emit_to("main", "browser-preview", json!({"lane":label.trim_start_matches("browser-"),"image":image,"fixed":state(label).size.is_some(),"error":error}));
}

/// Capture metadata and actual PNG dimensions are separate; page annotations never enter pixels.
pub async fn screenshot(view: &tauri::Webview) -> Result<capture::Output, String> {
    capture::supported()?;
    script(view, &json!({"op":"clear_marks"})).await?;
    let info = script(view, &json!({"op":"viewport_info"})).await?;
    let (image, width, height) = capture::capture(view).await?;
    Ok(capture::Output {
        text: format!(
            "Screenshot viewport: {info}. Image: {width} × {height} px. Mobile mode: unsupported."
        ),
        image: Some(image),
        data: Some(
            json!({"viewport":serde_json::from_str::<serde_json::Value>(&info).unwrap_or(serde_json::Value::Null),"image":{"width":width,"height":height}}),
        ),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recreated_views_start_at_retained_dimensions_until_the_lane_is_closed() {
        let label = "browser-initial-size-fixture";
        locked(&STATES).get_or_insert_with(HashMap::new).insert(
            label.into(),
            State {
                size: Some((375, 812)),
                ..Default::default()
            },
        );
        assert_eq!(initial_size(label), LogicalSize::new(375.0, 812.0));
        forget(label);
        assert_eq!(
            initial_size(label),
            LogicalSize::new(super::super::DEFAULT_SIZE.0, super::super::DEFAULT_SIZE.1)
        );
    }
    #[test]
    fn unsupported_settings_and_invalid_sizes_are_refused_before_the_view_is_touched() {
        for value in [
            json!({"width":375,"height":812,"mobile":false}),
            json!({"width":375,"height":812,"deviceScaleFactor":2}),
            json!({"width":0,"height":812}),
            json!({"width":375}),
            json!({"reset":true,"width":375}),
            json!({"width":4097,"height":812}),
        ] {
            assert!(validate(&value).is_err(), "{value}");
        }
        #[cfg(target_os = "macos")]
        {
            assert!(validate(&json!({"width":375,"height":812})).is_ok());
            assert!(validate(&json!({"reset":true})).is_ok());
        }
    }
}
