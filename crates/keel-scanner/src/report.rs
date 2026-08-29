use crate::finding::{Dimension, Finding, Severity};
use serde::Serialize;
use std::collections::BTreeMap;

/// The result of a scan: every finding, plus a score derived from them.
/// Reports are produced, never consumed: `Finding::id` is a `&'static str` so that check
/// identifiers are fixed at compile time, which rules out `Deserialize`. Anything needing to read a
/// report back gets its own owned DTO rather than weakening that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub score: u32,
    pub findings: Vec<Finding>,
    /// What the repository is and where it runs. `None` for a report built from findings
    /// alone (an empty daemon state).
    pub profile: Option<crate::Profile>,
    /// The findings as a road: which to do first and why, so a 40-item report reads as five
    /// steps. Empty when there is nothing to do.
    pub plan: Vec<Phase>,
}

/// One step on the road from "it runs on my machine" to production.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase {
    pub title: &'static str,
    pub why: &'static str,
    /// Finding ids in this phase, in report order.
    pub findings: Vec<&'static str>,
}

/// The phases, in order, and the finding ids that belong to each. A finding not named here
/// lands in the last phase it fits by dimension.
const PHASES: &[(&str, &str, &[&str])] = &[
    (
        "Make it safe",
        "Nothing else matters while someone can run code on your machine or read a secret from the repository.",
        &[
            "security/untrusted-agent-config",
            "security/committed-secret",
            "security/no-input-validation",
        ],
    ),
    (
        "Make it checkable",
        "One command that says whether it is good. Every later step is verified by this one.",
        &[
            "verify/no-gate",
            "verify/no-tests",
            "verify/no-ci",
            "agent/no-instructions",
            "agent/no-reviewers",
            "docs/no-readme",
        ],
    ),
    (
        "Make it deployable the same way twice",
        "An image, a compose file for the laptop, manifests for the cluster, dev and prod apart, a pipeline that does it — so production is not a person.",
        &[
            "deploy/no-env-example",
            "deploy/no-dockerfile",
            "deploy/no-dockerignore",
            "security/image-runs-as-root",
            "deploy/no-compose",
            "deploy/no-manifests",
            "deploy/no-environments",
            "deploy/no-pipeline",
            "env/no-wrangler-config",
            "env/shared-binding",
        ],
    ),
    (
        "Make it survive a bad day",
        "Rollouts that drop nothing, a schema the repository can recreate, limits that keep one client from being an outage.",
        &[
            "runtime/no-health-endpoint",
            "reliability/no-graceful-shutdown",
            "reliability/no-migrations",
            "security/no-rate-limit",
        ],
    ),
    (
        "Make it observable",
        "Logs you can query, errors that page, metrics per route. You cannot run what you cannot see.",
        &["observability/no-structured-logs"],
    ),
    (
        "Own where it runs",
        "Build around the cloud you already pay for; know the ceiling of the platform you started on; keep the data somewhere the repository can recreate.",
        &[
            "infra/backend-as-a-service",
            "infra/managed-platform",
            "infra/cloud-present",
            "infra/cloudflare-present",
        ],
    ),
];

impl Report {
    /// Score a set of findings.
    ///
    /// Deliberately simple and total: 100 minus the sum of per-severity penalties, floored at zero.
    /// Simplicity is the point — a score nobody can predict is a score nobody trusts, and the corpus
    /// tests assert exact values, so any change here is a visible, reviewed change.
    pub fn new(findings: Vec<Finding>) -> Self {
        let penalty: u32 = findings.iter().map(|f| f.severity.penalty()).sum();
        let plan = Self::plan_for(&findings);
        Self {
            score: 100u32.saturating_sub(penalty),
            findings,
            profile: None,
            plan,
        }
    }

    pub fn with_profile(findings: Vec<Finding>, profile: crate::Profile) -> Self {
        let mut r = Self::new(findings);
        r.profile = Some(profile);
        r
    }

    fn plan_for(findings: &[Finding]) -> Vec<Phase> {
        let mut phases: Vec<Phase> = PHASES
            .iter()
            .map(|(title, why, ids)| Phase {
                title,
                why,
                findings: findings
                    .iter()
                    .map(|f| f.id)
                    .filter(|id| ids.contains(id))
                    .collect(),
            })
            .collect();
        // Anything a phase did not claim goes by dimension, so a new check is never invisible.
        for f in findings {
            if phases.iter().any(|p| p.findings.contains(&f.id)) {
                continue;
            }
            let i = match f.dimension {
                Dimension::Security => 0,
                Dimension::Verifiability | Dimension::AgentLegibility => 1,
                Dimension::Deployability
                | Dimension::EnvironmentHygiene
                | Dimension::WorkersCompat => 2,
                Dimension::RuntimeContract | Dimension::StatePlacement => 3,
                Dimension::Observability => 4,
            };
            phases[i].findings.push(f.id);
        }
        phases.retain(|p| !p.findings.is_empty());
        phases
    }

    /// True when nothing critical or high is outstanding. This is the bar for "ready to deploy",
    /// not a perfect score — medium and low findings should not block a team from shipping.
    pub fn is_shippable(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|f| matches!(f.severity, Severity::Critical | Severity::High))
    }

    /// Findings that Keel can fix without review, for the "Fix all safe issues" action.
    pub fn automatic_fixes(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fix.is_automatic())
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }

    /// Findings grouped by dimension, for rendering the report by section.
    pub fn by_dimension(&self) -> BTreeMap<Dimension, Vec<&Finding>> {
        let mut grouped: BTreeMap<Dimension, Vec<&Finding>> = BTreeMap::new();
        for finding in &self.findings {
            grouped.entry(finding.dimension).or_default().push(finding);
        }
        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::Fix;

    fn finding(severity: Severity) -> Finding {
        Finding::new(
            "test/example",
            Dimension::Security,
            severity,
            "example",
            "detail",
            Fix::Automatic {
                description: "fix".to_string(),
            },
        )
    }

    #[test]
    fn a_clean_repo_scores_100() {
        assert_eq!(Report::new(vec![]).score, 100);
    }

    #[test]
    fn penalties_accumulate() {
        let report = Report::new(vec![finding(Severity::High), finding(Severity::Medium)]);
        assert_eq!(report.score, 100 - 15 - 6);
    }

    #[test]
    fn score_floors_at_zero_rather_than_underflowing() {
        let findings = (0..10).map(|_| finding(Severity::Critical)).collect();
        assert_eq!(Report::new(findings).score, 0);
    }

    #[test]
    fn info_findings_are_free() {
        assert_eq!(Report::new(vec![finding(Severity::Info)]).score, 100);
    }

    #[test]
    fn shippable_ignores_medium_and_below() {
        assert!(Report::new(vec![finding(Severity::Medium)]).is_shippable());
        assert!(!Report::new(vec![finding(Severity::High)]).is_shippable());
    }
}
