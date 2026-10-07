//! Durable, daemon-owned chat delivery. A browser subscribes; it never owns the agent.
//! A receipt is persisted before launch. Repeating that receipt cannot start a second turn,
//! including after a daemon restart. Disconnects only detach the event reader.

use crate::{agent::ChatQuery, lock::Locked, serve::AppState};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    convert::Infallible,
    io::Write,
    sync::{Arc, Mutex},
};
use tokio_stream::wrappers::ReceiverStream;

type Error = (StatusCode, String);
type Journal = Arc<Mutex<History>>;
#[derive(Default)]
pub(crate) struct Chats(Mutex<HashMap<String, Journal>>);

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    seq: usize,
    event: String,
    data: Value,
}

struct History {
    file: std::fs::File,
    records: Vec<Record>,
    running: bool,
}

impl History {
    fn seed(&mut self, state: &AppState, q: &ChatQuery, refresh: bool) -> Result<(), String> {
        if self.running
            || (!refresh && !self.records.is_empty())
            || q.provider.as_deref() == Some("codex")
        {
            return Ok(());
        }
        let (Some(session), Some(home)) = (&q.session, keel_workspace::claude_home()) else {
            return Ok(());
        };
        let cwd = state.checkout(q.wt.as_deref())?;
        let Some(path) = keel_workspace::transcript_path(&cwd, &home, session) else {
            return Err("The original conversation could not be found. Open it from History before resuming.".into());
        };
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let fingerprint = format!("{:x}", Sha256::digest(&bytes));
        if self
            .records
            .iter()
            .rev()
            .find(|r| r.event == "snapshot")
            .is_some_and(|r| r.data["fingerprint"] == fingerprint)
        {
            return Ok(());
        }
        use keel_workspace::conversation::{Decoder, Ending, Provider, Source};
        use std::io::BufRead;
        let mut decoder = Decoder::new(Provider::Claude, Source::Transcript);
        let file = bytes.as_slice();
        let mut frames = Vec::new();
        for line in std::io::BufReader::new(file).lines() {
            frames.extend(decoder.feed(
                &line.map_err(|e| e.to_string())?,
                None,
                std::time::Instant::now(),
            ));
        }
        frames.extend(decoder.close(Ending::Done));
        if !frames.is_empty() {
            self.push(
                "snapshot",
                json!({"frames": frames, "fingerprint": fingerprint}),
            )?;
        }
        self.push("idle", Value::Null)?;
        Ok(())
    }
    fn push(&mut self, event: &str, data: Value) -> Result<(), String> {
        let record = Record {
            seq: self.records.len() + 1,
            event: event.into(),
            data,
        };
        let mut bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("Could not save the conversation: {e}"))?;
        // Receipts must survive a crash before the process is launched.
        if event == "accepted" {
            self.file.sync_data().map_err(|e| e.to_string())?;
        }
        self.records.push(record);
        Ok(())
    }

    fn accept(&mut self, id: &str, fingerprint: &str) -> Result<bool, Error> {
        if let Some(old) = self
            .records
            .iter()
            .find(|r| r.event == "accepted" && r.data["id"] == id)
        {
            return if old.data["fingerprint"] == fingerprint {
                Ok(false)
            } else {
                Err((
                    StatusCode::CONFLICT,
                    "That submission id was already used for a different message.".into(),
                ))
            };
        }
        if self.running {
            return Err((
                StatusCode::CONFLICT,
                "This lane is still running. Queue the message or stop the current turn.".into(),
            ));
        }
        self.push("accepted", json!({"id": id, "fingerprint": fingerprint}))
            .map_err(internal)?;
        self.running = true;
        Ok(true)
    }
}

