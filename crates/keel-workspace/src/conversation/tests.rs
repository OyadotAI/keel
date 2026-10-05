use super::*;
use serde_json::json;

/// What a client would draw, folded from ops by the smallest reducer that can — the contract
/// the React store implements, in forty lines.
#[derive(Debug, Default, PartialEq)]
struct Fold {
    turns: Vec<FTurn>,
}

#[derive(Debug, Default, PartialEq)]
struct FTurn {
    key: Option<String>,
    prompt: Option<String>,
    /// Steps in order: ("say"|"think"|"call", id).
    steps: Vec<(String, String)>,
    text: HashMap<String, String>,
    calls: HashMap<String, FCall>,
    closed: Option<Ending>,
    raw: usize,
    usage: Option<(u64, u64)>,
}

#[derive(Debug, Default, PartialEq, Clone)]
struct FCall {
    tool: String,
    input: Option<Value>,
    output: Option<String>,
    state: Option<CallState>,
    parent: Option<String>,
}

impl Fold {
    fn apply(&mut self, frames: &[Frame]) {
        for f in frames {
            if let Op::Open { prompt, .. } = &f.op {
                self.turns.push(FTurn {
                    key: f.turn.clone(),
                    prompt: prompt.clone(),
                    ..FTurn::default()
                });
                continue;
            }
            let t = self.turns.last_mut().expect("an op before any open");
            assert!(t.closed.is_none(), "an op after close: {:?}", f.op);
            match &f.op {
                Op::Text { step, kind, append } => {
                    if !t.text.contains_key(step) {
                        let k = if *kind == TextKind::Say {
                            "say"
                        } else {
                            "think"
                        };
                        t.steps.push((k.into(), step.clone()));
                    }
                    t.text.entry(step.clone()).or_default().push_str(append);
                }
                Op::Call {
                    id,
                    tool,
                    input,
                    parent,
                    ..
                } => {
                    let c = t.calls.entry(id.clone()).or_insert_with(|| FCall {
                        tool: tool.clone(),
                        parent: parent.clone(),
                        ..FCall::default()
                    });
                    if input.is_some() {
                        c.input = input.clone();
                    }
                    if parent.is_none() && !t.steps.iter().any(|(_, s)| s == id) {
                        t.steps.push(("call".into(), id.clone()));
                    }
                }
                Op::Result {
                    id, state, output, ..
                } => {
                    let c = t
                        .calls
                        .get_mut(id)
                        .expect("a result for a call never announced");
                    c.state = Some(*state);
                    c.output = Some(output.clone());
                }
                Op::Usage { input, output, .. } => t.usage = Some((*input, *output)),
                Op::Raw { lines } => t.raw += lines.len(),
                Op::Close { reason, .. } => t.closed = Some(*reason),
                _ => {}
            }
        }
    }

    fn running(&self) -> Vec<String> {
        self.turns
            .iter()
            .flat_map(|t| t.calls.iter())
            .filter(|(_, c)| c.state.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }
}

fn fixture_lines() -> Vec<String> {
    let raw = include_str!("../../tests/fixtures/turn-edit-and-command.sse");
    let mut out = Vec::new();
    let mut event = "";
    for line in raw.lines() {
        if let Some(e) = line.strip_prefix("event: ") {
            event = e;
        } else if let Some(d) = line.strip_prefix("data: ")
            && event == "msg"
        {
            out.push(d.to_string());
        }
    }
    out
}

/// Feed every line with the clock advancing `step` per line, and close as the chat stream would.
fn live(lines: &[String], step: Duration) -> (Fold, Vec<Frame>) {
    let mut d = Decoder::new(Provider::Claude, Source::Chat);
    let mut frames = d.open(Some("ask".into()), None, None, None);
    let mut now = Instant::now();
    for l in lines {
        frames.extend(d.feed(l, None, now));
        now += step;
    }
    frames.extend(d.close(Ending::Done));
    let mut f = Fold::default();
    f.apply(&frames);
    (f, frames)
}

fn transcript(lines: &[String]) -> Fold {
    let mut d = Decoder::new(Provider::Claude, Source::Transcript);
    let now = Instant::now();
    let mut frames = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        frames.extend(d.feed(l, Some(i as u64 * 100), now));
    }
    frames.extend(d.flush_all());
    frames.extend(d.close(Ending::CaughtUp));
    let mut f = Fold::default();
    f.apply(&frames);
    f
}

