use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

/// One Claude Code conversation, summarised from its transcript.
///
/// Message bodies are deliberately absent. A session list needs to be scannable, and a transcript
/// contains everything the user has ever said in that repository.
#[derive(Debug, Clone, Serialize)]
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
}

impl Session {
    /// What to show when Claude Code never generated a title.
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("(untitled — {})", self.short_id()))
    }

    /// The first segment of the UUID, which is enough to identify a session by eye.
    pub fn short_id(&self) -> String {
        self.id.split('-').next().unwrap_or(&self.id).to_string()
    }
}

/// One exchange in a transcript, shaped for display.
#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    /// `user` or `assistant`.
    pub role: &'static str,
    pub text: String,
    /// Tool names called in this turn, in order.
    pub tools: Vec<String>,
}

/// One tool call, as recorded.
#[derive(Debug, Clone, Serialize)]
pub struct SessionCall {
    pub tool: String,
    /// The command for a Bash call, or the path for a file tool.
    pub subject: String,
    pub output: String,
    pub error: bool,
    /// Which turn of the conversation ran it, counted the way [`transcript`] counts turns.
    ///
    /// Without this the app had nowhere to put a replayed call but the last turn, so a session
    /// that edited forty files across nine turns showed eight turns that "changed nothing" and
    /// one that did everything — the opposite of what the trace is for.
    pub turn: usize,
}

/// A file a session wrote, and the turn that wrote it.
#[derive(Debug, Clone, Serialize)]
pub struct SessionFile {
    pub path: String,
    pub turn: usize,
}

/// What a session actually did: which files it changed, and what it ran.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionWork {
    /// Repository-relative, in the order they were first touched.
    pub files: Vec<SessionFile>,
    pub calls: Vec<SessionCall>,
    /// Whether the record was cut short by the caps below.
    pub truncated: bool,
}

/// Tools whose input names a file the session wrote.
const WRITE_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "Update"];

/// Shell verbs whose arguments are files they change. `sed` counts only with `-i`, which is why
/// the match is on the flag rather than on the name.
const WRITE_VERBS: &[&str] = &["tee", "touch", "cp", "mv", "rm", "patch"];

/// Fragments that mean the inline script in this command writes a file, so every path in it is a
/// path it wrote. A `python3 - <<PY` that opens a file for writing names it as a plain string.
const WRITE_FRAGMENTS: &[&str] = &[".write(", "writeFileSync", "write_text", "'w')", "\"w\")"];

