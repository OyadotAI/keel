//! Antigravity's own record of an interactive conversation, read as lines a follower can take.
//!
//! `agy` keeps each conversation as a SQLite database, `conversations/<id>.db` under its home,
//! with one row per step: the person's input, a model response, a tool. The payload is protobuf
//! with no published schema; the fields read here were measured against agy 1.4.3 (a headless run
//! and an interactive one, step by step as they were written), and nothing else is guessed at.
//!
//! Each finished step becomes one JSON line, so the follower, the decoder and the replay path are
//! the same ones a JSONL transcript uses. A step still being written — a response generating, a
//! tool pending or running — is withheld, and so is everything after it, until it settles: the
//! same rule as withholding a partial line. The cursor is a step index, not a byte offset.
//!
//! A turn opens on the person's input and ends on a finished response that calls no tool. Every
//! row can read "done" for seconds mid-turn — a response finished, the tool it called not yet
//! written — so "all done" is not the end; a response with no call is.

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Value, json};

use crate::{Kind, Real};

mod observation;
pub(crate) use observation::ObservationReader;
pub(crate) use observation::subagent;
pub use observation::{Observation, ReportedUsage};

/// Step types, from agy's own `step_type` column.
const USER_INPUT: i64 = 14;
const RESPONSE: i64 = 15;
const TOOL: i64 = 132;

/// Done.
const DONE: i64 = 3;
/// Cancelled by the person — Esc or ⌃C — and nothing is written after it.
const CANCELLED: i64 = 6;
/// A tool that failed or was refused; the reason is in field 31.
const FAILED: i64 = 7;
/// The statuses a step does not change from. Everything else — generating (8), pending and
/// running (1, 2), waiting on the person's permission (9, measured on agy 1.4.3) — is still being written, and a step read
/// in one of those was never read again: a tool waiting on a yes was drawn as answered, and its
/// cancel never seen.
const SETTLED: &[i64] = &[DONE, CANCELLED, FAILED];

/// Steps sent when catching up on a conversation that was already long.
pub(crate) const MAX_REPLAY_STEPS: u64 = 1_500;
/// What of one database file is searched for its workspace.
const HEAD_BYTES: u64 = 8 * 1024 * 1024;

/// Where `agy` keeps its conversations.
pub fn conversations_dir() -> Option<Utf8PathBuf> {
    Some(crate::home()?.join(".gemini/antigravity-cli/conversations"))
}

/// The database of conversation `id`, if it ran in `cwd`. The id is checked as a file name before
/// it is used as one, and the database must record this workspace: the path alone would let any
/// request read any conversation on the machine.
pub fn conversation(dir: &Utf8Path, cwd: &Utf8Path, id: &str) -> Option<Utf8PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let path = dir.join(format!("{id}.db"));
    let real = cwd.real().unwrap_or_else(|_| cwd.to_owned());
    let needles: Vec<Vec<u8>> = [cwd, real.as_path()]
        .iter()
        .map(|c| workspace_field(c.as_str()))
        .collect();
    let wal = Utf8PathBuf::from(format!("{path}-wal"));
    let recorded = [path.as_path(), wal.as_path()]
        .iter()
        .filter_map(|f| head(f))
        .any(|bytes| {
            needles
                .iter()
                .any(|n| bytes.windows(n.len()).any(|w| w == n.as_slice()))
        });
    recorded.then_some(path)
}

/// The workspace as agy records it: a protobuf string, the `file://` URI with its length in
/// front. Matching the whole field means `/repo/.keel/worktrees/x` is not taken for `/repo`.
pub fn workspace_field(cwd: &str) -> Vec<u8> {
    let text = format!("file://{cwd}");
    let mut out = varint(text.len() as u64);
    out.extend_from_slice(text.as_bytes());
    out
}

fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while n >= 0x80 {
        out.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
    out
}

fn head(path: &Utf8Path) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(path).ok()?, HEAD_BYTES),
        &mut out,
    )
    .ok()?;
    Some(out)
}

