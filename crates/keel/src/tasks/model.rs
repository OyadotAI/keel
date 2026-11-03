//! Provider-neutral task notes retain provenance and immutable inspected handoff revisions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const STORE_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const TASK_LIMIT: usize = 128;
pub(super) const EVENT_LIMIT: usize = 256;
pub(super) const REVISION_LIMIT: usize = 8;
pub(super) const TEXT_LIMIT: usize = 8 * 1024;
pub(super) const PROMPT_LIMIT: usize = 16;
pub(super) const FILE_LIMIT: usize = 100;

/// Source identity comes from a Keel launch, never an agent-supplied filesystem path.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub lane: String,
    pub provider: String,
    pub session: String,
    pub checkout: String,
    pub wt: Option<String>,
}
/// Claims are kept separate from git and gate facts so handoffs cannot manufacture verification.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Notes {
    pub decisions: String,
    pub completed: String,
    pub remaining: String,
    pub blockers: String,
    pub next_action: String,
}
/// A saved revision is immutable; a later checkpoint cannot change the preview being continued.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Saved {
    pub revision: u64,
    pub at: String,
    pub source: Source,
    pub branch: String,
    pub head: String,
    pub incomplete: bool,
    pub text: String,
}
/// A stable receipt lets a lost HTTP response recover the same destination instead of a second writer.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Continuation {
    pub id: String,
    pub revision: u64,
    pub source: Source,
    pub destination: Source,
    pub prompt: String,
}
/// Every event has one authoritative sequence, also rendered into the project's shared changelog.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Event {
    pub sequence: u64,
    pub task: String,
    pub revision: u64,
    pub at: String,
    pub source: Source,
    pub kind: String,
    pub summary: String,
    pub next_action: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Task {
    pub id: String,
    pub revision: u64,
    pub owner: Source,
    pub members: Vec<Source>,
    pub objective: String,
    pub steering: Vec<String>,
    pub notes: Notes,
    pub evidence: Vec<String>,
    pub incomplete: bool,
    #[serde(default)]
    pub stopped_at: Option<String>,
    pub saved: Vec<Saved>,
    pub continuation: Option<Continuation>,
    pub updated_at: String,
}
#[derive(Default, Deserialize, Serialize)]
pub(super) struct Store {
    pub version: u32,
    pub project: String,
    pub sequence: u64,
    pub truncated: bool,
    pub tasks: BTreeMap<String, Task>,
    pub events: Vec<Event>,
}
