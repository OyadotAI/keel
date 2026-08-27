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
}

impl Report {
    /// Score a set of findings.
    ///
    /// Deliberately simple and total: 100 minus the sum of per-severity penalties, floored at zero.
    /// Simplicity is the point — a score nobody can predict is a score nobody trusts, and the corpus
    /// tests assert exact values, so any change here is a visible, reviewed change.
    pub fn new(findings: Vec<Finding>) -> Self {
        let penalty: u32 = findings.iter().map(|f| f.severity.penalty()).sum();
        Self {
            score: 100u32.saturating_sub(penalty),
            findings,
        }
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
