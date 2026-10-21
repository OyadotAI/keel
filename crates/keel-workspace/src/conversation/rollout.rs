//! Codex's interactive transcript — the rollout's `event_msg` records — as turn ops.
//!
//! Not `codex exec --json`, which [`Decoder::codex`] reads: the rollout is a different shape, and
//! the reader (`crate::codex`) has already dropped everything but `event_msg`. A turn is keyed by
//! Codex's own `turn_id`, which every item carries, so live and replay fold to the same turn and a
//! turn whose start was cut off a long replay still files under its key.
//!
//! Ceiling: Codex writes an item when it completes and never when it starts, so a five-minute
//! `cargo test` has no row until it ends — the lane reads as working the whole time, which is
//! true. Rows could open from `response_item/custom_tool_call` if that ever matters.

use std::time::Instant;

use serde_json::{Value, json};

use super::{Decoder, Ending, Frame, Op, Source, Stream, TextKind, Tokens, clean_path, usage};

impl Decoder {
    pub(super) fn codex_rollout(&mut self, r: &Value, out: &mut Vec<Frame>, now: Instant) {
        let p = &r["payload"];
        let at = r["timestamp"].as_str().map(str::to_string);
        let turn_id = p["turn_id"].as_str().map(str::to_string);
        let key = self.turn.as_ref().and_then(|t| t.key.clone());
        // A turn that already ended. Codex files a background command's completion under the turn
        // that started it, whenever it exits — a dev server's, turns later — and reopening that
        // turn for it wiped two turns on screen and could leave the lane working forever.
        let ended = turn_id
            .as_ref()
            .is_some_and(|t| self.codex_closed.contains(t));
        // An end for a turn other than the open one ends nothing.
        let ours = turn_id.is_none() || turn_id == key;
        match p["type"].as_str() {
            Some("item_completed") => {
                let item = &p["item"];
                if item["type"] == "UserMessage" {
                    let said = crate::codex::said(item);
                    if (turn_id.is_some() && turn_id == key) || ended {
                        // Typed while Codex was working: more of this turn, not a new one.
                        self.raw(out, &format!("You: {said}"), Stream::Out, false, None);
                    } else {
                        self.open_rollout(Some(said), turn_id, at, out);
                    }
                    return;
                }
                // A turn opens without its prompt only where a long replay cut the prompt off: no
                // turn open, and one never seen to end. Anything else is drawn where it lands.
                if self.turn.is_none() {
                    if ended {
                        return;
                    }
                    self.open_rollout(None, turn_id, at.clone(), out);
                }
                self.rollout_item(item, at, out, now);
            }
            Some("token_count") => {
                let total = &p["info"]["total_token_usage"];
                let n = |k: &str| total[k].as_u64().unwrap_or(0);
                self.codex_total = [
                    n("input_tokens"),
                    n("cached_input_tokens"),
                    n("output_tokens"),
                ];
                let context = p["info"]["last_token_usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                if let Some(t) = self.turn.as_mut() {
                    t.tokens = share(self.codex_total, t.base, context);
                    let op = usage(&t.tokens, None, None, false);
                    self.push(out, op);
                }
            }
            Some("task_complete") if self.turn.is_some() && ours => {
                let t = self.turn.as_ref().expect("checked");
                let op = usage(&t.tokens, None, p["duration_ms"].as_u64(), true);
                self.push(out, op);
                self.end_rollout(Ending::Done, out);
            }
            Some("turn_aborted") if ours => self.end_rollout(Ending::Stopped, out),
            // Bookkeeping: it opens nothing, and between turns there is nothing to draw it under.
            Some(
                "task_started"
                | "task_complete"
                | "turn_aborted"
                | "thread_settings_applied"
                | "thread_goal_updated",
            ) => {}
            other => {
                if self.turn.is_some() {
                    let line = r.to_string();
                    self.raw(
                        out,
                        &line,
                        Stream::Out,
                        false,
                        Some(other.unwrap_or("?").to_string()),
                    );
                }
            }
        }
    }

    /// Close the open turn and remember its key, so nothing written for it later reopens it.
    fn end_rollout(&mut self, reason: Ending, out: &mut Vec<Frame>) {
        if let Some(key) = self.turn.as_ref().and_then(|t| t.key.clone()) {
            self.codex_closed.insert(key);
        }
        out.extend(self.close(reason));
    }