/// The files a shell command changed, as far as the command itself says.
///
/// The trace exists to answer "what did the agent do", and for a session driven through `Bash` —
/// heredocs, `sed -i`, a redirect — the tool names answer nothing: 131 calls, 28 files edited, and
/// a trace reporting no file changed at all, because `Edit` and `Write` were never used.
///
/// Deliberately conservative, in both directions. A path is kept only when the command carries a
/// write signal *and* the path exists in the repository, so a `grep` over a file, a `sed -n`, and
/// a heredoc that only reads are all silent. It will miss a path a script computes rather than
/// names — nothing in a transcript can recover that one.
///
/// ponytail: string matching, not a shell parser. A real parse would catch `eval`, an aliased
/// verb and a path in a variable; if that starts mattering, the upgrade is a `shlex` pass over
/// the pipeline rather than more fragments here.
fn shell_writes(command: &str, repo: &Utf8Path) -> Vec<String> {
    /// A token that could be a path: no flags, no globs, no shell expansion, and a name with an
    /// extension somewhere below a directory. `p='app/x.swift'` is one — an assignment inside a
    /// heredoc is how a script names the file it is about to write.
    fn candidate(token: &str) -> Option<&str> {
        let t = token.rsplit('=').next().unwrap_or(token);
        let t = t.trim_matches(|c: char| "'\"`;,()<>&|".contains(c));
        if t.is_empty() || t.starts_with('-') || t.contains(['$', '*', '?']) {
            return None;
        }
        let name = t.rsplit('/').next().unwrap_or(t);
        (t.contains('/') && name.contains('.') && !name.starts_with('.')).then_some(t)
    }

    /// Every `'…'` and `"…"` in the command. An inline script's paths are string literals, not
    /// whitespace-separated arguments: `open('app/x.swift', 'w')` is one token to a shell.
    fn quoted(command: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = command;
        while let Some(open) = rest.find(['\'', '"']) {
            let quote = rest.as_bytes()[open] as char;
            let after = &rest[open + 1..];
            let Some(close) = after.find(quote) else {
                break;
            };
            out.push(&after[..close]);
            rest = &after[close + 1..];
        }
        out
    }

    let tokens: Vec<&str> = command.split_whitespace().collect();
    // An inline script that writes names its file as a string literal, so the literals are the
    // arguments rather than the tokens after one verb.
    let inline = WRITE_FRAGMENTS.iter().any(|f| command.contains(f));
    let mut targets: Vec<&str> = if inline { quoted(command) } else { Vec::new() };

    for (i, token) in tokens.iter().enumerate() {
        // `> file`, `>>file`, `2> file` — the target is whatever the redirect points at.
        if token.contains('>') {
            let rest = token.rsplit('>').next().unwrap_or_default();
            if rest.is_empty() {
                targets.extend(tokens.get(i + 1));
            } else {
                targets.push(rest);
            }
        }
        let verb = token.rsplit('/').next().unwrap_or(token);
        if WRITE_VERBS.contains(&verb)
            || (verb == "sed" && tokens[i..].iter().any(|t| t.starts_with("-i")))
        {
            targets.extend(&tokens[i + 1..]);
        }
    }

    let mut out: Vec<String> = Vec::new();
    for path in targets.into_iter().filter_map(candidate) {
        let rel = path.strip_prefix(&format!("{repo}/")).unwrap_or(path);
        let rel = rel.trim_start_matches("./");
        // Inside the repository only: a turn that wrote a scratch file in `/tmp` changed nothing
        // anybody is reviewing, and listing it as a changed file is noise in the one pane that
        // has to be exact.
        if rel.starts_with('/') {
            continue;
        }
        // Only what is really there. Existence is the filter that keeps a command's other
        // arguments — a pattern, a branch name, a URL — from being reported as edits.
        if repo.join(rel).exists() && !out.iter().any(|p| p == rel) {
            out.push(rel.to_string());
        }
    }
    out
}

/// Bounds on what one session can put on screen. A long session is thousands of calls, and the
/// last few hundred are the ones anybody scrolls to — which is what this now keeps. It stopped
/// recording at the three hundredth call instead, so a long session's trace was its *first* three
/// hundred calls under a note in the app reading "capped at the most recent 300". Memory while
/// parsing is bounded by `MAX_OUTPUT` per call, not by this.
const MAX_CALLS: usize = 300;
const MAX_OUTPUT: usize = 8_000;

/// What one record of a transcript contributes to the conversation, or `None` when it is not part
/// of it at all.
///
/// One function because two readers have to agree on it: [`transcript`] turns these into the
/// conversation, and [`session_work`] counts the user ones to know which turn a tool call belongs
/// to. When the two disagreed by a single skipped record, every call in the session was filed
/// against the wrong turn.
fn spoken(record: &Value) -> Option<(&'static str, String, Vec<String>)> {
    let role = match record.get("type").and_then(Value::as_str) {
        Some("user") => "user",
        Some("assistant") => "assistant",
        _ => return None,
    };
    // Sidechain records are subagent chatter, not the conversation the user had.
    if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let (mut text, mut tools) = (String::new(), Vec::new());
    match record.get("message").and_then(|m| m.get("content")) {
        // A plain user message.
        Some(Value::String(s)) => text.push_str(s),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        if let Some(n) = block.get("name").and_then(Value::as_str) {
                            tools.push(n.to_string());
                        }
                    }
                    // Thinking and tool results are skipped: replaying an old session is for
                    // reading what was said and done, not for re-litigating the reasoning.
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if text.trim().is_empty() && tools.is_empty() {
        return None;
    }
    // Claude Code writes background-task notifications back into the transcript as user messages
    // so the model can consume them. They are transport envelopes, not something the person
    // typed: rendering the XML and an embedded subagent report as a giant blue chat bubble makes
    // a resumed conversation unreadable and misattributes the content.
    if role == "user" && text.trim_start().starts_with("<task-notification>") {
        return None;
    }
    Some((role, text.trim().to_string(), tools))
}