fn opener(uuid: &str, said: &str) -> String {
    json!({"type": "user", "uuid": uuid, "timestamp": "2026-10-04T10:00:00Z",
           "message": {"role": "user", "content": said}})
    .to_string()
}

fn tool_use(msg: &str, id: &str, name: &str, input: Value) -> String {
    json!({"type": "assistant", "message": {"id": msg, "content": [
        {"type": "tool_use", "id": id, "name": name, "input": input}]}})
    .to_string()
}

fn said(msg: &str, text: &str) -> String {
    json!({"type": "assistant", "message": {"id": msg, "content": [{"type": "text", "text": text}]}})
        .to_string()
}

fn tool_result(id: &str, output: &str) -> String {
    json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": id, "content": output}]}})
    .to_string()
}

/// The captured run draws what it did: a Write that names its file, a Bash with its output, and
/// the closing sentence once — not once from the deltas and again from the complete message.
#[test]
fn the_captured_turn_folds_to_what_it_did() {
    let (f, _) = live(&fixture_lines(), Duration::from_millis(5));
    let t = &f.turns[0];
    let kinds: Vec<&str> = t.steps.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["call", "call", "say"]);
    let write = &t.calls["toolu_018GCgMQ39vQNzkJhkmbBgGX"];
    assert_eq!(write.tool, "Write");
    assert_eq!(write.state, Some(CallState::Ok));
    assert!(
        write.input.as_ref().unwrap()["file_path"]
            .as_str()
            .unwrap()
            .ends_with("hello.txt")
    );
    assert_eq!(t.calls["toolu_01KdMu5HBoa4dJfUh3nw5FMM"].tool, "Bash");
    let say = &t.text[&t.steps[2].1];
    assert!(say.starts_with("Created `hello.txt`"), "{say}");
    assert_eq!(
        say.matches("Created").count(),
        1,
        "the sentence once: {say}"
    );
    assert_eq!(t.closed, Some(Ending::Done));
    assert!(f.running().is_empty());
}

/// The same conversation read from its transcript draws the same turn: the same steps with the
/// same ids, the same calls with the same inputs and answers, the same prose. The transcript half
/// is the captured run's own complete records, which is what Claude Code writes to disk. Raw lines
/// and usage are left out by name — a live run has a `result` that assigns the authoritative total
/// and a transcript has none.
#[test]
fn replay_and_live_fold_to_the_same_turn() {
    let lines = fixture_lines();
    let (live, _) = live(&lines, Duration::from_millis(5));
    let mut on_disk = vec![opener("u-1", "ask")];
    on_disk.extend(
        lines
            .iter()
            .filter(|l| {
                let r: Value = serde_json::from_str(l).unwrap_or_default();
                matches!(r["type"].as_str(), Some("assistant" | "user"))
            })
            .cloned(),
    );
    let replay = transcript(&on_disk);
    let (a, b) = (&live.turns[0], &replay.turns[0]);
    assert_eq!(a.steps, b.steps);
    assert_eq!(a.text, b.text);
    assert_eq!(a.calls, b.calls);
    assert_eq!(b.key.as_deref(), Some("u-1"));
}

/// The batching clock moves where frames split and nothing else: fed with every token its own
/// frame, or all of them in one, the turn is the same.
#[test]
fn batching_does_not_change_the_fold() {
    let lines = fixture_lines();
    let (every, _) = live(&lines, Duration::from_secs(1));
    let (none, _) = live(&lines, Duration::ZERO);
    let (some, _) = live(&lines, Duration::from_millis(17));
    assert_eq!(every, none);
    assert_eq!(every, some);
}

/// Each byte of a reply goes out once, and ten thousand tokens are not ten thousand frames.
#[test]
fn text_ops_carry_each_byte_once() {
    let mut d = Decoder::new(Provider::Claude, Source::Chat);
    let mut frames = d.open(None, None, None, None);
    let mut now = Instant::now();
    let start = now;
    frames.extend(d.feed(
        &json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "m"}}}).to_string(),
        None,
        now,
    ));
    let mut sent = 0;
    for i in 0..10_000 {
        let t = format!("w{i} ");
        sent += t.len();
        let delta = json!({"type": "stream_event", "event": {"type": "content_block_delta",
            "index": 0, "delta": {"type": "text_delta", "text": t}}});
        frames.extend(d.feed(&delta.to_string(), None, now));
        now += Duration::from_millis(1);
    }
    frames.extend(d.close(Ending::Done));
    let texts: Vec<&String> = frames
        .iter()
        .filter_map(|f| match &f.op {
            Op::Text { append, .. } => Some(append),
            _ => None,
        })
        .collect();
    assert_eq!(texts.iter().map(|t| t.len()).sum::<usize>(), sent);
    let budget = (now - start).as_millis() as usize / BATCH.as_millis() as usize + 2;
    assert!(
        texts.len() <= budget,
        "{} text frames for {budget}",
        texts.len()
    );
}

