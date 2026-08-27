use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

/// Whether changes are verified automatically before they ship.
pub struct ContinuousIntegration;

impl Check for ContinuousIntegration {
    fn id(&self) -> &'static str {
        "deploy/no-ci"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Deployability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let has_workflow = ctx
            .matching(".github/workflows/")
            .any(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"));

        if has_workflow {
            return Vec::new();
        }

        vec![Finding::new(
            self.id(),
            self.dimension(),
            Severity::Medium,
            "No CI workflow found",
            "Keel promotes a version from dev to prod only when its checks are green. With no CI \
             there is nothing to be green, so every promotion becomes a judgement call made without \
             evidence.",
            Fix::Automatic {
                description:
                    "Generate a GitHub Actions workflow that installs, typechecks, tests, \
                              builds an SBOM and uploads a Worker version on every pull request."
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
    fn silent_when_a_workflow_exists() {
        let (_dir, ctx) = fixture(&[(".github/workflows/ci.yml", "on: push")]);
        assert!(ContinuousIntegration.run(&ctx).is_empty());
    }

    #[test]
    fn flags_a_repo_with_no_workflows() {
        let (_dir, ctx) = fixture(&[("README.md", "hi")]);
        let findings = ContinuousIntegration.run(&ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].fix.is_automatic());
    }
}
