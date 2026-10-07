//! Claude Code's stream-json, and Codex's events, as the turn a person reads.
//!
//! This decoder used to live in the Swift app, about 4,300 lines across `SessionModel` and
//! `Turn`, and the bugs testers reported most were there: a tool call stopped mid-run stayed
//! "running" forever, with its spinner and its per-second clock; a replayed conversation and the
//! live one drew different turns; every token cost work proportional to the whole reply. A second
//! client (the cross-platform one) would have had to port all of it, bugs included. So the format
//! is read once, here, and every client draws the ops this produces.
//!
//! Three properties are the point, and each has a test:
//!
//! - **Every ending closes every call.** [`Decoder::close`] answers every call still running with
//!   `result{state: interrupted}` before it says `close`. There is no path that ends a turn and
//!   leaves one open, because there is one function that ends a turn.
//! - **Live and replay fold to the same turn.** A step's id is `<message id>:<block index>` on both
//!   paths — the stream says the index, and a transcript writes one record per block in order — so
//!   the same conversation read either way produces the same steps, calls and usage.
//! - **No op re-sends what was sent.** Text goes out as appends, batched on a 40 ms clock the
//!   caller drives, so a stream is at most ~25 text frames a second whatever the token rate and
//!   the work per frame is the bytes since the last one.
//!
//! Pure on purpose: no I/O, no clock of its own, no tokio. The daemon feeds it lines and sends what
//! it returns; what a turn *is* — the opener, the sidechain filter — is decided by the reader
//! beside it in `sessions.rs`, never re-derived here.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// How long text may wait to be batched with what follows it.
pub const BATCH: Duration = Duration::from_millis(40);
/// Lines of raw output kept per turn, the escape hatch's cap (the Swift `Turn.rawCap`).
pub const RAW_CAP: usize = 2_000;
/// One raw line, cut past this.
pub const RAW_LINE: usize = 20 * 1024;
/// A tool's output, cut past this to its head and tail. Unbounded before, against "nothing
/// unbounded reaches the app".
pub const OUTPUT_CAP: usize = 64 * 1024;
/// A call's arguments as they stream, before they parse. A `Write` streams its whole file this
/// way; past this the fragments are dropped and the complete message supplies the input.
pub const PARTIAL_CAP: usize = 2 * 1024 * 1024;

/// Tools whose use means a file changed. Must agree with the app's list, or two renderings of one
/// turn disagree about what was edited.
pub const WRITE_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "Update"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

/// Where the lines come from. A transcript splits itself into turns at each prompt and says
/// when one ended; a chat stream is one turn the daemon opened, and anything that went wrong in it
/// is happening now rather than history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Chat,
    Transcript,
}

