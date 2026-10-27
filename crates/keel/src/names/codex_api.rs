//! Bounded metadata-only Codex App Server calls; never start a thread or a model turn to rename.
use crate::signals::Leads;
use camino::Utf8Path;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: u64 = 1024 * 1024;
const REPLY_QUEUE: usize = 8;

/// Use Codex's supported persistence API, then verify its native name without resuming work.
pub(super) fn rename(cwd: &Utf8Path, id: &str, name: &str) -> Result<(), String> {
    rename_program(
        crate::permissions::program("codex"),
        cwd,
        id,
        name,
        RPC_TIMEOUT,
    )
}

fn rename_program(
    program: impl AsRef<std::ffi::OsStr>,
    cwd: &Utf8Path,
    id: &str,
    name: &str,
    timeout: Duration,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(["app-server", "--stdio"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.lead_group();
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let _tracked = crate::runtime::Tracked::new(child.id());
    let result = (|| {
        let mut input = child.stdin.take().ok_or("Codex stdin is unavailable")?;
        let output = child.stdout.take().ok_or("Codex stdout is unavailable")?;
        let (tx, rx) = mpsc::sync_channel(REPLY_QUEUE);
        let reader =
            std::thread::spawn(move || {
                for line in BufReader::new(output.take(OUTPUT_LIMIT)).lines() {
                    if tx
                        .send(line.map_err(|e| e.to_string()).and_then(|l| {
                            serde_json::from_str::<Value>(&l).map_err(|e| e.to_string())
                        }))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        let deadline = Instant::now() + timeout;
        let result = (|| {
            send(
                &mut input,
                json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"keel","version":env!("CARGO_PKG_VERSION")}}}),
            )?;
            reply(&rx, 1, deadline)?;
            send(&mut input, json!({"method":"initialized","params":{}}))?;
            send(
                &mut input,
                json!({"id":2,"method":"thread/name/set","params":{"threadId":id,"name":name}}),
            )?;
            reply(&rx, 2, deadline)?;
            send(
                &mut input,
                json!({"id":3,"method":"thread/read","params":{"threadId":id,"includeTurns":false}}),
            )?;
            let result = reply(&rx, 3, deadline)?;
            if result["thread"]["name"].as_str() != Some(name) {
                return Err("Codex did not persist the requested name".into());
            }
            Ok(())
        })();
        drop(input);
        drop(rx);
        crate::signals::end_tree(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        result
    })();
    // Includes failures before the reader starts and setup errors in the closure.
    let _ = child.kill();
    let _ = child.wait();
    result
}
fn send(input: &mut impl Write, value: Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    input.write_all(&line).map_err(|e| e.to_string())
}
fn reply(
    rx: &mpsc::Receiver<Result<Value, String>>,
    id: u64,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let wait = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Codex naming timed out")?;
        let message = rx
            .recv_timeout(wait)
            .map_err(|_| "Codex naming timed out or disconnected")??;
        if message["id"].as_u64() != Some(id) || message.get("method").is_some() {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(error["message"]
                .as_str()
                .unwrap_or("Codex rejected the rename")
                .into());
        }
        return Ok(message["result"].clone());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(body: &str) -> (tempfile::TempDir, camino::Utf8PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = camino::Utf8Path::from_path(tmp.path())
            .unwrap()
            .join("codex-fixture");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (tmp, path)
    }
    #[test]
    fn renaming_uses_only_metadata_methods_and_verifies_the_native_result() {
        let (tmp, program) = executable(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> requests.jsonl
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"method":"thread/name/set"'*) printf '%s\n' '{"id":2,"result":{}}' ;;
    *'"method":"thread/read"'*) printf '%s\n' '{"id":3,"result":{"thread":{"name":"Saved"}}}' ;;
  esac
done
"#,
        );
        let cwd = Utf8Path::from_path(tmp.path()).unwrap();
        rename_program(
            program.as_str(),
            cwd,
            "session-1",
            "Saved",
            Duration::from_secs(2),
        )
        .unwrap();
        let log = std::fs::read_to_string(cwd.join("requests.jsonl")).unwrap();
        assert!(log.contains("thread/name/set"));
        assert!(log.contains("thread/read"));
        assert!(!log.contains("turn/start"));
        assert!(!log.contains("thread/start"));
        assert!(!log.contains("thread/resume"));
    }
    #[test]
    fn a_silent_native_process_times_out_and_is_reaped() {
        let (tmp, program) = executable("#!/bin/sh\nexec sleep 30\n");
        let start = Instant::now();
        assert!(
            rename_program(
                program.as_str(),
                Utf8Path::from_path(tmp.path()).unwrap(),
                "session-1",
                "Saved",
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
