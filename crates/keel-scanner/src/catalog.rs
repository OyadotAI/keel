//! What each check is called when it passes.
//!
//! A finding names a problem ("No README"); a list of every check also has to name the ones that
//! are fine, and a finding's title cannot do that. Only ids a check can report as judged
//! (`Check::evaluate`) need a line here; the ones that appear only when they fire use their
//! finding's title. `every_judged_check_has_a_label` fails if a judged id is missing.

use crate::Dimension;

/// `(id, dimension, what it says when it passes)`.
pub const PASSING: &[(&str, Dimension, &str)] = &[
    (
        "security/untrusted-agent-config",
        Dimension::Security,
        "No agent hooks or MCP config shipped with the repository",
    ),
    (
        "security/committed-secrets",
        Dimension::Security,
        "No secrets committed",
    ),
    (
        "security/no-input-validation",
        Dimension::Security,
        "Request input is validated",
    ),
    (
        "security/no-rate-limit",
        Dimension::Security,
        "Requests are rate limited",
    ),
    (
        "security/image-runs-as-root",
        Dimension::Security,
        "The image runs as a non-root user",
    ),
    (
        "verify/no-gate",
        Dimension::Verifiability,
        "One command checks the project",
    ),
    ("verify/no-tests", Dimension::Verifiability, "A test suite"),
    (
        "verify/lint-rules-unwired",
        Dimension::Verifiability,
        "The linter enforces the engineering rules",
    ),
    (
        "agent/no-instructions",
        Dimension::AgentLegibility,
        "Agent instructions (CLAUDE.md or AGENTS.md)",
    ),
    (
        "agent/thin-instructions",
        Dimension::AgentLegibility,
        "CLAUDE.md names the gate, the layout and the rules",
    ),
    (
        "agent/no-reviewers",
        Dimension::AgentLegibility,
        "The agent team and the lifecycle that runs it",
    ),
    ("docs/no-readme", Dimension::AgentLegibility, "A README"),
    (
        "docs/no-architecture",
        Dimension::AgentLegibility,
        "An architecture map",
    ),
    (
        "deploy/no-ci",
        Dimension::Deployability,
        "CI runs on every change",
    ),
    (
        "deploy/no-pipeline",
        Dimension::Deployability,
        "A pipeline deploys it",
    ),
    (
        "deploy/no-environments",
        Dimension::Deployability,
        "Dev and prod are separate environments",
    ),
    (
        "deploy/no-dockerignore",
        Dimension::Deployability,
        "A .dockerignore",
    ),
    (
        "deploy/no-env-example",
        Dimension::Deployability,
        "A .env.example",
    ),
    (
        "deploy/no-dockerfile",
        Dimension::Deployability,
        "A Dockerfile",
    ),
    (
        "deploy/no-compose",
        Dimension::Deployability,
        "A compose file for local development",
    ),
    (
        "deploy/no-manifests",
        Dimension::Deployability,
        "Deploy manifests",
    ),
    (
        "runtime/no-health-endpoint",
        Dimension::RuntimeContract,
        "A health endpoint",
    ),
    (
        "reliability/no-graceful-shutdown",
        Dimension::RuntimeContract,
        "Shuts down gracefully",
    ),
    (
        "reliability/no-migrations",
        Dimension::StatePlacement,
        "Schema migrations in the repository",
    ),
    (
        "observability/no-structured-logs",
        Dimension::Observability,
        "Structured logs",
    ),
];

/// The passing line and dimension for `id`, if it has one.
pub fn passing(id: &str) -> Option<(Dimension, &'static str)> {
    PASSING
        .iter()
        .find(|(i, ..)| *i == id)
        .map(|(_, d, l)| (*d, *l))
}

#[cfg(test)]
pub(crate) use tests::emitted;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    /// The source of every check, for asserting that an id named elsewhere is one a check emits.
    pub(crate) const SOURCES: &[&str] = &[
        include_str!("checks/agent_instructions.rs"),
        include_str!("checks/ci.rs"),
        include_str!("checks/env_hygiene.rs"),
        include_str!("checks/hosting.rs"),
        include_str!("checks/pipeline.rs"),
        include_str!("checks/practices.rs"),
        include_str!("checks/production_shape.rs"),
        include_str!("checks/secrets.rs"),
        include_str!("checks/tests_present.rs"),
        include_str!("checks/untrusted_agent_config.rs"),
        include_str!("checks/workers_compat.rs"),
    ];

    /// True when some check's source spells `id` as a string literal.
    pub(crate) fn emitted(id: &str) -> bool {
        let quoted = format!("\"{id}\"");
        SOURCES.iter().any(|s| s.contains(&quoted))
    }

    /// Every branch that judges something: a service with routes, a database and an image, with
    /// a CLAUDE.md and a package.json. A judged id with no label would be dropped from the list.
    #[test]
    fn every_judged_check_has_a_label() {
        let (_d, ctx) = fixture(&[
            (
                "package.json",
                r#"{"dependencies":{"express":"4","pg":"8"}}"#,
            ),
            ("server.js", "app.get('/api/x', h); app.listen(3000)"),
            ("Dockerfile", "FROM node:22\nCMD node server.js"),
            ("CLAUDE.md", "# x"),
        ]);
        let mut judged = Vec::new();
        for check in crate::default_checks() {
            judged.extend(check.evaluate(&ctx).1);
        }
        assert!(judged.contains(&"security/no-rate-limit"), "{judged:?}");
        assert!(judged.contains(&"deploy/no-manifests"), "{judged:?}");
        for id in judged {
            assert!(
                passing(id).is_some(),
                "{id} is judged but has no passing label"
            );
        }
    }

    #[test]
    fn every_label_names_a_real_check() {
        for (id, ..) in PASSING {
            assert!(emitted(id), "{id} is labelled but no check emits it");
        }
    }

    /// A static site is judged on what applies to it, and passing ones are listed as passing.
    #[test]
    fn a_scan_lists_what_passed_beside_what_did_not() {
        let (_d, ctx) = fixture(&[
            ("package.json", r#"{"scripts":{"build":"astro build"}}"#),
            ("README.md", "# site"),
        ]);
        let report = crate::scan(&ctx);
        let row = |id: &str| report.checks.iter().find(|c| c.id == id);
        assert!(row("docs/no-readme").is_some_and(|c| c.passed && c.label == "A README"));
        assert!(row("agent/no-reviewers").is_some_and(|c| !c.passed));
        assert!(
            row("security/no-rate-limit").is_none(),
            "a site has no routes to limit"
        );
        let failing = report.checks.iter().filter(|c| !c.passed).count();
        assert_eq!(
            failing,
            report.findings.len(),
            "every finding is a failing row"
        );
    }
}
