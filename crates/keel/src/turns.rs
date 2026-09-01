//! What Keel knows about a turn that the transcript does not.
//!
//! Claude Code's transcript records what was said and which tools ran, and that is all it owes
//! anyone. Everything else on a turn is Keel's own reading of the machine: the files git says the
//! turn moved, the commit made of them, the gate's verdict, how long it took, what it cost. All of
//! it is computed at the end of the turn and, until this module, kept nowhere.
//!
//! So a relaunch rebuilt a turn from the transcript and it came back with its prose intact and its
//! work missing. The case is not rare — it is the normal one for a turn whose tool calls are
//! `Bash`: a heredoc, a `tee`, a formatter. Those paths were never in a tool call to begin with,
//! they were in `git status` at the time, and that time has passed. A turn that plainly changed
//! files, reporting none, is the "never weird" failure exactly: Keel showing a thing it cannot
//! explain, about its own history.
//!
//! One file per session under `.keel/turns/`, records upserted by turn number. Beside
//! `permissions.json` rather than in a database because it is the same kind of thing — small,
//! per-project, and readable by a person wondering what Keel thinks happened.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::serve::AppState;

/// Records kept per session. A long conversation is hundreds of turns, not thousands, and the
/// oldest are the ones nobody scrolls back to.
// ponytail: oldest-first truncation; nothing needs a real retention policy yet.
const MAX_RECORDS: usize = 1_000;

/// Paths kept per turn. A turn that touched more files than this has a diff nobody is reading file
/// by file, and the list is for a person rather than for a machine.
const MAX_FILES: usize = 500;

/// One turn's worth of what only Keel saw.
///
/// Every field past `n` is optional and defaults, because this file is read by versions that did
/// not write it — a record from an older Keel must load rather than take the session's history
/// down with it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// Which turn in the session, counted the way the transcript replays it.
    pub n: usize,
    /// The prompt that opened the turn, truncated.
    ///
    /// Not for display — the app has the real one. This is the guard that a record is still
    /// attached to the turn it was written for: a transcript can be compacted or resumed, and a
    /// record silently landing on the wrong turn would attribute one turn's files to another,
    /// which is worse than showing none.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub gate: Option<Gate>,
    /// How long the turn took, in milliseconds.
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub tokens: Option<Tokens>,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Gate {
    /// `not_run`, `running`, `passed`, `failed` or `none` — the app's own `Turn.Gate`, flattened.
    pub status: String,
    #[serde(default)]
    pub command: Option<String>,
    /// Problems the gate reported, as text. The structured form belongs to the run that produced
    /// it; what survives is enough to say the gate failed and roughly on what.
    #[serde(default)]
    pub problems: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Tokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
}

/// Whether a session id can be used as a filename.
///
/// The id arrives in a request, and it is about to become a path component. This is the same guard
/// `keel_workspace::tail` keeps for the same reason: an id carrying a `/` or a `..` writes outside
/// `.keel/turns`, and non-negotiable 7 is not a rule about one function.
fn safe(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn store_path(repo: &Utf8Path, session: &str) -> Utf8PathBuf {
    repo.join(".keel")
        .join("turns")
        .join(format!("{session}.json"))
}

/// Every record for a session, oldest first. A file that will not parse reads as no history, the
/// same way a broken `permissions.json` reads as no permissions: a session whose sidecar got
/// corrupted must still open.
pub fn read(repo: &Utf8Path, session: &str) -> Vec<Record> {
    if !safe(session) {
        return Vec::new();
    }
    std::fs::read_to_string(store_path(repo, session))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Record>>(&t).ok())
        .unwrap_or_default()
}