/// Read-only, and never a writer: `mode=ro` while agy has it open, `immutable` when the
/// write-ahead log's index cannot be opened (a conversation nobody is running).
fn open(path: &Utf8Path) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let try_open = |query: &str| {
        let c =
            rusqlite::Connection::open_with_flags(format!("file:{path}?{query}"), flags).ok()?;
        c.busy_timeout(std::time::Duration::from_millis(500)).ok()?;
        c.query_row("SELECT count(*) FROM steps", [], |_| Ok(()))
            .ok()?;
        Some(c)
    };
    try_open("mode=ro").or_else(|| try_open("immutable=1"))
}

/// The settled steps from index `from` on, as lines, and the index to ask from next.
pub(crate) fn steps(path: &Utf8Path, from: u64, max: u64) -> Option<(Vec<String>, u64)> {
    let c = open(path)?;
    let mut q = c
        .prepare("SELECT idx, step_type, status, step_payload FROM steps WHERE idx >= ?1 ORDER BY idx LIMIT ?2")
        .ok()?;
    let rows = q
        .query_map([from as i64, max as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
            ))
        })
        .ok()?;
    let mut lines = Vec::new();
    let mut next = from;
    for row in rows {
        let Ok((idx, kind, status, payload)) = row else {
            break;
        };
        // Out of order means a step not yet written between two that are: wait for it.
        if idx as u64 != next || !SETTLED.contains(&status) {
            break;
        }
        lines.push(line(idx, kind, status, &payload).to_string());
        next = idx as u64 + 1;
    }
    Some((lines, next))
}

/// How many steps the conversation has.
pub(crate) fn count(path: &Utf8Path) -> Option<u64> {
    open(path)?
        .query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, i64>(0))
        .ok()
        .map(|n| n as u64)
}

/// One step as the line the decoder and the follower read.
fn line(idx: i64, kind: i64, status: i64, payload: &[u8]) -> Value {
    let f = Fields::parse(payload);
    let mut v = json!({ "agy": idx, "step": kind, "status": status });
    if let (Some(secs), nanos) = (f.int(&[5, 1, 1]), f.int(&[5, 1, 2]).unwrap_or(0)) {
        v["at"] = crate::iso(secs, (nanos / 1_000_000) as u32).into();
    }
    match kind {
        USER_INPUT => put(&mut v, &f, "prompt", &[19, 2]),
        RESPONSE => {
            put(&mut v, &f, "text", &[20, 1]);
            put(&mut v, &f, "thinking", &[20, 3]);
            if let Some(name) = f.text(&[20, 7, 2]) {
                v["call"] = json!({
                    "id": f.text(&[20, 7, 1]).unwrap_or_default(),
                    "name": name,
                    "args": f.text(&[20, 7, 3]).and_then(|a| serde_json::from_str::<Value>(&a).ok()).unwrap_or(json!({})),
                });
            }
            let n = |i: u32| f.int(&[5, 9, i]).unwrap_or(0);
            if [2, 3, 5].iter().any(|i| f.int(&[5, 9, *i]).is_some()) {
                v["usage"] =
                    json!({ "input": n(2), "output": n(3), "thinking": n(9), "cache_read": n(5) });
            }
        }
        TOOL => {
            v["call"] = json!({
                "id": f.text(&[5, 4, 1]).unwrap_or_default(),
                "name": f.text(&[5, 4, 2]).unwrap_or_default(),
                "args": f.text(&[5, 4, 3]).and_then(|a| serde_json::from_str::<Value>(&a).ok()).unwrap_or(json!({})),
            });
            put(&mut v, &f, "output", &[140, 2, 1]);
            put(&mut v, &f, "error", &[31, 2]);
        }
        _ => {}
    }
    v
}

/// Set `key` to the text at `path`, when there is some.
fn put(v: &mut Value, f: &Fields, key: &str, path: &[u32]) {
    if let Some(s) = f.text(path) {
        v[key] = s.into();
    }
}

