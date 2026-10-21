//! Antigravity's interactive conversation — the steps `crate::antigravity` reads from its
//! database — as turn ops. Not `agy -p --output-format stream-json`, which
//! [`Decoder::antigravity`] reads: a step here arrives once, settled, with everything it carries.
//!
//! Ceiling: a step is read when it settles, so a reply appears whole rather than token by token,
//! and a command that runs for minutes has no row until it finishes.

use std::time::Instant;

use serde_json::Value;

use super::{Decoder, Ending, Frame, Stream, TextKind, Tokens, antigravity_tool, usage};
use crate::antigravity::{cancelled, ends, failed, key};

impl Decoder {
    pub(super) fn agy_transcript(&mut self, r: &Value, out: &mut Vec<Frame>, now: Instant) {
        let Some(idx) = r["agy"].as_i64() else {
            self.ensure_open(out);
            return self.raw(out, &r.to_string(), Stream::Out, false, None);
        };
        let at = r["at"].as_str().map(str::to_string);
        match r["step"].as_i64() {
            Some(14) => {
                let prompt = r["prompt"].as_str().unwrap_or_default().trim().to_string();
                out.extend(self.open(Some(prompt), Some(key(idx)), at, None));
            }
            Some(15) => {
                // A replay cut off before the prompt still draws what it has.
                self.ensure_open(out);
                if let Some(t) = r["thinking"].as_str().filter(|t| !t.is_empty()) {
                    self.text(format!("agy:{idx}:think"), TextKind::Think, t, out, now);
                }
                if let Some(t) = r["text"].as_str().filter(|t| !t.is_empty()) {
                    self.text(format!("agy:{idx}"), TextKind::Say, t, out, now);
                }
                if let Some(name) = r["call"]["name"].as_str() {
                    let (tool, input) = antigravity_tool(name, &r["call"]["args"]);
                    let id = call_id(r, idx);
                    self.begin(&id, &tool, Some(input), None, at.clone(), out);
                }
                self.agy_usage(&r["usage"], out);
                if ends(r) {
                    let t = &self.turn.as_ref().expect("open").tokens;
                    let op = usage(t, None, None, true);
                    self.push(out, op);
                    let reason = if r["status"] == 3 {
                        Ending::Done
                    } else {
                        Ending::Stopped
                    };
                    out.extend(self.close(reason));
                }
            }
            Some(132) => {
                self.ensure_open(out);
                let id = call_id(r, idx);
                let known = self
                    .turn
                    .as_ref()
                    .is_some_and(|t| t.calls.contains_key(&id));
                if !known {
                    // Its response was before the cut of a long replay.
                    let name = r["call"]["name"].as_str().unwrap_or("tool");
                    let (tool, input) = antigravity_tool(name, &r["call"]["args"]);
                    self.begin(&id, &tool, Some(input), None, at.clone(), out);
                }
                let bad = failed(r) || cancelled(r);
                let output = if cancelled(r) {
                    "Cancelled."
                } else if bad {
                    r["error"].as_str().unwrap_or("The tool failed.")
                } else {
                    r["output"].as_str().unwrap_or_default()
                };
                self.finish(&id, output.replace("\r\n", "\n"), bad, at, out);
                if ends(r) {
                    out.extend(self.close(Ending::Stopped));
                }
            }
            other => {
                if self.turn.is_some() {
                    let kind = other.map_or("?".into(), |k| format!("step {k}"));
                    self.raw(out, &r.to_string(), Stream::Out, false, Some(kind));
                }
            }
        }
    }

    /// A response's usage, added to the turn's: agy reports each response on its own.
    fn agy_usage(&mut self, u: &Value, out: &mut Vec<Frame>) {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        if n("input") == 0 && n("output") == 0 {
            return;
        }
        let t = &mut self.turn.as_mut().expect("open").tokens;
        *t = Tokens {
            context: n("input") + n("cache_read"),
            input: t.input + n("input"),
            output: t.output + n("output") + n("thinking"),
            cache_read: t.cache_read + n("cache_read"),
            cache_write: 0,
        };
        let op = usage(t, None, None, false);
        self.push(out, op);
    }
}