/// Add or replace one turn's record.
///
/// Upsert rather than append: a turn is written at its end, but a gate that finishes afterwards
/// and an auto-commit made after that both update the same turn, and three appends would be three
/// histories of one turn.
pub fn write(repo: &Utf8Path, session: &str, mut record: Record) -> Result<(), String> {
    if !safe(session) {
        return Err("that is not a session id".into());
    }
    record.files.truncate(MAX_FILES);

    let mut all = read(repo, session);
    match all.iter().position(|r| r.n == record.n) {
        Some(i) => all[i] = record,
        None => all.push(record),
    }
    all.sort_by_key(|r| r.n);
    if all.len() > MAX_RECORDS {
        all.drain(..all.len() - MAX_RECORDS);
    }

    let path = store_path(repo, session);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct SessionQuery {
    pub session: String,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionQuery>,
) -> Json<Vec<Record>> {
    let repo = state.repo();
    // Reading a file, off the executor: this runs at the end of every replay, beside the
    // transcript read that is already the slowest request Keel makes.
    Json(
        tokio::task::spawn_blocking(move || read(&repo, &q.session))
            .await
            .unwrap_or_default(),
    )
}

#[derive(Deserialize)]
pub struct RecordRequest {
    pub session: String,
    #[serde(flatten)]
    pub record: Record,
}

pub async fn record(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecordRequest>,
) -> Result<Json<Record>, (StatusCode, String)> {
    let repo = state.repo();
    let saved = req.record.clone();
    tokio::task::spawn_blocking(move || write(&repo, &req.session, req.record))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own per call, not per process. Two tests writing one store is the
    /// `keel-index-{pid}` bug in miniature, and it fails the way that one did: rarely.
    fn scratch() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            Utf8PathBuf::from(std::env::temp_dir().to_string_lossy().to_string()).join(format!(
                "keel-turns-{nanos}-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(n: usize, files: &[&str]) -> Record {
        Record {
            n,
            prompt: format!("turn {n}"),
            files: files.iter().map(|f| f.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_record_survives_the_round_trip() {
        let repo = scratch();
        write(
            &repo,
            "abc-123",
            record(8, &["src/main.rs", "/tmp/notes.html"]),
        )
        .unwrap();
        let back = read(&repo, "abc-123");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].files, vec!["src/main.rs", "/tmp/notes.html"]);
        assert_eq!(back[0].prompt, "turn 8");
    }

    /// The turn is written at its end and updated twice more — by the gate, and by the commit. A
    /// second write of the same turn replaces the first rather than filing a second history of it.
    #[test]
    fn writing_a_turn_twice_keeps_one_record() {
        let repo = scratch();
        write(&repo, "s", record(1, &["a.rs"])).unwrap();
        let mut second = record(1, &["a.rs", "b.rs"]);
        second.commit = Some("deadbeef".into());
        write(&repo, "s", second).unwrap();

        let back = read(&repo, "s");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].files.len(), 2);
        assert_eq!(back[0].commit.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn records_come_back_in_turn_order() {
        let repo = scratch();
        for n in [3, 1, 2] {
            write(&repo, "s", record(n, &[])).unwrap();
        }
        assert_eq!(
            read(&repo, "s").iter().map(|r| r.n).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    /// The id becomes a filename, and it arrives in a request. Non-negotiable 7's guard, kept here
    /// because this module is a second place that turns an id into a path.
    #[test]
    fn a_session_id_cannot_climb_out_of_the_store() {
        let repo = scratch();
        for bad in ["../../etc/passwd", "a/b", "", "with space", ".."] {
            assert!(
                write(&repo, bad, record(1, &["x"])).is_err(),
                "{bad} was accepted"
            );
            assert!(read(&repo, bad).is_empty(), "{bad} was read");
        }
        // Nothing was created outside the store on the way to refusing.
        assert!(!repo.join(".keel/turns").exists());
    }

    /// A sidecar somebody edited, or a half-written file from a crash, reads as no history. A
    /// session whose records will not parse must still open.
    #[test]
    fn an_unparseable_store_reads_as_no_history() {
        let repo = scratch();
        std::fs::create_dir_all(repo.join(".keel/turns")).unwrap();
        std::fs::write(repo.join(".keel/turns/s.json"), "{ not json").unwrap();
        assert!(read(&repo, "s").is_empty());
    }

    /// A record written by a Keel that knew fewer fields still loads. The store is read by versions
    /// that did not write it, and a missing `cost` must not take the session's history with it.
    #[test]
    fn an_older_record_still_loads() {
        let repo = scratch();
        std::fs::create_dir_all(repo.join(".keel/turns")).unwrap();
        std::fs::write(
            repo.join(".keel/turns/s.json"),
            r#"[{"n":2,"files":["a.rs"]}]"#,
        )
        .unwrap();
        let back = read(&repo, "s");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].files, vec!["a.rs"]);
        assert_eq!(back[0].cost, None);
    }

    /// A turn that touched a generated tree is not a list anyone reads file by file, and an
    /// unbounded one is what the bar forbids reaching the app.
    #[test]
    fn a_huge_file_list_is_capped() {
        let repo = scratch();
        let many: Vec<String> = (0..MAX_FILES + 200).map(|i| format!("f{i}.rs")).collect();
        write(
            &repo,
            "s",
            Record {
                n: 1,
                files: many,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(read(&repo, "s")[0].files.len(), MAX_FILES);
    }
}
