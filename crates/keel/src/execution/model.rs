//! User-owned receipts freeze execution inputs; project metadata can never enable a schedule.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Every action keeps uncertainty separate from failure before acceptance and confirmed completion.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Reserved,
    Preparing,
    Accepted,
    Queued,
    Running,
    Completed,
    Failed,
    FailedBeforeAcceptance,
    Unconfirmed,
    Interrupted,
    Stopping,
    Stopped,
}

/// Input hashes make identical retries idempotent while rejecting a reused ID with changed intent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub id: String,
    pub kind: String,
    pub task: Option<String>,
    pub fingerprint: String,
    pub input: serde_json::Value,
    pub phase: Phase,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Definition edits affect future occurrences; enabled authority is stored only on this host.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Schedule {
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub prompt: String,
    pub provider: String,
    pub base: String,
    pub interval: u64,
    pub duration: u64,
    pub enabled: bool,
    pub next_at: Option<u64>,
    pub trust: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub error: Option<String>,
}

/// Each occurrence has one durable reservation and never stores a PID as restart authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Occurrence {
    pub id: String,
    pub schedule: String,
    pub revision: u64,
    pub due_at: u64,
    pub receipt: String,
    pub task: Option<String>,
    pub phase: Phase,
    pub started_at: Option<u64>,
    pub deadline: Option<u64>,
    pub finished_at: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Store {
    pub version: u32,
    pub binding: String,
    pub revision: u64,
    pub receipts: BTreeMap<String, Receipt>,
    pub schedules: BTreeMap<String, Schedule>,
    pub occurrences: BTreeMap<String, Occurrence>,
    pub active: Option<String>,
}
