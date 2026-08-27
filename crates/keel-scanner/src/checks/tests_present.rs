use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

/// Whether the repository has a test suite at all.
///
/// This is the highest-ROI verification gate there is. A pre-existing suite is the one signal about
/// agent-written code that is genuinely independent of the agent — it was written before, by
/// someone with a different idea of what correct means.
pub struct TestsPresent;

/// Path fragments that indicate a test file across the ecosystems Keel targets.
const TEST_MARKERS: &[&str] = &[
    ".test.",
    ".spec.",
    "_test.",
    "test_",
    "/tests/",
    "/__tests__/",
];

impl Check for TestsPresent {
    fn id(&self) -> &'static str {
        "verify/no-tests"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Verifiability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let count = ctx
            .files()
            .filter(|p| {
                let s = p.as_str();
                TEST_MARKERS.iter().any(|m| s.contains(m))
            })
            .count();

        if count > 0 {
            return Vec::new();
        }

        vec![Finding::new(
            self.id(),
            self.dimension(),
            Severity::High,
            "No test suite found",
            "Without tests, nothing independently contradicts an agent that believes it is done. \
             Agent self-verification is close to worthless — amplifying a thin test suite has been \
             measured to drop apparent pass rates by 19-29%, so the tests you already have are \
             worth more than any the agent writes for itself.",
            Fix::Assisted {
                description:
                    "Add a test runner and cover the primary request path first, so every \
                              later change has something to fail against."
                        .to_string(),
            },
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn silent_when_tests_exist() {
        let (_dir, ctx) = fixture(&[("src/api.test.ts", "test('x', () => {})")]);
        assert!(TestsPresent.run(&ctx).is_empty());
    }

    #[test]
    fn flags_a_repo_with_no_tests() {
        let (_dir, ctx) = fixture(&[("src/api.ts", "export const x = 1")]);
        assert_eq!(TestsPresent.run(&ctx).len(), 1);
    }
}
