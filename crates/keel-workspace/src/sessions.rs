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
    fn a_project_with_no_sessions_is_not_an_error() {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        assert!(discover_sessions(Utf8Path::new("/nope"), &home).is_empty());
    }
}
