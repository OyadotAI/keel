use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

/// One Claude Code conversation, summarised from its transcript.
///
/// Message bodies are deliberately absent. A session list needs to be scannable, and a transcript
/// contains everything the user has ever said in that repository.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Session {
    /// The session UUID, which is what `claude --resume` takes.
    pub id: String,
    /// Where it was found relative to the repository: `here`, `above` (a parent directory),
    /// or `below` (a subdirectory). Claude Code keys transcripts by the directory it was
    /// launched from, and people launch it from the folder above as often as not.
    pub scope: String,
    /// Claude Code's own generated title, when it has produced one.
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// ISO-8601, so lexicographic ordering is chronological ordering.
    pub started: Option<String>,
    pub last_active: Option<String>,
    pub messages: usize,
    /// The Claude Code version that wrote the transcript.
    pub version: Option<String>,
    /// Who launched it, from the transcript's own `entrypoint`. Not part of the wire shape — it
    /// exists so [`discover_sessions`] can drop the runs nobody started, and it is cached with
    /// the rest of the summary so that filtering costs no extra pass.
    #[serde(skip)]
    entrypoint: Option<String>,
    /// A `claude` process has this session open right now — here, in a terminal, anywhere.
    ///
    /// From `~/.claude/sessions/<pid>.json`, which Claude Code writes on start and removes on
    /// exit, checked against a live pid so a crash cannot leave a ghost. It *was* "the transcript
    /// was written to in the last minute", and that heuristic was wrong in both directions: a
    /// turn inside a two-minute `cargo build` read as ended, and a session you had just quit read
    /// as running for a minute after. Set by [`discover_sessions`], never cached with the summary.
    pub live: bool,
    /// Claude Code's own word for what that process is doing: `busy` is a turn in flight, and
    /// not busy is a prompt waiting for someone. Only meaningful while `live`.
    pub busy: bool,
}

impl Session {
    /// What to show when Claude Code never generated a title.
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("(untitled — {})", self.short_id()))
    }

    /// Whether this is a conversation somebody had, rather than a headless run some other tool
    /// made in this repository.
    ///
    /// Measured here: 143 transcripts, of which 118 are `sdk-py` — `/security-review`, a `reviewer`
    /// subagent, one per invocation, each with a title good enough to look like a session anyone
    /// might want to resume. Claude Code's own `--resume` picker shows 13 of the 143 and hides
    /// exactly these; Keel showed all of them and the list was unreadable.
    ///
    /// The three kept are the ones a person or Keel drives: `cli` and `claude-desktop` are a
    /// terminal, and `sdk-cli` is Keel's own `agent::chat`. An unrecorded entrypoint is kept,
    /// because transcripts older than the field exist and hiding them would lose real history.
    pub fn is_conversation(&self) -> bool {
        match self.entrypoint.as_deref() {
            Some(e) => matches!(e, "cli" | "claude-desktop" | "sdk-cli"),
            None => true,
        }
    }

    /// The first segment of the UUID, which is enough to identify a session by eye.
    pub fn short_id(&self) -> String {
        self.id.split('-').next().unwrap_or(&self.id).to_string()
    }
}

/// Where one session's transcript lives, behind the same guard.
///
/// Split out of [`read_transcript`] because following a live session must not re-read the file:
/// a transcript here reaches 33 MB, and re-reading it every poll to find the few hundred bytes
/// that were appended is exactly the kind of work the bar exists to keep off the machine.
pub fn transcript_path(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> Option<Utf8PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    session_dirs(repo, claude_home)
        .into_iter()
        .map(|(dir, scope)| project_dir(&dir, claude_home, scope).join(format!("{id}.jsonl")))
        .find(|path| path.exists())
}