/// Read what a session changed and ran.
///
/// Shares [`transcript`]'s guard and its rule: this runs on a click, on one session the user named
/// — never in the listing. It returns paths, commands and command output, which is what "show me
/// what this session did" means; the conversation itself is [`transcript`]'s job.
pub fn session_work(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> SessionWork {
    let mut work = SessionWork::default();
    let Some(contents) = read_transcript(repo, claude_home, id) else {
        return work;
    };

    let root = format!("{repo}/");
    // A tool call and its result are separate records, so calls are held by id until the result
    // arrives rather than being emitted twice.
    let mut pending: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Which turn of the conversation is being read. `transcript` starts a turn on every user
    // record it keeps, and the app's replay does the same, so counting them here with the same
    // predicate is what makes the two line up.
    let mut turn = 0usize;
    let mut started = false;

    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(("user", _, _)) = spoken(&record) {
            // The first user record opens turn 0 rather than turn 1; anything before it — there
            // should be nothing — is filed against the same turn.
            if started {
                turn += 1;
            }
            started = true;
        }
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(Value::Array(blocks)) = record.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };

        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let Some(tool) = block.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let input = block.get("input");
                    let get = |k: &str| {
                        input
                            .and_then(|i| i.get(k))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };

                    if WRITE_TOOLS.contains(&tool) {
                        let path = if get("file_path").is_empty() {
                            get("path")
                        } else {
                            get("file_path")
                        };
                        let rel = path.strip_prefix(&root).unwrap_or(&path).to_string();
                        if !rel.is_empty()
                            && !work.files.iter().any(|f| f.path == rel && f.turn == turn)
                        {
                            work.files.push(SessionFile { path: rel, turn });
                        }
                    }

                    if tool == "Bash" {
                        for rel in shell_writes(&get("command"), repo) {
                            if !work.files.iter().any(|f| f.path == rel && f.turn == turn) {
                                work.files.push(SessionFile { path: rel, turn });
                            }
                        }
                    }
                    let subject = match tool {
                        "Bash" => get("command"),
                        _ => {
                            let p = if get("file_path").is_empty() {
                                get("path")
                            } else {
                                get("file_path")
                            };
                            let p = if p.is_empty() { get("pattern") } else { p };
                            p.strip_prefix(&root).unwrap_or(&p).to_string()
                        }
                    };
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        pending.insert(id.to_string(), work.calls.len());
                    }
                    work.calls.push(SessionCall {
                        tool: tool.to_string(),
                        subject,
                        output: String::new(),
                        error: false,
                        turn,
                    });
                }
                Some("tool_result") => {
                    let Some(at) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| pending.remove(id))
                    else {
                        continue;
                    };
                    let text = match block.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                        _ => String::new(),
                    };
                    if let Some(call) = work.calls.get_mut(at) {
                        if text.len() > MAX_OUTPUT {
                            // Keep the tail: an error is at the end of the output, not the start.
                            // Not `truncated`: that word is the app's "this session ran more calls
                            // than are shown", and setting it here made a 131-call session say it
                            // had been capped at 300 — a note that is not true, on a pane whose
                            // whole job is being accurate about what happened.
                            call.output = text[text.len() - MAX_OUTPUT..].to_string();
                        } else {
                            call.output = text;
                        }
                        call.error = block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    }
                }
                _ => {}
            }
        }
    }

    // The tail, not the head: a long session's last few hundred calls are the ones anybody
    // scrolls to, and they are the ones the app already says it is showing.
    if work.calls.len() > MAX_CALLS {
        work.calls.drain(..work.calls.len() - MAX_CALLS);
        work.truncated = true;
    }
    work
}

