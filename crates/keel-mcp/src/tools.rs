use serde::Serialize;

/// Grouping used for display and for deciding approval policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolGroup {
    Scan,
    Repo,
    Infra,
    Deploy,
    Envs,
    Observe,
}

/// One tool exposed to the agent.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub group: ToolGroup,
    pub description: &'static str,
    /// Whether this tool changes something outside Keel's own process.
    ///
    /// Mutating tools are gated by environment: free in dev, human-approved in prod.
    pub mutating: bool,
    /// Whether the tool takes an explicit `env` argument.
    ///
    /// Every deploy-family tool does, with no default. An agent that must name the environment
    /// cannot drift into production by omission.
    pub env_scoped: bool,
}

const fn tool(
    name: &'static str,
    group: ToolGroup,
    description: &'static str,
    mutating: bool,
    env_scoped: bool,
) -> Tool {
    Tool {
        name,
        group,
        description,
        mutating,
        env_scoped,
    }
}

/// The full tool catalog.
///
/// A `static` rather than a builder so the surface is fixed at compile time — the set of things an
/// agent can do is not something that should vary at runtime.
static CATALOG: &[Tool] = {
    use ToolGroup::*;
    &[
        tool(
            "run_check",
            Scan,
            "Run one readiness check and return its findings",
            false,
            false,
        ),
        tool(
            "explain_finding",
            Scan,
            "Explain a finding and what fixing it involves",
            false,
            false,
        ),
        tool(
            "apply_fix",
            Scan,
            "Apply the fix for a finding as a reviewable diff",
            true,
            false,
        ),
        tool(
            "read_file",
            Repo,
            "Read a file from the working tree",
            false,
            false,
        ),
        tool(
            "write_file",
            Repo,
            "Write a file in the working tree",
            true,
            false,
        ),
        tool(
            "apply_patch",
            Repo,
            "Apply a unified diff to the working tree",
            true,
            false,
        ),
        tool("search", Repo, "Search the repository", false, false),
        tool(
            "run_tests",
            Repo,
            "Run the test suite and return captured output",
            false,
            false,
        ),
        tool(
            "typecheck",
            Repo,
            "Run the typechecker and return captured output",
            false,
            false,
        ),
        tool(
            "plan_cloudflare",
            Infra,
            "Produce a plan of Cloudflare resource changes",
            false,
            true,
        ),
        tool(
            "diff_bindings",
            Infra,
            "Diff bindings between the config and what is deployed",
            false,
            true,
        ),
        tool(
            "estimate_cost",
            Infra,
            "Estimate the recurring cost of a planned change",
            false,
            true,
        ),
        tool(
            "validate_policy",
            Infra,
            "Check a planned change against Keel's policy set",
            false,
            true,
        ),
        tool(
            "open_pr",
            Deploy,
            "Open a pull request for the current changes",
            true,
            false,
        ),
        tool(
            "watch_ci",
            Deploy,
            "Wait for CI checks and return their result",
            false,
            false,
        ),
        tool(
            "upload_version",
            Deploy,
            "Upload a Worker version and return its preview URL",
            true,
            true,
        ),
        tool(
            "deploy",
            Deploy,
            "Deploy a version to a named environment",
            true,
            true,
        ),
        tool(
            "promote",
            Deploy,
            "Promote the version proven in dev to prod",
            true,
            true,
        ),
        tool(
            "rollback",
            Deploy,
            "Redeploy the previous version",
            true,
            true,
        ),
        tool(
            "list_versions",
            Envs,
            "List uploaded and deployed versions",
            false,
            true,
        ),
        tool(
            "env_diff",
            Envs,
            "Diff commits and bindings between two environments",
            false,
            true,
        ),
        tool(
            "pending_migrations",
            Envs,
            "List D1 migrations not yet applied to an environment",
            false,
            true,
        ),
        tool(
            "query_traces",
            Observe,
            "Query traces for the deployed service",
            false,
            true,
        ),
        tool(
            "error_rate",
            Observe,
            "Return the current error rate",
            false,
            true,
        ),
        tool(
            "invocations",
            Observe,
            "Return invocation counts over a window",
            false,
            true,
        ),
    ]
};

pub fn catalog() -> &'static [Tool] {
    CATALOG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<_> = catalog().iter().map(|t| t.name).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate tool name in catalog");
    }

    /// The guardrail that stops an agent drifting into production by omission.
    #[test]
    fn every_deploy_tool_is_environment_scoped() {
        for t in catalog().iter().filter(|t| t.group == ToolGroup::Deploy) {
            // open_pr and watch_ci act on the repo, not on a deployed environment.
            if matches!(t.name, "open_pr" | "watch_ci") {
                continue;
            }
            assert!(t.env_scoped, "{} must take an explicit env", t.name);
        }
    }

    #[test]
    fn observability_tools_never_mutate() {
        for t in catalog().iter().filter(|t| t.group == ToolGroup::Observe) {
            assert!(!t.mutating, "{} is read-only by definition", t.name);
        }
    }
}
