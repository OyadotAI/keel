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
        let by_path = ctx
            .files()
            .any(|p| TEST_MARKERS.iter().any(|m| p.as_str().contains(m)));

        if by_path || has_inline_tests(ctx) {
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

/// Detect test suites that live inside source files rather than beside them.
///
/// Rust puts unit tests in a `#[cfg(test)]` module in the file under test, so a path-based scan
/// misses them entirely — this check reported Keel's own 41 tests as "no test suite" until it
/// learned to look inside. Reading is restricted to `.rs` files so the scan stays cheap.
fn has_inline_tests(ctx: &RepoContext) -> bool {
    ctx.files()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter_map(|p| ctx.read(p.as_str()))
        .any(|contents| contents.contains("#[cfg(test)]") || contents.contains("#[test]"))
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
    fn recognises_rust_inline_test_modules() {
        let (_dir, ctx) = fixture(&[(
            "src/lib.rs",
            "pub fn f() {}\n#[cfg(test)]\nmod tests { #[test] fn t() {} }",
        )]);
        assert!(
            TestsPresent.run(&ctx).is_empty(),
            "Rust keeps unit tests inside the file under test"
        );
    }

    #[test]
    fn flags_a_repo_with_no_tests() {
        let (_dir, ctx) = fixture(&[("src/api.ts", "export const x = 1")]);
        assert_eq!(TestsPresent.run(&ctx).len(), 1);
    }
}
