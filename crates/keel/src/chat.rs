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
use serde_json::{Value, json, value::RawValue};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    convert::Infallible,
    io::Write,
    sync::{Arc, Mutex, Weak},
    time::SystemTime,
};
use tokio_stream::wrappers::ReceiverStream;

type Error = (StatusCode, String);
type Journal = Arc<Mutex<History>>;
/// Weak, so a lane nobody is running or watching gives its journal back. Every events stream
/// and every running turn holds the `Arc`; the journal on disk is the copy that lasts.
#[derive(Default)]
pub(crate) struct Chats(Mutex<HashMap<String, Weak<Mutex<History>>>>);

/// `data` stays the JSON text it arrived as. Parsed into `Value` a 58 MB journal was ~45k
/// `BTreeMap` nodes held for the life of the daemon; nothing here reads inside a record except
/// a receipt's id, which is parsed on demand.
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    seq: usize,
    event: String,
    data: Box<RawValue>,
}

struct History {
    file: std::fs::File,
    records: Vec<Record>,
    running: bool,
    /// The transcript's fingerprint as of the last `snapshot` or `seen` record.
    fingerprint: Option<String>,
    /// Length and mtime of the transcript when `fingerprint` was last checked, so a reconnect
    /// with nothing new does not read and hash the whole transcript again.
    stamp: Option<Vec<(u64, SystemTime)>>,
}

#[derive(Deserialize)]
struct Fingerprint {
    fingerprint: String,
}

use keel_workspace::{Follows as Agent, Transcript};

/// Length and mtime of every file whose change means the transcript moved. Antigravity writes
/// its write-ahead log before the database, so stamping the database alone missed turns taken
/// outside Keel until the next checkpoint.
fn stamp(t: &Transcript) -> Option<Vec<(u64, SystemTime)>> {
    let mut files = t.watched();
    if files.is_empty() {
        files.push(t.path().to_owned());
    }
    files
        .iter()
        .map(|f| {
            let meta = std::fs::metadata(f).ok();
            Some(meta.map_or((0, SystemTime::UNIX_EPOCH), |m| {
                (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH))
            }))
        })
        .collect()
}

