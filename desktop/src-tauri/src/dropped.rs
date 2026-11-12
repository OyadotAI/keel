//! Files the person dropped on the window, and the one way the page may read them.
//!
//! A drop reaches the page as paths, not as files, and a chat lane used to keep the path: nothing
//! was uploaded, so the chat had no image to draw and Antigravity never heard of the file. The
//! page cannot read a path itself, and a command that read any path it was given would hand every
//! file on the machine to whatever runs in the main page. So the app remembers what was last
//! dropped and reads only that.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The daemon's own attachment ceiling (`/api/attach`), refused here before the bytes cross IPC.
const LIMIT: u64 = 10 * 1024 * 1024;
const LIMIT_WORDS: &str = "10 MB";

/// The paths of the most recent drop on the main webview. A newer drop replaces them.
#[derive(Default)]
pub struct Dropped(Mutex<Vec<PathBuf>>);

/// Note a drop on the main page; nothing else changes what may be read. The preview and the
/// Browser tab load other people's pages and are not listened to.
pub fn record(webview: &tauri::Webview, event: &tauri::WebviewEvent) {
    use tauri::Manager;
    if webview.label() != "main" {
        return;
    }
    if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
        let state = webview.state::<Dropped>();
        *state.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = paths.clone();
    }
}

/// The file's name, for a sentence the person reads.
fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Whether `path` may be read: it was in the last drop, it is a file, and it fits the limit.
fn allowed(dropped: &[PathBuf], path: &Path) -> Result<(), String> {
    if !dropped.iter().any(|d| d == path) {
        return Err(format!("Could not attach {}: it is not a file you just dropped.", name(path)));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("Could not attach {}: {e}", name(path)))?;
    if meta.is_dir() {
        return Err("Folders can't be attached. Drop the files inside, or type the folder's path.".into());
    }
    if !meta.is_file() {
        return Err(format!("Could not attach {}: it is not a file.", name(path)));
    }
    if meta.len() > LIMIT {
        return Err(format!("{} exceeds the {LIMIT_WORDS} attachment limit.", name(path)));
    }
    Ok(())
}

/// The bytes of a file from the last drop, for the page to upload like a pasted one.
#[tauri::command]
pub async fn read_dropped(path: String, dropped: tauri::State<'_, Dropped>) -> Result<tauri::ipc::Response, String> {
    let path = PathBuf::from(path);
    let last = dropped.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    // A read of up to 10 MB: off the event loop.
    let (read, path) = tauri::async_runtime::spawn_blocking(move || (allowed(&last, &path).and_then(|()| read(&path)), path))
        .await
        .map_err(|e| e.to_string())?;
    // Read once: a path stays a path, and what it names can change after the drop.
    if read.is_ok() {
        dropped.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|d| d != &path);
    }
    read.map(tauri::ipc::Response::new)
}

/// The bytes, bounded by what is actually read rather than by an earlier `stat`: the path can be
/// swapped for a larger file between the check and the read.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let failed = |e: std::io::Error| format!("Could not attach {}: {e}", name(path));
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(failed)?.take(LIMIT + 1).read_to_end(&mut bytes).map_err(failed)?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!("{} exceeds the {LIMIT_WORDS} attachment limit.", name(path)));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_dropped_path_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let (shot, other) = (dir.path().join("shot.png"), dir.path().join("secret.txt"));
        std::fs::write(&shot, b"png").unwrap();
        std::fs::write(&other, b"no").unwrap();
        let dropped = vec![shot.clone(), dir.path().to_path_buf()];
        assert!(allowed(&dropped, &shot).is_ok());
        assert!(allowed(&dropped, &other).unwrap_err().contains("not a file you just dropped"));
        assert!(allowed(&[], &shot).is_err());
        assert_eq!(read(&shot).unwrap(), b"png");
    }

    #[test]
    fn a_folder_and_a_file_past_the_limit_say_why() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.bin");
        std::fs::File::create(&big).unwrap().set_len(LIMIT + 1).unwrap();
        let dropped = vec![dir.path().to_path_buf(), big.clone()];
        assert!(allowed(&dropped, dir.path()).unwrap_err().starts_with("Folders can't be attached."));
        assert_eq!(allowed(&dropped, &big).unwrap_err(), "big.bin exceeds the 10 MB attachment limit.");
    }
}
