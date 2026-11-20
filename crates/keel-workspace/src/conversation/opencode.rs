//! OpenCode's lines as turn ops: what `opencode run --format json` prints for a chat turn, and
//! the same shape `crate::opencode` reads back out of its database for a followed or replayed
//! one. One arm for both, so the turn a person watched and the one they reopen cannot differ.
//!
//! Ceiling, measured on 1.18.30: the stream says settled work only. A tool appears once, when it
//! has finished, and text arrives a whole part at a time. A chat turn's rows open earlier than
//! that because Keel's own gate announces each call before it runs.

use std::time::Instant;

use serde_json::{Value, json};

use super::{Decoder, Ending, Frame, Stream, TextKind, usage};

/// The MCP server every Keel-run turn is given; OpenCode names its tools `<server>_<tool>`.
const KEEL_SERVER: &str = "keel";

impl Decoder {
    pub(super) fn opencode(&mut self, r: &Value, out: &mut Vec<Frame>, now: Instant) {
        let part = &r["part"];
        match r["type"].as_str() {
            Some("user") => self.opencode_user(r, out),
            Some("turn_end") => self.opencode_error(&r["error"], out, true),
            Some("error") => self.opencode_error(&r["error"], out, false),
            Some("step_start") => {}
            Some("text") => self.opencode_text(part, TextKind::Say, out, now),
            Some("reasoning") => self.opencode_text(part, TextKind::Think, out, now),
            Some("tool_start") => {
                self.opencode_begin(part, out);
            }
            Some("tool_use") => self.opencode_call(part, out),
            Some("step_finish") => self.opencode_step(part, out),
            other => {
                if self.turn.is_some() {
                    let kind = other.unwrap_or("?").to_string();
                    self.raw(out, &r.to_string(), Stream::Out, false, Some(kind));
                }
            }
        }
    }

    /// The person's message opens a turn, keyed by the message's id — stable, because OpenCode
    /// never renumbers one. A second text part of the same message is the same turn.
    fn opencode_user(&mut self, r: &Value, out: &mut Vec<Frame>) {
        let id = r["id"].as_str().unwrap_or_default().to_string();
        if self
            .turn
            .as_ref()
            .is_some_and(|t| t.key.as_deref() == Some(&id))
        {
            return;
        }
        // Keel's own notes ride ahead of a lane's first task; the person typed only the task.
        let text = r["text"].as_str().unwrap_or_default();
        let prompt = crate::antigravity::without_notes(text).trim().to_string();
        let at = r["at"].as_str().map(str::to_string);
        self.opencode_cost = 0.0;
        out.extend(self.open(Some(prompt), Some(id), at, None));
    }

    fn opencode_text(&mut self, part: &Value, kind: TextKind, out: &mut Vec<Frame>, now: Instant) {
        let Some(text) = part["text"].as_str().filter(|t| !t.is_empty()) else {
            return;
        };
        // A replay cut off before the prompt still draws what it has.
        self.ensure_open(out);
        let step = part["id"].as_str().unwrap_or_default().to_string();
        self.text(step, kind, text, out, now);
    }

    /// A call's row, opened. Keel's gate says `tool_start` when a call asks to run, which is the
    /// only word of it before it settles; the settled record opens the same row again, harmlessly.
    fn opencode_begin(&mut self, part: &Value, out: &mut Vec<Frame>) -> String {
        self.ensure_open(out);
        let id = part["callID"].as_str().unwrap_or_default().to_string();
        let name = part["tool"].as_str().unwrap_or("tool");
        let (tool, input) = opencode_tool(name, &part["state"]["input"], &[KEEL_SERVER]);
        self.begin(&id, &tool, Some(input), None, None, out);
        id
    }

    /// A finished call. The row may already be open — Keel's gate announced it before it ran —
    /// in which case this is its answer and nothing more.
    fn opencode_call(&mut self, part: &Value, out: &mut Vec<Frame>) {
        let id = self.opencode_begin(part, out);
        let state = &part["state"];
        let failed = state["status"] == "error";
        let output = if failed {
            &state["error"]
        } else {
            &state["output"]
        };
        let output = output.as_str().unwrap_or_default().replace("\r\n", "\n");
        self.finish(&id, output, failed, None, out);
    }

