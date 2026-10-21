//! Which agent's transcript a follower reads, and how — the one place the answer lives.
//! Claude Code's and Codex's are JSONL; Antigravity's is a SQLite database read as lines.
//!
//! Following a terminal session was Claude's alone: the tail handler always read Claude's home and
//! always built Claude's decoder, so a Codex terminal lane was idle in the sidebar and empty under
//! Turns whatever it did. Each question a follower asks of a transcript — where it is, which
//! records it shows, what opens and ends a turn, how it decodes — is answered here per agent, and
//! nothing else branches on the agent.

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

use crate::Kind;
use crate::conversation::{Decoder, Provider, Source};

/// An agent whose terminal session Keel can follow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
}

impl Agent {
    /// The agent a request names; absent is Claude, which is what every client sent before.
    pub fn from_query(s: Option<&str>) -> Result<Agent, &'static str> {
        match s {
            None | Some("claude") => Ok(Agent::Claude),
            Some("codex") => Ok(Agent::Codex),
            Some("antigravity") => Ok(Agent::Antigravity),
            Some(_) => Err("unknown agent"),
        }
    }

    /// The transcript of session `id` run from `dir`, if there is one on this machine.
    pub fn locate(self, dir: &Utf8Path, id: &str) -> Option<Transcript> {
        let path = match self {
            Agent::Claude => crate::transcript_path(dir, &crate::claude_home()?, id)?,
            Agent::Codex => crate::codex::rollout(&crate::codex_home()?.join("sessions"), dir, id)?,
            Agent::Antigravity => crate::antigravity::conversation(
                &crate::antigravity::conversations_dir()?,
                dir,
                id,
            )?,
        };
        Some(Transcript { agent: self, path })
    }

    /// A decoder for this agent's transcript.
    pub fn decoder(self) -> Decoder {
        let provider = match self {
            Agent::Claude => Provider::Claude,
            Agent::Codex => Provider::Codex,
            Agent::Antigravity => Provider::Antigravity,
        };
        Decoder::new(provider, Source::Transcript)
    }
}

/// One session's transcript, found.
#[derive(Clone, Debug)]
pub struct Transcript {
    agent: Agent,
    path: Utf8PathBuf,
}

impl Transcript {
    /// A transcript at a known path, for tests and for a caller that already found it.
    pub fn at(agent: Agent, path: Utf8PathBuf) -> Self {
        Self { agent, path }
    }

    pub fn agent(&self) -> Agent {
        self.agent
    }

    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    fn keep(&self) -> fn(&Value) -> bool {
        match self.agent {
            Agent::Claude => crate::sessions::followable,
            Agent::Codex => crate::codex::followable,
            Agent::Antigravity => crate::antigravity::followable,
        }
    }

    /// The records appended since `from`; see [`crate::tail`]. For Antigravity `from` is a step
    /// index rather than a byte offset — the caller treats it as a cursor either way.
    pub fn tail(&self, from: u64) -> Option<(Vec<String>, u64)> {
        match self.agent {
            Agent::Antigravity => {
                crate::antigravity::steps(&self.path, from, crate::antigravity::MAX_REPLAY_STEPS)
            }
            _ => crate::sessions::tail_file(&self.path, from, self.keep()),
        }
    }

    /// The last `max` bytes of records; see [`crate::tail_last`]. For Antigravity, the last
    /// steps, at most as many as a replay sends.
    pub fn tail_last(&self, max: u64) -> Option<(Vec<String>, u64, u64)> {
        match self.agent {
            Agent::Antigravity => {
                let total = crate::antigravity::count(&self.path)?;
                let start = total.saturating_sub(crate::antigravity::MAX_REPLAY_STEPS);
                let (lines, next) = self.tail(start)?;
                Some((lines, next, start))
            }
            _ => crate::sessions::tail_last_file(&self.path, max, self.keep()),
        }
    }

