//! Delivery records keep source, correction resolution and evidence distinct from execution authority.
use crate::tasks::Source;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A fetched issue is a saved source snapshot, never permission or a mutable replacement prompt.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Issue {
    pub id: String,
    pub number: u64,
    pub repository: String,
    pub url: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub updated_at: String,
    pub fetched_at: u64,
    pub truncated: bool,
}

/// Preview IDs bind Start to what the person actually inspected, including the change repository.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssuePreview {
    pub id: String,
    pub issue: Issue,
    pub change_repository: String,
    pub parent: Option<String>,
}

/// Source provenance survives provider changes without converting external text into authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Provenance {
    Conversation,
    Issue {
        snapshot: Issue,
    },
    Recurring {
        definition: String,
        revision: u64,
        name: String,
        instruction: String,
    },
}

/// HEAD and content tree distinguish a tested checkout from edits at the same commit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckoutIdentity {
    pub head: String,
    pub tree: String,
}

/// Original diff anchors remain readable when later edits move or remove the same lines.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Anchor {
    pub identity: CheckoutIdentity,
    pub path: String,
    pub side: String,
    pub first: Option<u32>,
    pub last: Option<u32>,
    pub quote: String,
}

/// Resolution is an explicit human or authoritative remote fact, independent of message delivery.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Resolution {
    Unresolved,
    HumanResolved,
    RemoteResolved,
}

/// A correction has immutable source identity and separate revisioned body/delivery history.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Correction {
    pub id: String,
    pub revision: u64,
    pub source: String,
    pub remote_id: Option<String>,
    pub anchor: Option<Anchor>,
    pub body: String,
    pub url: Option<String>,
    pub resolution: Resolution,
    pub remote_open: Option<bool>,
    pub outdated: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub deliveries: Vec<String>,
}

/// Check history records the actual before/after captures; missing captures cannot become a pass.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckEvidence {
    pub id: String,
    pub status: String,
    pub source: String,
    pub command: Option<String>,
    pub plan: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub before: Option<CheckoutIdentity>,
    pub after: Option<CheckoutIdentity>,
    pub successor: Option<String>,
    pub problems: Vec<crate::turns::Problem>,
    #[serde(default)]
    pub warning: Option<String>,
}

/// Advisory review remains bound to its reviewed commit/base, separate from project checks.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewEvidence {
    pub id: String,
    pub sha: String,
    pub base: String,
    pub provider: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub verdict: String,
    pub report: String,
    pub confirmed: bool,
}

/// Partial remote reads retain their identities and errors rather than manufacturing an empty inbox.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Feedback {
    pub head: String,
    pub fetched_at: u64,
    pub complete: bool,
    pub checks: Vec<RemoteCheck>,
    pub threads: Vec<RemoteThread>,
    pub errors: Vec<String>,
}

/// Check-run conclusion and legacy status remain separate facts tied to the queried SHA.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteCheck {
    pub id: String,
    pub source: String,
    pub name: String,
    pub sha: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

/// Nullable remote locations are preserved rather than replaced with an invented current line.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteThread {
    pub id: String,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<RemoteComment>,
}

/// Individual comment IDs support independent pagination and deduplication across refreshes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteComment {
    pub id: String,
    pub url: String,
    pub body: String,
    pub path: String,
    pub line: Option<u32>,
    pub original_line: Option<u32>,
    pub original_commit: Option<String>,
}

/// One saved form survives tab changes, review reads and publication retries.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PrForm {
    pub title: String,
    pub body: String,
    pub draft: bool,
    pub base: String,
    pub edited: bool,
    pub seeded: bool,
}

/// Publication freezes the intended hosting identity and exact commit before a network mutation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicationIntent {
    pub id: String,
    pub task: String,
    pub owner: Source,
    pub repository: String,
    pub source_repository: String,
    pub push_identity: String,
    pub branch: String,
    pub base: String,
    pub identity: CheckoutIdentity,
    pub form: PrForm,
}

/// Requested and observed publication commits are independent, including after a lost response.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publication {
    pub receipt: String,
    pub intent: PublicationIntent,
    pub status: String,
    pub url: Option<String>,
    pub actual_head: Option<String>,
    pub error: Option<String>,
    pub updated_at: u64,
}

/// Delivery metadata is strict project data; its owner is last-observed, never writer authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub task: String,
    pub revision: u64,
    pub title: String,
    pub source: Provenance,
    pub owner: Source,
    pub base: String,
    pub base_sha: String,
    pub branch: String,
    #[serde(default)]
    pub ownership_guard: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub corrections: Vec<Correction>,
    pub checks: Vec<CheckEvidence>,
    pub reviews: Vec<ReviewEvidence>,
    pub feedback: Option<Feedback>,
    pub publication: Option<Publication>,
    pub form: PrForm,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Store {
    pub version: u32,
    pub project: String,
    pub revision: u64,
    pub records: BTreeMap<String, Record>,
    pub previews: BTreeMap<String, IssuePreview>,
}