/// Every way a turn can end answers every call still running. The bug this replaces: Stop, a
/// dead stream, or a replay ending on an unanswered call left a row spinning for good.
#[test]
fn every_ending_closes_every_open_call() {
    let start = json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0,
        "content_block": {"type": "tool_use", "id": "c1", "name": "Bash"}}})
    .to_string();
    for ending in [
        Ending::Stopped,
        Ending::Failed,
        Ending::Interrupted,
        Ending::Done,
    ] {
        let mut d = Decoder::new(Provider::Claude, Source::Chat);
        let mut frames = d.open(Some("x".into()), None, None, None);
        frames.extend(d.feed(&start, None, Instant::now()));
        frames.extend(d.close(ending));
        let mut f = Fold::default();
        f.apply(&frames);
        assert!(f.running().is_empty(), "{ending:?} left a call running");
        assert_eq!(f.turns[0].calls["c1"].state, Some(CallState::Interrupted));
    }

    // A replay that ends on an unanswered call, and one superseded by the next prompt.
    let lines = vec![
        opener("u-1", "first"),
        tool_use("m1", "c1", "Bash", json!({"command": "sleep 99"})),
        opener("u-2", "second"),
        tool_use("m2", "c2", "Read", json!({"file_path": "a"})),
    ];
    let f = transcript(&lines);
    assert!(f.running().is_empty());
    assert_eq!(f.turns[0].closed, Some(Ending::Superseded));
    assert_eq!(f.turns[1].closed, Some(Ending::CaughtUp));
}

/// A transcript says a turn ended with `turn_duration`, not with `end_turn` — a turn carries on
/// after one of those — and a background job's notification is not a prompt.
#[test]
fn a_transcript_splits_on_prompts_and_ends_on_turn_duration() {
    let notification = json!({"type": "user", "uuid": "n", "message": {"content":
        "<task-notification><task-id>b1</task-id></task-notification>"}})
    .to_string();
    let lines = vec![
        opener("u-1", "first"),
        said("m1", "one"),
        notification,
        said("m2", "two"),
        json!({"type": "system", "subtype": "turn_duration", "durationMs": 5}).to_string(),
    ];
    let f = transcript(&lines);
    assert_eq!(f.turns.len(), 1, "the notification did not split the turn");
    assert_eq!(f.turns[0].steps.len(), 2);
    assert_eq!(f.turns[0].closed, Some(Ending::Done));
}

/// The head of a long replay is cut off. What is left opens a turn with no prompt, and a result
/// for a call that was cut off is not drawn as a call.
#[test]
fn a_truncated_head_opens_a_headless_turn() {
    let f = transcript(&[tool_result("gone", "out"), said("m1", "after")]);
    assert_eq!(f.turns.len(), 1);
    assert_eq!(f.turns[0].prompt, None);
    assert!(f.turns[0].calls.is_empty());
}

/// A transcript repeats a message's usage on every block's record. Counted once.
#[test]
fn usage_is_counted_once_per_message() {
    let rec = |block: Value| {
        json!({"type": "assistant", "message": {"id": "m1", "content": [block],
            "usage": {"input_tokens": 10, "output_tokens": 5}}})
        .to_string()
    };
    let f = transcript(&[
        opener("u", "x"),
        rec(json!({"type": "text", "text": "a"})),
        rec(json!({"type": "tool_use", "id": "c", "name": "Read", "input": {"file_path": "f"}})),
    ]);
    assert_eq!(f.turns[0].usage, Some((10, 5)));
}

