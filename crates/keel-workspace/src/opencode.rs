//! OpenCode's own record of a conversation, read as lines a follower can take.
//!
//! OpenCode keeps every session of every project in one SQLite database (`opencode.db` under its
//! data directory): a `session` row, its `message` rows, and each message's `part` rows — a piece
//! of text, a tool call, the end of a step. Measured on opencode 1.18.30; sessions written by
//! 1.16 read the same.
//!
//! Each settled part becomes one line in the shape `opencode run --format json` prints, so a live
//! chat turn and a followed or replayed one go through one decoder. Two lines the stream never
//! says are added: `user`, where the person's message opens a turn, and `turn_end`, where a
//! message ended in an error and wrote no part to say so.
//!
//! The database is shared and rows are rewritten in place, so the cursor is neither a byte nor a
//! row count: it is the time-ordered prefix of the last part id read.

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Value, json};

use crate::Kind;

/// Parts sent in one read, and when catching up on a session that was already long.
pub(crate) const MAX_PARTS: u64 = 1_500;
/// A part larger than this is sent as a stub: one image attached to a prompt is 1.5 MB.
const MAX_PART_BYTES: i64 = 256 * 1024;
/// What one read returns at most, whatever the parts are.
const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
/// How long a read waits on OpenCode's own write lock before giving the follower nothing new.
const BUSY: std::time::Duration = std::time::Duration::from_millis(500);
/// Where a part id's time-ordered prefix starts and how long it is: `prt_` then twelve hex.
const ID_PREFIX: usize = 4;
const ID_HEX: usize = 12;
/// What a prompt is cut to in a follower's summary of a turn.
const SUMMARY_CHARS: usize = 200;

