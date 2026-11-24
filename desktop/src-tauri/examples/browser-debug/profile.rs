//! Remove only the store observed from this disposable fixture after its owned webview closes.
use objc2::{
    class, msg_send,
    rc::{Allocated, Retained},
    runtime::AnyObject,
};
use objc2_foundation::NSString;
use std::sync::Mutex;

/// Browser-native removal scopes cleanup to the exact UUID returned by this fixture's view.
pub async fn remove(app: &tauri::AppHandle, identifier: String) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || unsafe {
        let text = NSString::from_str(&identifier);
        let allocated: Allocated<AnyObject> = msg_send![class!(NSUUID), alloc];
        let uuid: Option<Retained<AnyObject>> = msg_send![allocated, initWithUUIDString: &*text];
        let Some(uuid) = uuid else { let _ = tx.send(Err("Invalid fixture store UUID".into())); return; };
        let tx = Mutex::new(Some(tx));
        let done = block2::RcBlock::new(move |error: *mut AnyObject| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let result = if error.is_null() { Ok(()) } else { Err("Native fixture store removal failed".into()) };
                let _ = tx.send(result);
            }
        });
        let _: () = msg_send![class!(WKWebsiteDataStore), removeDataStoreForIdentifier: &*uuid, completionHandler: &*done];
    }).map_err(|e| e.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Fixture store cleanup timed out".to_string())?
        .map_err(|_| "Fixture store cleanup closed".to_string())?
}