/// How long the transcript is right now, or `None` if there is no such session.
///
/// One `stat`. Following a session costs this and nothing else for as long as it stays quiet.
pub fn transcript_len(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> Option<u64> {
    let path = transcript_path(repo, claude_home, id)?;
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// The records appended since `from`, and the offset to ask from next.
///
/// Two rules make this safe to poll against a file somebody else is writing:
///
/// * **A partial trailing line is not returned.** The writer appends a whole JSON object followed
///   by a newline, and a read that lands between the two would otherwise hand out half a record —
///   which parses as nothing, is dropped, and is never asked for again. The offset advances only
///   past the last newline, so the remainder is re-read once it is complete.
/// * **The offset is a byte position, not a line count.** Transcripts are append-only, so a
///   position stays valid; counting lines would mean reading all of them to find the end.
///
/// Sidechains are skipped, and so are the record types the display readers hide, so what a
/// follower sees and what a reader sees agree.
pub fn tail(
    repo: &Utf8Path,
    claude_home: &Utf8Path,
    id: &str,
    from: u64,
) -> Option<(Vec<String>, u64)> {
    use std::io::{Read, Seek, SeekFrom};

    let path = transcript_path(repo, claude_home, id)?;
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    // Shorter than we last read it: not an append. A session cleared or replaced under us is
    // read from the start rather than from an offset into something that is no longer there.
    let from = if len < from { 0 } else { from };
    if len == from {
        return Some((Vec::new(), from));
    }
    // Never more than `MAX_READ` in one call. A follower that fell far behind — or a first read
    // that was not bounded by `tail_last` — reads the rest on its next call; a 33 MB transcript
    // was three copies of 33 MB on the executor's behalf before this.
    let end = len.min(from.saturating_add(MAX_READ));
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buffer = Vec::with_capacity((end - from) as usize);
    file.take(end - from).read_to_end(&mut buffer).ok()?;

    // Everything up to the last newline is whole; what follows it is the writer mid-append.
    let complete = match buffer.iter().rposition(|b| *b == b'\n') {
        Some(i) => i + 1,
        None => return Some((Vec::new(), from)),
    };
    let text = String::from_utf8_lossy(&buffer[..complete]);
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| serde_json::from_str::<Value>(line).is_ok_and(|record| followable(&record)))
        .map(str::to_string)
        .collect();
    Some((lines, from + complete as u64))
}

/// The most a single read of a transcript may take in.
pub const MAX_READ: u64 = 8 * 1024 * 1024;

/// The tail of a transcript: at most `max_bytes` of it, starting on a record boundary, and the
/// byte the read started at — nonzero means the head was not read.
///
/// The first read of a session used to be the whole file. The tail is what a person opening a
/// conversation is looking for, and the head of a 33 MB transcript is thousands of records the
/// app would have decoded on its main actor before drawing anything.
pub fn tail_last(
    repo: &Utf8Path,
    claude_home: &Utf8Path,
    id: &str,
    max_bytes: u64,
) -> Option<(Vec<String>, u64, u64)> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let path = transcript_path(repo, claude_home, id)?;
    let len = std::fs::metadata(&path).ok()?.len();
    let mut start = 0;
    if len > max_bytes {
        // Land just past the first newline at or after the cut, so the read opens on a whole
        // record rather than the middle of one.
        let mut file = BufReader::new(std::fs::File::open(&path).ok()?);
        let cut = len - max_bytes;
        file.seek(SeekFrom::Start(cut)).ok()?;
        let mut skipped = Vec::new();
        let n = file.read_until(b'\n', &mut skipped).ok()?;
        start = cut + n as u64;
    }
    let (lines, next) = tail(repo, claude_home, id, start)?;
    Some((lines, next, start))
}

/// What a transcript line is, for a follower deciding what the session is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A person asking: the turn opens here. `cwd` is where the session runs, from the record.
    Opener {
        uuid: String,
        prompt: String,
        at: String,
        cwd: Option<String>,
    },
    /// Claude Code's own note that a turn ended.
    TurnEnd,
    /// The agent left a command running in the background — Claude Code's own shell, which
    /// Keel cannot see or stop, but can at least list.
    JobStarted {
        id: String,
        command: String,
    },
    /// Claude Code's note to itself that a background command finished.
    JobDone {
        id: String,
    },
    Other,
}

pub fn kind_of(line: &str) -> Kind {
    let Ok(record) = serde_json::from_str::<Value>(line) else {
        return Kind::Other;
    };
    if record["type"].as_str() == Some("system")
        && record["subtype"].as_str() == Some("turn_duration")
    {
        return Kind::TurnEnd;
    }
    if record["type"].as_str() == Some("assistant")
        && let Some(blocks) = record["message"]["content"].as_array()
        && let Some(b) = blocks.iter().find(|b| {
            b["type"] == "tool_use"
                && b["name"] == "Bash"
                && b["input"]["run_in_background"] == true
        })
        && let Some(id) = b["id"].as_str()
    {
        return Kind::JobStarted {
            id: id.to_string(),
            command: b["input"]["command"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        };
    }
    if record["type"].as_str() == Some("user")
        && let Some(said) = record["message"]["content"].as_str()
        && said.trim_start().starts_with("<task-notification>")
    {
        let id = said
            .split_once("<task-id>")
            .and_then(|(_, rest)| rest.split_once("</task-id>"))
            .map(|(id, _)| id.trim().to_string())
            .unwrap_or_default();
        return Kind::JobDone { id };
    }
    match opener(&record) {
        Some((uuid, prompt, at)) => Kind::Opener {
            uuid,
            prompt,
            at,
            cwd: record["cwd"].as_str().map(str::to_string),
        },
        None => Kind::Other,
    }
}

/// What a `claude` process says it is doing with a session, from its pid file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
}

/// The status of one session's process, or `None` when no live process has it open.
pub fn status(claude_home: &Utf8Path, session: &str) -> Option<Status> {
    running(claude_home)
        .get(session)
        .map(|busy| if *busy { Status::Busy } else { Status::Idle })
}

/// The `uuid` of the human `user` record that opens a turn, if this line is one.
///
/// The key every fact about a turn is filed under. Not an ordinal: the daemon drops the head of a
/// long replay, a compaction summary looks like a prompt, and any change to `followable` renumbers
/// everything after it. The uuid is stable for the life of the transcript.
pub fn opener_of(line: &str) -> Option<String> {
    let record: Value = serde_json::from_str(line).ok()?;
    opener(&record).map(|(uuid, _, _)| uuid)
}

