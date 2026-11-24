//! Native requests use bounded IDs, immutable view generations and cancellation before UI effects.
use super::super::locked;
use serde_json::Value;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
type Callback = extern "C" fn(*mut c_void, *const c_char, *const c_char);
type Sender = tokio::sync::oneshot::Sender<Result<Value, String>>;
unsafe extern "C" {
    fn keel_debug_install(
        view: *mut c_void,
        key: *const c_char,
        runtime: *const c_char,
        bootstrap: *const c_char,
    );
    fn keel_debug_presentation(
        view: *mut c_void,
        key: *const c_char,
        reply: Callback,
        context: *mut c_void,
    );
    fn keel_debug_frames(
        view: *mut c_void,
        key: *const c_char,
        reply: Callback,
        context: *mut c_void,
    );
    fn keel_debug_identity(
        view: *mut c_void,
        key: *const c_char,
        reply: Callback,
        context: *mut c_void,
    );
    fn keel_debug_run(
        view: *mut c_void,
        key: *const c_char,
        reference: *const c_char,
        source: *const c_char,
        page: bool,
        asynchronous: bool,
        reply: Callback,
        context: *mut c_void,
    );
    fn keel_debug_mouse(
        view: *mut c_void,
        key: *const c_char,
        reference: *const c_char,
        document: *const c_char,
        x: f64,
        y: f64,
        kind: i32,
        reply: Callback,
        context: *mut c_void,
    );
    fn keel_debug_parse(body: *const c_char) -> bool;
    fn keel_debug_forget(key: *const c_char);
}
use super::constants::{EVAL_WAIT, MAX_NATIVE_JSON as MAX_JSON, MAX_PENDING, REQUEST_WAIT as WAIT};
struct Pending {
    sender: Sender,
    deadline: Instant,
    label: String,
    generation: String,
}
fn requests() -> &'static Mutex<HashMap<usize, Pending>> {
    static MAP: OnceLock<Mutex<HashMap<usize, Pending>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}
fn generations() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}
struct Waiting(usize);
impl Drop for Waiting {
    fn drop(&mut self) {
        locked(requests()).remove(&self.0);
    }
}
fn c(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| "A diagnostic string contains a NUL byte.".into())
}
fn generation(label: &str) -> Result<String, String> {
    locked(generations())
        .get(label)
        .cloned()
        .ok_or("The owned native browser generation closed.".into())
}
fn admitted(id: usize) -> bool {
    locked(requests()).get(&id).is_some_and(|p| {
        !p.sender.is_closed()
            && Instant::now() < p.deadline
            && generation(&p.label).ok().as_ref() == Some(&p.generation)
    })
}
extern "C" fn receive(context: *mut c_void, value: *const c_char, error: *const c_char) {
    let Some(pending) = locked(requests()).remove(&(context as usize)) else {
        return;
    };
    if pending.sender.is_closed() || Instant::now() >= pending.deadline {
        return;
    }
    let result = unsafe {
        if !error.is_null() {
            Err(CStr::from_ptr(error).to_string_lossy().into_owned())
        } else if value.is_null() {
            Err("The native frame returned no result.".into())
        } else {
            let bytes = CStr::from_ptr(value).to_bytes();
            if bytes.len() > MAX_JSON {
                Err("The native result exceeded 256 KiB.".into())
            } else {
                serde_json::from_slice(bytes)
                    .map_err(|_| "The native frame returned invalid JSON.".into())
            }
        }
    };
    let _ = pending.sender.send(result);
}
fn register(
    view: &tauri::Webview,
    wait: Duration,
) -> Result<
    (
        Waiting,
        tokio::sync::oneshot::Receiver<Result<Value, String>>,
    ),
    String,