    /// The end of one model response: its tokens and cost join the turn's. A step that did not
    /// ask for a tool is the last one, and in a transcript that is the turn's end; a chat turn
    /// ends when its process does, which `agent::translate` says.
    fn opencode_step(&mut self, part: &Value, out: &mut Vec<Frame>) {
        self.ensure_open(out);
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let tokens = &part["tokens"];
        let (read, write) = (n(&tokens["cache"]["read"]), n(&tokens["cache"]["write"]));
        self.opencode_cost += part["cost"].as_f64().unwrap_or(0.0);
        let last = crate::opencode::ends(&json!({"type": "step_finish", "part": part}));
        let t = &mut self.turn.as_mut().expect("open").tokens;
        t.context = n(&tokens["input"]) + read + write;
        t.input += n(&tokens["input"]);
        t.output += n(&tokens["output"]) + n(&tokens["reasoning"]);
        t.cache_read += read;
        t.cache_write += write;
        let cost = (self.opencode_cost > 0.0).then_some(self.opencode_cost);
        let op = usage(t, cost, None, last);
        self.push(out, op);
        if last && self.source == super::Source::Transcript {
            out.extend(self.close(Ending::Done));
        }
    }

    /// An error OpenCode reported about the turn itself. An abort is the person stopping it, not
    /// a failure. In a transcript the turn is over; a chat turn's process is about to exit.
    fn opencode_error(&mut self, error: &Value, out: &mut Vec<Frame>, ended: bool) {
        if ended && self.turn.is_none() {
            return;
        }
        self.ensure_open(out);
        let name = error["name"].as_str().unwrap_or("UnknownError");
        let aborted = name == "MessageAbortedError";
        if !aborted {
            let message = error["data"]["message"]
                .as_str()
                .unwrap_or(name)
                .to_string();
            self.failure(out, failure_kind(name, error), message, None);
        }
        if ended {
            let reason = if aborted {
                Ending::Stopped
            } else {
                Ending::Done
            };
            out.extend(self.close(reason));
        }
    }
}

/// Which of the failures the page already has words for an OpenCode error is. Only what its
/// records can tell apart: a provider refusing the credentials, and everything else.
fn failure_kind(name: &str, error: &Value) -> &'static str {
    let status = error["data"]["statusCode"].as_u64();
    if name == "ProviderAuthError" || matches!(status, Some(401 | 403)) {
        "authentication_failed"
    } else {
        "api_error"
    }
}

/// An OpenCode tool as the Claude tool it does the work of, with Claude's argument names added
/// beside OpenCode's own. The decoder draws rows with it and the approval hook asks with it, so
/// a row and the card asking about it can never disagree. `servers` are the MCP servers of the
/// run: OpenCode joins server and tool with one underscore, which only their names can split
/// again. A tool with no counterpart keeps its name — and is asked about, never assumed.
pub fn opencode_tool(name: &str, args: &Value, servers: &[&str]) -> (String, Value) {
    let mut input = args.as_object().cloned().unwrap_or_default();
    // The longest server that fits: with `keel` and `keel_ask` both configured, `keel_ask_user`
    // read by the shorter one is Keel's own question tool, which is never asked about.
    let mcp = servers
        .iter()
        .filter(|s| {
            name.strip_prefix(**s)
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('_'))
        })
        .max_by_key(|s| s.len())
        .map(|s| format!("mcp__{s}__{}", &name[s.len() + 1..]));
    if let Some(tool) = mcp {
        return (tool, Value::Object(input));
    }
    let (tool, renamed): (&str, &[(&str, &str)]) = match name {
        "bash" => ("Bash", &[]),
        "write" => ("Write", &[("filePath", "file_path")]),
        "edit" => (
            "Edit",
            &[
                ("filePath", "file_path"),
                ("oldString", "old_string"),
                ("newString", "new_string"),
            ],
        ),
        "read" => ("Read", &[("filePath", "file_path")]),
        "list" => ("LS", &[]),
        "grep" => ("Grep", &[]),
        "glob" => ("Glob", &[]),
        "webfetch" => ("WebFetch", &[]),
        "websearch" => ("WebSearch", &[]),
        "todowrite" => ("TodoWrite", &[]),
        // A subagent is an `Agent` row named for its type, the way Claude's are.
        "task" => ("Agent", &[]),
        _ => (name, &[]),
    };
    for (theirs, ours) in renamed {
        if let Some(v) = args.get(*theirs) {
            input.insert((*ours).to_string(), v.clone());
        }
    }
    (tool.to_string(), Value::Object(input))
}