/// A person asking, as `(uuid, prompt, timestamp)`; `None` for a tool result, a compaction
/// summary, a meta record, or a subagent's chatter — none of which opens a turn.
fn opener(record: &Value) -> Option<(String, String, String)> {
    if record["type"].as_str() != Some("user")
        || record["isSidechain"].as_bool() == Some(true)
        || record["isCompactSummary"].as_bool() == Some(true)
        || record["isMeta"].as_bool() == Some(true)
        || !record["toolUseResult"].is_null()
    {
        return None;
    }
    let content = &record["message"]["content"];
    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(blocks) = content.as_array() {
        if blocks.iter().any(|b| b["type"] == "tool_result") {
            return None;
        }
        blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        return None;
    };
    let said = text.trim();
    if said.is_empty() || said.starts_with("<task-notification>") {
        return None;
    }
    Some((
        record["uuid"].as_str()?.to_string(),
        said.chars().take(200).collect(),
        record["timestamp"].as_str().unwrap_or_default().to_string(),
    ))
}

/// The first turn opened at or after a byte offset: `(uuid, prompt, timestamp)`.
///
/// How a Keel-driven turn learns its own key: the daemon notes where the transcript ended before
/// it spawned the agent, and the prompt record it is looking for is the first thing written after
/// that. Reads from the offset only, so a long conversation costs nothing here.
pub fn opening_turn(
    repo: &Utf8Path,
    claude_home: &Utf8Path,
    id: &str,
    from: u64,
) -> Option<(String, String, String)> {
    let (lines, _) = tail(repo, claude_home, id, from)?;
    lines.iter().find_map(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|r| opener(&r))
    })
}

/// Whether a transcript record is one a follower should be shown.
///
/// Two exclusions, in the one place the reader goes through so nothing can drift from it.
///
/// A **sidechain** is a subagent's chatter, which belongs under the `Task` that started it rather
/// than in the conversation.
///
/// A **task notification** — Claude Code telling itself that a background job finished — is
/// kept here, because it is the one record that says a job ended; the tail handler reads it as
/// `Kind::JobDone` and does not forward it as a message. Drawn as one it was a blue bubble
/// attributed to the person, which also *split the turn*, so every reply after it landed against
/// the XML rather than against what they actually asked. Measured on this machine: 75 of them
/// across one project's transcripts.
fn followable(record: &Value) -> bool {
    if record["isSidechain"].as_bool() == Some(true) {
        return false;
    }
    matches!(
        record["type"].as_str(),
        Some("assistant" | "user" | "result" | "system")
    )
}

/// Claude Code's directory name for a working directory.
///
/// Separators *and dots* become dashes, so `/Users/mk/Dev/oya` is stored as `-Users-mk-Dev-oya`
/// and `/repo/.keel/worktrees/x` as `-repo--keel-worktrees-x`.
///
/// The dot was missing, and every lane is `.keel/worktrees/<name>`: asked for a lane's session
/// directly — which is what happens when the app reopens a conversation that ran in a lane, since
/// the request carries the lane — this named a directory that has never existed, `read_dir` failed
/// and the answer was an empty conversation. The session was still *listed*, because the listing
/// finds it by scanning for directories that extend the repository's key rather than by building
/// one, so History showed 81 messages and opening it showed a blank pane.
pub fn project_key(cwd: &Utf8Path) -> String {
    cwd.as_str().replace(['/', '.'], "-")
}

/// The directories whose sessions belong to this repository: itself, up to two parents (never
/// the home directory), and every subdirectory.
///
/// Claude Code keys transcripts by the launch directory, so a person who ran it from the
/// folder above — the monorepo, the client's folder — has their sessions there, and a Keel that
/// only looked at the exact path listed one session where they remembered thirteen.
pub fn session_dirs(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<(Utf8PathBuf, &'static str)> {
    let home = std::env::var("HOME").map(Utf8PathBuf::from).ok();
    let mut out = vec![(repo.to_owned(), "here")];
    let mut up = repo.parent();
    for _ in 0..2 {
        let Some(dir) = up else { break };
        if Some(dir) == home.as_deref() || dir.as_str() == "/" {
            break;
        }
        out.push((dir.to_owned(), "above"));
        up = dir.parent();
    }
    // Subdirectories: every project key that extends this one.
    let prefix = project_key(repo) + "-";
    if let Ok(entries) = std::fs::read_dir(claude_home.join("projects")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) {
                // The key is lossy; the real path is in each transcript's `cwd`, read later.
                //
                // This joined `claude_home` and the key directly, leaving out `projects` — a
                // directory that does not exist, whose `read_dir` failed, which `continue`
                // skipped. Every session run in a subdirectory was therefore missing from
                // History, and every lane is a subdirectory: `.keel/worktrees/<name>`. So a
                // feature you named, worked in and closed left no trace anywhere in the app.
                out.push((claude_home.join("projects").join(&name), "below"));
            }
        }
    }
    out
}