/// A call's id: agy's own, which the response and the tool step share; the step index when a
/// call has none, so the row still answers.
fn call_id(r: &Value, idx: i64) -> String {
    r["call"]["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("agy:{idx}"), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::{CallState, Op, Provider, Source};
    use super::*;
    use serde_json::json;

    fn decode(lines: &[Value]) -> Vec<Frame> {
        let mut d = Decoder::new(Provider::Antigravity, Source::Transcript);
        let now = Instant::now();
        let mut out: Vec<Frame> = lines
            .iter()
            .flat_map(|l| d.feed(&l.to_string(), None, now))
            .collect();
        out.extend(d.flush_all());
        out
    }

    /// The turn measured on agy 1.4.3: input, a response calling a tool, the tool, a response
    /// with the answer — one turn, keyed by its input step, closed done.
    #[test]
    fn an_agy_turn_reads_as_one_turn() {
        let frames = decode(&[
            json!({"agy":0,"step":14,"status":3,"prompt":"run it","at":"2026-10-21T13:52:19.672Z"}),
            json!({"agy":1,"step":15,"status":3,"thinking":"plan","call":{"id":"call_1","name":"run_command","args":{"CommandLine":"echo hi"}},"usage":{"input":100,"output":5,"thinking":3,"cache_read":0}}),
            json!({"agy":2,"step":132,"status":3,"call":{"id":"call_1","name":"run_command"},"output":"hi\r\n"}),
            json!({"agy":3,"step":15,"status":3,"text":"hi","usage":{"input":20,"output":2,"thinking":0,"cache_read":90}}),
        ]);
        assert!(
            frames.iter().all(|f| f.turn.as_deref() == Some("turn-0")),
            "{frames:#?}"
        );
        let ops: Vec<&Op> = frames.iter().map(|f| &f.op).collect();
        assert!(matches!(ops[0], Op::Open { prompt: Some(p), .. } if p == "run it"));
        assert!(ops.iter().any(
            |o| matches!(o, Op::Text { kind: TextKind::Think, append, .. } if append == "plan")
        ));
        assert!(
            ops.iter().any(
                |o| matches!(o, Op::Call { id, tool, .. } if id == "call_1" && tool == "Bash")
            )
        );
        assert!(ops.iter().any(|o| matches!(o, Op::Result { id, state: CallState::Ok, output, .. } if id == "call_1" && output == "hi\n")));
        assert!(
            ops.iter().any(
                |o| matches!(o, Op::Text { kind: TextKind::Say, append, .. } if append == "hi")
            )
        );
        assert!(ops.iter().any(|o| matches!(
            o,
            Op::Usage {
                input: 120,
                output: 10,
                cache_read: 90,
                complete: true,
                ..
            }
        )));
        assert!(matches!(
            ops.last(),
            Some(Op::Close {
                reason: Ending::Done,
                ..
            })
        ));
    }

    /// Measured on agy 1.4.3: Esc and ⌃C at its permission question cancel the tool step and
    /// write nothing after it. The row answers and the turn stops.
    #[test]
    fn a_cancelled_agy_tool_stops_the_turn() {
        let frames = decode(&[
            json!({"agy":4,"step":14,"status":3,"prompt":"sleep"}),
            json!({"agy":5,"step":15,"status":3,"call":{"id":"c","name":"run_command","args":{"CommandLine":"sleep 60"}}}),
            json!({"agy":6,"step":132,"status":6,"call":{"id":"c","name":"run_command"}}),
        ]);
        assert!(frames.iter().any(|f| matches!(&f.op, Op::Result { state: CallState::Failed, output, .. } if output == "Cancelled.")));
        assert!(matches!(
            frames.last().map(|f| &f.op),
            Some(Op::Close {
                reason: Ending::Stopped,
                ..
            })
        ));
    }

    /// A refused tool answers its row as failed, with agy's reason.
    #[test]
    fn a_refused_agy_tool_fails_its_row() {
        let frames = decode(&[
            json!({"agy":0,"step":14,"status":3,"prompt":"go"}),
            json!({"agy":1,"step":15,"status":3,"call":{"id":"c","name":"read_file","args":{"Path":"/x"}}}),
            json!({"agy":2,"step":132,"status":7,"call":{"id":"c"},"error":"denied by hook"}),
        ]);
        assert!(frames.iter().any(|f| matches!(&f.op, Op::Result { state: CallState::Failed, output, .. } if output == "denied by hook")));
    }

    /// A response cancelled mid-turn ends the turn as stopped, and the call it was making is
    /// answered rather than left spinning.
    #[test]
    fn a_cancelled_agy_response_stops_the_turn() {
        let frames = decode(&[
            json!({"agy":0,"step":14,"status":3,"prompt":"go"}),
            json!({"agy":1,"step":15,"status":3,"call":{"id":"c","name":"run_command","args":{}}}),
            json!({"agy":2,"step":15,"status":6}),
        ]);
        assert!(frames.iter().any(|f| matches!(
            f.op,
            Op::Result {
                state: CallState::Interrupted,
                ..
            }
        )));
        assert!(matches!(
            frames.last().map(|f| &f.op),
            Some(Op::Close {
                reason: Ending::Stopped,
                ..
            })
        ));
    }
}