fn internal(e: impl ToString) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn safe(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn history(state: &AppState, lane: &str) -> Result<Journal, Error> {
    if !safe(lane) {
        return Err((StatusCode::BAD_REQUEST, "Invalid lane id".into()));
    }
    let repo = state.repo();
    let key = format!("{repo}/{lane}");
    let mut all = state.chats.0.locked();
    if let Some(history) = all.get(&key) {
        return Ok(history.clone());
    }
    // Outside the checkout: conversation text must never become a changed file or commit.
    let dir = crate::permissions::permissions_dir()
        .map_err(internal)?
        .join("conversations");
    std::fs::create_dir_all(&dir).map_err(internal)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(internal)?;
    }
    let path = dir.join(format!("{:x}.jsonl", Sha256::digest(key.as_bytes())));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(internal(e)),
    };
    // Only a partial final write can be discarded. Corrupt complete records are an error,
    // never silently interpreted as an empty conversation.
    let complete = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |at| at + 1);
    let records: Vec<Record> = bytes[..complete]
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()
        .map_err(internal)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(internal)?;
    file.set_len(complete as u64).map_err(internal)?;
    let mut h = History {
        file,
        records,
        running: false,
    };
    if h.records.last().is_some_and(|r| r.event != "idle") {
        h.push(
            "turn",
            json!([{"turn": null, "op": "close", "reason": "interrupted"}]),
        )
        .map_err(internal)?;
        h.push("fatal", json!("The daemon restarted during this turn. Review the partial result before sending again.")).map_err(internal)?;
        h.push("idle", Value::Null).map_err(internal)?;
    }
    let h = Arc::new(Mutex::new(h));
    all.insert(key, h.clone());
    Ok(h)
}

#[derive(Deserialize)]
pub struct Send {
    id: String,
    #[serde(flatten)]
    query: ChatQuery,
}

pub async fn send(
    State(state): State<Arc<AppState>>,
    Json(mut request): Json<Send>,
) -> Result<Json<Value>, Error> {
    let lane = request
        .query
        .lane
        .clone()
        .ok_or((StatusCode::BAD_REQUEST, "A lane is required".into()))?;
    if !safe(&request.id) || request.query.prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "A submission id and message are required".into(),
        ));
    }
    request.query.ops = Some("1".into());
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&request.query).map_err(internal)?)
    );
    let owner = state.clone();
    let h = tokio::task::spawn_blocking(move || history(&owner, &lane))
        .await
        .map_err(internal)??;
    let receipt = request.id.clone();
    let held = h.clone();
    let seed = request.query.clone();
    let owner = state.clone();
    let accepted = tokio::task::spawn_blocking(move || {
        let mut h = held.locked();
        h.seed(&owner, &seed, false).map_err(internal)?;
        h.accept(&receipt, &fingerprint)
    })
    .await
    .map_err(internal)??;
    if accepted {
        tokio::spawn(async move {
            // Keep consuming after every subscriber disappears. The existing runner still owns
            // checkout locking, cancellation, checkpoints and verification.
            let response = crate::runtime::chat(state, request.query).await;
            let mut stream = response.into_body().into_data_stream();
            let mut parser = Parser::default();
            let mut failure = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        let events = parser.feed(&bytes);
                        let held = h.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let mut h = held.locked();
                            for (name, data) in events {
                                h.push(&name, data)?;
                            }
                            Ok::<_, String>(())
                        })
                        .await;
                        if !matches!(result, Ok(Ok(()))) {
                            failure = Some("Could not persist the agent's output.");
                            break;
                        }
                    }
                    Err(_) => {
                        failure = Some("The agent stream ended unexpectedly.");
                        break;
                    }
                }
            }
            let _ = tokio::task::spawn_blocking(move || {
                let mut h = h.locked();
                if let Some(error) = failure {
                    let _ = h.push("fatal", json!(error));
                }
                let _ = h.push("idle", Value::Null);
                h.running = false;
            })
            .await;
        });
    }
    Ok(Json(json!({"id": request.id, "accepted": accepted})))
}

#[derive(Deserialize)]
pub struct Listen {
    lane: String,
    #[serde(default)]
    after: usize,
    session: Option<String>,
    provider: Option<String>,
    wt: Option<String>,
}

