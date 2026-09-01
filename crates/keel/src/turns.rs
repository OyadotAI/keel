//! What Keel knows about a turn that the transcript does not.
//!
//! Claude Code's transcript records what was said and which tools ran, and that is all it owes
//! anyone. Everything else on a turn is Keel's own reading of the machine: the tree before it
//! ran, the files git says it moved, the gate's verdict, the commit made of them, how long it
//! took, what it cost, what it asked. Each of those is a **fact**, and every fact takes one path:
//! it is written to `.keel/turns/<session>.json` and broadcast to whoever is watching the session
//! — the lane that is driving it, a second window following it, or nobody.
//!
//! The daemon writes the facts, for every turn — the ones it drives and, once the follower
//! lifecycle lands, the ones a terminal drives. The app used to compute all of this at the end of
//! a turn it owned and post some of it back, which left a followed turn with nothing and a
//! reopened one with whatever the app had thought to send. One writer, in the process that sees
//! every window, is what makes "everything that shows up live shows up on replay" a property
//! rather than a hope.
//!
//! A record is keyed by the `uuid` of the human `user` record that opened the turn. Not by an
//! ordinal: the daemon drops the head of a long replay, a compaction summary looks like a prompt,
//! and any change to what a follower is shown renumbers everything after it. The uuid is stable
//! for the life of the transcript, because transcripts are append-only.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::lock::Locked;
use crate::serve::AppState;

/// Records kept per session. A long conversation is hundreds of turns, not thousands, and the
/// oldest are the ones nobody scrolls back to.
// ponytail: oldest-first truncation; nothing needs a real retention policy yet.
const MAX_RECORDS: usize = 1_000;
/// Paths kept per turn. A turn that touched more files than this has a diff nobody is reading
/// file by file, and the list is for a person rather than for a machine.
const MAX_FILES: usize = 500;
/// Problems kept per gate. Past this the verdict is "it failed a lot", which the count says.
const MAX_PROBLEMS: usize = 200;
const MAX_APPROVALS: usize = 100;
const MAX_PINS: usize = 50;
/// Session files kept under `.keel/turns/`. Nothing else ever deleted one.
const MAX_STORES: usize = 500;
/// Facts a lane can hold before its turn is keyed. The key arrives with the first assistant
/// record, which precedes the first tool call, so in practice this holds the `started` fact.
pub const MAX_EARLY: usize = 16;
/// A question's `input` past this is not something a card renders; it is a paste.
const MAX_INPUT: usize = 16 * 1024;
const BUS: usize = 256;

/// One turn's worth of what only Keel saw.
///
/// Every field past `turn` is optional and defaults, because this file is read by versions that
/// did not write it — a record from an older Keel must load rather than take the session's
/// history down with it. A record with no `turn` at all is from before the key existed and is
/// dropped on read: it was keyed by an ordinal nothing can match any more.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// The `uuid` of the `user` record that opened the turn. Defaulted so a record from before
    /// the key existed still parses — and is then dropped by `read`, rather than taking the
    /// whole file down with it.
    #[serde(default)]
    pub turn: String,
    /// The prompt that opened the turn, truncated to 200 characters.
    ///
    /// Not for display — the app has the real one. It is the guard that a record is still
    /// attached to the turn it was written for, and the one thing here that reads like the
    /// conversation, which is why it is short.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub started: Option<String>,
    #[serde(default)]
    pub ended: Option<String>,
    #[serde(default)]
    pub ms: Option<u64>,
    /// The tree before the turn, as a git tree id — what "restore files to before this turn"
    /// restores to.
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub gate: Option<Gate>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub failed: Option<Failed>,
    /// Why the gate and the commit were skipped, when they were: the turn ran in a tree another
    /// lane was writing, it was a plan turn, the folder is not a repository.
    #[serde(default)]
    pub shared: Option<String>,
    #[serde(default)]
    pub approvals: Vec<Approval>,
    /// The pixel verdicts, which only the app can take — the one fact it still posts.
    #[serde(default)]
    pub design: Option<Vec<Pin>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Gate {
    /// `running`, `passed`, `failed`, `none` or `aborted`.
    pub status: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub problems: Vec<Problem>,
}

/// `verify::Problem`, owned. That one borrows its severity for the life of the process; this
/// one is read back from disk.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Problem {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub severity: String,
    pub message: String,
}

