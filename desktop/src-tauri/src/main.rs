//! The desktop shell: a window, and one `keel serve` per open project.
//!
//! Everything that knows anything stays in the daemon. This process only starts daemons, keeps
//! them tied to its own life, and hands the page their ports. One daemon per project rather than
//! one for all of them: a daemon's state — the lane claims, the dev server, the watchers — is one
//! project's, and running several side by side changes nothing inside any of them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod preview;

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A running daemon. Its stdin is the tie: when this process goes — quit, crash, force-kill — the
/// pipe closes, the daemon sees end of file (`--exit-on-stdin-eof`) and stops everything it owns.
/// On every platform, which `getppid` was not.
struct Daemon {
    port: u16,
    token: String,
    child: Child,
    /// The write end of the daemon's stdin; dropping it is what tells the daemon to exit.
    _tie: Option<ChildStdin>,
}

#[derive(Default)]
struct Daemons(Mutex<HashMap<String, Daemon>>, Mutex<HashMap<String, Arc<Mutex<()>>>>);

impl Daemons {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Daemon>> {
        // A panic while holding this leaves a map that is still correct; carry on with it.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One start at a time per project. Two at once spawned two daemons, and the second's insert
    /// dropped the first — closing its stdin, so it exited under whoever was using it.
    fn gate(&self, path: &str) -> Arc<Mutex<()>> {
        let mut gates = self.1.lock().unwrap_or_else(|e| e.into_inner());
        gates.entry(path.to_string()).or_default().clone()
    }
}

/// What the page needs to talk to a project's daemon.
#[derive(serde::Serialize, Clone)]
struct Endpoint {
    port: u16,
    /// Sent as `Authorization: Bearer`. The webview's origin is shared by every Tauri app on the
    /// machine, so the daemon lets a page in only with this beside it (`pair::app_token`).
    token: String,
}

/// 32 random bytes, hex. Written to the daemon's stdin — never argv, env or a URL.
fn token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("No randomness for the daemon's token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// How long a daemon may take to start listening before the page is told it could not.
const START: Duration = Duration::from_secs(15);

fn keel_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("KEEL_BIN") {
        return Ok(PathBuf::from(path));
    }
    let name = if cfg!(windows) { "keel.exe" } else { "keel" };
    // A development build uses the workspace's own daemon first: the copy staged beside this
    // executable is a snapshot from the last `make dev`, and goes stale with every daemon rebuild.
    if cfg!(debug_assertions) {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug").join(name);
        if dev.exists() {
            return Ok(dev);
        }
    }
    // Bundled beside this executable (Tauri's `externalBin`).
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        let beside = dir.join(name);
        if beside.exists() {
            return Ok(beside);
        }
    }
    Err(format!("Could not find the `{name}` daemon next to the app."))
}

fn free_port() -> Result<u16, String> {
    // The port is free when asked and handed straight to the daemon; another process taking it
    // in between fails the start with a message, which beats guessing a fixed range.
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("No free port for the daemon: {e}"))
}

/// Start a project's daemon, or return the one already running. Blocks until it accepts
/// connections, on a thread of its own (`async`), never the window's.
#[tauri::command(async)]
fn open_project(path: String, daemons: tauri::State<'_, Daemons>) -> Result<Endpoint, String> {
    let gate = daemons.gate(&path);
    let _starting = gate.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut all = daemons.lock();
        if let Some(d) = all.get_mut(&path) {
            if matches!(d.child.try_wait(), Ok(None)) {
                return Ok(Endpoint { port: d.port, token: d.token.clone() });
            }
            all.remove(&path);
        }
    }
    let port = free_port()?;
    let mut command = Command::new(keel_binary()?);
    command
        .args(["serve", &path, "--port", &port.to_string(), "--no-open", "--exit-on-stdin-eof"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(if cfg!(debug_assertions) { Stdio::inherit() } else { Stdio::null() });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up behind the app for a background process.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("Could not start the daemon: {e}"))?;
    let mut tie = child.stdin.take().ok_or("The daemon has no stdin to tie to.")?;
    let token = token()?;
    {
        use std::io::Write;
        // First, before any request: until it has this, the daemon refuses the page.
        writeln!(tie, "{token}")
            .and_then(|_| tie.flush())
            .map_err(|e| format!("Could not hand the daemon its token: {e}"))?;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + START;
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("The daemon for this project exited ({status}) before it started."));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The daemon did not start within 15 seconds.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    daemons.lock().insert(path, Daemon { port, token: token.clone(), child, _tie: Some(tie) });
    Ok(Endpoint { port, token })
}

