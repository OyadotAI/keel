//! Child execution identity remains separate from process identity and from observation health.
use serde::Serialize;

/// Unknown is loss of evidence, never inferred completion or permission to run queued work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    Waiting,
    Done,
    Failed,
    Cancelled,
    Interrupted,
    Unknown,
}
impl Status {
    pub(super) fn active(self) -> bool {
        matches!(self, Self::Working | Self::Waiting | Self::Unknown)
    }
}

/// Output may be unavailable even when the provider reports a trustworthy lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Waiting,
    Unavailable,
    Disconnected,
}

/// One execution, including a later turn on a reused child, has its own stable identity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Invocation {
    pub id: String,
    pub child: Option<String>,
    pub member: Option<String>,
    pub status: Status,
    pub activity: Option<String>,
    pub started: Option<String>,
    pub finished: Option<String>,
    pub output: Availability,
}

/// A bounded authoritative lifecycle snapshot replaces legacy terminal-only background guesses.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub invocations: Vec<Invocation>,
    pub truncated: bool,
}

/// Selectable output excludes raw metadata, injected system prompts, and hidden reasoning.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputLine {
    pub kind: String,
    pub text: String,
}

/// Selected output health never changes the independent lifecycle status of its invocation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Detail {
    pub items: Vec<OutputLine>,
    pub availability: Availability,
    pub truncated: bool,
}