impl From<&crate::verify::Problem> for Problem {
    fn from(p: &crate::verify::Problem) -> Self {
        Self {
            file: p.file.clone(),
            line: p.line,
            col: p.col,
            severity: p.severity.to_string(),
            message: p.message.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Failed {
    pub code: i32,
    /// From `agent::classify`: `auth`, `limits`, `no-such-session`, `silent`, …
    #[serde(default)]
    pub cause: String,
    /// The last lines of stderr, redacted, at most 2 KB.
    #[serde(default)]
    pub tail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub command: String,
    /// `allow`, `deny`, or the answer to a question. `None` while it waits.
    #[serde(default)]
    pub decision: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Pin {
    #[serde(default)]
    pub note: String,
    pub verdict: String,
}

/// One thing Keel learned about a turn. The same shape on the wire and in the store: the wire
/// carries it as it happens, the store replays it in the same order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum Fact {
    #[serde(rename = "turn.started")]
    Started {
        started: String,
        snapshot: Option<String>,
        prompt: String,
    },
    #[serde(rename = "turn.files")]
    Files { files: Vec<String> },
    #[serde(rename = "turn.gate")]
    Gate {
        status: String,
        command: Option<String>,
        ms: Option<u64>,
        #[serde(default)]
        problems: Vec<Problem>,
    },
    #[serde(rename = "turn.commit")]
    Commit { sha: String },
    #[serde(rename = "turn.usage")]
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost_usd: Option<f64>,
    },
    #[serde(rename = "turn.failed")]
    Failed {
        code: i32,
        cause: String,
        tail: String,
    },
    #[serde(rename = "turn.shared")]
    Shared { reason: String },
    #[serde(rename = "approval.asked")]
    ApprovalAsked {
        id: String,
        tool: String,
        command: String,
        #[serde(default)]
        rules: Vec<String>,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(rename = "approval.answered")]
    ApprovalAnswered {
        id: String,
        decision: String,
        #[serde(default)]
        answer: Option<String>,
    },
    #[serde(rename = "turn.design")]
    Design { pins: Vec<Pin> },
    #[serde(rename = "turn.ended")]
    Ended { ended: String, ms: Option<u64> },
}

impl Fact {
    fn gate(g: &Gate) -> Fact {
        Fact::Gate {
            status: g.status.clone(),
            command: g.command.clone(),
            ms: g.ms,
            problems: g.problems.clone(),
        }
    }
    fn usage(u: &Usage) -> Fact {
        Fact::Usage {
            input: u.input,
            output: u.output,
            cache_read: u.cache_read,
            cache_write: u.cache_write,
            cost_usd: u.cost_usd,
        }
    }
}

/// A fact as it travels: which turn, when, and what.
///
/// `turn` is `None` on the chat stream before the turn is keyed — the app attaches those to the
/// turn it has open, which is the only turn a chat stream can be about.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Emitted {
    pub turn: Option<String>,
    pub at: String,
    #[serde(flatten)]
    pub fact: Fact,
}

impl Record {
    /// Fold a fact into the record. Every field it names is replaced, because a fact is the
    /// newest reading of the thing it is about.
    fn absorb(&mut self, at: &str, fact: &Fact) {
        match fact {
            Fact::Started {
                started,
                snapshot,
                prompt,
            } => {
                self.started = Some(started.clone());
                self.snapshot = snapshot.clone();
                if self.prompt.is_empty() {
                    self.prompt = truncated(prompt, 200);
                }
            }
            Fact::Files { files } => self.files = files.clone(),
            Fact::Gate {
                status,
                command,
                ms,
                problems,
            } => {
                self.gate = Some(Gate {
                    status: status.clone(),
                    command: command.clone(),
                    ms: *ms,
                    problems: problems.clone(),
                })
            }
            Fact::Commit { sha } => self.commit = Some(sha.clone()),
            Fact::Usage {
                input,
                output,
                cache_read,
                cache_write,
                cost_usd,
            } => {
                self.usage = Some(Usage {
                    input: *input,
                    output: *output,
                    cache_read: *cache_read,
                    cache_write: *cache_write,
                    cost_usd: *cost_usd,
                })
            }
            Fact::Failed { code, cause, tail } => {
                self.failed = Some(Failed {
                    code: *code,
                    cause: cause.clone(),
                    tail: truncated(tail, 2048),
                })
            }
            Fact::Shared { reason } => self.shared = Some(reason.clone()),
            Fact::ApprovalAsked {
                id, tool, command, ..
            } => {
                if !self.approvals.iter().any(|a| a.id == *id) {
                    self.approvals.push(Approval {
                        id: id.clone(),
                        tool: tool.clone(),
                        command: command.clone(),
                        decision: None,
                    });
                }
            }
            Fact::ApprovalAnswered {
                id,
                decision,
                answer,
            } => {
                if let Some(a) = self.approvals.iter_mut().find(|a| a.id == *id) {
                    a.decision = Some(answer.clone().unwrap_or_else(|| decision.clone()));
                }
            }
            Fact::Design { pins } => self.design = Some(pins.clone()),
            Fact::Ended { ended, ms } => {
                self.ended = Some(ended.clone());
                self.ms = *ms;
                let _ = at;
            }
        }
    }