/// Where a scope's transcripts actually live. `below` already names its own project directory;
/// the others name a working directory that has to be keyed first.
fn project_dir(dir: &Utf8Path, claude_home: &Utf8Path, scope: &str) -> Utf8PathBuf {
    if scope == "below" {
        dir.to_owned()
    } else {
        claude_home.join("projects").join(project_key(dir))
    }
}

/// The sessions a `claude` process has open right now, by session id, with whether it is busy.
///
/// One small file per process under `~/.claude/sessions`. A file whose pid is gone is a crash
/// that never got to clean up, and is ignored rather than shown as running forever.
fn running(claude_home: &Utf8Path) -> std::collections::HashMap<String, bool> {
    /// One parse per pid file per mtime. The files of crashed processes are never removed by
    /// anything, and this runs on every session listing; before the memo each of them was read
    /// and parsed again every time. Liveness is *not* memoised — a process can die under an
    /// unchanged file — so the `kill(pid, 0)` still runs per file, which is a syscall.
    type Parsed = Option<(i32, String, bool)>;
    type Memo = std::collections::HashMap<std::path::PathBuf, (std::time::SystemTime, Parsed)>;
    static MEMO: std::sync::OnceLock<std::sync::Mutex<Memo>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);

    let mut out = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(claude_home.join("sessions")) else {
        return out;
    };
    let mut seen = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(mtime) = e.metadata().ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        seen.push(path.clone());
        let parsed = {
            let held = memo.lock().unwrap_or_else(|p| p.into_inner());
            held.get(&path)
                .filter(|(at, _)| *at == mtime)
                .map(|(_, p)| p.clone())
        };
        let parsed = match parsed {
            Some(p) => p,
            None => {
                let p: Parsed = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|record| {
                        let pid = i32::try_from(record["pid"].as_i64()?).ok()?;
                        let id = record["sessionId"].as_str()?.to_owned();
                        Some((pid, id, record["status"].as_str() == Some("busy")))
                    });
                memo.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(path.clone(), (mtime, p.clone()));
                p
            }
        };
        let Some((pid, id, busy)) = parsed else {
            continue;
        };
        // SAFETY: signal 0 delivers nothing; it only asks whether the pid exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if alive {
            out.insert(id, busy);
        }
    }
    // Files that are gone leave the memo with them.
    memo.lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|path, _| seen.contains(path));
    out
}

/// Transcripts parsed for a listing, newest first. Past this the list is history nobody scrolls
/// to, and the cost — a `stat` each warm, a read each cold — is paid on every listing.
const MAX_LISTED: usize = 300;