/// Why a turn closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ending {
    /// It finished.
    Done,
    /// Somebody pressed Stop.
    Stopped,
    /// The agent exited with an error, or never started.
    Failed,
    /// The stream broke: a read failed, the connection went.
    Interrupted,
    /// The next prompt arrived without this one saying it ended.
    Superseded,
    /// A replay reached the end and nothing is still writing it.
    CaughtUp,
    /// The process that was writing it is gone.
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextKind {
    Say,
    Think,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    Ok,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawLine {
    pub text: String,
    #[serde(skip_serializing_if = "is_out")]
    pub stream: Stream,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Out,
    Err,
}

fn is_out(s: &Stream) -> bool {
    *s == Stream::Out
}

/// One thing that happened to the turn, in the order it happened.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A turn begins. `prompt` is `None` when the head of a long replay was cut off before it.
    Open {
        prompt: Option<String>,
        at: Option<String>,
        offset: Option<u64>,
        replayed: bool,
    },
    /// The agent named its session. Lane-level rather than turn-level.
    Session {
        id: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        commands: Vec<String>,
    },
    /// Prose or reasoning, appended to the step it names. A step the client has not seen is a new
    /// step, after the last one.
    Text {
        step: String,
        kind: TextKind,
        append: String,
    },
    /// A tool call, announced or repaired. Idempotent in `id`: the first is a new row, the rest
    /// fill it in. `input` is absent until the arguments are whole.
    Call {
        id: String,
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        subject: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        writes: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// A call's answer. `cut` is how many bytes of output were dropped from its middle.
    Result {
        id: String,
        state: CallState,
        output: String,
        #[serde(skip_serializing_if = "is_zero")]
        cut: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// Files written by something that is not a call (Codex's `file_change`).
    Wrote { paths: Vec<String> },
    /// What the turn has spent so far. Assigned, never added: `final` is the run's own total.
    Usage {
        context: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
        #[serde(rename = "final")]
        complete: bool,
    },
    /// Claude Code reporting a failure about itself, not something the agent said. `history`
    /// when it is in a transcript: a rate limit that reset weeks ago is not a thing to act on.
    Failure {
        kind: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        resets_at: Option<f64>,
        history: bool,
    },
    /// Everything the agent emitted that is not a token, so there is no state in which Keel saw
    /// something and the person cannot.
    Raw { lines: Vec<RawLine> },
    /// The conversation part of the turn is over (the gate and the commit are facts, not ops).
    /// Every call still running was answered `interrupted` immediately before this.
    Close {
        reason: Ending,
        #[serde(skip_serializing_if = "is_zero")]
        raw_dropped: usize,
    },
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// An op and the turn it belongs to: the opener's uuid, or `None` for the one turn a chat stream
/// has open before its key is known — the same rule facts follow.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Frame {
    pub turn: Option<String>,
    #[serde(flatten)]
    pub op: Op,
}

/// What the stream said about the run as a whole, for the daemon's own bookkeeping.
#[derive(Debug, Default, Clone)]
pub struct Seen {
    pub session_id: Option<String>,
    /// `result.is_error`, and its text when it had any.
    pub failed: Option<String>,
    pub cost: Option<f64>,
}

#[derive(Default)]
struct Tokens {
    context: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

struct OpenCall {
    tool: String,
    running: bool,
    input: Option<Value>,
}

/// The open turn. Dropped whole at `close`, so memory is one turn, not the history.
#[derive(Default)]
struct Turn {
    key: Option<String>,
    calls: HashMap<String, OpenCall>,
    /// Content-block index → the call it announced, from `content_block_start` to `stop`.
    blocks: HashMap<u64, String>,
    /// Content-block index → the text step it opened.
    steps: HashMap<u64, (String, TextKind)>,
    partial: HashMap<String, String>,
    /// Messages whose prose already arrived as deltas; the complete message skips it.
    streamed: HashSet<String>,
    /// Message id → blocks seen so far, so a transcript's one-block records count positions.
    positions: HashMap<String, u64>,
    /// Messages whose usage was counted. A transcript repeats it on every block's record.
    counted: HashSet<String>,
    tokens: Tokens,
    raw: usize,
    raw_dropped: usize,
    ordinal: u64,
    pending: Option<(String, TextKind, String, Instant)>,
}

pub struct Decoder {
    provider: Provider,
    source: Source,
    turn: Option<Turn>,
    /// The message the stream is inside, from `message_start`.
    message: Option<String>,
    seen: Seen,
}

impl Decoder {
    pub fn new(provider: Provider, source: Source) -> Self {
        Self {
            provider,
            source,
            turn: None,
            message: None,
            seen: Seen::default(),
        }
    }

    pub fn state(&self) -> &Seen {
        &self.seen
    }

    /// Whether a turn is open.
    pub fn is_open(&self) -> bool {
        self.turn.is_some()
    }

    /// The open turn now has a key — a chat stream learns its opener's uuid after it starts.
    pub fn set_key(&mut self, key: String) {
        if let Some(t) = self.turn.as_mut() {
            t.key = Some(key);
        }
    }

    /// Begin a turn, closing any still open as superseded.
    pub fn open(
        &mut self,
        prompt: Option<String>,
        key: Option<String>,
        at: Option<String>,
        offset: Option<u64>,
    ) -> Vec<Frame> {
        let mut out = self.close(Ending::Superseded);
        self.turn = Some(Turn {
            key,
            ..Turn::default()
        });
        self.message = None;
        let replayed = self.source == Source::Transcript;
        self.push(
            &mut out,
            Op::Open {
                prompt,
                at,
                offset,
                replayed,
            },
        );
        out
    }

    /// One whole line from the agent's stdout or a transcript. `offset` is where it starts in the
    /// transcript, when there is one.
    pub fn feed(&mut self, line: &str, offset: Option<u64>, now: Instant) -> Vec<Frame> {
        let mut out = Vec::new();
        let record: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                self.ensure_open(&mut out);
                self.raw(&mut out, line, Stream::Out, true, None);
                return out;
            }
        };
        if self.provider == Provider::Codex {
            self.codex(&record, &mut out, now);
        } else {
            if self.source == Source::Transcript
                && let Some((uuid, _, at)) = crate::sessions::opener(&record)
            {
                // Session-list summaries are capped at 200 characters. The conversation must
                // retain the complete prompt, including after a terminal/chat handoff.
                let prompt = flat_text(&record["message"]["content"]);
                if matches!(
                    prompt.trim(),
                    "[Request interrupted by user]" | "[Request interrupted by user for tool use]"
                ) {
                    out.extend(self.close(Ending::Stopped));
                    return out;
                }
                out.extend(self.open(Some(prompt), Some(uuid), Some(at), offset));
                self.raw(&mut out, line, Stream::Out, false, None);
                return out;
            }
            // Between turns a transcript holds Claude Code's own bookkeeping — an away summary, a
            // scheduled task firing — which opened a turn with no prompt and nothing in it. Only
            // the agent itself speaking opens one: that is a real turn, started by a background
            // job finishing, or the remains of one whose prompt a long replay cut off.
            if self.turn.is_none()
                && self.source == Source::Transcript
                && !matches!(record["type"].as_str(), Some("assistant"))
            {
                return out;
            }
            self.ensure_open(&mut out);
            self.claude(&record, line, &mut out, now);
        }
        if self.due(now) {
            self.flush_into(&mut out);
        }
        out
    }

    /// A line the agent wrote to stderr.
    pub fn stderr(&mut self, line: &str) -> Vec<Frame> {
        let mut out = Vec::new();
        self.ensure_open(&mut out);
        self.raw(&mut out, line, Stream::Err, false, None);
        out
    }

    /// When batched text must go out, if any is waiting.
    pub fn deadline(&self) -> Option<Instant> {
        self.turn
            .as_ref()
            .and_then(|t| t.pending.as_ref())
            .map(|(_, _, _, since)| *since + BATCH)
    }

    /// Send batched text if its time has come.
    pub fn flush(&mut self, now: Instant) -> Vec<Frame> {
        let mut out = Vec::new();
        if self.due(now) {
            self.flush_into(&mut out);
        }
        out
    }

    /// Send batched text now: the end of a replay batch, where no clock is coming.
    pub fn flush_all(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        self.flush_into(&mut out);
        out
    }

    /// End the open turn. The one way a turn ends: every call still running is answered
    /// `interrupted` first, so no client can be left drawing one that will never return.
    pub fn close(&mut self, reason: Ending) -> Vec<Frame> {
        let mut out = Vec::new();
        if self.turn.is_none() {
            return out;
        }
        self.flush_into(&mut out);
        let turn = self.turn.as_mut().expect("checked");
        let mut running: Vec<String> = turn
            .calls
            .iter()
            .filter(|(_, c)| c.running)
            .map(|(id, _)| id.clone())
            .collect();
        running.sort();
        for id in running {
            self.push(
                &mut out,
                Op::Result {
                    id,
                    state: CallState::Interrupted,
                    output: String::new(),
                    cut: 0,
                    at: None,
                },
            );
        }
        let raw_dropped = self.turn.as_ref().map_or(0, |t| t.raw_dropped);
        self.push(
            &mut out,
            Op::Close {
                reason,
                raw_dropped,
            },
        );
        self.turn = None;
        self.message = None;
        out
    }

    // --- Claude Code -------------------------------------------------------------------------

    fn claude(&mut self, r: &Value, line: &str, out: &mut Vec<Frame>, now: Instant) {
        let kind = r["type"].as_str().unwrap_or_default();
        // Deltas are the reply, and the reply is on screen; logging them filled the raw cap with
        // noise in the first couple of thousand tokens and dropped everything after.
        if kind != "stream_event" {
            let unknown = (!matches!(kind, "system" | "assistant" | "user" | "result"))
                .then(|| kind.to_string());
            self.raw(out, line, Stream::Out, false, unknown);
        }
        let parent = r["parent_tool_use_id"].as_str().map(str::to_string);
        let at = r["timestamp"].as_str().map(str::to_string);
        match kind {
            "system" => self.system(r, out),
            "stream_event" => self.event(&r["event"], parent, out, now),
            "assistant" => self.assistant(r, parent, at, out),
            "user" => {
                for b in r["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] != "tool_result" {
                        continue;
                    }
                    let Some(id) = b["tool_use_id"].as_str() else {
                        continue;
                    };
                    let failed = b["is_error"].as_bool() == Some(true);
                    self.finish(id, flat_text(&b["content"]), failed, at.clone(), out);
                }
            }
            "result" => self.result(r, out),
            _ => {}
        }
    }

    fn system(&mut self, r: &Value, out: &mut Vec<Frame>) {
        match r["subtype"].as_str() {
            Some("init") => {
                if let Some(id) = r["session_id"].as_str() {
                    self.seen.session_id = Some(id.to_string());
                    let commands = r["slash_commands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect();
                    self.push(
                        out,
                        Op::Session {
                            id: id.to_string(),
                            commands,
                        },
                    );
                }
            }
            Some("turn_duration") if self.source == Source::Transcript => {
                out.extend(self.close(Ending::Done));
            }
            Some("compact_boundary") => {
                self.raw(
                    out,
                    "the conversation was compacted",
                    Stream::Out,
                    false,
                    None,
                );
            }
            _ if r["level"] == "warning" => {
                if let Some(said) = r["content"].as_str().filter(|s| !s.is_empty()) {
                    self.failure(out, "warning", said.to_string(), None);
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, e: &Value, parent: Option<String>, out: &mut Vec<Frame>, now: Instant) {
        let index = e["index"].as_u64();
        if e["type"] == "message_start" {
            self.message = e["message"]["id"].as_str().map(str::to_string);
            return;
        }
        // Matched on the delta rather than the envelope's name: a renamed event would otherwise
        // drop the whole reply with no symptom.
        if let Some(d) = e.get("delta").filter(|d| d.is_object()) {
            let Some(index) = index else { return };
            match d["type"].as_str() {
                Some("text_delta") => {
                    let t = d["text"].as_str().unwrap_or_default();
                    self.delta(index, TextKind::Say, t, out, now);
                }
                Some("thinking_delta") => {
                    let t = d["thinking"].as_str().unwrap_or_default();
                    self.delta(index, TextKind::Think, t, out, now);
                }
                Some("input_json_delta") => {
                    let turn = self.turn.as_mut().expect("open");
                    let Some(id) = turn.blocks.get(&index) else {
                        return;
                    };
                    let buf = turn.partial.entry(id.clone()).or_default();
                    if buf.len() < PARTIAL_CAP {
                        buf.push_str(d["partial_json"].as_str().unwrap_or_default());
                    }
                }
                _ => {}
            }
            return;
        }
        match e["type"].as_str() {
            Some("content_block_start") => {
                let Some(index) = index else { return };
                let b = &e["content_block"];
                match b["type"].as_str() {
                    Some("text") => self.start_step(index, TextKind::Say),
                    Some("thinking" | "redacted_thinking") => {
                        self.start_step(index, TextKind::Think)
                    }
                    Some("tool_use" | "server_tool_use") => {
                        let (Some(id), Some(name)) = (b["id"].as_str(), b["name"].as_str()) else {
                            return;
                        };
                        self.turn
                            .as_mut()
                            .expect("open")
                            .blocks
                            .insert(index, id.to_string());
                        self.begin(id, name, None, parent, None, out);
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let Some(index) = index else { return };
                let turn = self.turn.as_mut().expect("open");
                turn.steps.remove(&index);
                let Some(id) = turn.blocks.remove(&index) else {
                    return;
                };
                let partial = turn.partial.remove(&id).unwrap_or_default();
                if partial.len() >= PARTIAL_CAP {
                    return;
                }
                if let Ok(input @ Value::Object(_)) = serde_json::from_str::<Value>(&partial) {
                    let tool = turn
                        .calls
                        .get(&id)
                        .map(|c| c.tool.clone())
                        .unwrap_or_default();
                    self.begin(&id, &tool, Some(input), None, None, out);
                }
            }
            _ => {}
        }
    }

    fn start_step(&mut self, index: u64, kind: TextKind) {
        let id = self.step_id(index);
        let turn = self.turn.as_mut().expect("open");
        turn.steps.insert(index, (id, kind));
    }

    fn step_id(&mut self, index: u64) -> String {
        match &self.message {
            Some(m) => format!("{m}:{index}"),
            None => {
                let turn = self.turn.as_mut().expect("open");
                turn.ordinal += 1;
                format!("s{}", turn.ordinal)
            }
        }
    }

    fn delta(
        &mut self,
        index: u64,
        kind: TextKind,
        text: &str,
        out: &mut Vec<Frame>,
        now: Instant,
    ) {
        if text.is_empty() {
            return;
        }
        let step = match self.turn.as_ref().expect("open").steps.get(&index) {
            Some((id, k)) if *k == kind => id.clone(),
            _ => {
                self.start_step(index, kind);
                self.turn.as_ref().expect("open").steps[&index].0.clone()
            }
        };
        if let Some(m) = self.message.clone() {
            self.turn.as_mut().expect("open").streamed.insert(m);
        }
        self.text(step, kind, text, out, now);
    }

    fn assistant(
        &mut self,
        r: &Value,
        parent: Option<String>,
        at: Option<String>,
        out: &mut Vec<Frame>,
    ) {
        if let Some((kind, message, resets_at)) = classify(r) {
            self.failure(out, &kind, message, resets_at);
            return;
        }
        let message = &r["message"];
        let id = message["id"].as_str().map(str::to_string);
        let blocks = message["content"].as_array().cloned().unwrap_or_default();
        let turn = self.turn.as_mut().expect("open");
        let streamed = id.as_ref().is_some_and(|m| turn.streamed.contains(m));
        // ponytail: positions count from the first record of a message this decoder saw, so a
        // replay cut mid-message numbers the survivors from 0 where live said 2. Harmless until a
        // client reconciles a replay against a live stream of the same message.
        let base = id
            .as_ref()
            .map(|m| *turn.positions.get(m).unwrap_or(&0))
            .unwrap_or(0);
        if let Some(m) = &id {
            turn.positions.insert(m.clone(), base + blocks.len() as u64);
        }
        // Counted once per message: a transcript writes one record per block, each repeating the
        // message's usage, and summing them was a turn that cost three times what it did.
        let u = &message["usage"];
        let fresh = id.as_ref().is_none_or(|m| turn.counted.insert(m.clone()));
        if fresh && u.is_object() {
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let context =
                n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
            if context > 0 || n("output_tokens") > 0 {
                let t = &mut turn.tokens;
                t.input += n("input_tokens");
                t.output += n("output_tokens");
                t.cache_read += n("cache_read_input_tokens");
                t.cache_write += n("cache_creation_input_tokens");
                if context > 0 {
                    t.context = context;
                }
                let op = usage(t, None, None, false);
                self.push(out, op);
            }
        }
        // One pass, in the order the message holds them: prose, a call, prose again stays in
        // that order. Two passes reversed it on replay, where it was the only order there was.
        for (i, b) in blocks.iter().enumerate() {
            let step = match &id {
                Some(m) => format!("{m}:{}", base + i as u64),
                None => {
                    let turn = self.turn.as_mut().expect("open");
                    turn.ordinal += 1;
                    format!("s{}", turn.ordinal)
                }
            };
            match b["type"].as_str() {
                Some("text") if !streamed => {
                    let t = b["text"].as_str().unwrap_or_default();
                    if !t.is_empty() {
                        self.flush_into(out);
                        self.push(
                            out,
                            Op::Text {
                                step,
                                kind: TextKind::Say,
                                append: t.to_string(),
                            },
                        );
                    }
                }
                Some("thinking" | "redacted_thinking") if !streamed => {
                    let t = b["thinking"].as_str().unwrap_or_default();
                    if !t.is_empty() {
                        self.flush_into(out);
                        self.push(
                            out,
                            Op::Text {
                                step,
                                kind: TextKind::Think,
                                append: t.to_string(),
                            },
                        );
                    }
                }
                Some("tool_use") => {
                    let (Some(cid), Some(name)) = (b["id"].as_str(), b["name"].as_str()) else {
                        continue;
                    };
                    let input = b.get("input").filter(|i| i.is_object()).cloned();
                    self.begin(cid, name, input, parent.clone(), at.clone(), out);
                }
                _ => {}
            }
        }
    }

    fn result(&mut self, r: &Value, out: &mut Vec<Frame>) {
        if let Some(id) = r["session_id"].as_str() {
            self.seen.session_id = Some(id.to_string());
        }
        let cost = r["total_cost_usd"].as_f64();
        self.seen.cost = cost.or(self.seen.cost);
        if r["is_error"] == true {
            let said = r["result"].as_str().unwrap_or_default().trim().to_string();
            self.seen.failed = Some(said.clone());
            if !said.is_empty() {
                self.failure(out, "run", said, None);
            }
        }
        let u = &r["usage"];
        let turn = self.turn.as_mut().expect("open");
        if u.is_object() {
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let t = &mut turn.tokens;
            t.input = n("input_tokens");
            t.output = n("output_tokens");
            t.cache_read = n("cache_read_input_tokens");
            t.cache_write = n("cache_creation_input_tokens");
        }
        let op = usage(&turn.tokens, cost, r["duration_ms"].as_u64(), true);
        self.push(out, op);
    }

    // --- Codex -------------------------------------------------------------------------------

    fn codex(&mut self, r: &Value, out: &mut Vec<Frame>, now: Instant) {
        self.ensure_open(out);
        let line = r.to_string();
        let kind = r["type"].as_str().unwrap_or_default();
        if !kind.starts_with("item.") {
            self.raw(out, &line, Stream::Out, false, None);
        }
        match kind {
            "thread.started" => {
                if let Some(id) = r["thread_id"].as_str() {
                    self.seen.session_id = Some(id.to_string());
                    self.push(
                        out,
                        Op::Session {
                            id: id.to_string(),
                            commands: Vec::new(),
                        },
                    );
                }
            }
            "item.started" | "item.updated" | "item.completed" => {
                let item = &r["item"];
                let id = item["id"].as_str().unwrap_or_default().to_string();
                match item["type"].as_str() {
                    Some("command_execution") if kind == "item.started" => {
                        let input = serde_json::json!({
                            "command": item["command"].as_str().unwrap_or_default(),
                            "description": "Command selected by Codex",
                        });
                        self.begin(&id, "Bash", Some(input), None, None, out);
                    }
                    Some("command_execution") if kind == "item.completed" => {
                        let failed = item["status"] == "failed"
                            || item["exit_code"].as_i64().is_some_and(|c| c != 0);
                        let output = item["aggregated_output"].as_str().unwrap_or_default();
                        self.finish(&id, output.to_string(), failed, None, out);
                    }
                    Some("file_change") if kind == "item.completed" => {
                        let paths: Vec<String> = item["changes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|c| c["path"].as_str().map(clean_path))
                            .collect();
                        if !paths.is_empty() {
                            self.push(out, Op::Wrote { paths });
                        }
                    }
                    // Each message is its own step. The Swift reader only appended to the turn's
                    // merged text, so a Codex reply never appeared in the ordered view at all.
                    Some("agent_message") if kind == "item.completed" => {
                        if let Some(t) = item["text"].as_str().filter(|t| !t.is_empty()) {
                            self.text(format!("codex:{id}"), TextKind::Say, t, out, now);
                        }
                    }
                    Some("reasoning") if kind == "item.completed" => {
                        if let Some(t) = item["text"].as_str().filter(|t| !t.is_empty()) {
                            self.text(format!("codex:{id}"), TextKind::Think, t, out, now);
                        }
                    }
                    Some("error") => {
                        let m = item["message"]
                            .as_str()
                            .unwrap_or("Codex reported an error");
                        self.failure(out, "codex", m.to_string(), None);
                    }
                    _ => {}
                }
            }
            "turn.completed" => {
                let u = &r["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let t = Tokens {
                    context: 0,
                    input: n("input_tokens"),
                    output: n("output_tokens"),
                    cache_read: n("cached_input_tokens"),
                    cache_write: 0,
                };
                self.push(out, usage(&t, None, None, true));
            }
            "turn.failed" | "error" => {
                let m = r["error"]["message"]
                    .as_str()
                    .or(r["message"].as_str())
                    .unwrap_or("Codex turn failed");
                self.seen.failed = Some(m.to_string());
                self.failure(out, "codex", m.to_string(), None);
            }
            _ => {}
        }
    }

    // --- Shared ------------------------------------------------------------------------------

    fn ensure_open(&mut self, out: &mut Vec<Frame>) {
        if self.turn.is_none() {
            out.extend(self.open(None, None, None, None));
        }
    }

    fn push(&mut self, out: &mut Vec<Frame>, op: Op) {
        let turn = self.turn.as_ref().and_then(|t| t.key.clone());
        out.push(Frame { turn, op });
    }

    fn text(
        &mut self,
        step: String,
        kind: TextKind,
        text: &str,
        out: &mut Vec<Frame>,
        now: Instant,
    ) {
        let turn = self.turn.as_mut().expect("open");
        match &mut turn.pending {
            Some((s, k, buf, _)) if *s == step && *k == kind => buf.push_str(text),
            _ => {
                self.flush_into(out);
                self.turn.as_mut().expect("open").pending =
                    Some((step, kind, text.to_string(), now));
            }
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|d| now >= d)
    }

    fn flush_into(&mut self, out: &mut Vec<Frame>) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if let Some((step, kind, append, _)) = turn.pending.take() {
            self.push(out, Op::Text { step, kind, append });
        }
    }

    /// Announce a call, or repair one already announced. Text waiting to go out goes first, so
    /// a client never sees the call before the sentence that introduced it.
    fn begin(
        &mut self,
        id: &str,
        tool: &str,
        input: Option<Value>,
        parent: Option<String>,
        at: Option<String>,
        out: &mut Vec<Frame>,
    ) {
        self.flush_into(out);
        let turn = self.turn.as_mut().expect("open");
        if let Some(existing) = turn.calls.get_mut(id) {
            // The complete message after the stream: the same call, its input authoritative.
            if input.is_none() || existing.input == input {
                return;
            }
            existing.input = input.clone();
        } else {
            turn.calls.insert(
                id.to_string(),
                OpenCall {
                    tool: tool.to_string(),
                    running: true,
                    input: input.clone(),
                },
            );
        }
        let tool = turn.calls[id].tool.clone();
        let subject = input.as_ref().map(subject).unwrap_or_default();
        let reason = input
            .as_ref()
            .and_then(|i| i["description"].as_str())
            .map(str::to_string);
        let writes = match (&input, WRITE_TOOLS.contains(&tool.as_str())) {
            (Some(i), true) => ["file_path", "path", "notebook_path"]
                .iter()
                .find_map(|k| i[*k].as_str().filter(|p| !p.is_empty()))
                .map(|p| vec![clean_path(p)])
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.push(
            out,
            Op::Call {
                id: id.to_string(),
                tool,
                parent,
                input,
                subject,
                reason,
                writes,
                at,
            },
        );
    }

    fn finish(
        &mut self,
        id: &str,
        output: String,
        failed: bool,
        at: Option<String>,
        out: &mut Vec<Frame>,
    ) {
        self.flush_into(out);
        let turn = self.turn.as_mut().expect("open");
        // A result for a call this turn never announced — its head was cut off a long replay —
        // is already in the raw log. Drawing it would be a phantom call.
        let Some(call) = turn.calls.get_mut(id) else {
            return;
        };
        call.running = false;
        let (output, cut) = cap_middle(output, OUTPUT_CAP);
        let state = if failed {
            CallState::Failed
        } else {
            CallState::Ok
        };
        self.push(
            out,
            Op::Result {
                id: id.to_string(),
                state,
                output,
                cut,
                at,
            },
        );
    }

    fn failure(
        &mut self,
        out: &mut Vec<Frame>,
        kind: &str,
        message: String,
        resets_at: Option<f64>,
    ) {
        self.flush_into(out);
        let history = self.source == Source::Transcript;
        self.push(
            out,
            Op::Failure {
                kind: kind.to_string(),
                message,
                resets_at,
                history,
            },
        );
    }

    fn raw(
        &mut self,
        out: &mut Vec<Frame>,
        line: &str,
        stream: Stream,
        unreadable: bool,
        unknown: Option<String>,
    ) {
        let turn = self.turn.as_mut().expect("open");
        if turn.raw >= RAW_CAP {
            turn.raw_dropped += 1;
            return;
        }
        turn.raw += 1;
        let text = cut_at(line, RAW_LINE).to_string();
        let raw = RawLine {
            text,
            stream,
            unreadable,
            unknown,
        };
        // Joined onto the previous op when that was raw too: a replay is mostly raw lines, and a
        // frame per line is a frame per record for nothing.
        if let Some(Frame {
            op: Op::Raw { lines },
            ..
        }) = out.last_mut()
        {
            lines.push(raw);
            return;
        }
        self.flush_into(out);
        self.push(out, Op::Raw { lines: vec![raw] });
    }
}

fn usage(t: &Tokens, cost: Option<f64>, ms: Option<u64>, complete: bool) -> Op {
    Op::Usage {
        context: t.context,
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_write: t.cache_write,
        cost,
        ms,
        complete,
    }
}

/// A failure Claude Code generated about itself — `model: "<synthetic>"`, or flagged as an API
/// error — as `(kind, message, resets_at)`. Rendered as the agent's own prose before, which is how
/// "Not logged in · Please run /login" arrived looking like a remark.
fn classify(r: &Value) -> Option<(String, String, Option<f64>)> {
    let synthetic = r["message"]["model"] == "<synthetic>";
    let flagged = r["is_api_error_message"] == true || r["isApiErrorMessage"] == true;
    let error = r["error"].as_str();
    if !synthetic && !flagged && error.is_none() {
        return None;
    }
    let said = r["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    let or = |fallback: &str| {
        if said.is_empty() {
            fallback.to_string()
        } else {
            said.clone()
        }
    };
    Some(match error {
        Some(e @ ("authentication_failed" | "oauth_org_not_allowed")) => {
            (e.to_string(), or("Claude Code is not logged in."), None)
        }
        Some("rate_limit") => (
            "rate_limit".into(),
            or("You have hit your usage limit."),
            r["quotaLimits"]["resetsAt"].as_f64(),
        ),
        Some("server_error") => (
            "server_error".into(),
            or("The API had a server error."),
            None,
        ),
        other => {
            if said.is_empty() {
                return None;
            }
            (other.unwrap_or("api_error").to_string(), said, None)
        }
    })
}

/// The one-line summary of a call, in the order the tools actually carry it.
pub fn subject(input: &Value) -> String {
    let s = [
        "command",
        "file_path",
        "path",
        "notebook_path",
        "pattern",
        "description",
    ]
    .iter()
    .find_map(|k| input[*k].as_str().filter(|s| !s.is_empty()))
    .unwrap_or_default();
    let line = s.lines().next().unwrap_or_default();
    line.chars().take(160).collect()
}

fn clean_path(p: &str) -> String {
    p.strip_prefix("./").unwrap_or(p).to_string()
}

/// A tool result's content, as text: a string, or the text of its blocks.
fn flat_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(flat_text).collect::<Vec<_>>().join(" "),
        Value::Object(o) => o.get("text").map(flat_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn cut_at(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Keep the head and the tail; the middle of a long output is the part nobody reads.
fn cap_middle(s: String, max: usize) -> (String, usize) {
    if s.len() <= max {
        return (s, 0);
    }
    let half = max / 2;
    let mut head = half;
    while !s.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = s.len() - half;
    while !s.is_char_boundary(tail) {
        tail += 1;
    }
    let cut = tail - head;
    (format!("{}\n…\n{}", &s[..head], &s[tail..]), cut)
}

#[cfg(test)]
mod tests;
