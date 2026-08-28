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

/// Read one session's transcript for display.
///
/// This is deliberately separate from [`discover_sessions`], which stays metadata-only. Listing
/// every session in a repository is not licence to render what was said in them; opening one the
/// user explicitly asked for is. Keeping the two apart means the cheap, always-on path can never
/// leak a conversation, and the expensive one only runs on a click.
pub fn transcript(repo: &Utf8Path, claude_home: &Utf8Path, id: &str) -> Vec<Turn> {
    // The id comes from the UI. Reject anything that could climb out of the project directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Vec::new();
    }

    let path = claude_home
        .join("projects")
        .join(project_key(repo))
        .join(format!("{id}.jsonl"));
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut turns: Vec<Turn> = Vec::new();
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = match record.get("type").and_then(Value::as_str) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        // Sidechain records are subagent chatter, not the conversation the user had.
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }

        let content = record.get("message").and_then(|m| m.get("content"));
        let (mut text, mut tools) = (String::new(), Vec::new());

        match content {
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
            continue;
        }
        turns.push(Turn {
            role,
            text: text.trim().to_string(),
            tools,
        });
    }
    turns
}

/// Claude Code's directory name for a working directory.
///
/// Path separators become dashes, so `/Users/mk/Dev/oya` is stored as `-Users-mk-Dev-oya`.
pub fn project_key(cwd: &Utf8Path) -> String {
    cwd.as_str().replace('/', "-")
}

/// Every session recorded for `repo`, most recently active first.
pub fn discover_sessions(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Session> {
    let dir = claude_home.join("projects").join(project_key(repo));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut sessions: Vec<Session> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.extension() == Some("jsonl"))
        .filter_map(|p| parse_session(&p))
        .collect();

    // Most recent first: an unstarted session sorts last rather than crashing the ordering.
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    sessions
}

/// Summarise one transcript.
///
/// Transcripts are append-only JSONL and can reach megabytes, so this makes a single pass and holds
/// no message content. A line that fails to parse is skipped rather than aborting the file — a
/// truncated tail from a session killed mid-write should not hide the whole conversation.
fn parse_session(path: &Utf8Path) -> Option<Session> {
    let contents = std::fs::read_to_string(path).ok()?;
    let id = path.file_stem()?.to_string();

    let mut session = Session {
        id,
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

    Some(session)
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

    fn transcript_of(home: &Utf8Path) -> Vec<Turn> {
        transcript(Utf8Path::new("/repo"), home, "abc-1")
    }

    #[test]
    fn a_session_id_cannot_escape_the_project_directory() {
        let (_d, home) = home_with("-repo", "x.jsonl", "{}");
        assert!(transcript(Utf8Path::new("/repo"), &home, "../../../etc/passwd").is_empty());
        assert!(transcript(Utf8Path::new("/repo"), &home, "").is_empty());
    }

    #[test]
    fn a_project_with_no_sessions_is_not_an_error() {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        assert!(discover_sessions(Utf8Path::new("/nope"), &home).is_empty());
    }
}
