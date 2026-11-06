//! Typed coverage keeps an unfinished focused review from becoming a passing verdict.
use serde::{Deserialize, Serialize};

/// Fixed review areas are distinct from agents; checking security never claims a security child ran.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Area {
    Correctness,
    Security,
    Reliability,
    Tests,
}
const AREAS: &[Area] = &[
    Area::Correctness,
    Area::Security,
    Area::Reliability,
    Area::Tests,
];

/// A checked area carries evidence; an incomplete area prevents a passing review.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Checked,
    NotApplicable,
    Incomplete,
}

/// Evidence records what was inspected; tests coverage is source inspection, never test execution.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    pub area: Area,
    pub status: Status,
    pub evidence: String,
}

pub(super) fn complete(checks: &[Check]) -> bool {
    checks.len() == AREAS.len()
        && AREAS.iter().all(|area| {
            let matching: Vec<_> = checks.iter().filter(|check| check.area == *area).collect();
            matching.len() == 1
                && matching[0].status != Status::Incomplete
                && !matching[0].evidence.trim().is_empty()
                && (*area != Area::Correctness || matching[0].status == Status::Checked)
        })
}

pub(super) fn markdown(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|check| {
            let area = match check.area {
                Area::Correctness => "Correctness",
                Area::Security => "Security",
                Area::Reliability => "Reliability",
                Area::Tests => "Tests (source inspection)",
            };
            let status = match check.status {
                Status::Checked => "checked",
                Status::NotApplicable => "not applicable",
                Status::Incomplete => "incomplete",
            };
            format!("- **{area}: {status}.** {}", check.evidence.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked() -> Vec<Check> {
        AREAS
            .iter()
            .map(|area| Check {
                area: *area,
                status: Status::Checked,
                evidence: "inspected changed callers".into(),
            })
            .collect()
    }

    #[test]
    fn complete_coverage_is_accepted() {
        assert!(complete(&checked()));
    }

    #[test]
    fn missing_areas_are_refused() {
        let mut checks = checked();
        checks.pop();
        assert!(!complete(&checks));
    }

    #[test]
    fn duplicate_areas_are_refused() {
        let mut checks = checked();
        checks[1].area = Area::Correctness;
        assert!(!complete(&checks));
    }

    #[test]
    fn incomplete_areas_are_refused() {
        let mut checks = checked();
        checks[2].status = Status::Incomplete;
        assert!(!complete(&checks));
    }

    #[test]
    fn empty_evidence_is_refused() {
        let mut checks = checked();
        checks[3].evidence.clear();
        assert!(!complete(&checks));
    }

    #[test]
    fn correctness_cannot_be_not_applicable() {
        let mut checks = checked();
        checks[0].status = Status::NotApplicable;
        assert!(!complete(&checks));
    }

    #[test]
    fn other_areas_need_a_reason_to_be_not_applicable() {
        let mut checks = checked();
        checks[1].status = Status::NotApplicable;
        assert!(complete(&checks));
        checks[1].evidence.clear();
        assert!(!complete(&checks));
    }

    #[test]
    fn unknown_areas_and_statuses_are_refused() {
        for value in [
            serde_json::json!({"area":"unknown","status":"checked","evidence":"source"}),
            serde_json::json!({"area":"tests","status":"unknown","evidence":"source"}),
        ] {
            assert!(serde_json::from_value::<Check>(value).is_err());
        }
    }
}