> {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let generation = generation(view.label())?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return Err("Native request IDs exhausted.".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut requests = locked(requests());
    if requests.len() >= MAX_PENDING {
        return Err("Native inspection capacity exceeded.".into());
    }
    requests.insert(
        id,
        Pending {
            sender: tx,
            deadline: Instant::now() + wait,
            label: view.label().into(),
            generation,
        },
    );
    Ok((Waiting(id), rx))
}
/// Initialization installs scripts before navigation and cannot run after its awaiting call expires.
pub async fn install(view: &tauri::Webview) -> Result<(), String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let id = super::super::hex(random);
    let key = c(&id)?;
    let runtime = c(include_str!("../debug-runtime.js"))?;
    let bootstrap = c(include_str!("page-world.js"))?;
    locked(generations()).insert(view.label().into(), id);
    let (waiting, rx) = register(view, WAIT)?;
    let request = waiting.0;
    view.with_webview(move |platform| {
        if !admitted(request) {
            return;
        }
        unsafe {
            keel_debug_install(
                platform.inner(),
                key.as_ptr(),
                runtime.as_ptr(),
                bootstrap.as_ptr(),
            );
            receive(request as *mut c_void, c"null".as_ptr(), std::ptr::null());
        }
    })
    .map_err(|e| e.to_string())?;
    await_reply(rx, WAIT, false).await?;
    Ok(())
}
async fn await_reply(
    rx: tokio::sync::oneshot::Receiver<Result<Value, String>>,
    wait: Duration,
    eval: bool,
) -> Result<Value, String> {
    tokio::time::timeout(wait, rx)
        .await
        .map_err(|_| {
            if eval {
                "JavaScript timed out. The expression may have run; inspect before retrying."
            } else {
                "The native frame did not answer before its deadline."
            }
        })?
        .map_err(|_| "The native browser closed.".to_string())?
}
/// A timeout or dropped future removes its ID; late callbacks neither retain Rust state nor execute queued work.
pub async fn call(
    view: &tauri::Webview,
    kind: &str,
    reference: &str,
    source: &str,
    page: bool,
) -> Result<Value, String> {
    let key = c(&generation(view.label())?)?;
    let reference = c(reference)?;
    let source = c(source)?;
    let kind = kind.to_string();
    let wait = if page { EVAL_WAIT } else { WAIT };
    let (waiting, rx) = register(view, wait)?;
    let request = waiting.0;
    view.with_webview(move |platform| unsafe {
        if !admitted(request) {
            return;
        }
        let pointer = platform.inner();
        let context = request as *mut c_void;
        match kind.as_str() {
            "presentation" => keel_debug_presentation(pointer, key.as_ptr(), receive, context),
            "frames" => keel_debug_frames(pointer, key.as_ptr(), receive, context),
            "identity" => keel_debug_identity(pointer, key.as_ptr(), receive, context),
            _ => keel_debug_run(
                pointer,
                key.as_ptr(),
                reference.as_ptr(),
                source.as_ptr(),
                page,
                !page,
                receive,
                context,
            ),
        }
    })
    .map_err(|e| e.to_string())?;
    await_reply(rx, wait, page).await
}
/// Native input validates the exact host generation and document immediately before dispatch.
pub async fn mouse(
    view: &tauri::Webview,
    frame: &super::frames::Frame,
    x: f64,
    y: f64,
    kind: i32,
) -> Result<Value, String> {
    let label = view.label().to_string();
    let key = c(&generation(view.label())?)?;
    let reference = c(&frame.reference)?;
    let document = c(&frame.document)?;
    let (waiting, rx) = register(view, WAIT)?;
    let request = waiting.0;
    view.with_webview(move |platform| unsafe {
        if !admitted(request) {
            return;
        }
        if super::super::viewport::input_reason(&label).is_some() {
            receive(
                request as *mut c_void,
                std::ptr::null(),
                c"Native presentation changed. No further input was sent.".as_ptr(),
            );
            return;
        }
        keel_debug_mouse(
            platform.inner(),
            key.as_ptr(),
            reference.as_ptr(),
            document.as_ptr(),
            x,
            y,
            kind,
            receive,
            request as *mut c_void,
        )
    })
    .map_err(|e| e.to_string())?;
    await_reply(rx, WAIT, false)
        .await
        .map_err(|why| format!("{why} Native input may have been sent; inspect before retrying."))
}
/// Parse in a fresh JavaScriptCore context without invoking the payload or page-controlled functions.
pub fn parse(body: &str) -> Result<(), String> {
    if unsafe { keel_debug_parse(c(body)?.as_ptr()) } {
        Ok(())
    } else {
        Err("Invalid JavaScript expression. It was not run.".into())
    }
}
/// Removing the generation first revokes queued effects even before the native close reaches its UI thread.
pub fn forget(view: &tauri::Webview) {
    if let Some(key) = locked(generations())
        .remove(view.label())
        .and_then(|key| c(&key).ok())
    {
        let _ = view.with_webview(move |_| unsafe { keel_debug_forget(key.as_ptr()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(
        label: &str,
        deadline: Instant,
    ) -> (
        Waiting,
        tokio::sync::oneshot::Receiver<Result<Value, String>>,
    ) {
        static NEXT_TEST: AtomicUsize = AtomicUsize::new(usize::MAX / 2);
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        locked(generations()).insert(label.into(), "first-native-view".into());
        locked(requests()).insert(
            id,
            Pending {
                sender: tx,
                deadline,
                label: label.into(),
                generation: "first-native-view".into(),
            },
        );
        (Waiting(id), rx)
    }
    #[test]
    fn a_dropped_receiver_cannot_dispatch_queued_native_effects() {
        let (waiting, rx) = request("cancelled-native-fixture", Instant::now() + WAIT);
        assert!(admitted(waiting.0));
        drop(rx);
        assert!(!admitted(waiting.0));
    }
    #[test]
    fn a_deadline_rejects_work_even_before_the_awaiter_is_polled() {
        let (waiting, _rx) = request("expired-native-fixture", Instant::now() - WAIT);
        assert!(!admitted(waiting.0));
    }
    #[test]
    fn recreating_a_label_cannot_reauthorize_the_previous_native_view() {
        let (waiting, _rx) = request("replacement-native-fixture", Instant::now() + WAIT);
        locked(generations()).insert(
            "replacement-native-fixture".into(),
            "second-native-view".into(),
        );
        assert!(!admitted(waiting.0));
    }
    #[test]
    fn a_late_callback_uses_an_id_without_dereferencing_expired_memory() {
        let (waiting, mut rx) = request("late-native-fixture", Instant::now() + WAIT);
        let id = waiting.0;
        drop(waiting);
        receive(id as *mut c_void, c"1".as_ptr(), std::ptr::null());
        assert!(rx.try_recv().is_err());
        assert!(!admitted(id));
    }
}