/// The one place a transcript path is built, so its guard cannot be forgotten by a second reader.
fn read_transcript(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> Option<String> {
    // The id comes from the UI. Reject anything that could climb out of the project directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    // Every directory the listing draws from, not just the repository's own. A session listed
    // from a parent folder or a lane's checkout could be seen in History and then opened to
    // nothing, because this looked in one place and the list came from several.
    session_dirs(repo, claude_home)
        .into_iter()
        .map(|(dir, scope)| project_dir(&dir, claude_home, scope).join(format!("{id}.jsonl")))
        .find_map(|path| std::fs::read_to_string(path).ok())
}

/// Read one session's transcript for display.
///
/// This is deliberately separate from [`discover_sessions`], which stays metadata-only. Listing
/// every session in a repository is not licence to render what was said in them; opening one the
/// user explicitly asked for is. Keeping the two apart means the cheap, always-on path can never
/// leak a conversation, and the expensive one only runs on a click.
pub fn transcript(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> Vec<Turn> {
    let Some(contents) = read_transcript(repo, claude_home, id) else {
        return Vec::new();
    };

    let mut turns: Vec<Turn> = Vec::new();
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some((role, text, tools)) = spoken(&record) else {
            continue;
        };
        turns.push(Turn { role, text, tools });
    }
    turns
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

/// Every session recorded for `repo` and the directories around it, most recently active first.
pub fn discover_sessions(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for (dir, scope) in session_dirs(repo, claude_home) {
        let project = project_dir(&dir, claude_home, scope);
        let Ok(entries) = std::fs::read_dir(&project) else {
            continue;
        };
        for p in entries
            .filter_map(Result::ok)
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.extension() == Some("jsonl"))
        {
            if let Some(mut s) = parse_session(&p) {
                s.scope = scope.to_string();
                // A session below the repo is only listed if its transcript says where.
                if scope == "below" && s.cwd.is_none() {
                    continue;
                }
                if scope != "below" && s.cwd.is_none() {
                    s.cwd = Some(dir_of(&project, claude_home, repo, scope));
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

    /// A lane is a checkout under `.keel/worktrees/<name>`, so every feature Keel starts is a
    /// session in a subdirectory. The subdirectory's project path was built without `projects`,
    /// so `read_dir` failed and the whole scope was skipped: a feature you named, worked in and
    /// closed was in no list anywhere.
    #[test]
    fn a_session_run_in_a_lane_is_listed_and_can_be_opened() {
        let transcript = concat!(
            r#"{"type":"ai-title","aiTitle":"the lane"}"#,
            "\n",
            r#"{"type":"user","cwd":"/repo/.keel/worktrees/pricing","timestamp":"2026-01-01T00:00:00Z","message":{"content":"go"}}"#,
            "\n",
        );
        let (_d, home) = home_with("-repo--keel-worktrees-pricing", "lane-1.jsonl", transcript);

        let sessions = discover_sessions(Utf8Path::new("/repo"), &home);
        assert_eq!(
            sessions.len(),
            1,
            "the lane's session is part of the repository"
        );
        assert_eq!(sessions[0].scope, "below");
        assert_eq!(sessions[0].title.as_deref(), Some("the lane"));

        // And listed is not enough: opening it looked only in the repository's own directory.
        assert_eq!(transcript_turns(&home).len(), 1);
    }

    fn transcript_turns(home: &Utf8Path) -> Vec<Turn> {
        transcript(Utf8Path::new("/repo"), home, "lane-1")
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

    #[test]
    fn reads_a_transcript_for_display() {
        let transcript = concat!(
            r#"{"type":"user","message":{"content":"fix the build"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"On it."},{"type":"tool_use","name":"Read"}]}}"#,
            "\n",
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent noise"}}"#,
            "\n",
        );
        let (_d, home) = home_with("-repo", "abc-1.jsonl", transcript);

        let turns = transcript_of(&home);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "fix the build");
        assert_eq!(turns[1].text, "On it.");
        assert_eq!(turns[1].tools, vec!["Read"]);
        // Thinking is not replayed, and subagent chatter is not the user's conversation.
        assert!(!turns.iter().any(|t| t.text.contains("hmm")));
        assert!(!turns.iter().any(|t| t.text.contains("subagent")));
    }

    #[test]
    fn transcript_hides_internal_task_notifications() {
        let transcript = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"hello\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Working on it.\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"<task-notification>\\n<result>internal report</result>\\n</task-notification>\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
        );
        let (_d, home) = home_with("-repo", "abc-1.jsonl", transcript);

        let turns = transcript_of(&home);

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[1].text, "Working on it.");
        assert_eq!(turns[2].text, "Done.");
        assert!(
            turns
                .iter()
                .all(|turn| !turn.text.contains("task-notification"))
        );
    }

    /// The trace is a record of what the agent did *when*, so every call has to land on the turn
    /// that ran it. Everything used to be handed to the app in one flat list, which had nowhere
    /// to put it but the last turn: nine turns of work read as eight that changed nothing and one
    /// that did all of it.
    #[test]
    fn every_call_is_filed_against_the_turn_that_ran_it() {
        let lines = [
            r#"{"type":"user","message":{"content":"add the endpoint"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"/repo/api.ts"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"user","message":{"content":"now the tests"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"make check"}}]}}"#,
            // Not the person typing: a background job report must not open a third turn.
            r#"{"type":"user","message":{"content":"<task-notification>done</task-notification>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Edit","input":{"file_path":"/repo/api.test.ts"}}]}}"#,
        ]
        .join("\n");

        let (_d, home) = home_with("-repo", "abc-1.jsonl", &lines);
        let work = session_work(Utf8Path::new("/repo"), &home, "abc-1");
        let turns = transcript(Utf8Path::new("/repo"), &home, "abc-1");

        assert_eq!(
            work.calls.iter().map(|c| c.turn).collect::<Vec<_>>(),
            vec![0, 1, 1],
            "the write is the first ask, the check and the edit the second"
        );
        assert_eq!(
            work.files
                .iter()
                .map(|f| (f.path.as_str(), f.turn))
                .collect::<Vec<_>>(),
            vec![("api.ts", 0), ("api.test.ts", 1)]
        );
        // The count the app replays into, so an index out of it would be a call with no home.
        assert_eq!(
            turns.iter().filter(|t| t.role == "user").count(),
            2,
            "both readers count the same turns"
        );
    }

    /// An agent that edits through the shell — a heredoc, `sed -i`, a redirect — changed files
    /// just as much as one that used `Edit`, and a trace that says "changed nothing" over 28 of
    /// them is worse than no trace. Conservative on purpose: a read is not a write.
    #[test]
    fn shell_edits_count_as_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap().to_path_buf();
        std::fs::create_dir_all(repo.join("app/Sources")).unwrap();
        for f in [
            "app/Sources/LaneRail.swift",
            "app/Sources/Theme.swift",
            "Makefile.md",
        ] {
            std::fs::write(repo.join(f), "x").unwrap();
        }

        let wrote = |cmd: &str| shell_writes(cmd, &repo);

        assert_eq!(
            wrote("python3 - <<'PY'\np='app/Sources/LaneRail.swift'\nopen(p,'w').write(s)\nPY"),
            vec!["app/Sources/LaneRail.swift"],
            "an inline script that writes names its file as a string"
        );
        assert_eq!(
            wrote("sed -i.bak 's/a/b/' app/Sources/Theme.swift"),
            vec!["app/Sources/Theme.swift"]
        );
        assert_eq!(
            wrote("cat >> app/Sources/Theme.swift <<EOF"),
            vec!["app/Sources/Theme.swift"],
            "the target of a redirect"
        );
        assert!(
            wrote("sed -n '1,200p' app/Sources/Theme.swift").is_empty(),
            "a read is not a write"
        );
        assert!(wrote("grep -rn \"Reading\" app/Sources/LaneRail.swift | head -20").is_empty());
        assert!(
            wrote("cp app/Sources/Nowhere.swift app/Sources/Gone.swift").is_empty(),
            "a path that is not in the repository is not reported as edited"
        );
        assert!(
            wrote("cp /tmp/shot.png /tmp/before.png").is_empty(),
            "a scratch file outside the repository is not a change to review"
        );
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

    /// Opening a session that ran in a lane, asked for the way the app asks for it: with the lane
    /// as the checkout. It came back empty.
    #[test]
    fn a_session_that_ran_in_a_lane_opens_from_that_lane() {
        let (_d, home) = home_with(
            "-repo--keel-worktrees-hi",
            "abc-1.jsonl",
            r#"{"type":"user","message":{"content":"hi"}}"#,
        );
        let lane = Utf8Path::new("/repo/.keel/worktrees/hi");
        assert_eq!(transcript(lane, &home, "abc-1").len(), 1);
        // And from the project root, which finds it by scanning instead.
        assert_eq!(transcript(Utf8Path::new("/repo"), &home, "abc-1").len(), 1);
    }

    fn transcript_of(home: &Utf8Path) -> Vec<Turn> {
        transcript(Utf8Path::new("/repo"), home, "abc-1")
    }

    #[test]
    fn a_session_id_cannot_escape_the_project_directory() {
        let (_d, home) = home_with("-repo", "x.jsonl", "{}");
        assert!(transcript(Utf8Path::new("/repo"), &home, "../../../etc/passwd").is_empty());
        assert!(transcript(Utf8Path::new("/repo"), &home, "").is_empty());
    }

    /// Selecting a session should answer "what did this do to my repository", which means the
    /// files it wrote and the commands it ran with their output — paired across two records,
    /// since a call and its result arrive separately.
    #[test]
    fn a_session_reports_what_it_changed_and_ran() {
        // One record per line: a transcript is JSONL, and a pretty-printed fixture would be
        // split by `lines()` into fragments that all fail to parse — silently, into an empty
        // result that looks like "this session did nothing".
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"/repo/src/a.ts"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"make check"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"1 failed"}]}}"#,
            // The same file twice is one entry; a read is not a change.
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Edit","input":{"file_path":"/repo/src/a.ts"}},{"type":"tool_use","id":"t4","name":"Read","input":{"file_path":"/repo/src/b.ts"}}]}}"#,
            // Subagent chatter is not the session's own work.
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"t5","name":"Write","input":{"file_path":"/repo/side.ts"}}]}}"#,
        ]
        .join("\n");

        let (_d, home) = home_with("-repo", "abc-1.jsonl", &lines);
        let work = session_work(Utf8Path::new("/repo"), &home, "abc-1");

        assert_eq!(
            work.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.ts"],
            "written once, relative"
        );
        // A record that is only a tool result is not something anybody said, so it does not
        // start a turn — everything here belongs to the first one.
        assert!(work.calls.iter().all(|c| c.turn == 0));
        assert_eq!(
            work.calls.len(),
            4,
            "reads are calls even though they are not changes"
        );

        let bash = work
            .calls
            .iter()
            .find(|c| c.tool == "Bash")
            .expect("the bash call");
        assert_eq!(bash.subject, "make check");
        assert_eq!(bash.output, "1 failed", "its result, paired by tool_use_id");
        assert!(bash.error);

        // A call whose result never arrived is still shown; it just has nothing under it.
        let write = work
            .calls
            .iter()
            .find(|c| c.tool == "Write")
            .expect("the write");
        assert_eq!(write.subject, "src/a.ts");
        assert!(write.output.is_empty());
    }

    #[test]
    fn session_work_cannot_escape_the_project_directory_either() {
        let (_d, home) = home_with("-repo", "x.jsonl", "{}");
        assert!(
            session_work(Utf8Path::new("/repo"), &home, "../../../etc/passwd")
                .files
                .is_empty()
        );
        assert!(
            session_work(Utf8Path::new("/repo"), &home, "a/b")
                .calls
                .is_empty()
        );
        assert!(
            session_work(Utf8Path::new("/repo"), &home, "")
                .calls
                .is_empty()
        );
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
