use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// How bad a finding is. Ordering is worst-first so `sort` puts critical issues at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Actively dangerous. A human should look before anything else happens in this repo.
    Critical,
    /// Will cause a production incident or block deployment outright.
    High,
    /// Real risk, but the repo can still ship.
    Medium,
    /// Worth fixing; no immediate consequence.
    Low,
    /// Informational only; never lowers the score.
    Info,
}

impl Severity {
    /// Score penalty applied per finding. Info is free by design — a scanner that punishes you for
    /// things it merely noticed trains people to ignore it.
    pub fn penalty(self) -> u32 {
        match self {
            Severity::Critical => 40,
            Severity::High => 15,
            Severity::Medium => 6,
            Severity::Low => 2,
            Severity::Info => 0,
        }
    }
}

/// The area of readiness a finding belongs to. Reports group by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dimension {
    AgentLegibility,
    Verifiability,
    WorkersCompat,
    RuntimeContract,
    StatePlacement,
    EnvironmentHygiene,
    Deployability,
    Observability,
    Security,
}

impl Dimension {
    pub fn label(self) -> &'static str {
        match self {
            Dimension::AgentLegibility => "Agent legibility",
            Dimension::Verifiability => "Verifiability",
            Dimension::WorkersCompat => "Workers compatibility",
            Dimension::RuntimeContract => "Runtime contract",
            Dimension::StatePlacement => "State placement",
            Dimension::EnvironmentHygiene => "Environment hygiene",
            Dimension::Deployability => "Deployability",
            Dimension::Observability => "Observability",
            Dimension::Security => "Security",
        }
    }
}

/// What Keel can do about a finding.
///
/// Every finding must carry a fix. A finding without one is a bug in the scanner: it makes the
/// report feel like a lint run nobody acts on, which is the failure mode this product cannot afford.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Fix {
    /// Safe to apply without review. Included in "Fix all safe issues".
    Automatic { description: String },
    /// The agent can write it, but a human should read the diff before it merges.
    Assisted { description: String },
    /// Needs a decision Keel is not entitled to make on the user's behalf.
    Manual { description: String },
}

impl Fix {
    pub fn is_automatic(&self) -> bool {
        matches!(self, Fix::Automatic { .. })
    }

    pub fn description(&self) -> &str {
        match self {
            Fix::Automatic { description }
            | Fix::Assisted { description }
            | Fix::Manual { description } => description,
        }
    }
}

/// One thing the scanner noticed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// Stable slug, e.g. `security/untrusted-agent-config`. Never changes once shipped — users and
    /// CI configs pin to these.
    pub id: &'static str,
    pub dimension: Dimension,
    pub severity: Severity,
    pub title: String,
    /// Why this matters, in plain language. Shown expanded in the report.
    pub detail: String,
    /// Repo-relative path, when the finding points at one file.
    pub path: Option<Utf8PathBuf>,
    pub fix: Fix,
}

impl Finding {
    pub fn new(
        id: &'static str,
        dimension: Dimension,
        severity: Severity,
        title: impl Into<String>,
        detail: impl Into<String>,
        fix: Fix,
    ) -> Self {
        Self {
            id,
            dimension,
            severity,
            title: title.into(),
            detail: detail.into(),
            path: None,
            fix,
        }
    }

    pub fn at(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}