impl History {
    fn seed(&mut self, state: &AppState, q: &ChatQuery, refresh: bool) -> Result<(), String> {
        if self.running || (!refresh && !self.records.is_empty()) {
            return Ok(());
        }
        match transcript(state, q)? {
            Some(path) => self.snapshot(&path),
            None => Ok(()),
        }
    }
    /// Append the transcript as a `snapshot` unless the journal already holds what it says.
    /// Every agent's, through the one reader that knows each format (`follow`): this was Claude's
    /// alone, so a Codex or Antigravity lane opened in chat showed nothing it had said before.
    fn snapshot(&mut self, t: &Transcript) -> Result<(), String> {
        let now = stamp(t);
        if now.is_some() && now == self.stamp {
            return Ok(());
        }
        let (lines, _, start) = t
            .tail_last(keel_workspace::MAX_READ)
            .ok_or("The conversation could not be read.")?;
        let fingerprint = fingerprint(&lines, start);
        self.stamp = now;
        if self.fingerprint.as_deref() == Some(fingerprint.as_str()) {
            return Ok(());
        }
        use keel_workspace::conversation::Ending;
        let mut decoder = t.agent().decoder();
        let mut frames = Vec::new();
        for line in &lines {
            frames.extend(decoder.feed(line, None, std::time::Instant::now()));
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
    /// After a turn Keel drove, the transcript holds exactly what the journal's `turn` records
    /// already replay. Recording its fingerprint keeps the next reconnect from appending a whole
    /// second copy of the conversation as a `snapshot` — which is how one journal reached 58 MB.
    fn seen(&mut self, state: &AppState, q: &ChatQuery) -> Result<(), String> {
        match transcript(state, q)? {
            Some(t) => self.mark(&t),
            None => Ok(()),
        }
    }
    fn mark(&mut self, t: &Transcript) -> Result<(), String> {
        let now = stamp(t);
        let (lines, _, start) = t
            .tail_last(keel_workspace::MAX_READ)
            .ok_or("The conversation could not be read.")?;
        let fingerprint = fingerprint(&lines, start);
        self.stamp = now;
        if self.fingerprint.as_deref() != Some(fingerprint.as_str()) {
            self.push("seen", json!({"fingerprint": fingerprint}))?;
        }
        Ok(())
    }
    /// A turn whose stream broke ends like any other: closed, then the reason. Without the
    /// `close` the page's last turn stayed open — its clock ticking, its calls spinning — for as
    /// long as the lane was open. A keyless `close` with nothing open folds to nothing (`reduce.ts`).
    fn end_broken(&mut self, reason: &str) -> Result<(), String> {
        self.push(
            "turn",
            json!([{"turn": null, "op": "close", "reason": "interrupted"}]),
        )?;
        self.push("fatal", json!(reason))
    }
    fn push(&mut self, event: &str, data: Value) -> Result<(), String> {
        if let Some(Ok(f)) = matches!(event, "snapshot" | "seen")
            .then(|| serde_json::from_value::<Fingerprint>(data.clone()))
        {
            self.fingerprint = Some(f.fingerprint);
        }
        let record = Record {
            seq: self.records.len() + 1,
            event: event.into(),
            data: serde_json::value::to_raw_value(&data).map_err(|e| e.to_string())?,
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
            .filter(|r| r.event == "accepted")
            .filter_map(|r| serde_json::from_str::<Value>(r.data.get()).ok())
            .find(|data| data["id"] == id)
        {
            return if old["fingerprint"] == fingerprint {
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

/// What the transcript said, as one value: its records and where they started, so a cut head
/// that moved is a change too.
fn fingerprint(lines: &[String], start: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(start.to_le_bytes());
    for line in lines {
        hash.update(line.as_bytes());
        hash.update(b"\n");
    }
    format!("transcript-v3:{:x}", hash.finalize())
}

fn transcript(state: &AppState, q: &ChatQuery) -> Result<Option<Transcript>, String> {
    let Some(session) = &q.session else {
        return Ok(None);
    };
    let agent = Agent::from_query(q.provider.as_deref()).map_err(str::to_string)?;
    let cwd = state.checkout(q.wt.as_deref())?;
    match agent.locate(&cwd, session) {
        Some(t) => Ok(Some(t)),
        // Codex and Antigravity name a conversation only once it has been written; a lane with
        // nothing on disk yet has nothing to seed. Claude's id is chosen first, so missing is lost.
        None if agent != Agent::Claude => Ok(None),
        None => Err(
            "The original conversation could not be found. Open it from History before resuming."
                .into(),
        ),
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
    if let Some(history) = all.get(&key).and_then(Weak::upgrade) {
        return Ok(history);
    }
    all.retain(|_, h| h.strong_count() > 0);
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
    // Windows append-only handles cannot truncate. Repair a partial final record with
    // a write handle, then retain append semantics for every subsequent journal write.
    if complete != bytes.len() {
        options
            .clone()
            .append(false)
            .write(true)
            .open(&path)
            .and_then(|file| file.set_len(complete as u64))
            .map_err(internal)?;
    }
    let file = options.open(path).map_err(internal)?;
    let fingerprint = records
        .iter()
        .rev()
        .filter(|r| r.event == "snapshot" || r.event == "seen")
        .find_map(|r| serde_json::from_str::<Fingerprint>(r.data.get()).ok())
        .map(|f| f.fingerprint);
    let mut h = History {
        file,
        records,
        running: false,
        fingerprint,
        stamp: None,
    };
    if h.records.last().is_some_and(|r| r.event != "idle") {
        h.end_broken("The daemon restarted during this turn. Review the partial result before sending again.")
            .map_err(internal)?;
        h.push("idle", Value::Null).map_err(internal)?;
    }
    let h = Arc::new(Mutex::new(h));
    all.insert(key, Arc::downgrade(&h));
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
    // The receipt and the turn it starts are one task, awaited here but not owned by this
    // request: a request dropped between the two (a reload, an aborted fetch, the window's own
    // timeout) used to leave the journal `running` with no turn behind it, and every later send
    // was refused as "still running" for the life of the daemon.
    let id = request.id.clone();
    let journal = h.clone();
    let accepted = tokio::spawn(async move {
        let receipt = request.id.clone();
        let held = h.clone();
        let seed = request.query.clone();
        let owner = state.clone();
        let accepted = tokio::task::spawn_blocking(move || {
            let mut h = held.locked();
            // Existing receipts must remain idempotent even when their referenced file later changes.
            let repeated = h
                .records
                .iter()
                .filter(|r| r.event == "accepted")
                .filter_map(|r| serde_json::from_str::<Value>(r.data.get()).ok())
                .any(|r| r["id"] == receipt);
            if !repeated {
                let checkout = owner.checkout(seed.wt.as_deref()).map_err(internal)?;
                let source = crate::tasks::Source {
                    lane: seed.lane.clone().unwrap_or_default(),
                    provider: seed.provider.clone().unwrap_or_else(|| "claude".into()),
                    session: seed
                        .session
                        .clone()
                        .unwrap_or_else(|| seed.lane.clone().unwrap_or_default()),
                    checkout: checkout.to_string(),
                    wt: seed.wt.clone(),
                };
                let task_session = source.session.clone();
                let task_provider = source.provider.clone();
                crate::tasks::register(&owner.repo(), source).map_err(internal)?;
                crate::guidance::prepare(&owner.repo()).map_err(internal)?;
                crate::tasks::input(&owner.repo(), &task_provider, &task_session, &seed.prompt)
                    .map_err(internal)?;
                crate::references::validate_query(&owner, &seed)
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            }
            // Always against the transcript, not only on a lane's first send: `seen` at this turn's
            // end vouches for the whole transcript, so turns taken in a terminal since must be in the
            // journal first. Unchanged costs one `stat`.
            // A transcript that cannot be read is the runtime's to report, as a failed turn the
            // person can read — not a 500 that leaves the submission unconfirmed and the lane stuck.
            if let Err(e) = h.seed(&owner, &seed, true) {
                tracing::debug!("chat send: transcript not read: {e}");
            }
            h.accept(&receipt, &fingerprint)
        })
        .await
        .map_err(internal)??;
        if accepted {
            let h = h.clone();
            tokio::spawn(async move {
                let owner = state.clone();
                let query = request.query.clone();
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
                        let _ = h.end_broken(error);
                    }
                    // A new conversation learns its session id during the turn; its first reconnect
                    // takes one snapshot, which is right, since the journal had no transcript yet.
                    let _ = h.seen(&owner, &query);
                    let _ = h.push("idle", Value::Null);
                    h.running = false;
                })
                .await;
            });
        }
        Ok::<_, Error>(accepted)
    })
    .await
    .map_err(internal)??;
    let running = tokio::task::spawn_blocking(move || journal.locked().running)
        .await
        .map_err(internal)?;
    Ok(Json(
        json!({"id": id, "accepted": accepted, "running": running}),
    ))
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
        if q.session.is_some() {
            let query = serde_json::from_value::<ChatQuery>(
                json!({"prompt":"", "session": q.session, "provider":q.provider, "wt":q.wt}),
            )
            .map_err(internal)?;
            // The journal is the conversation; the transcript only adds turns taken elsewhere.
            // A transcript that cannot be found (a checkout moved, a project renamed) must not
            // refuse the stream: the page read a 500 as a dropped connection and retried every
            // 1.5 s for good, composer locked, while the journal it wanted was right here.
            if let Err(e) = h.locked().seed(&state, &query, true) {
                tracing::debug!("chat events: transcript not read: {e}");
            }
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
            // The journal's mutex is held across transcript reads and file writes by blocking
            // threads, so taking it here would park an executor thread behind them.
            let held = h.clone();
            let cursor = after;
            let (batch, running) = crate::serve::blocking(
                move || {
                    let h = held.locked();
                    (
                        h.records
                            .iter()
                            .skip(cursor)
                            .take(128)
                            .cloned()
                            .collect::<Vec<_>>(),
                        h.running,
                    )
                },
                (Vec::new(), false),
            )
            .await;
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
    /// A Codex or Antigravity lane with nothing written yet seeds nothing and fails nothing; a
    /// Claude conversation that is missing says so.
    #[test]
    fn a_conversation_not_written_yet_is_only_missing_for_claude() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let q = |p: &str| {
            serde_json::from_value::<ChatQuery>(
                json!({"prompt": "x", "provider": p, "session": "no-such-session-0000"}),
            )
            .unwrap()
        };
        assert!(transcript(&state, &q("codex")).unwrap().is_none());
        assert!(transcript(&state, &q("antigravity")).unwrap().is_none());
        assert!(transcript(&state, &q("claude")).is_err());
    }
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
    fn partial_final_record_is_repaired_before_appending() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "partial-lane").unwrap();
        {
            let mut h = h.locked();
            h.accept("send-one", "prompt-a").unwrap();
            h.push("idle", Value::Null).unwrap();
            h.file.write_all(b"{\"seq\":3").unwrap();
            h.file.sync_data().unwrap();
        }
        state.chats.0.locked().clear();
        drop(h);
        let h = history(&state, "partial-lane").unwrap();
        assert_eq!(h.locked().records.len(), 2);
        assert!(!h.locked().accept("send-one", "prompt-a").unwrap());
        assert!(h.locked().accept("send-two", "prompt-b").unwrap());
        h.locked().push("idle", Value::Null).unwrap();
        state.chats.0.locked().clear();
        drop(h);
        let h = history(&state, "partial-lane").unwrap();
        assert_eq!(h.locked().records.len(), 4);
        assert!(!h.locked().accept("send-two", "prompt-b").unwrap());
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
    fn claude(path: &std::path::Path) -> Transcript {
        Transcript::at(
            Agent::Claude,
            camino::Utf8PathBuf::from_path_buf(path.into()).unwrap(),
        )
    }
    fn transcript_file(dir: &std::path::Path, prompts: &[&str]) -> std::path::PathBuf {
        let path = dir.join("t.jsonl");
        let lines: String = prompts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{}\n",
                    json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":p}})
                )
            })
            .collect();
        std::fs::write(&path, lines).unwrap();
        path
    }
    #[test]
    fn a_reconnect_with_nothing_new_appends_nothing() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "seed-lane").unwrap();
        let path = transcript_file(root.path(), &["hello"]);
        h.locked().snapshot(&claude(&path)).unwrap();
        let after_first = h.locked().records.len();
        assert!(after_first > 0);
        h.locked().snapshot(&claude(&path)).unwrap();
        h.locked().stamp = None;
        h.locked().snapshot(&claude(&path)).unwrap();
        assert_eq!(h.locked().records.len(), after_first);
    }
    #[test]
    fn a_turn_keel_drove_is_not_snapshotted_again_after_a_restart() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "seen-lane").unwrap();
        let path = transcript_file(root.path(), &["hello"]);
        h.locked().snapshot(&claude(&path)).unwrap();
        // The turn's own records went to the journal; the transcript grew by the same turn.
        let path = transcript_file(root.path(), &["hello", "and again"]);
        h.locked().mark(&claude(&path)).unwrap();
        state.chats.0.locked().clear();
        drop(h);
        let h = history(&state, "seen-lane").unwrap();
        let before = h.locked().records.len();
        h.locked().snapshot(&claude(&path)).unwrap();
        assert_eq!(h.locked().records.len(), before);
        // Something written outside Keel is still picked up.
        let path = transcript_file(root.path(), &["hello", "and again", "from the terminal"]);
        h.locked().snapshot(&claude(&path)).unwrap();
        assert!(h.locked().records.len() > before);
    }
    #[test]
    fn a_journal_nobody_holds_is_released_and_reloads_whole() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "weak-lane").unwrap();
        h.locked().accept("send-one", "prompt-a").unwrap();
        h.locked().push("idle", Value::Null).unwrap();
        h.locked().running = false;
        let weak = Arc::downgrade(&h);
        drop(h);
        assert!(weak.upgrade().is_none());
        let h = history(&state, "weak-lane").unwrap();
        assert_eq!(h.locked().records.len(), 2);
        assert!(!h.locked().accept("send-one", "prompt-a").unwrap());
    }
    #[test]
    fn a_broken_stream_closes_its_turn_before_saying_why() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(camino::Utf8PathBuf::from_path_buf(root.path().into()).unwrap());
        let h = history(&state, "broken-lane").unwrap();
        h.locked().accept("send-one", "prompt-a").unwrap();
        h.locked().end_broken("gone").unwrap();
        let events: Vec<_> = h.locked().records.iter().map(|r| r.event.clone()).collect();
        assert_eq!(events, ["accepted", "turn", "fatal"]);
        assert!(h.locked().records[1].data.get().contains("\"close\""));
    }
    #[test]
    fn lane_ids_cannot_escape_storage() {
        for id in ["", "..", "a/b", "a\\b", "/tmp", "hello world"] {
            assert!(!safe(id));
        }
    }
}