    /// The record as the facts that made it, in the order a live run would have sent them. This
    /// is what a replay interleaves after the record that opened the turn.
    pub fn facts(&self) -> Vec<Emitted> {
        let at = |t: &Option<String>| t.clone().unwrap_or_default();
        let mut out = Vec::new();
        let mut push = |fact: Fact, when: String| {
            out.push(Emitted {
                turn: Some(self.turn.clone()),
                at: when,
                fact,
            })
        };
        if self.started.is_some() {
            push(
                Fact::Started {
                    started: at(&self.started),
                    snapshot: self.snapshot.clone(),
                    prompt: self.prompt.clone(),
                },
                at(&self.started),
            );
        }
        if !self.files.is_empty() {
            push(
                Fact::Files {
                    files: self.files.clone(),
                },
                at(&self.ended),
            );
        }
        if let Some(g) = &self.gate {
            push(Fact::gate(g), at(&self.ended));
        }
        if let Some(sha) = &self.commit {
            push(Fact::Commit { sha: sha.clone() }, at(&self.ended));
        }
        if let Some(u) = &self.usage {
            push(Fact::usage(u), at(&self.ended));
        }
        if let Some(f) = &self.failed {
            push(
                Fact::Failed {
                    code: f.code,
                    cause: f.cause.clone(),
                    tail: f.tail.clone(),
                },
                at(&self.ended),
            );
        }
        if let Some(reason) = &self.shared {
            push(
                Fact::Shared {
                    reason: reason.clone(),
                },
                at(&self.ended),
            );
        }
        for a in &self.approvals {
            push(
                Fact::ApprovalAsked {
                    id: a.id.clone(),
                    tool: a.tool.clone(),
                    command: a.command.clone(),
                    rules: Vec::new(),
                    input: serde_json::Value::Null,
                },
                at(&self.started),
            );
            if let Some(d) = &a.decision {
                push(
                    Fact::ApprovalAnswered {
                        id: a.id.clone(),
                        decision: d.clone(),
                        answer: None,
                    },
                    at(&self.started),
                );
            }
        }
        if let Some(pins) = &self.design {
            push(Fact::Design { pins: pins.clone() }, at(&self.ended));
        }
        if self.ended.is_some() {
            push(
                Fact::Ended {
                    ended: at(&self.ended),
                    ms: self.ms,
                },
                at(&self.ended),
            );
        }
        out
    }