/// The fields of a protobuf message, by path, from the wire format alone. A length-delimited
/// field that parses as a message is walked into; one that does not is kept as bytes.
#[derive(Default)]
struct Fields(Vec<(Vec<u32>, Field)>);

enum Field {
    Int(i64),
    Bytes(Vec<u8>),
}

impl Fields {
    fn parse(b: &[u8]) -> Self {
        let mut out = Fields::default();
        walk(b, &mut Vec::new(), &mut out.0, 0);
        out
    }

    fn int(&self, path: &[u32]) -> Option<i64> {
        self.0.iter().find_map(|(p, f)| match f {
            Field::Int(n) if p == path => Some(*n),
            _ => None,
        })
    }

    fn text(&self, path: &[u32]) -> Option<String> {
        self.0.iter().find_map(|(p, f)| match f {
            Field::Bytes(b) if p == path => std::str::from_utf8(b).ok().map(str::to_string),
            _ => None,
        })
    }
}

/// How deep a message is walked. agy's deepest field read here is three levels down.
const MAX_DEPTH: usize = 10;

/// Parse `b` as a message at `path`, or report that it is not one. `path` is left as it was on
/// every way out: an early return from a blob that is not a message left a field number on it,
/// and every field read after that was filed under the wrong path.
fn walk(b: &[u8], path: &mut Vec<u32>, out: &mut Vec<(Vec<u32>, Field)>, depth: usize) -> bool {
    let n = path.len();
    let ok = walk_fields(b, path, out, depth);
    path.truncate(n);
    ok
}

fn walk_fields(
    b: &[u8],
    path: &mut Vec<u32>,
    out: &mut Vec<(Vec<u32>, Field)>,
    depth: usize,
) -> bool {
    let mut i = 0;
    let mut found = Vec::new();
    while i < b.len() {
        let Some((key, n)) = read_varint(&b[i..]) else {
            return false;
        };
        i += n;
        let (field, wire) = ((key >> 3) as u32, key & 7);
        if field == 0 {
            return false;
        }
        path.push(field);
        match wire {
            0 => {
                let Some((v, n)) = read_varint(&b[i..]) else {
                    return false;
                };
                i += n;
                found.push((path.clone(), Field::Int(v as i64)));
            }
            1 => i += 8,
            5 => i += 4,
            2 => {
                let Some((len, n)) = read_varint(&b[i..]) else {
                    return false;
                };
                i += n;
                // A length near `u64::MAX` from a malformed blob overflows the index otherwise.
                let Some(end) = usize::try_from(len).ok().and_then(|l| i.checked_add(l)) else {
                    return false;
                };
                let Some(chunk) = b.get(i..end) else {
                    return false;
                };
                i = end;
                // Kept as bytes *and* walked into: the wire cannot say whether a length-delimited
                // field is text or a message, and a message whose bytes all happen to be printable
                // read as text lost every field inside it. Lookups are by exact path, so text that
                // also parses as a message adds paths nothing asks for.
                found.push((path.clone(), Field::Bytes(chunk.to_vec())));
                let mut inner = Vec::new();
                if depth < MAX_DEPTH
                    && !chunk.is_empty()
                    && walk(chunk, path, &mut inner, depth + 1)
                {
                    found.extend(inner);
                }
            }
            _ => return false,
        }
        path.pop();
    }
    if i != b.len() {
        return false;
    }
    out.extend(found);
    true
}

fn read_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut n = 0u64;
    for (i, byte) in b.iter().enumerate().take(10) {
        n |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((n, i + 1));
        }
    }
    None
}

/// The conversations an `invoke_subagent` created, from its output: agy answers the call as soon
/// as they exist ("Created the following subagents: {"conversationId": …}") and each then runs in
/// a database of its own, so the call finishing says nothing about the subagent finishing.
pub(crate) fn children(output: &str) -> Vec<String> {
    const KEY: &str = "\"conversationId\": \"";
    output
        .match_indices(KEY)
        .filter_map(|(i, _)| {
            let rest = &output[i + KEY.len()..];
            let id = &rest[..rest.find('"')?];
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
                .then(|| id.to_string())
        })
        .collect()
}