/// A `Write` streaming a huge file does not grow a buffer without bound; the complete message
/// supplies the input instead.
#[test]
fn partial_input_is_capped() {
    let mut d = Decoder::new(Provider::Claude, Source::Chat);
    let now = Instant::now();
    let mut frames = d.open(None, None, None, None);
    frames.extend(
        d.feed(
            &json!({"type": "stream_event", "event": {"type": "content_block_start",
        "index": 0, "content_block": {"type": "tool_use", "id": "w", "name": "Write"}}})
            .to_string(),
            None,
            now,
        ),
    );
    let chunk = "x".repeat(64 * 1024);
    for _ in 0..80 {
        let e = json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": chunk}}});
        frames.extend(d.feed(&e.to_string(), None, now));
    }
    assert!(d.turn.as_ref().unwrap().partial["w"].len() <= PARTIAL_CAP + chunk.len());
    frames.extend(
        d.feed(
            &json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
                .to_string(),
            None,
            now,
        ),
    );
    frames.extend(d.feed(
        &tool_use(
            "m",
            "w",
            "Write",
            json!({"file_path": "big.txt", "content": "…"}),
        ),
        None,
        now,
    ));
    let mut f = Fold::default();
    f.apply(&frames);
    assert_eq!(
        f.turns[0].calls["w"].input.as_ref().unwrap()["file_path"],
        "big.txt"
    );
}

/// A failure Claude Code wrote about itself is a failure, not prose — and in a transcript it is
/// history, not something to act on now.
#[test]
fn a_synthetic_failure_is_a_failure_and_history_on_replay() {
    let rec = json!({"type": "assistant", "error": "rate_limit", "quotaLimits": {"resetsAt": 1.0},
        "message": {"id": "m", "model": "<synthetic>", "content": [{"type": "text", "text": "Limit hit"}]}})
    .to_string();
    let mut d = Decoder::new(Provider::Claude, Source::Transcript);
    let frames = d.feed(&rec, None, Instant::now());
    let failure = frames.iter().find_map(|f| match &f.op {
        Op::Failure {
            kind,
            history,
            resets_at,
            ..
        } => Some((kind.clone(), *history, *resets_at)),
        _ => None,
    });
    assert_eq!(failure, Some(("rate_limit".into(), true, Some(1.0))));
    assert!(!frames.iter().any(|f| matches!(f.op, Op::Text { .. })));
}

/// A line that is not JSON is kept, marked, and counted against nothing else.
#[test]
fn an_unreadable_line_is_raw_and_marked() {
    let mut d = Decoder::new(Provider::Claude, Source::Chat);
    let frames = d.feed("not json {", None, Instant::now());
    assert!(
        frames
            .iter()
            .any(|f| matches!(&f.op, Op::Raw { lines } if lines[0].unreadable))
    );
}

/// A long output keeps its head and tail and says how much went.
#[test]
fn a_long_output_is_cut_in_the_middle() {
    let (out, cut) = cap_middle("a".repeat(OUTPUT_CAP) + &"b".repeat(1000), OUTPUT_CAP);
    assert!(out.starts_with('a') && out.ends_with('b'));
    assert_eq!(cut, 1000);
}

/// Codex: a command is a call with its answer, and a message is a step of its own.
#[test]
fn codex_commands_and_messages() {
    let mut d = Decoder::new(Provider::Codex, Source::Chat);
    let now = Instant::now();
    let mut frames = d.open(Some("x".into()), None, None, None);
    for e in [
        json!({"type": "item.started", "item": {"id": "i1", "type": "command_execution", "command": "ls"}}),
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "aggregated_output": "a", "exit_code": 0}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message", "text": "done"}}),
    ] {
        frames.extend(d.feed(&e.to_string(), None, now));
    }
    frames.extend(d.close(Ending::Done));
    let mut f = Fold::default();
    f.apply(&frames);
    let t = &f.turns[0];
    assert_eq!(t.calls["i1"].state, Some(CallState::Ok));
    assert_eq!(t.steps.last().unwrap().0, "say");
    assert_eq!(t.text["codex:i2"], "done");
}

/// The wire shape a client reads.
#[test]
fn a_frame_serialises_flat() {
    let f = Frame {
        turn: Some("u".into()),
        op: Op::Text {
            step: "m:0".into(),
            kind: TextKind::Say,
            append: "hi".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&f).unwrap(),
        json!({"turn": "u", "op": "text", "step": "m:0", "kind": "say", "append": "hi"})
    );
}

/// Claude Code's bookkeeping between turns is not a turn. It opened empty, prompt-less ones.
#[test]
fn bookkeeping_between_turns_is_not_a_turn() {
    let f = transcript(&[
        opener("u-1", "first"),
        said("m1", "one"),
        json!({"type": "system", "subtype": "turn_duration"}).to_string(),
        json!({"type": "system", "subtype": "away_summary", "content": "…"}).to_string(),
        json!({"type": "system", "subtype": "scheduled_task_fire"}).to_string(),
    ]);
    assert_eq!(f.turns.len(), 1);
    assert_eq!(f.turns[0].closed, Some(Ending::Done));
}