    fn cap(&mut self) {
        self.files.truncate(MAX_FILES);
        if let Some(g) = &mut self.gate {
            g.problems.truncate(MAX_PROBLEMS);
        }
        self.approvals.truncate(MAX_APPROVALS);
        if let Some(d) = &mut self.design {
            d.truncate(MAX_PINS);
        }
    }
}

fn truncated(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// MARK: - The store

/// Whether a session id can be used as a filename.
///
/// The id arrives in a request, and it is about to become a path component. This is the same
/// guard `keel_workspace::tail` keeps for the same reason: an id carrying a `/` or a `..` writes
/// outside `.keel/turns`, and non-negotiable 7 is not a rule about one function.
fn safe(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn store_dir(repo: &Utf8Path) -> Utf8PathBuf {
    repo.join(".keel").join("turns")
}

fn store_path(repo: &Utf8Path, session: &str) -> Utf8PathBuf {
    store_dir(repo).join(format!("{session}.json"))
}

/// Every record for a session, oldest first. A file that will not parse reads as no history, the
/// same way a broken `permissions.json` reads as no permissions: a session whose sidecar got
/// corrupted must still open. Records without a key are from before the key existed and are
/// dropped: nothing could match them to a turn any more.
pub fn read(repo: &Utf8Path, session: &str) -> Vec<Record> {
    if !safe(session) {
        return Vec::new();
    }
    std::fs::read_to_string(store_path(repo, session))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Record>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !r.turn.is_empty())
        .collect()
}

/// One lock for every session file.
///
/// Two writers to one session are real — the gate finishing and an approval being answered are
/// different tasks — and a read-modify-write without one loses whichever landed second.
// ponytail: one lock for every session file; per-session if two lanes ever contend measurably.
fn store_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Add to one turn's record, creating it if this is the first fact about it.
///
/// Atomic: written beside the file and renamed over it, so a crash mid-write leaves the previous
/// history rather than half a JSON array. Serialised under `store_lock`.
pub fn upsert(
    repo: &Utf8Path,
    session: &str,
    turn: &str,
    change: impl FnOnce(&mut Record),
) -> Result<(), String> {
    if !safe(session) {
        return Err("that is not a session id".into());
    }
    if turn.is_empty() {
        return Err("a record needs the turn that opened it".into());
    }
    let _held = store_lock().locked();
    let mut all = read(repo, session);
    let record = match all.iter_mut().find(|r| r.turn == turn) {
        Some(r) => r,
        None => {
            all.push(Record {
                turn: turn.to_string(),
                ..Default::default()
            });
            all.last_mut().expect("just pushed")
        }
    };
    change(record);
    record.cap();
    if all.len() > MAX_RECORDS {
        all.drain(..all.len() - MAX_RECORDS);
    }

    let path = store_path(repo, session);
    let dir = store_dir(repo);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!("{session}.tmp-{}-{nanos}", std::process::id()));
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    prune(&dir);
    Ok(())
}

/// Keep the store to the newest `MAX_STORES` session files. One `read_dir` per write; a session
/// file is a few KB, and nothing else ever deleted one.
fn prune(dir: &Utf8Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= MAX_STORES {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    for (_, path) in files.iter().take(files.len() - MAX_STORES) {
        let _ = std::fs::remove_file(path);
    }
}

// MARK: - The bus

fn bus() -> &'static Mutex<HashMap<String, broadcast::Sender<Arc<Emitted>>>> {
    static BUSES: OnceLock<Mutex<HashMap<String, broadcast::Sender<Arc<Emitted>>>>> =
        OnceLock::new();
    BUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Watch a session's facts as they are emitted. The entry is dropped once nobody is listening,
/// so an idle daemon holds no channel per session it ever served.
pub fn subscribe(session: &str) -> broadcast::Receiver<Arc<Emitted>> {
    let mut map = bus().locked();
    map.entry(session.to_string())
        .or_insert_with(|| broadcast::channel(BUS).0)
        .subscribe()
}

fn publish(session: &str, emitted: Emitted) {
    let mut map = bus().locked();
    if let Some(tx) = map.get(session)
        && tx.send(Arc::new(emitted)).is_err()
    {
        // No receiver left. The sender goes with it, or every session ever watched would keep
        // a channel for the life of the daemon.
        map.remove(session);
    }
}

/// Now, as the transcript writes it: ISO-8601, UTC, milliseconds.
pub fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let millis = d.subsec_millis();
    // Civil-from-days (Howard Hinnant), because pulling a date crate in for one timestamp
    // format is the dependency the bar says to look before adding.
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// The one path every fact takes: into the store, then onto the bus. Blocking — the store is a
/// file — so callers are already off the executor.
pub fn emit(repo: &Utf8Path, session: &str, turn: &str, fact: Fact) -> Emitted {
    let at = now();
    if let Err(e) = upsert(repo, session, turn, |r| r.absorb(&at, &fact)) {
        tracing::warn!("could not record a fact for {session}: {e}");
    }
    let emitted = Emitted {
        turn: Some(turn.to_string()),
        at,
        fact,
    };
    publish(session, emitted.clone());
    emitted
}

/// A fact about whatever turn a lane is running, from a handler that knows the lane and not the
/// turn — the approval hook, the manual gate.
///
/// Off the executor on its own thread. Before the turn is keyed the fact is held on the lane and
/// written when the key arrives; a lane with no turn at all drops it, with a line saying so,
/// because a fact about nothing has nowhere to go.
pub fn emit_for_lane(state: &Arc<AppState>, lane: &str, fact: Fact) {
    match state.turn_key(lane) {
        Some((session, turn)) => {
            let repo = state.repo();
            tokio::task::spawn_blocking(move || {
                emit(&repo, &session, &turn, fact);
            });
        }
        None if state.hold_early(lane, fact.clone()) => {}
        None => tracing::debug!("a fact for lane {lane} had no turn to belong to"),
    }
}

/// Persist the facts that arrived before the turn was keyed. Not broadcast: the chat stream sent
/// its own `started` straight to the app when it happened, and the approval cards came from the
/// poll. What was missing was only the record.
pub fn flush_early(repo: &Utf8Path, session: &str, turn: &str, early: Vec<(String, Fact)>) {
    if early.is_empty() {
        return;
    }
    if let Err(e) = upsert(repo, session, turn, |r| {
        for (at, fact) in &early {
            r.absorb(at, fact);
        }
    }) {
        tracing::warn!("could not record early facts for {session}: {e}");
    }
}

/// A question's input, bounded. A card renders the questions and their options; a paste the
/// agent put in a command is not that.
pub fn bounded_input(input: &serde_json::Value) -> serde_json::Value {
    match serde_json::to_string(input) {
        Ok(s) if s.len() <= MAX_INPUT => input.clone(),
        _ => serde_json::json!({ "truncated": true }),
    }
}

// MARK: - The lifecycle

/// What was true before the turn ran.
pub struct Begun {
    pub started: String,
    pub snapshot: Option<String>,
    /// `path + status` for every change git saw. The turn's files are what moved against this.
    pub fingerprint: HashSet<String>,
}

/// Photograph the tree and note what git already had to say about it. Blocking: two gits.
///
/// Ceiling, said plainly: the photograph is taken after the prompt was submitted, so for a
/// followed turn an `Edit` that lands in the first few tens of milliseconds is inside it.
pub fn begin(checkout: &Utf8Path) -> Begun {
    Begun {
        started: now(),
        snapshot: crate::snapshot::snapshot(checkout).ok(),
        fingerprint: fingerprint(checkout),
    }
}

fn fingerprint(checkout: &Utf8Path) -> HashSet<String> {
    crate::repo::git_status(checkout)
        .changes
        .iter()
        .map(|c| format!("{}{}", c.path, c.status))
        .collect()
}

/// Everything `finish` needs to know about the turn that just ended.
pub struct Finish {
    pub repo: Utf8PathBuf,
    pub checkout: Utf8PathBuf,
    pub session: String,
    pub turn: String,
    pub prompt: String,
    pub begun: Begun,
    /// False for a plan turn: files only, no gate, no commit.
    pub writes: bool,
    pub usage: Option<Usage>,
    pub failed: Option<Failed>,
    /// The app sent pins with this turn and will post their verdicts; the commit waits for them.
    pub expect_design: bool,
    /// The person's "commit after every turn" setting.
    pub auto_commit: bool,
}

/// How long the gate may run inside a turn before the commit is made without it.
const DESIGN_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// What Keel does when a turn ends, whichever way it ended and whoever drove it.
///
/// In order, each step one fact: the files git says moved against `begun`; the gate, if the turn
/// wrote anything, with `running` said first so a follower sees the same spinner the lane does;
/// the design verdict, waited for when one is coming; the commit, if the gate did not say no and
/// the tree is still this turn's to commit; what it cost and whether it failed; and `ended`.
///
/// `still_ours` is asked before anything that changes the tree and once a second while the gate
/// runs: for a lane it is always true, because the claim is held for the whole of this; for a
/// followed terminal session it is "the pid file still says idle". A gate that loses the tree is
/// aborted — its process group, never `claude`'s — and nothing is committed. `window_open` is
/// whether anyone is still looking: a closed window is not worth a test run, but the commit is
/// made regardless, because a checkpoint nobody watched beats work left uncommitted.
pub async fn finish(
    state: &Arc<AppState>,
    f: Finish,
    still_ours: impl Fn() -> bool + Send + Sync,
    window_open: impl Fn() -> bool + Send + Sync,
) {
    let repo = f.repo.clone();
    let session = f.session.clone();
    let turn = f.turn.clone();
    let emit_now = |fact: Fact| {
        let (repo, session, turn) = (repo.clone(), session.clone(), turn.clone());
        async move {
            crate::serve::blocking(
                move || {
                    emit(&repo, &session, &turn, fact);
                },
                (),
            )
            .await;
        }
    };

    // 1. Files. The daemon's reading of what moved, which is the only one that catches a heredoc.
    let checkout = f.checkout.clone();
    let before = f.begun.fingerprint.clone();
    let files: Vec<String> = crate::serve::blocking(
        move || {
            let mut moved: Vec<String> = fingerprint(&checkout)
                .difference(&before)
                .map(|entry| entry[..entry.len().saturating_sub(2)].to_string())
                .collect();
            moved.sort();
            moved.dedup();
            moved
        },
        Vec::new(),
    )
    .await;
    if !files.is_empty() {
        emit_now(Fact::Files {
            files: files.clone(),
        })
        .await;
    }

    // 2. The gate. The agent does not grade its own work — and a turn that wrote nothing has no
    // work to grade, so a question answered costs no test run.
    let mut gate: Option<Gate> = None;
    let mut shared: Option<String> = None;
    if !f.writes {
        shared = Some("a plan turn writes nothing, so nothing was checked or committed".into());
    } else if !files.is_empty() && window_open() {
        let checkout = f.checkout.clone();
        let started = std::time::Instant::now();
        // Said before it runs, so a follower sees the same spinner the lane does. The command
        // is detected here rather than reported by the run, because the run's sink is
        // synchronous and a fact is a file write.
        let announced: String = crate::serve::blocking(
            {
                let checkout = checkout.clone();
                move || {
                    crate::verify::detect_all(&checkout)
                        .iter()
                        .map(|c| {
                            if c.dir.is_empty() {
                                c.command.clone()
                            } else {
                                format!("{} · {}", c.dir, c.command)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            },
            String::new(),
        )
        .await;
        if !announced.is_empty() {
            emit_now(Fact::Gate {
                status: "running".into(),
                command: Some(announced.clone()),
                ms: None,
                problems: Vec::new(),
            })
            .await;
        }
        let mut problems = Vec::new();
        let verdict = crate::verify::run_all(
            &checkout,
            |event| {
                if let crate::verify::GateEvent::Problem(p) = event
                    && problems.len() < MAX_PROBLEMS
                {
                    problems.push(Problem::from(&p));
                }
            },
            || !still_ours() || !window_open(),
        )
        .await;
        let command = if announced.is_empty() {
            verdict.command.clone()
        } else {
            announced
        };
        let g = Gate {
            status: verdict.status.to_string(),
            command: Some(command),
            ms: Some(started.elapsed().as_millis() as u64),
            problems,
        };
        emit_now(Fact::gate(&g)).await;
        gate = Some(g);
    }

    // 3. The design verdict, when the app owes one. "It edited the wrong file" is a reason not
    // to commit, and it used to arrive after the commit it should have questioned.
    let mut design_says_no = false;
    if f.expect_design {
        let mut rx = subscribe(&session);
        let already = crate::serve::blocking(
            {
                let (repo, session, turn) = (repo.clone(), session.clone(), turn.clone());
                move || {
                    read(&repo, &session)
                        .into_iter()
                        .find(|r| r.turn == turn)
                        .and_then(|r| r.design)
                }
            },
            None,
        )
        .await;
        let pins = match already {
            Some(pins) => Some(pins),
            None => {
                let wait = tokio::time::timeout(DESIGN_WAIT, async {
                    loop {
                        match rx.recv().await {
                            Ok(e) if e.turn.as_deref() == Some(&turn) => {
                                if let Fact::Design { pins } = &e.fact {
                                    return Some(pins.clone());
                                }
                            }
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                })
                .await;
                wait.ok().flatten()
            }
        };
        design_says_no = pins
            .as_deref()
            .is_some_and(|p| p.iter().any(|pin| pin.verdict == "nothingChanged"));
    }

    // 4. The commit. Accepted work becomes a checkpoint, so the tree stays small and every step
    // is a place to go back to.
    let gate_allows = gate
        .as_ref()
        .is_none_or(|g| matches!(g.status.as_str(), "passed" | "none"));
    if f.writes && f.auto_commit && !files.is_empty() && shared.is_none() {
        if design_says_no {
            shared = Some(
                "the pixel check found a pinned element unchanged, so this turn was not committed"
                    .into(),
            );
        } else if !gate_allows {
            // Said by the gate itself; nothing to add.
        } else if !still_ours() {
            shared = Some("the working tree was taken over before the commit".into());
        } else if state.writer_in_other(&f.checkout, &session) {
            shared = Some(
                "another feature is editing this working tree, so this turn was not committed — \
                 a commit now would hold its half-written files. The changes are still here; \
                 commit them once it finishes."
                    .into(),
            );
        } else {
            let checkout = f.checkout.clone();
            let message = commit_message(&f.prompt, gate.as_ref());
            let sha: Option<String> = crate::serve::blocking(
                move || match crate::worktree::commit_all(&checkout, &message) {
                    Ok(true) => crate::repo::git_log(&checkout, 1)
                        .first()
                        .map(|c| c.sha.clone()),
                    Ok(false) => None,
                    Err(e) => {
                        tracing::warn!("the automatic commit failed: {e}");
                        None
                    }
                },
                None,
            )
            .await;
            if let Some(sha) = sha {
                emit_now(Fact::Commit { sha }).await;
            }
        }
    }
    if let Some(reason) = shared {
        emit_now(Fact::Shared { reason }).await;
    }

    // 5. What it cost and whether it failed.
    if let Some(u) = &f.usage {
        emit_now(Fact::usage(u)).await;
    }
    if let Some(fail) = &f.failed {
        emit_now(Fact::Failed {
            code: fail.code,
            cause: fail.cause.clone(),
            tail: fail.tail.clone(),
        })
        .await;
    }

    // 6. Over.
    let ms = crate::serve::blocking(
        {
            let started = f.begun.started.clone();
            move || elapsed_ms(&started)
        },
        None,
    )
    .await;
    emit_now(Fact::Ended { ended: now(), ms }).await;
}

/// Milliseconds since an ISO-8601 stamp this module wrote, or `None` if it will not parse.
fn elapsed_ms(started: &str) -> Option<u64> {
    let then = parse_iso(started)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some(now.saturating_sub(then))
}

/// The inverse of `now()`, for stamps `now()` wrote. Milliseconds since the epoch.
fn parse_iso(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let (hms, millis) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi, sec) = (t.next()??, t.next()??, t.next()??);
    let millis: u64 = millis.chars().take(3).collect::<String>().parse().ok()?;
    // Days-from-civil, the inverse of the algorithm in `now()`.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    Some((secs as u64) * 1000 + millis)
}

/// The checkpoint's message, built the way the app built it: the first line of the ask as the
/// subject, and one sentence saying whose commit this is and what the checks said.
fn commit_message(prompt: &str, gate: Option<&Gate>) -> String {
    let subject: String = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('@'))
        .unwrap_or("agent turn")
        .chars()
        .take(72)
        .collect();
    let checks = match gate.map(|g| g.status.as_str()) {
        Some("passed") => "passed.",
        Some("none") => "found no check to run.",
        _ => "were not run.",
    };
    format!("{subject}\n\nMade by the agent in Keel. The project's checks {checks}")
}

// MARK: - The one thing the app still posts

#[derive(Deserialize)]
pub struct DesignRequest {
    /// The lane, not the turn: the app never learns its own turn's key before the verdict is
    /// due, and the daemon knows which turn the lane is on.
    pub lane: String,
    pub pins: Vec<Pin>,
}

/// `POST /api/turns`: the pixel verdicts, which only the app can take. Everything else a turn
/// record holds is the daemon's own observation.
///
/// `emit_for_lane` does its writing on its own thread.
// no-blocking: the write is spawned inside `emit_for_lane`.
pub async fn record(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DesignRequest>,
) -> Result<Json<bool>, (StatusCode, String)> {
    if req.lane.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "a design verdict needs its lane".into()));
    }
    emit_for_lane(&state, &req.lane, Fact::Design { pins: req.pins });
    Ok(Json(true))
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

    fn files(paths: &[&str]) -> Fact {
        Fact::Files {
            files: paths.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn a_record_is_keyed_by_the_record_that_opened_the_turn() {
        let repo = scratch();
        emit(
            &repo,
            "abc-123",
            "u-8",
            files(&["src/main.rs", "/tmp/notes.html"]),
        );
        emit(
            &repo,
            "abc-123",
            "u-8",
            Fact::Started {
                started: now(),
                snapshot: None,
                prompt: "turn 8".into(),
            },
        );
        let back = read(&repo, "abc-123");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].turn, "u-8");
        assert_eq!(back[0].files, vec!["src/main.rs", "/tmp/notes.html"]);
        assert_eq!(back[0].prompt, "turn 8");
    }

    /// A gate finishing and an approval being answered are different tasks writing one turn.
    /// Whichever lands second must add to the record, not replace it.
    #[test]
    fn two_writers_to_one_session_keep_both_facts() {
        let repo = scratch();
        emit(&repo, "s", "u-1", files(&["a.rs"]));
        emit(
            &repo,
            "s",
            "u-1",
            Fact::Commit {
                sha: "deadbeef".into(),
            },
        );
        emit(
            &repo,
            "s",
            "u-1",
            Fact::ApprovalAsked {
                id: "p1".into(),
                tool: "Bash".into(),
                command: "ls".into(),
                rules: vec![],
                input: serde_json::Value::Null,
            },
        );
        let back = read(&repo, "s");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].files, vec!["a.rs"]);
        assert_eq!(back[0].commit.as_deref(), Some("deadbeef"));
        assert_eq!(back[0].approvals.len(), 1);
    }

    /// The tmp file a crash leaves behind is not the store, and the store still parses.
    #[test]
    fn a_write_is_atomic_under_a_crash_mid_write() {
        let repo = scratch();
        emit(&repo, "s", "u-1", files(&["a.rs"]));
        std::fs::write(repo.join(".keel/turns/s.tmp-1-2"), "{ half").unwrap();
        assert_eq!(read(&repo, "s")[0].files, vec!["a.rs"]);
        emit(&repo, "s", "u-2", files(&["b.rs"]));
        assert_eq!(read(&repo, "s").len(), 2);
    }

    /// A record from before the key existed cannot be matched to a turn; it is dropped rather
    /// than shown against the wrong one.
    #[test]
    fn a_v1_record_without_a_key_is_dropped_not_loaded() {
        let repo = scratch();
        std::fs::create_dir_all(repo.join(".keel/turns")).unwrap();
        std::fs::write(
            repo.join(".keel/turns/s.json"),
            r#"[{"n":2,"files":["a.rs"]},{"turn":"u-9","files":["b.rs"]}]"#,
        )
        .unwrap();
        let back = read(&repo, "s");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].turn, "u-9");
    }

    #[tokio::test]
    async fn every_fact_of_a_turn_reaches_a_subscriber_in_order() {
        let repo = scratch();
        let mut rx = subscribe("bus-1");
        emit(&repo, "bus-1", "u-1", files(&["a.rs"]));
        emit(&repo, "bus-1", "u-1", Fact::Commit { sha: "abc".into() });
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert!(matches!(first.fact, Fact::Files { .. }));
        assert!(matches!(second.fact, Fact::Commit { .. }));
        assert_eq!(first.turn.as_deref(), Some("u-1"));
    }

    #[test]
    fn a_bus_with_no_listener_holds_nothing() {
        let repo = scratch();
        {
            let _rx = subscribe("bus-2");
        }
        emit(&repo, "bus-2", "u-1", files(&["a.rs"]));
        assert!(!bus().locked().contains_key("bus-2"));
    }

    #[test]
    fn gate_problems_are_capped() {
        let repo = scratch();
        let problems: Vec<Problem> = (0..MAX_PROBLEMS + 50)
            .map(|i| Problem {
                file: format!("f{i}.rs"),
                ..Default::default()
            })
            .collect();
        emit(
            &repo,
            "s",
            "u-1",
            Fact::Gate {
                status: "failed".into(),
                command: Some("make check".into()),
                ms: Some(1),
                problems,
            },
        );
        assert_eq!(
            read(&repo, "s")[0].gate.as_ref().unwrap().problems.len(),
            MAX_PROBLEMS
        );
    }

    /// The id becomes a filename, and it arrives in a request. Non-negotiable 7's guard, kept here
    /// because this module is a second place that turns an id into a path.
    #[test]
    fn a_session_id_cannot_climb_out_of_the_store() {
        let repo = scratch();
        for bad in ["../../etc/passwd", "a/b", "", "with space", ".."] {
            assert!(
                upsert(&repo, bad, "u-1", |_| {}).is_err(),
                "{bad} was accepted"
            );
            assert!(read(&repo, bad).is_empty(), "{bad} was read");
        }
        assert!(!repo.join(".keel/turns").exists());
    }

    /// A sidecar somebody edited, or a half-written file from a crash, reads as no history.
    #[test]
    fn an_unparseable_store_reads_as_no_history() {
        let repo = scratch();
        std::fs::create_dir_all(repo.join(".keel/turns")).unwrap();
        std::fs::write(repo.join(".keel/turns/s.json"), "{ not json").unwrap();
        assert!(read(&repo, "s").is_empty());
    }

    /// A record written by a Keel that knew fewer fields still loads.
    #[test]
    fn an_older_record_still_loads() {
        let repo = scratch();
        std::fs::create_dir_all(repo.join(".keel/turns")).unwrap();
        std::fs::write(
            repo.join(".keel/turns/s.json"),
            r#"[{"turn":"u-2","files":["a.rs"]}]"#,
        )
        .unwrap();
        let back = read(&repo, "s");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].files, vec!["a.rs"]);
        assert_eq!(back[0].usage, None);
    }

    #[test]
    fn a_huge_file_list_is_capped() {
        let repo = scratch();
        let many: Vec<String> = (0..MAX_FILES + 200).map(|i| format!("f{i}.rs")).collect();
        emit(&repo, "s", "u-1", Fact::Files { files: many });
        assert_eq!(read(&repo, "s")[0].files.len(), MAX_FILES);
    }

    /// The wire shape is the store shape: a fact round-trips through JSON with its `kind`.
    #[test]
    fn a_fact_round_trips_with_its_kind() {
        let e = Emitted {
            turn: Some("u-1".into()),
            at: now(),
            fact: Fact::Gate {
                status: "passed".into(),
                command: Some("make check".into()),
                ms: Some(4100),
                problems: vec![],
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""kind":"turn.gate""#), "{json}");
        assert!(json.contains(r#""turn":"u-1""#));
        let back: Emitted = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    /// A replay carries the same facts, in the order a live run sends them.
    #[test]
    fn a_record_replays_as_the_facts_that_made_it() {
        let repo = scratch();
        emit(
            &repo,
            "s",
            "u-1",
            Fact::Started {
                started: now(),
                snapshot: Some("t".repeat(40)),
                prompt: "do it".into(),
            },
        );
        emit(&repo, "s", "u-1", files(&["a.rs"]));
        emit(&repo, "s", "u-1", Fact::Commit { sha: "abc".into() });
        emit(
            &repo,
            "s",
            "u-1",
            Fact::Ended {
                ended: now(),
                ms: Some(10),
            },
        );
        let kinds: Vec<&str> = read(&repo, "s")[0]
            .facts()
            .iter()
            .map(|e| match e.fact {
                Fact::Started { .. } => "started",
                Fact::Files { .. } => "files",
                Fact::Commit { .. } => "commit",
                Fact::Ended { .. } => "ended",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["started", "files", "commit", "ended"]);
    }

    #[test]
    fn the_clock_reads_back_what_it_wrote() {
        let stamp = now();
        let parsed = parse_iso(&stamp).unwrap();
        let actual = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(
            actual.abs_diff(parsed) < 2_000,
            "{stamp} → {parsed} vs {actual}"
        );
        assert_eq!(
            parse_iso("2026-09-01T18:31:49.259Z"),
            Some(1_788_287_509_259)
        );
    }

    #[test]
    fn the_commit_message_is_the_ask_and_the_verdict() {
        let m = commit_message("@file.txt\nfix the bug\nmore", None);
        assert!(m.starts_with("fix the bug\n\n"));
        assert!(m.ends_with("were not run."));
    }
}
