use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

/// Whether a service project has the files that make it deployable the same way twice.
///
/// An imported project is usually a working application with the production shape missing:
/// no `.env.example` so nobody knows what to set, no Dockerfile so it runs only where it was
/// written, no compose so a second person cannot start it, no manifests so "deploy" is a
/// person's memory, no health route so a balancer cannot tell alive from dead. Each is one
/// finding with the fix, and each is what Keel's own scaffold ships — so the fix is "make it
/// look like the scaffold", which the agent can do from the pattern.
///
/// Scoped to TypeScript/JavaScript services (a `package.json`), and skipped for a project on
/// Cloudflare Workers: a repository with a wrangler config is never asked about containers.
pub struct ProductionShape;

impl Check for ProductionShape {
    fn id(&self) -> &'static str {
        "deploy/production-shape"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Deployability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let is_service = ctx.has_any([
            "package.json",
            "frontend/package.json",
            "backend/package.json",
        ]);
        let on_workers = ctx
            .files()
            .any(|p| p.file_name().is_some_and(|f| f.starts_with("wrangler.")));
        if !is_service || on_workers {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut want = |id: &'static str, sev: Severity, title: &str, detail: &str, fix: &str| {
            out.push(Finding::new(
                id,
                Dimension::Deployability,
                sev,
                title,
                detail,
                Fix::Assisted {
                    description: fix.to_string(),
                },
            ));
        };

        if !ctx.has_any([".env.example", "backend/.env.example", ".env.sample"]) {
            want(
                "deploy/no-env-example",
                Severity::Medium,
                "No .env.example",
                "Nothing says which environment variables the project needs, so the first person to \
                 deploy it discovers them one crash at a time.",
                "Add a .env.example listing every variable the code reads, with a comment each and no \
                 values — and read config only from the environment.",
            );
        }
        let has_dockerfile = ctx.files().any(|p| p.file_name() == Some("Dockerfile"));
        if !has_dockerfile {
            want(
                "deploy/no-dockerfile",
                Severity::Medium,
                "No Dockerfile",
                "The project runs where it was written and nowhere else. An image is the unit every \
                 deploy target accepts.",
                "Add a multi-stage Dockerfile per service: install and build with the lockfile, run \
                 on a slim image as a non-root user, expose the port, no secrets baked in.",
            );
        }
        if !ctx.has_any([
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
        ]) {
            want(
                "deploy/no-compose",
                Severity::Low,
                "No docker-compose",
                "A second person cannot start the stack — database, cache, the services — with one \
                 command, so onboarding is a document nobody keeps current.",
                "Add a docker-compose.yml with the datastores for development and an `app` profile \
                 that runs the services behind a reverse proxy.",
            );
        }
        let has_manifests = ctx.matching("k8s/").next().is_some()
            || ctx.matching("kustomization.yaml").next().is_some()
            || ctx.matching("helm/").next().is_some()
            || ctx.matching("deploy/").next().is_some();
        if has_dockerfile && !has_manifests {
            want(
                "deploy/no-manifests",
                Severity::Low,
                "An image but nothing that runs it",
                "There is a Dockerfile and no manifests, so how it is deployed — replicas, probes, \
                 rollout, secrets — lives in somebody's memory.",
                "Add kustomize manifests: a base with Deployment, Service, HPA and PDB per service \
                 (rolling update with maxUnavailable 0, preStop drain, startup/readiness/liveness \
                 probes) and dev/prod overlays.",
            );
        }
        let mentions_health = ctx
            .files()
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| matches!(e, "ts" | "js" | "tsx" | "mjs"))
            })
            .filter(|p| !p.as_str().contains("node_modules"))
            .take(400)
            .any(|p| {
                ctx.read(p.as_str()).is_some_and(|s| {
                    s.contains("/health") || s.contains("/healthz") || s.contains("/readyz")
                })
            });
        if !mentions_health {
            want(
                "runtime/no-health-endpoint",
                Severity::Medium,
                "No health endpoint",
                "A load balancer or a cluster cannot tell this service is alive or ready, so it \
                 routes to it while it boots and keeps routing to it when it hangs.",
                "Add `/api/health` (alive, cheap) and `/api/health/ready` (checks the database) and \
                 point the probes at them.",
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn a_workers_project_is_never_asked_about_containers() {
        let (_d, ctx) = fixture(&[("package.json", "{}"), ("wrangler.jsonc", "{}")]);
        assert!(ProductionShape.run(&ctx).is_empty());
    }

    #[test]
    fn a_bare_node_service_gets_the_list() {
        let (_d, ctx) = fixture(&[("package.json", "{}"), ("src/index.ts", "export {}")]);
        let ids: Vec<&str> = ProductionShape.run(&ctx).iter().map(|f| f.id).collect();
        assert!(ids.contains(&"deploy/no-env-example"));
        assert!(ids.contains(&"deploy/no-dockerfile"));
        assert!(ids.contains(&"deploy/no-compose"));
        assert!(ids.contains(&"runtime/no-health-endpoint"));
        assert!(
            !ids.contains(&"deploy/no-manifests"),
            "no image, so manifests are not the next step"
        );
    }

    #[test]
    fn the_scaffold_shape_is_silent() {
        let (_d, ctx) = fixture(&[
            ("backend/package.json", "{}"),
            ("backend/.env.example", ""),
            ("backend/Dockerfile", "FROM x"),
            ("docker-compose.yml", ""),
            ("k8s/base/kustomization.yaml", ""),
            ("backend/src/app.ts", "app.get('/api/health/ready')"),
        ]);
        assert!(ProductionShape.run(&ctx).is_empty());
    }

    #[test]
    fn not_a_service_at_all_is_left_alone() {
        let (_d, ctx) = fixture(&[("Cargo.toml", ""), ("src/main.rs", "")]);
        assert!(ProductionShape.run(&ctx).is_empty());
    }
}