/// OpenCode's database. `OPENCODE_DB` is OpenCode's own override; the default is its XDG data
/// directory, which it uses on every platform.
// ponytail: the Windows location is unmeasured (`opencode debug paths` on a real machine).
pub fn db_path() -> Option<Utf8PathBuf> {
    if let Some(p) = std::env::var_os("OPENCODE_DB").and_then(|p| p.into_string().ok()) {
        return Some(Utf8PathBuf::from(p));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .and_then(|p| p.into_string().ok())
        .map(Utf8PathBuf::from)
        .or_else(|| Some(crate::home()?.join(".local/share")))?;
    Some(data.join("opencode/opencode.db"))
}

/// Whether `id` has the shape of an OpenCode session id. Checked before it reaches a query or a
/// command line.
pub fn is_session_id(id: &str) -> bool {
    id.strip_prefix("ses_")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Read-only, and never a writer: no pragma, no `immutable` (OpenCode has it open and a reader
/// that ignores the write-ahead log reads a database that is minutes old).
pub(crate) fn open(db: &Utf8Path) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let c = rusqlite::Connection::open_with_flags(format!("file:{db}?mode=ro"), flags).ok()?;
    c.busy_timeout(BUSY).ok()?;
    Some(c)
}

/// Whether session `id` ran in `cwd`. The id alone would let any request read any conversation
/// on the machine.
pub fn session_in(db: &Utf8Path, cwd: &Utf8Path, id: &str) -> bool {
    if !is_session_id(id) {
        return false;
    }
    let Some(c) = open(db) else { return false };
    let dir: Option<String> = c
        .query_row("SELECT directory FROM session WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .ok();
    dir.is_some_and(|d| crate::moved::same_place(Utf8Path::new(&d), cwd))
}

/// The session a terminal started in `cwd` at or after `since` (milliseconds): the earliest one
/// nobody else runs. A subagent's session has a parent and is never the terminal's own.
// ponytail: a chat lane's `opencode run` begun in the same checkout in the same seconds could be
// taken for the terminal's; `taken` covers every session a lane already holds.
pub fn started(
    db: &Utf8Path,
    cwd: &Utf8Path,
    since: i64,
    taken: fn(&str) -> bool,
) -> Option<String> {
    let c = open(db)?;
    let mut q = c
        .prepare(
            "SELECT id, directory FROM session WHERE parent_id IS NULL AND time_created >= ?1 \
             ORDER BY time_created LIMIT 64",
        )
        .ok()?;
    let found = q
        .query_map([since], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok()?;
    found
        .filter_map(Result::ok)
        .find(|(id, dir)| !taken(id) && crate::moved::same_place(Utf8Path::new(dir), cwd))
        .map(|(id, _)| id)
}

/// A part id's cursor: its time-ordered prefix as a number.
fn cursor_of(part_id: &str) -> Option<u64> {
    let hex = part_id.get(ID_PREFIX..ID_PREFIX + ID_HEX)?;
    u64::from_str_radix(hex, 16).ok()
}

fn hex(cursor: u64) -> String {
    format!("{cursor:0width$x}", width = ID_HEX)
}

/// One part row with what its message says about it.
struct Row {
    id: String,
    message: String,
    updated: i64,
    /// `None` when the part was over [`MAX_PART_BYTES`].
    part: Option<Value>,
    msg: Value,
}

/// The settled parts of `session` past `from`, as lines, and the cursor to ask from next.
/// Nothing after a part still being written is read — the same rule as withholding a partial
/// line — unless the person has since said something else, which is what a killed run leaves.
pub(crate) fn parts(
    db: &Utf8Path,
    session: &str,
    from: u64,
    max: u64,
) -> Option<(Vec<String>, u64)> {
    let c = open(db)?;
    let last_user = last_user(&c, session);
    let rows = rows(&c, session, from, max)?;
    let whole = (rows.len() as u64) < max;
    let mut lines = Vec::new();
    let (mut next, mut bytes, mut blocked) = (from, 0usize, false);
    let mut prev: Option<(String, Value)> = None;
    for row in rows {
        let superseded = last_user
            .as_deref()
            .is_some_and(|u| row.message.as_str() < u);
        if !settled(&row, superseded) || bytes > MAX_READ_BYTES {
            blocked = true;
            break;
        }
        // A message that ended in an error says so on itself, not in a part.
        if let Some((id, msg)) = prev.take().filter(|(id, _)| *id != row.message) {
            lines.extend(ended(&id, &msg));
        }
        if let Some(line) = line(session, &row, superseded) {
            bytes += line.len();
            lines.push(line);
        }
        next = cursor_of(&row.id).unwrap_or(next);
        prev = Some((row.message, row.msg));
    }
    // Everything there is has been read: the newest message may have ended with no part after.
    if whole && !blocked {
        lines.extend(newest(&c, session).and_then(|(id, msg)| ended(&id, &msg)));
    }
    Some((lines, next))
}

fn rows(c: &rusqlite::Connection, session: &str, from: u64, max: u64) -> Option<Vec<Row>> {
    let mut q = c
        .prepare(
            "SELECT p.id, p.message_id, p.time_updated, \
             CASE WHEN length(p.data) > ?4 THEN NULL ELSE p.data END, m.data \
             FROM part p JOIN message m ON m.id = p.message_id \
             WHERE p.session_id = ?1 AND substr(p.id, 5, 12) > ?2 ORDER BY p.id LIMIT ?3",
        )
        .ok()?;
    let found = q
        .query_map(
            rusqlite::params![session, hex(from), max as i64, MAX_PART_BYTES],
            |r| {
                Ok(Row {
                    id: r.get(0)?,
                    message: r.get(1)?,
                    updated: r.get(2)?,
                    part: r
                        .get::<_, Option<String>>(3)?
                        .and_then(|d| serde_json::from_str(&d).ok()),
                    msg: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                })
            },
        )
        .ok()?;
    Some(found.filter_map(Result::ok).collect())
}

/// The id of the last thing the person said that OpenCode has begun to answer. Begun, not said:
/// words typed mid-turn are written while the running tool is still running (8 such on the
/// machine this was measured on, one with the tool finishing 69 s later), and a reader that took
/// them as the end of that work drew the call as interrupted and never read its result.
fn last_user(c: &rusqlite::Connection, session: &str) -> Option<String> {
    c.query_row(
        "SELECT max(u.id) FROM message u WHERE u.session_id = ?1 \
         AND json_extract(u.data, '$.role') = 'user' \
         AND EXISTS (SELECT 1 FROM message a WHERE a.session_id = ?1 AND a.id > u.id \
                     AND json_extract(a.data, '$.role') = 'assistant')",
        [session],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn newest(c: &rusqlite::Connection, session: &str) -> Option<(String, Value)> {
    let (id, data): (String, String) = c
        .query_row(
            "SELECT id, data FROM message WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    Some((id, serde_json::from_str(&data).ok()?))
}

/// Whether a part will not change again. A tool waiting on a decision — Keel's or OpenCode's own
/// — reads `running`, exactly like one executing, so `running` is never settled on its own; and
/// 149 of this machine's text parts have no end time, so the message finishing settles them too.
fn settled(row: &Row, superseded: bool) -> bool {
    let Some(p) = &row.part else { return true };
    let done =
        superseded || row.msg["time"]["completed"].is_number() || row.msg["error"].is_object();
    match p["type"].as_str() {
        Some("tool") => {
            done || matches!(p["state"]["status"].as_str(), Some("completed" | "error"))
        }
        Some("text" | "reasoning") => {
            done || row.msg["role"] == "user" || p["time"]["end"].is_number()
        }
        _ => true,
    }
}

/// The line for a part, in the `run --format json` shape; `None` for the ones nothing draws.
fn line(session: &str, row: &Row, superseded: bool) -> Option<String> {
    let Some(mut part) = row.part.clone() else {
        let stub = json!({"type": "oversized", "sessionID": session, "part": {"id": row.id}});
        return Some(stub.to_string());
    };
    if row.msg["role"] == "user" {
        return user(row, &part);
    }
    let kind = match part["type"].as_str()? {
        "tool" => "tool_use",
        "text" => "text",
        "reasoning" => "reasoning",
        "step-finish" => "step_finish",
        _ => return None,
    };
    // A call a killed run left `running`: it will never be answered, and it must not look live.
    if kind == "tool_use" {
        let status = part["state"]["status"].as_str().unwrap_or_default();
        if superseded && !matches!(status, "completed" | "error") {
            part["state"]["status"] = "error".into();
            part["state"]["error"] = "Interrupted.".into();
        }
    }
    part["id"] = row.id.clone().into();
    part["messageID"] = row.message.clone().into();
    part["sessionID"] = session.into();
    let v = json!({"type": kind, "timestamp": row.updated, "sessionID": session, "part": part});
    Some(v.to_string())
}

/// The person's message: what opens a turn. Text OpenCode added on its own is not theirs.
fn user(row: &Row, part: &Value) -> Option<String> {
    if part["type"] != "text" || part["synthetic"] == true {
        return None;
    }
    let ms = row.msg["time"]["created"].as_i64().unwrap_or(row.updated);
    let v = json!({
        "type": "user",
        "id": row.message,
        "text": part["text"],
        "at": crate::iso(ms.div_euclid(1000), ms.rem_euclid(1000) as u32),
    });
    Some(v.to_string())
}

/// The line for an assistant message that ended in an error, if it did.
fn ended(id: &str, msg: &Value) -> Option<String> {
    if msg["role"] != "assistant" || !msg["error"].is_object() {
        return None;
    }
    Some(json!({"type": "turn_end", "id": id, "error": msg["error"]}).to_string())
}

/// Where to start so that a replay sends the last [`MAX_PARTS`] parts and no more.
pub(crate) fn replay_start(db: &Utf8Path, session: &str) -> Option<u64> {
    let c = open(db)?;
    let id: Option<String> = c
        .query_row(
            "SELECT id FROM part WHERE session_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2",
            rusqlite::params![session, MAX_PARTS as i64],
            |r| r.get(0),
        )
        .ok();
    Some(id.as_deref().and_then(cursor_of).unwrap_or(0))
}

/// A session another session started: OpenCode's `task` tool.
pub(crate) struct Child {
    pub id: String,
    /// The kind of subagent, as OpenCode names it (`general`, `explore`, or one of the person's).
    pub member: Option<String>,
    pub created: i64,
    pub updated: i64,
    /// The child's newest message: how it stands.
    pub last: Value,
}

/// The sessions `parent` started, oldest first.
pub(crate) fn children(db: &Utf8Path, parent: &str, max: usize) -> Option<Vec<Child>> {
    let c = open(db)?;
    let mut q = c
        .prepare(
            "SELECT s.id, s.agent, s.time_created, s.time_updated, \
             (SELECT data FROM message m WHERE m.session_id = s.id ORDER BY m.id DESC LIMIT 1) \
             FROM session s WHERE s.parent_id = ?1 ORDER BY s.id LIMIT ?2",
        )
        .ok()?;
    let found = q
        .query_map(rusqlite::params![parent, max as i64], |r| {
            Ok(Child {
                id: r.get(0)?,
                member: r.get(1)?,
                created: r.get(2)?,
                updated: r.get(3)?,
                last: r
                    .get::<_, Option<String>>(4)?
                    .and_then(|d| serde_json::from_str(&d).ok())
                    .unwrap_or(Value::Null),
            })
        })
        .ok()?;
    Some(found.filter_map(Result::ok).collect())
}

/// Whether `child` is a session `parent` started. Asked again before a child's output is shown.
pub(crate) fn child_of(db: &Utf8Path, parent: &str, child: &str) -> bool {
    open(db).is_some_and(|c| {
        c.query_row(
            "SELECT 1 FROM session WHERE id = ?1 AND parent_id = ?2",
            [child, parent],
            |_| Ok(()),
        )
        .is_ok()
    })
}

/// What a line is, for a follower deciding what the session is doing.
pub fn kind_of(line: &str) -> Kind {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Kind::Other;
    };
    if v["type"] == "user" {
        return Kind::Opener {
            uuid: v["id"].as_str().unwrap_or_default().to_string(),
            prompt: v["text"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(SUMMARY_CHARS)
                .collect(),
            at: v["at"].as_str().unwrap_or_default().to_string(),
            cwd: None,
        };
    }
    if ends(&v) { Kind::TurnEnd } else { Kind::Other }
}

/// The key of the turn a line opens, if it is the person's message.
pub fn opener_of(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    (v["type"] == "user").then(|| v["id"].as_str().map(str::to_string))?
}

/// A step that finished without asking for a tool, or a message that ended in an error: the turn
/// is over. A step that ends in `tool-calls` is followed by another.
pub fn ends(v: &Value) -> bool {
    v["type"] == "turn_end" || (v["type"] == "step_finish" && v["part"]["reason"] != "tool-calls")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A database with OpenCode's own columns, as far as the reader looks.
    pub(crate) fn db() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("opencode.db")).unwrap();
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, parent_id TEXT,
               agent TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
               time_updated INTEGER, data TEXT);",
        )
        .unwrap();
        (dir, path)
    }

    pub(crate) fn message(db: &Utf8Path, session: &str, id: &str, data: Value) {
        let c = rusqlite::Connection::open(db).unwrap();
        c.execute(
            "INSERT OR REPLACE INTO message VALUES (?1, ?2, ?3)",
            [id, session, &data.to_string()],
        )
        .unwrap();
    }

    pub(crate) fn part(db: &Utf8Path, session: &str, msg: &str, id: &str, data: Value) {
        let c = rusqlite::Connection::open(db).unwrap();
        c.execute(
            "INSERT OR REPLACE INTO part VALUES (?1, ?2, ?3, 0, ?4)",
            [id, msg, session, &data.to_string()],
        )
        .unwrap();
    }

    fn pid(n: u64) -> String {
        format!("prt_{n:012x}tail")
    }

    fn asked(db: &Utf8Path, session: &str, msg: &str, n: u64, text: &str) {
        message(
            db,
            session,
            msg,
            json!({"role": "user", "time": {"created": 1000}}),
        );
        part(
            db,
            session,
            msg,
            &pid(n),
            json!({"type": "text", "text": text}),
        );
    }

    fn kinds(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn tool(status: &str) -> Value {
        json!({"type": "tool", "tool": "bash", "callID": "c1",
               "state": {"status": status, "input": {"command": "sleep 60"}}})
    }

    /// The Antigravity status-9 mistake, for OpenCode: a call held for a decision reads
    /// `running`, and reading it as answered would draw it finished and never see its result.
    #[test]
    fn an_opencode_call_waiting_on_a_decision_is_not_settled() {
        let (_d, db) = db();
        asked(&db, "ses_a", "msg_1", 1, "go");
        message(&db, "ses_a", "msg_2", json!({"role": "assistant"}));
        part(&db, "ses_a", "msg_2", &pid(2), tool("running"));
        part(
            &db,
            "ses_a",
            "msg_2",
            &pid(3),
            json!({"type": "step-finish", "reason": "stop"}),
        );
        let (lines, next) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user"]);
        assert_eq!(next, 1, "the cursor waits at the unsettled call");
        part(&db, "ses_a", "msg_2", &pid(2), tool("completed"));
        let (lines, next) = parts(&db, "ses_a", next, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["tool_use", "step_finish"]);
        assert_eq!(next, 3);
    }

    /// A run killed by Stop leaves its call `running` for good (measured: SIGINT, and three
    /// older rows on this machine). The next thing the person says must still be read.
    #[test]
    fn an_opencode_part_left_running_by_a_killed_run_does_not_hold_the_cursor() {
        let (_d, db) = db();
        asked(&db, "ses_a", "msg_1", 1, "go");
        message(&db, "ses_a", "msg_2", json!({"role": "assistant"}));
        part(&db, "ses_a", "msg_2", &pid(2), tool("running"));
        asked(&db, "ses_a", "msg_3", 3, "again");
        // Words typed while the tool runs end nothing: the call is still waited for.
        let (lines, next) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user"]);
        assert_eq!(next, 1);
        // OpenCode answering them is what says the old run is gone.
        message(&db, "ses_a", "msg_4", json!({"role": "assistant"}));
        let (lines, next) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user", "tool_use", "user"]);
        let call: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(call["part"]["state"]["status"], "error");
        assert_eq!(next, 3);
    }

    /// One database holds every session, so a cursor that counted rows or bytes would move when
    /// another session wrote.
    #[test]
    fn the_opencode_cursor_survives_another_sessions_writes() {
        let (_d, db) = db();
        asked(&db, "ses_a", "msg_1", 1, "mine");
        let (_, next) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        asked(&db, "ses_b", "msg_2", 2, "theirs");
        asked(&db, "ses_a", "msg_3", 3, "mine again");
        let (lines, next) = parts(&db, "ses_a", next, MAX_PARTS).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("mine again"));
        assert_eq!(parts(&db, "ses_a", next, MAX_PARTS).unwrap().0.len(), 0);
    }

    /// One attached image is 1.5 MB of data URL; a read must not carry it.
    #[test]
    fn an_oversized_opencode_part_is_a_stub() {
        let (_d, db) = db();
        asked(&db, "ses_a", "msg_1", 1, "look");
        let big = "x".repeat(MAX_PART_BYTES as usize + 1);
        part(
            &db,
            "ses_a",
            "msg_1",
            &pid(2),
            json!({"type": "file", "url": big}),
        );
        let (lines, next) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user", "oversized"]);
        assert!(lines[1].len() < 200);
        assert_eq!(next, 2);
    }

    /// An aborted or failed message writes its error on itself and no part after it; the turn
    /// must still end, in the middle of a history and at the end of one.
    #[test]
    fn an_opencode_message_that_ended_in_an_error_ends_its_turn() {
        let (_d, db) = db();
        let aborted = json!({"role": "assistant",
            "error": {"name": "MessageAbortedError", "data": {"message": "Aborted"}}});
        asked(&db, "ses_a", "msg_1", 1, "go");
        message(&db, "ses_a", "msg_2", aborted.clone());
        part(
            &db,
            "ses_a",
            "msg_2",
            &pid(2),
            json!({"type": "text", "text": "half"}),
        );
        let (lines, _) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user", "text", "turn_end"]);
        assert_eq!(kind_of(&lines[2]), Kind::TurnEnd);
        asked(&db, "ses_a", "msg_3", 3, "again");
        let (lines, _) = parts(&db, "ses_a", 0, MAX_PARTS).unwrap();
        assert_eq!(kinds(&lines), ["user", "text", "turn_end", "user"]);
    }

    /// A terminal's session is the first one begun in its checkout after it started — not an
    /// older one, not another lane's, and not a subagent's.
    #[test]
    fn an_opencode_terminal_finds_the_session_it_started() {
        let (d, db) = db();
        let here = Utf8PathBuf::from_path_buf(d.path().to_path_buf()).unwrap();
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO session (id, directory, parent_id, time_created) VALUES
               ('ses_old', ?1, NULL, 10), ('ses_taken', ?1, NULL, 110),
               ('ses_child', ?1, 'ses_taken', 120), ('ses_far', '/elsewhere', NULL, 130),
               ('ses_mine', ?1, NULL, 140)",
            [here.as_str()],
        )
        .unwrap();
        let found = started(&db, &here, 100, |id| id == "ses_taken");
        assert_eq!(found.as_deref(), Some("ses_mine"));
    }

    /// The id reaches a query and, later, a command line; the directory is what stops one
    /// project reading another's conversation.
    #[test]
    fn an_opencode_session_is_read_only_from_its_own_directory() {
        let (d, db) = db();
        let here = Utf8PathBuf::from_path_buf(d.path().to_path_buf()).unwrap();
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO session (id, directory) VALUES ('ses_a', ?1)",
            [here.as_str()],
        )
        .unwrap();
        assert!(session_in(&db, &here, "ses_a"));
        assert!(!session_in(&db, Utf8Path::new("/elsewhere"), "ses_a"));
        assert!(!session_in(&db, &here, "ses_a' OR 1=1 --"));
        assert!(!is_session_id("../ses_a"));
    }
}
