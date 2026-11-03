//! Bounded, metadata-only provider configuration inspection supports guidance and verified local integrations.
use crate::signals::Leads;
use camino::Utf8Path;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
const WAIT: Duration = Duration::from_secs(5);
const BYTES: u64 = 1024 * 1024;
const QUEUE: usize = 8;

pub(crate) fn codex(cwd: &Utf8Path) -> Result<Value, String> {
    read(crate::permissions::program("codex"), cwd)
}
fn read(program: impl AsRef<std::ffi::OsStr>, cwd: &Utf8Path) -> Result<Value, String> {
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
        let mut input = child.stdin.take().ok_or("Codex stdin unavailable")?;
        let output = child.stdout.take().ok_or("Codex stdout unavailable")?;
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let reader =
            std::thread::spawn(move || {
                for line in BufReader::new(output.take(BYTES)).lines() {
                    if tx
                        .send(line.map_err(|e| e.to_string()).and_then(|s| {
                            serde_json::from_str::<Value>(&s).map_err(|e| e.to_string())
                        }))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        let result = (|| {
            let deadline = Instant::now() + WAIT;
            send(
                &mut input,
                json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"keel","version":env!("CARGO_PKG_VERSION")}}}),
            )?;
            reply(&rx, 1, deadline)?;
            send(&mut input, json!({"method":"initialized","params":{}}))?;
            send(
                &mut input,
                json!({"id":2,"method":"config/read","params":{"cwd":cwd,"includeLayers":false}}),
            )?;
            let result = reply(&rx, 2, deadline)?;
            result
                .get("config")
                .filter(|v| v.is_object())
                .cloned()
                .ok_or("Codex did not report effective configuration.".into())
        })();
        drop(input);
        drop(rx);
        crate::signals::end_tree(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        result
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
fn send(input: &mut impl Write, value: Value) -> Result<(), String> {
    writeln!(input, "{value}").map_err(|e| e.to_string())
}
fn reply(
    rx: &mpsc::Receiver<Result<Value, String>>,
    id: u64,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let wait = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Codex instruction metadata timed out")?;
        let message = rx
            .recv_timeout(wait)
            .map_err(|_| "Codex instruction metadata disconnected")??;
        if message["id"].as_u64() != Some(id) {
            continue;
        }
        if message.get("error").is_some() {
            return Err("Codex rejected instruction metadata inspection.".into());
        }
        return Ok(message["result"].clone());
    }
}