    /// Files whose change means the transcript moved: agy writes its write-ahead log first.
    pub fn watched(&self) -> Vec<Utf8PathBuf> {
        match self.agent {
            Agent::Antigravity => vec![
                self.path.clone(),
                Utf8PathBuf::from(format!("{}-wal", self.path)),
            ],
            _ => vec![self.path.clone()],
        }
    }

    pub fn kind_of(&self, line: &str) -> Kind {
        match self.agent {
            Agent::Claude => crate::kind_of(line),
            Agent::Codex => crate::codex::kind_of(line),
            Agent::Antigravity => crate::antigravity::kind_of(line),
        }
    }

    pub fn opener_of(&self, line: &str) -> Option<String> {
        match self.agent {
            Agent::Claude => crate::opener_of(line),
            Agent::Codex => crate::codex::opener_of(line),
            Agent::Antigravity => crate::antigravity::opener_of(line),
        }
    }

    /// Whether a new turn was asked for past byte `from`: the next prompt, typed while the last
    /// turn's checks ran. A turn whose tree that is no longer has no commit of its own to make.
    pub fn opens_after(&self, from: u64, open: &str) -> bool {
        self.tail(from).is_some_and(|(lines, _)| {
            lines
                .iter()
                .filter_map(|l| self.opener_of(l))
                .any(|key| key != open)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader keeps Codex's typed events and nothing else, and withholds a line still being
    /// written.
    #[test]
    fn the_codex_reader_keeps_only_event_msgs_and_withholds_a_partial_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("r.jsonl");
        let ev = r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"T"}}"#;
        let body = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{}}}}\n{{\"type\":\"response_item\",\"payload\":{{}}}}\n{ev}\n{{\"type\":\"event_msg\""
        );
        std::fs::write(&path, &body).unwrap();
        let t = Transcript::at(Agent::Codex, path);
        let (lines, next) = t.tail(0).unwrap();
        assert_eq!(lines, vec![ev.to_string()]);
        assert_eq!(
            next as usize,
            body.rfind('\n').unwrap() + 1,
            "the partial line waits"
        );
    }

    /// A next prompt past the offset is a new turn; more words for the open turn are not.
    #[test]
    fn a_steer_is_not_the_next_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("r.jsonl");
        let user = |turn: &str| {
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,"item":{"type":"UserMessage","content":[{"type":"text","text":"x"}]}}}).to_string() + "\n"
        };
        std::fs::write(&path, user("T1")).unwrap();
        let t = Transcript::at(Agent::Codex, path.clone());
        assert!(!t.opens_after(0, "T1"));
        std::fs::write(&path, user("T1") + &user("T2")).unwrap();
        assert!(t.opens_after(0, "T1"));
    }

    /// For Antigravity the cursor is a step index: a next input past it is the next turn.
    #[test]
    fn an_antigravity_next_input_is_the_next_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("c.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE steps (idx integer PRIMARY KEY, step_type integer, status integer, step_payload blob);
             INSERT INTO steps VALUES (0, 14, 3, x''), (1, 15, 3, x'');",
        )
        .unwrap();
        let t = Transcript::at(Agent::Antigravity, path);
        let (lines, next, start) = t.tail_last(0).unwrap();
        assert_eq!((lines.len(), next, start), (2, 2, 0));
        assert_eq!(t.kind_of(&lines[1]), Kind::TurnEnd);
        assert!(!t.opens_after(next, "turn-0"));
        c.execute("INSERT INTO steps VALUES (2, 14, 3, x'')", [])
            .unwrap();
        assert!(t.opens_after(next, "turn-0"));
    }

    #[test]
    fn the_tail_names_the_agents_it_follows() {
        assert_eq!(Agent::from_query(None), Ok(Agent::Claude));
        assert_eq!(Agent::from_query(Some("codex")), Ok(Agent::Codex));
        assert_eq!(
            Agent::from_query(Some("antigravity")),
            Ok(Agent::Antigravity)
        );
        assert!(Agent::from_query(Some("../x")).is_err());
    }
}