/// Whether a subagent's own conversation, beside its parent's, has finished its turn. A database
/// that is not there or cannot be read counts as finished: a subagent marked working forever is
/// the worse failure.
pub(crate) fn finished(parent: &Utf8Path, child: &str) -> bool {
    if child.is_empty() || !child.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return true;
    }
    let path = parent.with_file_name(format!("{child}.db"));
    let Some(total) = count(&path) else {
        return true;
    };
    let Some((lines, next)) = steps(&path, total.saturating_sub(1), 1) else {
        return true;
    };
    next == total
        && lines
            .last()
            .and_then(|l| serde_json::from_str::<Value>(l).ok())
            .is_some_and(|v| ends(&v))
}

/// The line that says a subagent finished.
pub(crate) fn child_done(id: &str) -> String {
    json!({ "agy_child": id }).to_string()
}

/// Every settled step is shown.
pub(crate) fn followable(_: &Value) -> bool {
    true
}

/// What a step line is, for a follower deciding what the conversation is doing.
pub fn kind_of(line: &str) -> Kind {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Kind::Other;
    };
    if let Some((uuid, prompt)) = opener(&v) {
        return Kind::Opener {
            uuid,
            prompt: prompt.chars().take(200).collect(),
            at: v["at"].as_str().unwrap_or_default().to_string(),
            cwd: None,
        };
    }
    if ends(&v) { Kind::TurnEnd } else { Kind::Other }
}

/// The key of the turn a step opens, if it is the person's input.
pub fn opener_of(line: &str) -> Option<String> {
    opener(&serde_json::from_str(line).ok()?).map(|(k, _)| k)
}

fn opener(v: &Value) -> Option<(String, String)> {
    if v["step"] != USER_INPUT {
        return None;
    }
    let prompt = v["prompt"].as_str().unwrap_or_default().trim().to_string();
    Some((key(v["agy"].as_i64()?), prompt))
}

/// The key a turn is filed under: the index of the step that opened it, stable for the life of
/// the conversation because steps are only ever appended.
pub(crate) fn key(idx: i64) -> String {
    format!("turn-{idx}")
}

/// A finished response that calls no tool: the turn is over. A step cancelled by Esc or ⌃C ends
/// it too — agy writes nothing after one, and waits for the person.
pub(crate) fn ends(v: &Value) -> bool {
    v["status"] == CANCELLED
        || denied_by_the_person(v)
        || (v["step"] == RESPONSE && (v["status"] != DONE || v["call"].is_null()))
}

/// A tool the person refused at agy's own permission question. agy marks it failed and writes
/// nothing after it, unlike a refusal by Keel's hook, after which it carries on — so it is the
/// error, not the status, that says the turn is over. Measured as the last step of four local
/// conversations.
fn denied_by_the_person(v: &Value) -> bool {
    v["step"] == TOOL
        && v["status"] == FAILED
        && v["error"]
            .as_str()
            .is_some_and(|e| e.contains("user denied permission"))
}

/// Whether a step was cancelled.
pub(crate) fn cancelled(v: &Value) -> bool {
    v["status"] == CANCELLED
}

