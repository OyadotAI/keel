//! Native page snapshots avoid exposing the app or relying on page-controlled canvas capture.
use serde::Serialize;

const CAPTURE_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
const MAX_ENCODED_BYTES: usize = 6 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_777_216;

/// PNG content travels independently of text so daemon truncation cannot corrupt it.
#[derive(Clone, Serialize)]
pub struct Image {
    pub data: String,
    pub mime_type: String,
}

/// Both old textual operations and captures use the same desktop response envelope.
#[derive(Serialize)]
pub struct Output {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
}
impl Output {
    /// Normal actions remain text, including when a preview refresh fails after their effect.
    pub fn text(text: String) -> Self {
        Self { text, image: None }
    }
}

/// Platforms without a native capture adapter reject viewport mode before changing the page.
pub fn supported() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("Native screenshot and fixed viewport capture are unavailable in this Keel build. Keel did not switch browsers.".into())
    }
}

/// Snapshot only the exact lane's webview, on the native main thread, with a finite callback wait.
#[cfg(target_os = "macos")]
pub async fn capture(view: &tauri::Webview) -> Result<(Image, u32, u32), String> {
    use block2::RcBlock;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSError;
    use objc2_web_kit::WKWebView;
    let size = view.size().map_err(|e| e.to_string())?;
    if u64::from(size.width) * u64::from(size.height) > MAX_PIXELS {
        return Err(
            "Screenshot exceeds 16,777,216 physical pixels. Use a smaller viewport.".into(),
        );
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    view.with_webview(move |platform| {
        let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(tx) = super::locked(&tx).take() else {
                return;
            };
            if tx.is_closed() {
                return;
            }
            let result = if error.is_null() {
                unsafe { image.as_ref() }
                    .ok_or("The native snapshot returned no image.".into())
                    .and_then(encode)
            } else {
                Err("The native page snapshot failed. Retry browser_screenshot.".into())
            };
            let _ = tx.send(result);
        });
        // wry provides this live WKWebView on its main thread; WebKit retains the completion block.
        unsafe {
            (&*(platform.inner() as *const WKWebView))
                .takeSnapshotWithConfiguration_completionHandler(None, &block);
        }
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(CAPTURE_WAIT, rx)
        .await
        .map_err(|_| "Native screenshot timed out. Retry browser_screenshot.")?
        .map_err(|_| "The page closed during capture.")?
}

/// Unsupported platforms fail explicitly rather than returning an empty or stale image.
#[cfg(not(target_os = "macos"))]
pub async fn capture(_view: &tauri::Webview) -> Result<(Image, u32, u32), String> {
    supported()?;
    unreachable!()
}

/// Bound native pixels and encoded bytes before carrying them through Tauri and MCP.
#[cfg(target_os = "macos")]
fn encode(image: &objc2_app_kit::NSImage) -> Result<(Image, u32, u32), String> {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::{NSDataBase64EncodingOptions, NSDictionary};
    let tiff = image
        .TIFFRepresentation()
        .ok_or("Could not encode the native screenshot.")?;
    let bitmap =
        NSBitmapImageRep::imageRepWithData(&tiff).ok_or("Could not read the native screenshot.")?;
    let width = u32::try_from(bitmap.pixelsWide()).map_err(|_| "Invalid native image width.")?;
    let height = u32::try_from(bitmap.pixelsHigh()).map_err(|_| "Invalid native image height.")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("Screenshot exceeds 16,777,216 pixels. Use a smaller viewport.".into());
    }
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("Could not encode a PNG screenshot.")?;
    if png.len().div_ceil(3) * 4 > MAX_ENCODED_BYTES {
        return Err("Screenshot exceeds 6 MiB. Use a smaller viewport.".into());
    }
    Ok((
        Image {
            data: png
                .base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty())
                .to_string(),
            mime_type: "image/png".into(),
        },
        width,
        height,
    ))
}