/// Stop a project's daemon: closing its stdin is the shutdown it already handles, and it takes
/// its dev server and background jobs with it.
#[tauri::command(async)]
async fn close_project(path: String, daemons: tauri::State<'_, Daemons>) -> Result<(), ()> {
    // Async, so the wait below is on a worker and not the main thread the window draws on.
    // After any start in flight, so a close never races past the daemon it was meant to stop.
    let gate = daemons.gate(&path);
    let _starting = gate.lock().unwrap_or_else(|e| e.into_inner());
    let Some(d) = daemons.lock().remove(&path) else { return Ok(()) };
    stop(vec![d]);
    Ok(())
}

/// Every daemon, before an update installs. The installer replaces `keel` beside the app, and on
/// Windows a file a process is running cannot be written: an update clicked with a project open
/// raced the daemons' own shutdown and could end half-installed. All asked at once, so the wait
/// is the slowest one rather than the sum.
#[tauri::command]
async fn stop_all(daemons: tauri::State<'_, Daemons>) -> Result<(), ()> {
    let all: Vec<Daemon> = daemons.lock().drain().map(|(_, d)| d).collect();
    stop(all);
    Ok(())
}

/// Let go of each tie (the daemon exits on stdin EOF), give them three seconds, then kill and reap
/// what is left.
fn stop(mut all: Vec<Daemon>) {
    for d in &mut all {
        d._tie.take();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && all.iter_mut().any(|d| matches!(d.child.try_wait(), Ok(None))) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for d in &mut all {
        let _ = d.child.kill();
        let _ = d.child.wait();
    }
}

/// An empty folder for a daemon with no project yet — the one that makes a new project or clones
/// one, and then hands over to a daemon on the folder it made.
///
/// A new one per use, made atomically with a random name and owner-only permissions. One name per
/// process was shared by every use, so whichever finished first closed the daemon under the
/// others, and a predictable name in a shared temp directory is one another user can take first.
#[tauri::command]
fn scratch_dir() -> Result<String, String> {
    let dir = tempfile::Builder::new()
        .prefix("keel-scratch-")
        .tempdir()
        .map_err(|e| e.to_string())?
        .keep();
    dir.to_str().map(str::to_string).ok_or_else(|| "The temp folder's path is not text.".into())
}

/// The device's review-packet key: made once, kept in the system's own credential store
/// (Keychain, Windows Credential Manager), never in the page or on disk.
fn signing_key() -> Result<ed25519_dalek::SigningKey, String> {
    let entry = keyring::Entry::new("ai.oya.keel.review-packet", "desktop-signing-key-v1").map_err(|e| e.to_string())?;
    let hex = match entry.get_password() {
        Ok(hex) => hex,
        Err(keyring::Error::NoEntry) => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
            let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
            entry.set_password(&hex).map_err(|e| e.to_string())?;
            hex
        }
        Err(e) => return Err(format!("The credential store refused: {e}")),
    };
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    let seed: [u8; 32] = bytes.try_into().map_err(|_| "The stored signing key is damaged.".to_string())?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
}

#[derive(serde::Serialize)]
struct Signed {
    signature: String,
    public_key: String,
}

/// Sign a review packet's canonical JSON with this device's key. The page sends the bytes it
/// will save; anyone with the public key can check they were not changed since.
#[tauri::command]
fn sign_packet(payload: String) -> Result<Signed, String> {
    use ed25519_dalek::Signer;
    let key = signing_key()?;
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    Ok(Signed {
        signature: hex(&key.sign(payload.as_bytes()).to_bytes()),
        public_key: hex(key.verifying_key().as_bytes()),
    })
}

/// Save text where the person chooses, through the system's own save dialog. `false` when they
/// cancelled. Async so the dialog never blocks the window's event loop.
#[tauri::command(async)]
fn save_text(app: tauri::AppHandle, name: String, contents: String) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = app.dialog().file().set_file_name(&name).blocking_save_file() else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, contents).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Whether a dropped path is a folder: only a folder can be opened as a project.
#[tauri::command]
fn is_dir(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Daemons::default())
        .invoke_handler(tauri::generate_handler![open_project, close_project, scratch_dir, sign_packet, save_text, is_dir, stop_all, preview::preview_show, preview::preview_bounds, preview::preview_hide, preview::preview_send, preview::preview_msg])
        .build(tauri::generate_context!())
        .expect("the window")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                // Dropping each tie is enough; this just does it before the process unwinds.
                app.state::<Daemons>().lock().clear();
            }
        });
}