/// Whether a tool step failed.
pub(crate) fn failed(v: &Value) -> bool {
    v["status"] == FAILED || v["error"].is_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny protobuf writer, for payloads in the shape agy writes.
    fn msg(fields: &[(u32, Pb)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, v) in fields {
            match v {
                Pb::I(i) => {
                    out.extend(varint(u64::from(*n) << 3));
                    out.extend(varint(*i));
                }
                Pb::S(s) => {
                    out.extend(varint((u64::from(*n) << 3) | 2));
                    out.extend(varint(s.len() as u64));
                    out.extend(s.as_bytes());
                }
                Pb::M(m) => {
                    out.extend(varint((u64::from(*n) << 3) | 2));
                    out.extend(varint(m.len() as u64));
                    out.extend(m);
                }
            }
        }
        out
    }
    enum Pb {
        I(u64),
        S(&'static str),
        M(Vec<u8>),
    }
    use Pb::*;

    fn created() -> (u32, Pb) {
        (
            5,
            M(msg(&[(
                1,
                M(msg(&[(1, I(1_792_590_739)), (2, I(672_000_000))])),
            )])),
        )
    }

    /// The fields measured on agy 1.4.3 come out as the line: the prompt, a response's text,
    /// thinking, tool call and usage, a tool's output and its refusal.
    #[test]
    fn a_step_reads_as_its_line() {
        let input = line(
            0,
            USER_INPUT,
            DONE,
            &msg(&[(1, I(14)), created(), (19, M(msg(&[(2, S("fix it"))])))]),
        );
        assert_eq!(input["prompt"], "fix it");
        assert_eq!(input["at"], "2026-10-21T13:52:19.672Z");
        let call = msg(&[
            (1, S("call_1")),
            (2, S("run_command")),
            (3, S(r#"{"CommandLine":"ls"}"#)),
        ]);
        let usage = msg(&[(2, I(6778)), (3, I(85)), (5, I(4724)), (9, I(38))]);
        let resp = line(
            1,
            RESPONSE,
            DONE,
            &msg(&[
                (5, M(msg(&[(9, M(usage))]))),
                (20, M(msg(&[(3, S("hmm")), (7, M(call.clone()))]))),
            ]),
        );
        assert_eq!(resp["thinking"], "hmm");
        assert_eq!(resp["call"]["name"], "run_command");
        assert_eq!(resp["call"]["args"]["CommandLine"], "ls");
        assert_eq!(
            resp["usage"],
            json!({"input":6778,"output":85,"thinking":38,"cache_read":4724})
        );
        let tool = line(
            2,
            TOOL,
            DONE,
            &msg(&[
                (5, M(msg(&[(4, M(call))]))),
                (140, M(msg(&[(2, M(msg(&[(1, S("a.rs\n"))])))]))),
            ]),
        );
        assert_eq!(tool["output"], "a.rs\n");
        assert_eq!(tool["call"]["id"], "call_1");
        let refused = line(
            3,
            TOOL,
            FAILED,
            &msg(&[(31, M(msg(&[(2, S("denied by hook"))])))]),
        );
        assert!(failed(&refused));
        assert_eq!(refused["error"], "denied by hook");
    }

    /// A turn ends on a finished response with no call — not on "every step done", which is true
    /// for seconds mid-turn — or on a response that stopped any other way.
    #[test]
    fn a_turn_ends_on_a_response_that_calls_nothing() {
        let open = json!({"agy":0,"step":14,"status":3,"prompt":"go"}).to_string();
        assert!(
            matches!(kind_of(&open), Kind::Opener { uuid, prompt, .. } if uuid == "turn-0" && prompt == "go")
        );
        let calling = json!({"agy":1,"step":15,"status":3,"call":{"name":"read_file"}}).to_string();
        assert_eq!(kind_of(&calling), Kind::Other);
        let tool = json!({"agy":2,"step":132,"status":3}).to_string();
        assert_eq!(kind_of(&tool), Kind::Other);
        let done = json!({"agy":3,"step":15,"status":3,"text":"ok"}).to_string();
        assert_eq!(kind_of(&done), Kind::TurnEnd);
        let failed_response = json!({"agy":3,"step":15,"status":7,"call":{"name":"x"}}).to_string();
        assert_eq!(kind_of(&failed_response), Kind::TurnEnd);
        // Measured: Esc then ⌃C at agy's permission question leaves the tool step cancelled and
        // writes nothing after it.
        let cancelled =
            json!({"agy":6,"step":132,"status":6,"call":{"name":"run_command"}}).to_string();
        assert_eq!(kind_of(&cancelled), Kind::TurnEnd);
        // Refused at agy's own question: over. Refused by Keel's hook: agy carries on.
        let denied = json!({"agy":7,"step":132,"status":7,"error":"permission check failed for unsandboxed \"rm x\": user denied permission to run command"}).to_string();
        assert_eq!(kind_of(&denied), Kind::TurnEnd);
        let hooked =
            json!({"agy":7,"step":132,"status":7,"error":"tool call denied by pre-tool hook: no"})
                .to_string();
        assert_eq!(kind_of(&hooked), Kind::Other);
    }

    /// A length that would run past the end, or overflow, is a malformed message, never a panic;
    /// and a nested message whose bytes are all printable is still walked into.
    #[test]
    fn a_malformed_or_printable_message_is_read_safely() {
        let mut huge = vec![0x0a];
        huge.extend([0xff; 9]);
        huge.push(0x01);
        assert!(Fields::parse(&huge).0.is_empty());
        let inner = msg(&[(1, S("hi there, this is printable text!"))]);
        let f = Fields::parse(&msg(&[(20, M(inner))]));
        assert_eq!(
            f.text(&[20, 1]).as_deref(),
            Some("hi there, this is printable text!")
        );
    }

    fn database(path: &Utf8Path, rows: &[(i64, i64, i64, Vec<u8>)]) {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE steps (idx integer PRIMARY KEY, step_type integer, status integer, step_payload blob);")
            .unwrap();
        for (i, t, s, p) in rows {
            c.execute(
                "INSERT INTO steps VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![i, t, s, p],
            )
            .unwrap();
        }
    }

    /// A step still being written holds back itself and everything after it, as a partial line
    /// does; the cursor moves only past settled steps.
    #[test]
    fn a_step_still_being_written_is_withheld() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("c.db");
        database(
            &path,
            &[(0, 14, 3, vec![]), (1, 15, 8, vec![]), (2, 132, 3, vec![])],
        );
        let (lines, next) = steps(&path, 0, 100).unwrap();
        assert_eq!((lines.len(), next), (1, 1));
        assert_eq!(
            steps(&path, 1, 100).unwrap(),
            (vec![], 1),
            "the cursor waits on the step"
        );
        assert_eq!(count(&path), Some(3));
        // A status nobody has named yet — waiting on a permission answer — waits too.
        let waiting = Utf8Path::from_path(tmp.path()).unwrap().join("w.db");
        database(&waiting, &[(0, 14, 3, vec![]), (1, 132, 9, vec![])]);
        assert_eq!(steps(&waiting, 0, 100).unwrap().1, 1);
    }

    /// A subagent has finished when its own conversation's last step ends a turn; one still
    /// working has not; one whose database is gone counts as finished rather than working forever.
    #[test]
    fn a_subagent_finishes_when_its_own_conversation_does() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let parent = dir.join("parent.db");
        database(
            &dir.join("done-1.db"),
            &[(0, 14, 3, vec![]), (1, 15, 3, vec![])],
        );
        database(
            &dir.join("busy-1.db"),
            &[(0, 14, 3, vec![]), (1, 15, 8, vec![])],
        );
        assert!(finished(&parent, "done-1"));
        assert!(!finished(&parent, "busy-1"));
        assert!(finished(&parent, "gone-1"));
        assert!(finished(&parent, "../x"));
        let out = "Created the following subagents:\n{\n  \"conversationId\": \"done-1\",\n}\n{\n  \"conversationId\": \"busy-1\"\n}";
        assert_eq!(children(out), ["done-1", "busy-1"]);
    }

    /// A conversation is found only by its own id, in its own workspace.
    #[test]
    fn a_conversation_is_found_only_in_its_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut body = b"SQLite format 3\0junk".to_vec();
        body.extend(workspace_field(repo.as_str()));
        std::fs::write(root.join("abc-1.db"), &body).unwrap();
        assert_eq!(
            conversation(root, &repo, "abc-1"),
            Some(root.join("abc-1.db"))
        );
        assert_eq!(
            conversation(root, &repo.join("sub"), "abc-1"),
            None,
            "another workspace"
        );
        assert_eq!(conversation(root, &repo, "../abc-1"), None);
        assert_eq!(conversation(root, &repo, "missing"), None);
    }
}