    fn open_rollout(
        &mut self,
        prompt: Option<String>,
        key: Option<String>,
        at: Option<String>,
        out: &mut Vec<Frame>,
    ) {
        // A turn superseded by the next prompt is over too.
        if let Some(open) = self.turn.as_ref().and_then(|t| t.key.clone()) {
            self.codex_closed.insert(open);
        }
        out.extend(self.open(prompt, key, at, None));
        let base = self.codex_total;
        if let Some(t) = self.turn.as_mut() {
            t.base = base;
        }
    }

    fn rollout_item(
        &mut self,
        item: &Value,
        at: Option<String>,
        out: &mut Vec<Frame>,
        now: Instant,
    ) {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let kind = item["type"].as_str().unwrap_or_default();
        match kind {
            "AgentMessage" | "Plan" => {
                let text = blocks_text(&item["content"])
                    .or_else(|| item["text"].as_str().map(str::to_string));
                if let Some(t) = text.filter(|t| !t.is_empty()) {
                    self.text(format!("codex:{id}"), TextKind::Say, &t, out, now);
                }
            }
            "Reasoning" => {
                let t = strings(&item["summary_text"]);
                if !t.is_empty() {
                    self.text(format!("codex:{id}"), TextKind::Think, &t, out, now);
                }
            }
            "CommandExecution" => {
                let input = json!({ "command": command(&item["command"]) });
                self.begin(&id, "Bash", Some(input), None, at.clone(), out);
                let failed = item["status"] == "failed"
                    || item["exit_code"].as_i64().is_some_and(|c| c != 0);
                let output = item["aggregated_output"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                self.finish(&id, output, failed, at, out);
            }
            "FileChange" => {
                let paths: Vec<String> = item["changes"]
                    .as_object()
                    .into_iter()
                    .flat_map(|c| c.keys())
                    .map(|p| clean_path(p))
                    .collect();
                if !paths.is_empty() {
                    self.push(out, Op::Wrote { paths });
                }
            }
            "McpToolCall" => {
                let tool = format!(
                    "mcp__{}__{}",
                    item["server"].as_str().unwrap_or("?"),
                    item["tool"].as_str().unwrap_or("?")
                );
                let input = match &item["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(json!({})),
                    v => v.clone(),
                };
                self.begin(&id, &tool, Some(input), None, at.clone(), out);
                let failed = item["status"] == "failed";
                let output = if failed {
                    item["error"]["message"]
                        .as_str()
                        .unwrap_or("The tool failed.")
                        .to_string()
                } else {
                    blocks_text(&item["result"]["content"]).unwrap_or_default()
                };
                self.finish(&id, output, failed, at, out);
            }
            "SubAgentActivity" => self.subagent(item, at, out),
            "ContextCompaction" => {
                self.raw(
                    out,
                    "the conversation was compacted",
                    Stream::Out,
                    false,
                    None,
                );
            }
            _ => {
                let line = item.to_string();
                self.raw(out, &line, Stream::Out, false, Some(kind.to_string()));
            }
        }
    }
}

impl Decoder {
    /// A Codex subagent starting or finishing, drawn the way Claude's are: an `Agent` row named
    /// for it, and a background entry that says it is working until Codex says it stopped — it may
    /// outlive the turn that started it. Keyed by the agent's own thread, which the start and the
    /// end both carry (the end's item id is not the start's). Its name is the last part of its path:
    /// `/root/pm` is the `pm` the sidebar lists. Messages sent to a running agent draw nothing.
    pub(super) fn subagent(&mut self, item: &Value, at: Option<String>, out: &mut Vec<Frame>) {
        let Some(thread) = item["agent_thread_id"].as_str() else {
            return;
        };
        let name = item["agent_path"]
            .as_str()
            .and_then(|p| p.rsplit('/').next())
            .filter(|n| !n.is_empty())
            .map(str::to_string);
        match item["kind"].as_str() {
            Some("started") => {
                let input = json!({ "subagent_type": name.clone().unwrap_or_default(), "description": item["agent_path"] });
                self.begin(thread, "Agent", Some(input), None, at, out);
                if self.source == Source::Chat {
                    self.chat_agents.push(thread.to_string());
                }
                self.push(
                    out,
                    Op::Background {
                        id: thread.to_string(),
                        agent: name,
                        running: true,
                    },
                );
            }
            Some(kind @ ("completed" | "interrupted")) => {
                let running = self
                    .turn
                    .as_ref()
                    .is_some_and(|t| t.calls.get(thread).is_some_and(|c| c.running));
                if running {
                    self.finish(thread, String::new(), kind == "interrupted", at, out);
                }
                self.chat_agents.retain(|a| a != thread);
                self.push(
                    out,
                    Op::Background {
                        id: thread.to_string(),
                        agent: None,
                        running: false,
                    },
                );
            }
            _ => {}
        }
    }
}

/// The turn's share of the session's running total, measured from where it stood when the turn
/// opened. Codex counts cached tokens inside `input_tokens`; Keel's `input` is the uncached part,
/// as Claude's is.
fn share(total: [u64; 3], base: [u64; 3], context: u64) -> Tokens {
    let [input, cached, output] = [0, 1, 2].map(|i| total[i].saturating_sub(base[i]));
    Tokens {
        context,
        input: input.saturating_sub(cached),
        output,
        cache_read: cached,
        cache_write: 0,
    }
}

/// The text of `[{type, text}]` blocks, joined; `None` when there are none.
fn blocks_text(v: &Value) -> Option<String> {
    let parts: Vec<&str> = v
        .as_array()?
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn strings(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a command was, as a person would type it: the script of `zsh -lc <script>`, otherwise
/// the argv joined.
fn command(argv: &Value) -> String {
    let parts: Vec<&str> = argv
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match parts.as_slice() {
        [_, flag, script] if matches!(*flag, "-c" | "-lc") => script.to_string(),
        _ => parts.join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CallState, Provider, Source};
    use super::*;

    fn ev(payload: Value) -> String {
        json!({"timestamp":"2026-10-21T00:00:00Z","type":"event_msg","payload":payload}).to_string()
    }
    fn user(turn: &str, text: &str) -> String {
        ev(
            json!({"type":"item_completed","turn_id":turn,"item":{"type":"UserMessage","id":"u","content":[{"type":"text","text":text}]}}),
        )
    }
    fn item(turn: &str, item: Value) -> String {
        ev(json!({"type":"item_completed","turn_id":turn,"item":item}))
    }
    fn tokens(input: u64, cached: u64, output: u64, last: u64) -> String {
        ev(json!({"type":"token_count","info":{
            "total_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output},
            "last_token_usage":{"input_tokens":last}}}))
    }
    fn decode(lines: &[String]) -> Vec<Frame> {
        let mut d = Decoder::new(Provider::Codex, Source::Transcript);
        let now = Instant::now();
        let mut out: Vec<Frame> = lines.iter().flat_map(|l| d.feed(l, None, now)).collect();
        out.extend(d.flush_all());
        out
    }
    fn ops(frames: &[Frame]) -> Vec<&Op> {
        frames.iter().map(|f| &f.op).collect()
    }

    /// One turn, keyed by Codex's `turn_id` throughout: its prompt, its reply, a command and its
    /// output, a file it wrote, a failed MCP call, its usage and its end.
    #[test]
    fn a_codex_rollout_turn_is_keyed_by_its_turn_id() {
        let frames = decode(&[
            ev(json!({"type":"task_started","turn_id":"T1"})),
            user("T1", "add a button"),
            item(
                "T1",
                json!({"type":"AgentMessage","id":"m1","content":[{"type":"Text","text":"On it."}]}),
            ),
            item(
                "T1",
                json!({"type":"CommandExecution","id":"c1","command":["/bin/zsh","-lc","ls src"],"status":"completed","exit_code":0,"aggregated_output":"main.rs\n"}),
            ),
            item(
                "T1",
                json!({"type":"FileChange","id":"f1","changes":{"/repo/src/a.rs":{"type":"add"}},"status":"completed"}),
            ),
            item(
                "T1",
                json!({"type":"McpToolCall","id":"x1","server":"oya","tool":"open_tab","arguments":"{\"url\":\"u\"}","status":"failed","error":{"message":"no browser"}}),
            ),
            tokens(1000, 400, 50, 1000),
            ev(json!({"type":"task_complete","turn_id":"T1","duration_ms":1200})),
        ]);
        assert!(
            frames.iter().all(|f| f.turn.as_deref() == Some("T1")),
            "{frames:#?}"
        );
        let ops = ops(&frames);
        assert!(matches!(ops[0], Op::Open { prompt: Some(p), .. } if p == "add a button"));
        assert!(ops.iter().any(
            |o| matches!(o, Op::Text { append, kind: TextKind::Say, .. } if append == "On it.")
        ));
        assert!(ops.iter().any(|o| matches!(o, Op::Call { tool, input: Some(i), .. } if tool == "Bash" && i["command"] == "ls src")));
        assert!(ops.iter().any(|o| matches!(o, Op::Result { id, state: CallState::Ok, output, .. } if id == "c1" && output == "main.rs\n")));
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Wrote { paths } if paths == &["/repo/src/a.rs"]))
        );
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Call { tool, .. } if tool == "mcp__oya__open_tab"))
        );
        assert!(ops.iter().any(|o| matches!(o, Op::Result { id, state: CallState::Failed, output, .. } if id == "x1" && output == "no browser")));
        assert!(ops.iter().any(|o| matches!(
            o,
            Op::Usage {
                input: 600,
                cache_read: 400,
                output: 50,
                context: 1000,
                complete: true,
                ms: Some(1200),
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

    /// Words typed while Codex works are part of the open turn: one `open`, not two.
    #[test]
    fn a_codex_steer_stays_in_its_turn() {
        let frames = decode(&[
            user("T1", "first"),
            user("T1", "also this"),
            user("T2", "next"),
        ]);
        let opens = frames
            .iter()
            .filter(|f| matches!(f.op, Op::Open { .. }))
            .count();
        assert_eq!(opens, 2);
        assert!(
            frames
                .iter()
                .any(|f| matches!(&f.op, Op::Raw { lines } if lines[0].text == "You: also this"))
        );
    }

    #[test]
    fn an_aborted_codex_turn_closes_stopped() {
        let frames = decode(&[
            user("T1", "go"),
            item(
                "T1",
                json!({"type":"McpToolCall","id":"x","server":"s","tool":"t","arguments":{},"status":"completed","result":{"content":[{"type":"text","text":"ok"}]}}),
            ),
            ev(json!({"type":"turn_aborted","turn_id":"T1","reason":"interrupted"})),
        ]);
        assert!(matches!(
            frames.last().map(|f| &f.op),
            Some(Op::Close {
                reason: Ending::Stopped,
                ..
            })
        ));
    }

    /// A replay that starts mid-turn still files what it has under that turn's key.
    #[test]
    fn a_codex_rollout_cut_mid_turn_opens_keyed_without_a_prompt() {
        let frames = decode(&[item(
            "T9",
            json!({"type":"AgentMessage","id":"m","content":[{"type":"Text","text":"done"}]}),
        )]);
        assert!(
            matches!(&frames[0], Frame { turn: Some(t), op: Op::Open { prompt: None, .. } } if t == "T9")
        );
    }

    /// A background command started in turn A finishes during turn B, and later between turns:
    /// it reopens nothing, B stays open until its own end, and nothing is left open after it.
    #[test]
    fn a_late_item_for_an_ended_turn_reopens_nothing() {
        let late = || {
            item(
                "A",
                json!({"type":"CommandExecution","id":"bg","command":["sh","-c","pnpm dev"],"status":"completed","exit_code":0,"aggregated_output":""}),
            )
        };
        let say = |t: &str| {
            item(
                t,
                json!({"type":"AgentMessage","id":"m","content":[{"type":"Text","text":"x"}]}),
            )
        };
        let frames = decode(&[
            user("A", "start the server"),
            ev(json!({"type":"task_complete","turn_id":"A"})),
            user("B", "next"),
            say("B"),
            late(),
            ev(json!({"type":"task_complete","turn_id":"A"})),
            say("B"),
            ev(json!({"type":"task_complete","turn_id":"B"})),
            late(),
        ]);
        let opens: Vec<_> = frames
            .iter()
            .filter(|f| matches!(f.op, Op::Open { .. }))
            .map(|f| f.turn.clone())
            .collect();
        assert_eq!(opens, [Some("A".into()), Some("B".into())], "{frames:#?}");
        let closes: Vec<_> = frames
            .iter()
            .filter(|f| matches!(f.op, Op::Close { .. }))
            .map(|f| f.turn.clone())
            .collect();
        assert_eq!(closes, [Some("A".into()), Some("B".into())]);
        assert!(matches!(
            frames.last().map(|f| &f.op),
            Some(Op::Close {
                reason: Ending::Done,
                ..
            })
        ));
    }

    /// A subagent Codex starts is an `Agent` row named for it and a background agent until Codex
    /// says it finished — so the sidebar shows `pm` working, as it does for Claude's.
    #[test]
    fn a_codex_subagent_is_an_agent_row_and_works_until_it_finishes() {
        let sub = |kind: &str, id: &str| {
            item(
                "T1",
                json!({"type":"SubAgentActivity","id":id,"kind":kind,"agent_thread_id":"A1","agent_path":"/root/pm"}),
            )
        };
        let frames = decode(&[
            user("T1", "run the pm"),
            sub("started", "call_1"),
            sub("interacted", "call_2"),
            sub("completed", "subagent-completed-x"),
        ]);
        let ops: Vec<&Op> = frames.iter().map(|f| &f.op).collect();
        assert!(ops.iter().any(|o| matches!(o, Op::Call { id, tool, input: Some(i), .. } if id == "A1" && tool == "Agent" && i["subagent_type"] == "pm")));
        assert!(ops.iter().any(|o| matches!(o, Op::Background { id, agent: Some(a), running: true } if id == "A1" && a == "pm")));
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Result { id, state: CallState::Ok, .. } if id == "A1"))
        );
        assert!(matches!(
            ops.last(),
            Some(Op::Background { running: false, .. })
        ));
    }

    /// A Codex chat turn learns of its subagent from the transcript, passed on as an
    /// `exec --json` item; the subagent ends with the turn's process if Codex never said so.
    #[test]
    fn a_codex_chat_subagent_works_until_it_finishes_or_the_turn_ends() {
        let mut d = Decoder::new(Provider::Codex, Source::Chat);
        let now = Instant::now();
        let started = json!({"type":"item.completed","item":{"type":"subagent_activity","kind":"started","agent_thread_id":"A1","agent_path":"/root/pm"}}).to_string();
        let mut frames = d.feed(&started, None, now);
        frames.extend(d.close(Ending::Done));
        let ops: Vec<&Op> = frames.iter().map(|f| &f.op).collect();
        assert!(ops.iter().any(|o| matches!(o, Op::Call { tool, input: Some(i), .. } if tool == "Agent" && i["subagent_type"] == "pm")));
        assert!(ops.iter().any(|o| matches!(o, Op::Background { id, agent: Some(a), running: true } if id == "A1" && a == "pm")));
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Background { id, running: false, .. } if id == "A1")),
            "{ops:#?}"
        );
    }

    #[test]
    fn codex_bookkeeping_opens_no_turn() {
        let frames = decode(&[
            ev(json!({"type":"task_started","turn_id":"T1"})),
            ev(json!({"type":"thread_settings_applied"})),
            tokens(10, 0, 1, 10),
            ev(json!({"type":"task_complete","turn_id":"T0"})),
            ev(json!({"type":"something_new"})),
        ]);
        assert!(frames.is_empty(), "{frames:#?}");
    }

    #[test]
    fn an_unknown_codex_record_is_raw_not_a_crash() {
        let frames = decode(&[
            user("T1", "go"),
            item("T1", json!({"type":"Hologram","id":"h"})),
            ev(json!({"type":"weather"})),
        ]);
        let unknown: Vec<_> = frames
            .iter()
            .filter_map(|f| match &f.op {
                Op::Raw { lines } => lines[0].unknown.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(unknown, ["Hologram", "weather"]);
    }

    /// Usage is this turn's share of the session's running total, not the total.
    #[test]
    fn codex_usage_is_the_turns_share_of_the_running_total() {
        let frames = decode(&[
            user("T1", "a"),
            tokens(1000, 200, 100, 1000),
            ev(json!({"type":"task_complete","turn_id":"T1"})),
            user("T2", "b"),
            tokens(1500, 500, 130, 1400),
            ev(json!({"type":"task_complete","turn_id":"T2"})),
        ]);
        let last = frames
            .iter()
            .rev()
            .find_map(|f| match &f.op {
                Op::Usage {
                    input,
                    output,
                    cache_read,
                    context,
                    ..
                } => Some((*input, *output, *cache_read, *context)),
                _ => None,
            })
            .unwrap();
        assert_eq!(last, (200, 30, 300, 1400));
    }
}