/// Every session recorded for `repo` and the directories around it, most recently active first.
pub fn discover_sessions(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Session> {
    let running = running(claude_home);
    // Every transcript is stat'd; only the newest are parsed.
    let mut found: Vec<(std::time::SystemTime, Utf8PathBuf, &'static str)> = Vec::new();
    for (dir, scope) in session_dirs(repo, claude_home) {
        let project = project_dir(&dir, claude_home, scope);
        let Ok(entries) = std::fs::read_dir(&project) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(p) = Utf8PathBuf::from_path_buf(e.path()) else {
                continue;
            };
            if p.extension() != Some("jsonl") {
                continue;
            }
            let at = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            found.push((at, p, scope));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.truncate(MAX_LISTED);

    let mut sessions: Vec<Session> = Vec::new();
    for (_, p, scope) in found {
        let project = p.parent().map(|d| d.to_owned()).unwrap_or_default();
        {
            if let Some(mut s) = parse_session(&p) {
                if !s.is_conversation() {
                    continue;
                }
                s.scope = scope.to_string();
                // A session below the repo is only listed if its transcript says where.
                if scope == "below" && s.cwd.is_none() {
                    continue;
                }
                if scope != "below" && s.cwd.is_none() {
                    s.cwd = Some(dir_of(&project, claude_home, repo, scope));
                }
                if let Some(busy) = running.get(&s.id) {
                    s.live = true;
                    s.busy = *busy;
                }
                sessions.push(s);
            }
        }
    }

    // Most recent first: an unstarted session sorts last rather than crashing the ordering.
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    sessions
}

/// The launch directory for a session whose transcript did not record one.
fn dir_of(_project: &Utf8Path, _home: &Utf8Path, repo: &Utf8Path, scope: &str) -> String {
    match scope {
        "here" => repo.to_string(),
        _ => repo.parent().map(|p| p.to_string()).unwrap_or_default(),
    }
}

/// Summarise one transcript.
///
/// Transcripts are append-only JSONL and can reach megabytes, so this makes a single pass and holds
/// no message content. A line that fails to parse is skipped rather than aborting the file — a
/// truncated tail from a session killed mid-write should not hide the whole conversation.
fn parse_session(path: &Utf8Path) -> Option<Session> {
    // Transcripts are append-only, and `discover_sessions` runs on every `/api/state` — which is
    // every panel that opens and every turn that ends. Re-reading and JSON-parsing the whole
    // project's history each time is work with a known answer: measured on this repository, 151
    // sessions and 127 MB of JSONL, 240 ms per call for a list of titles and timestamps. Length
    // and mtime together identify a file that has not been appended to since, which is the case
    // for all but the one session actually in use.
    let stat = std::fs::metadata(path).ok()?;
    let stamp = (stat.len(), stat.modified().ok()?);
    if let Some(hit) = cache().get(path, stamp) {
        return Some(hit);
    }

    let contents = std::fs::read_to_string(path).ok()?;
    let id = path.file_stem()?.to_string();

    let mut session = Session {
        id,
        scope: "here".into(),
        title: None,
        cwd: None,
        branch: None,
        started: None,
        last_active: None,
        messages: 0,
        version: None,
        entrypoint: None,
        live: false,
        busy: false,
    };

    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(kind) = record.get("type").and_then(Value::as_str) else {
            continue;
        };

        match kind {
            "ai-title" => {
                session.title = record
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "user" | "assistant" => {
                session.messages += 1;
                let string = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);

                // The first record establishes the context; later ones only move the clock.
                session.cwd = session.cwd.take().or_else(|| string("cwd"));
                session.branch = session.branch.take().or_else(|| string("gitBranch"));
                session.version = session.version.take().or_else(|| string("version"));
                session.entrypoint = session.entrypoint.take().or_else(|| string("entrypoint"));

                if let Some(timestamp) = string("timestamp") {
                    session.started.get_or_insert_with(|| timestamp.clone());
                    session.last_active = Some(timestamp);
                }
            }
            _ => {}
        }
    }

    cache().put(path, stamp, &session);
    Some(session)
}

/// Summaries that are still true, keyed by the file and the moment it was last written.
///
/// Deliberately small in what it promises: a file whose length *and* mtime are unchanged has not
/// been appended to, and a transcript is only ever appended to. Anything else — a file rewritten
/// in place to exactly its old length within the same mtime tick — reads stale, which is a
/// session list one refresh behind rather than a wrong answer.
struct Cache(std::sync::Mutex<std::collections::HashMap<Utf8PathBuf, (Stamp, Session)>>);
type Stamp = (u64, std::time::SystemTime);

fn cache() -> &'static Cache {
    static C: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    C.get_or_init(|| Cache(std::sync::Mutex::new(std::collections::HashMap::new())))
}

impl Cache {
    /// A poisoned lock here must not take the session list down with it: this crate is the one
    /// that "degrades to an empty list" rather than failing, and a cache is the last thing worth
    /// a panic. (`crate::lock::Locked` lives in `keel`, and `keel-workspace` depends on nothing
    /// in the workspace on purpose.)
    fn get(&self, path: &Utf8Path, stamp: Stamp) -> Option<Session> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(path)
            .filter(|(seen, _)| *seen == stamp)
            .map(|(_, session)| session.clone())
    }

    fn put(&self, path: &Utf8Path, stamp: Stamp, session: &Session) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // A daemon that has been open all day across several projects should not grow a map
        // forever. Emptying it is the whole eviction policy: the next call rebuilds what it needs
        // and nothing else, which is cheaper to reason about than an LRU nobody will tune.
        if map.len() > 2_000 {
            map.clear();
        }
        map.insert(path.to_owned(), (stamp, session.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    fn home_with(project: &str, file: &str, contents: &str) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        let project_dir = home.join("projects").join(project);
        std::fs::create_dir_all(&project_dir).expect("mkdir");
        std::fs::write(project_dir.join(file), contents).expect("write");
        (dir, home)
    }

    #[test]
    fn encodes_a_working_directory_the_way_claude_code_does() {
        assert_eq!(
            project_key(Utf8Path::new("/Users/mk/Dev/oya")),
            "-Users-mk-Dev-oya"
        );
    }

    #[test]
    fn summarises_a_transcript_without_reading_message_bodies() {
        let transcript = concat!(
            r#"{"type":"mode","sessionId":"abc"}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"keel local ide"}"#,
            "\n",
            r#"{"type":"user","cwd":"/repo","gitBranch":"main","timestamp":"2026-08-27T10:00:00Z","version":"2.1.248","message":{"content":"secret"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-08-27T11:00:00Z","message":{"content":"also secret"}}"#,
            "\n",
        );
        let (_d, home) = home_with("-repo", "abc-123-def.jsonl", transcript);

        let sessions = discover_sessions(Utf8Path::new("/repo"), &home);
        assert_eq!(sessions.len(), 1);

        let s = &sessions[0];
        assert_eq!(s.title.as_deref(), Some("keel local ide"));
        assert_eq!(s.messages, 2);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.started.as_deref(), Some("2026-08-27T10:00:00Z"));
        assert_eq!(s.last_active.as_deref(), Some("2026-08-27T11:00:00Z"));
        assert_eq!(s.short_id(), "abc");

        // Nothing in the serialised form should carry conversation content.
        let json = serde_json::to_string(s).expect("serialises");
        assert!(
            !json.contains("secret"),
            "message bodies must never leave the transcript"
        );
    }

    /// 143 transcripts in this repository, 118 of them `sdk-py`: one `/security-review` run per
    /// change, each with an `ai-title` that reads like a session somebody had. Claude Code's own
    /// picker hides them; Keel listed all 143 and the switcher was unusable. Keel's own chat is
    /// `sdk-cli`, so "hide the SDK" is the wrong rule — it would hide the lanes.
    #[test]
    fn a_headless_run_by_another_tool_is_not_a_session() {
        let record = |entrypoint: &str, title: &str| {
            format!(
                concat!(
                    r#"{{"type":"ai-title","aiTitle":"{}"}}"#,
                    "\n",
                    r#"{{"type":"user","cwd":"/repo","entrypoint":"{}","timestamp":"2026-01-01T00:00:00Z","message":{{"content":"x"}}}}"#,
                    "\n",
                ),
                title, entrypoint
            )
        };
        let (_d, home) = home_with("-repo", "keel.jsonl", &record("sdk-cli", "a lane"));
        let project = home.join("projects").join("-repo");
        std::fs::write(project.join("term.jsonl"), record("cli", "a terminal")).expect("write");
        let old = concat!(
            r#"{"type":"ai-title","aiTitle":"before the field"}"#,
            "\n",
            r#"{"type":"user","cwd":"/repo","timestamp":"2026-01-01T00:00:00Z","message":{"content":"x"}}"#,
            "\n",
        );
        std::fs::write(project.join("old.jsonl"), old).expect("write");
        std::fs::write(
            project.join("rev.jsonl"),
            record("sdk-py", "Security review"),
        )
        .expect("write");

        let mut titles: Vec<String> = discover_sessions(Utf8Path::new("/repo"), &home)
            .into_iter()
            .map(|s| s.display_title())
            .collect();
        titles.sort();
        assert_eq!(titles, ["a lane", "a terminal", "before the field"]);
    }

    #[test]
    fn a_truncated_line_does_not_hide_the_rest() {
        let transcript = concat!(
            r#"{"type":"ai-title","aiTitle":"kept"}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-02T00:00:00Z"}"#,
            "\n",
        );
        let (_d, home) = home_with("-repo", "x.jsonl", transcript);
        let sessions = discover_sessions(Utf8Path::new("/repo"), &home);
        assert_eq!(sessions[0].title.as_deref(), Some("kept"));
        assert_eq!(sessions[0].messages, 1);
    }

    /// Every lane lives in `.keel/worktrees/<name>`, and Claude Code writes a dot as a dash the
    /// same way it writes a slash as one. Keying only the slashes named a directory that has
    /// never existed, so a lane's own conversation reopened blank — while still being listed,
    /// because the listing scans for directories rather than building the name.
    #[test]
    fn a_lanes_own_directory_is_keyed_the_way_claude_code_writes_it() {
        assert_eq!(
            project_key(Utf8Path::new("/Users/mk/Dev/oya/keel/.keel/worktrees/hi")),
            "-Users-mk-Dev-oya-keel--keel-worktrees-hi"
        );
        assert_eq!(
            project_key(Utf8Path::new("/Users/mk/Dev/oya/keel")),
            "-Users-mk-Dev-oya-keel"
        );
    }

    /// Following a session someone else is writing.
    ///
    /// The reason this exists: a conversation running in a terminal appends to the same JSONL
    /// Keel reads, so the file is already a live feed and nothing was reading it as one. Opening
    /// such a session showed a snapshot from the moment of the click and then sat still.
    mod following {
        use super::*;

        fn write(home: &Utf8Path, contents: &str) {
            let path = home.join("projects").join("-repo").join("abc-1.jsonl");
            std::fs::write(path, contents).unwrap();
        }
        fn read(home: &Utf8Path, from: u64) -> (Vec<String>, u64) {
            tail(Utf8Path::new("/repo"), home, "abc-1", from).unwrap()
        }
        fn user(text: &str) -> String {
            format!(r#"{{"type":"user","message":{{"content":"{text}"}}}}"#)
        }

        /// A record half-written is not a record.
        ///
        /// The writer appends a whole JSON object and then a newline, so a poll landing between
        /// the two would hand out half a line — which parses as nothing, is dropped, and is never
        /// asked for again, because the offset had already moved past it. The offset stops at the
        /// last newline instead, so the remainder is re-read once it is complete.
        #[test]
        fn a_partial_trailing_line_is_withheld_until_it_is_complete() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(
                &home,
                &format!("{}\n{}", user("one"), r#"{"type":"user","mess"#),
            );

            let (lines, at) = read(&home, 0);
            assert_eq!(lines.len(), 1, "only the line that was finished");
            assert_eq!(
                at,
                user("one").len() as u64 + 1,
                "the offset stops at the newline"
            );

            // The writer finishes it.
            write(&home, &format!("{}\n{}\n", user("one"), user("two")));
            let (lines, _) = read(&home, at);
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains("two"), "and nothing was lost: {lines:?}");
        }

        /// An offset returns what was appended and not the whole file again. A transcript in this
        /// repository's own history reaches 33 MB; re-reading it every 400 ms to find a few
        /// hundred new bytes is the work the bar exists to keep off the machine.
        #[test]
        fn an_offset_returns_only_the_new_range() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(&home, &format!("{}\n", user("one")));
            let (first, at) = read(&home, 0);
            assert_eq!(first.len(), 1);

            let (none, still) = read(&home, at);
            assert!(none.is_empty(), "nothing was appended");
            assert_eq!(still, at, "and the offset did not move");

            write(&home, &format!("{}\n{}\n", user("one"), user("two")));
            let (next, _) = read(&home, at);
            assert_eq!(next.len(), 1);
            assert!(next[0].contains("two"));
        }

        /// Nobody typed a task notification.
        ///
        /// Claude Code writes one as a `user` record when a background job finishes, because that
        /// is the turn it occupies. Drawn as one it is a blue bubble attributed to the person —
        /// and it *splits the turn*, so every reply after it lands against the XML rather than
        /// against what they actually asked. 75 of them in one project's transcripts here. It is
        /// kept by the tail — it is the one record that says the job ended — and classified as
        /// exactly that, so the handler files it and never draws it.
        #[test]
        fn a_task_notification_is_not_something_a_person_said() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(
                &home,
                &format!(
                    "{}\n{}\n",
                    user("what I actually asked"),
                    r#"{"type":"user","message":{"content":"<task-notification>\n<task-id>x</task-id>\n</task-notification>"}}"#,
                ),
            );
            let (lines, _) = read(&home, 0);
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(lines[0].contains("what I actually asked"));
            assert_eq!(kind_of(&lines[1]), Kind::JobDone { id: "x".into() });
            assert!(opener_of(&lines[1]).is_none(), "never a prompt");
        }

        /// A command left running in the background is a job the transcript reports, from the
        /// call that started it.
        #[test]
        fn a_background_command_is_a_job_the_transcript_reports() {
            let started = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t-9","name":"Bash","input":{"command":"gh run watch 1","run_in_background":true}}]}}"#;
            assert_eq!(
                kind_of(started),
                Kind::JobStarted {
                    id: "t-9".into(),
                    command: "gh run watch 1".into()
                }
            );
            let plain = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{"command":"ls"}}]}}"#;
            assert_eq!(kind_of(plain), Kind::Other);
        }

        /// A subagent's chatter belongs under the `Task` that started it, which is the same
        /// exclusion the display reader makes — so a followed session and a read one agree.
        #[test]
        fn sidechains_and_bookkeeping_records_are_skipped() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(
                &home,
                &format!(
                    "{}\n{}\n{}\n{}\n",
                    user("kept"),
                    r#"{"type":"user","isSidechain":true,"message":{"content":"subagent"}}"#,
                    r#"{"type":"ai-title","aiTitle":"a name"}"#,
                    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
                ),
            );
            let (lines, _) = read(&home, 0);
            assert_eq!(lines.len(), 2, "the user turn and the reply: {lines:?}");
        }

        /// A transcript replaced rather than appended to is read from the start, not from an
        /// offset into something that is no longer there.
        #[test]
        fn a_shorter_file_is_read_from_the_beginning() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(
                &home,
                &format!("{}\n{}\n{}\n", user("a"), user("b"), user("c")),
            );
            let (_, at) = read(&home, 0);

            write(&home, &format!("{}\n", user("only")));
            let (lines, _) = read(&home, at);
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains("only"));
        }

        /// The same guard the readers use. It has to be the same one: a second path to a
        /// transcript is a second place to forget it.
        #[test]
        fn a_session_id_still_cannot_escape_the_project_directory() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "{}");
            assert!(tail(Utf8Path::new("/repo"), &home, "../../../etc/passwd", 0).is_none());
            assert!(tail(Utf8Path::new("/repo"), &home, "", 0).is_none());
            assert!(tail(Utf8Path::new("/repo"), &home, "no-such-session", 0).is_none());
        }

        /// Whether a `claude` has the session open right now comes from Claude Code's own
        /// per-process file, checked against a live pid — never from the summary cache, which is
        /// valid for as long as the transcript is unchanged and would say "running" forever.
        #[test]
        fn liveness_comes_from_the_process_file_and_is_never_cached() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", &format!("{}\n", user("one")));
            let sessions_dir = home.join("sessions");
            std::fs::create_dir_all(&sessions_dir).unwrap();
            let me = std::process::id();
            let file = sessions_dir.join(format!("{me}.json"));
            std::fs::write(
                &file,
                format!(r#"{{"pid":{me},"sessionId":"abc-1","status":"busy"}}"#),
            )
            .unwrap();
            // A crash that never cleaned up: a pid nothing has.
            std::fs::write(
                sessions_dir.join("1.json"),
                r#"{"pid":2147483000,"sessionId":"abc-1","status":"busy"}"#,
            )
            .unwrap();

            let first = &discover_sessions(Utf8Path::new("/repo"), &home)[0];
            assert!(first.live && first.busy);

            std::fs::remove_file(&file).unwrap();
            let again = &discover_sessions(Utf8Path::new("/repo"), &home)[0];
            assert!(
                !again.live,
                "the process is gone, the cached summary is not"
            );
        }

        /// The first read of a long transcript is its tail, on a record boundary, and says
        /// where it started.
        #[test]
        fn the_first_read_of_a_large_transcript_reads_only_its_tail() {
            let (_d, home) = home_with("-repo", "big-1.jsonl", "");
            let path = home.join("projects").join("-repo").join("big-1.jsonl");
            let mut body = String::new();
            for i in 0..20_000 {
                body.push_str(&user(&format!("line {i} {}", "x".repeat(200))));
                body.push('\n');
            }
            std::fs::write(&path, &body).unwrap();
            let len = std::fs::metadata(&path).unwrap().len();
            let (lines, next, started_at) =
                tail_last(Utf8Path::new("/repo"), &home, "big-1", 1024 * 1024).unwrap();
            assert!(started_at > 0 && started_at >= len - 1024 * 1024);
            assert_eq!(next, len);
            assert!(!lines.is_empty());
            assert!(
                lines
                    .iter()
                    .all(|l| serde_json::from_str::<Value>(l).is_ok()),
                "every line read is a whole record"
            );
            let (all, _, at) = tail_last(Utf8Path::new("/repo"), &home, "big-1", len * 2).unwrap();
            assert_eq!(at, 0);
            assert_eq!(all.len(), 20_000);
        }

        #[test]
        fn an_opener_is_a_person_not_a_tool_result_or_a_compaction() {
            let person = r#"{"type":"user","uuid":"u-1","cwd":"/repo","timestamp":"t","message":{"role":"user","content":"hi"}}"#;
            assert!(
                matches!(kind_of(person), Kind::Opener { ref uuid, ref cwd, .. } if uuid == "u-1" && cwd.as_deref() == Some("/repo"))
            );
            let result = r#"{"type":"user","uuid":"u-2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#;
            assert_eq!(kind_of(result), Kind::Other);
            let compact = r#"{"type":"user","uuid":"u-3","isCompactSummary":true,"message":{"role":"user","content":"summary"}}"#;
            assert_eq!(kind_of(compact), Kind::Other);
            let side = r#"{"type":"user","uuid":"u-4","isSidechain":true,"message":{"role":"user","content":"sub"}}"#;
            assert_eq!(kind_of(side), Kind::Other);
            let note = r#"{"type":"user","uuid":"u-5","message":{"role":"user","content":"<task-notification>done"}}"#;
            assert!(
                matches!(kind_of(note), Kind::JobDone { .. }),
                "a job ending, not a person"
            );
        }

        #[test]
        fn turn_duration_marks_the_end_of_a_turn() {
            let end = r#"{"type":"system","subtype":"turn_duration","durationMs":4100}"#;
            assert_eq!(kind_of(end), Kind::TurnEnd);
            let other = r#"{"type":"system","subtype":"stop_hook_summary"}"#;
            assert_eq!(kind_of(other), Kind::Other);
        }

        /// One `stat`, so a session that is sitting idle costs nothing to keep following.
        #[test]
        fn the_length_is_readable_without_reading_the_file() {
            let (_d, home) = home_with("-repo", "abc-1.jsonl", "");
            write(&home, &format!("{}\n", user("one")));
            let len = transcript_len(Utf8Path::new("/repo"), &home, "abc-1").unwrap();
            assert_eq!(len, user("one").len() as u64 + 1);
            assert!(transcript_len(Utf8Path::new("/repo"), &home, "nope").is_none());
        }
    }

    #[test]
    fn a_project_with_no_sessions_is_not_an_error() {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        assert!(discover_sessions(Utf8Path::new("/nope"), &home).is_empty());
    }

    /// The cache must not outlive the truth: a transcript that grew is read again.
    ///
    /// Transcripts are appended to constantly — the session you are in the middle of grows with
    /// every message — so a cache that missed an append would freeze the switcher on a message
    /// count and a timestamp from whenever the daemon started.
    #[test]
    fn an_appended_transcript_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("s.jsonl").to_path_buf()).unwrap();

        let line = |ts: &str| {
            format!(
                r#"{{"type":"user","cwd":"/x","timestamp":"{ts}","message":{{"content":"hi"}}}}"#
            )
        };
        std::fs::write(&path, format!("{}\n", line("2026-01-01T00:00:00Z"))).unwrap();
        let first = parse_session(&path).unwrap();
        assert_eq!(first.messages, 1);

        // Same call, no change on disk: the summary comes back identical.
        assert_eq!(parse_session(&path).unwrap().messages, 1);

        // Appended. Length differs, so the stamp differs, so it is parsed again.
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                line("2026-01-01T00:00:00Z"),
                line("2026-01-01T00:05:00Z")
            ),
        )
        .unwrap();
        let second = parse_session(&path).unwrap();
        assert_eq!(second.messages, 2, "the append was missed");
        assert_eq!(
            second.last_active.as_deref(),
            Some("2026-01-01T00:05:00Z"),
            "and the clock moved with it"
        );
    }
}