pub async fn events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<Listen>,
) -> Result<axum::response::Response, Error> {
    let h = tokio::task::spawn_blocking(move || {
        let h = history(&state, &q.lane)?;
        if q.session.is_some() && q.provider.as_deref() != Some("codex") {
            let query = serde_json::from_value::<ChatQuery>(
                json!({"prompt":"", "session": q.session, "provider":q.provider, "wt":q.wt}),
            )
            .map_err(internal)?;
            h.locked().seed(&state, &query, true).map_err(internal)?;
        }
        Ok::<_, Error>(h)
    })
    .await
    .map_err(internal)??;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);
    tokio::spawn(async move {
        let mut after = q.after;
        let mut first = true;
        loop {
            let (batch, running) = {
                let h = h.locked();
                (
                    h.records
                        .iter()
                        .skip(after)
                        .take(128)
                        .cloned()
                        .collect::<Vec<_>>(),
                    h.running,
                )
            };
            let full = batch.len() == 128;
            for record in batch {
                after = record.seq;
                if tx
                    .send(Ok(Event::default()
                        .event("record")
                        .json_data(record)
                        .expect("serializable record")))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if full {
                continue;
            }
            if first {
                first = false;
                if tx
                    .send(Ok(Event::default()
                        .event("caught-up")
                        .json_data(json!({"running": running}))
                        .expect("status")))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            tokio::select! { _ = tx.closed() => return, _ = tokio::time::sleep(std::time::Duration::from_millis(32)) => {} }
        }
    });
    Ok(Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[derive(Default)]
struct Parser {
    bytes: Vec<u8>,
}
impl Parser {
    fn feed(&mut self, chunk: &[u8]) -> Vec<(String, Value)> {
        self.bytes.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(at) = self.bytes.windows(2).position(|w| w == b"\n\n") {
            let record = String::from_utf8_lossy(&self.bytes[..at]);
            let name = record
                .lines()
                .find_map(|l| l.strip_prefix("event:"))
                .map(str::trim);
            let data = record
                .lines()
                .filter_map(|l| {
                    l.strip_prefix("data:")
                        .map(|s| s.strip_prefix(' ').unwrap_or(s))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(name) = name {
                out.push((
                    name.into(),
                    serde_json::from_str(&data).unwrap_or(Value::String(data)),
                ));
            }
            self.bytes.drain(..at + 2);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_is_idempotent_and_survives_reopening() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "lane-one").unwrap();
        assert!(h.locked().accept("send-one", "prompt-a").unwrap());
        assert!(!h.locked().accept("send-one", "prompt-a").unwrap());
        assert_eq!(
            h.locked().accept("send-one", "prompt-b").unwrap_err().0,
            StatusCode::CONFLICT
        );
        assert!(h.locked().accept("send-two", "prompt-b").is_err());
        state.chats.0.locked().clear();
        let h = history(&state, "lane-one").unwrap();
        assert!(!h.locked().running);
        assert!(!h.locked().accept("send-one", "prompt-a").unwrap());
        assert!(h.locked().records.iter().any(|e| e.event == "fatal"));
    }
    #[test]
    fn sse_handles_split_unicode_multiline_data_and_heartbeats() {
        let mut p = Parser::default();
        let bytes = "event: turn\ndata: {\ndata: \"text\":\"こんにちは\"}\n\n: ping\n\nevent: done\ndata: 0\n\n".as_bytes();
        let out: Vec<_> = bytes.iter().flat_map(|b| p.feed(&[*b])).collect();
        assert_eq!(
            out,
            vec![
                ("turn".into(), json!({"text":"こんにちは"})),
                ("done".into(), json!(0))
            ]
        );
    }
    #[test]
    fn lane_ids_cannot_escape_storage() {
        for id in ["", "..", "a/b", "a\\b", "/tmp", "hello world"] {
            assert!(!safe(id));
        }
    }
}
