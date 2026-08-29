//! Where it runs, and what to do about that.
//!
//! Three kinds of answer, because three kinds of hosting:
//! - a cloud the team already pays for (AWS, GCP, Azure): build around it — image registry,
//!   managed Postgres, secrets manager, a cluster if one exists — never suggest leaving;
//! - a managed platform (Vercel, Render, Railway, Fly, Heroku, Netlify): fine to start, with
//!   ceilings named honestly and the migration path — the containers stack — laid out;
//! - a backend-as-a-service (Supabase, Firebase): the question is ownership of the data and
//!   auth; the migration moves the schema into the repository and the database into
//!   something the team runs.

use crate::profile::{Hosting, detect};
use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

pub struct HostingFit;

impl Check for HostingFit {
    fn id(&self) -> &'static str {
        "infra/*"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Deployability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let p = detect(ctx);
        let mut out = Vec::new();
        let in_cluster = p.hosting.contains(&Hosting::Kubernetes);
        let containerised = p.hosting.contains(&Hosting::Containers);

        for h in &p.hosting {
            match h {
                Hosting::Vercel
                | Hosting::Netlify
                | Hosting::Render
                | Hosting::Railway
                | Hosting::Fly
                | Hosting::Heroku => {
                    if in_cluster {
                        continue;
                    }
                    let (ceiling, first_step) = match h {
                        Hosting::Vercel | Hosting::Netlify => (
                            "serverless functions: no long-lived processes (no queues, workers, WebSockets, cron beyond a ping), execution time limits, cold starts, and a bill that scales with invocations and bandwidth rather than with what you run",
                            "keep the frontend where it is if you like it; move the API and any worker into the Hono service the `fullstack` template ships, in a container you own",
                        ),
                        _ => (
                            "one process per service with the platform's scaling and pricing, no control over the network path, and a migration that gets harder with every dashboard setting that is not in the repository",
                            "put the platform's configuration into the repository as a Dockerfile, a compose file and kustomize overlays — the same image then runs on the platform today and on a cluster tomorrow",
                        ),
                    };
                    out.push(Finding::new(
                        "infra/managed-platform",
                        Dimension::Deployability,
                        Severity::Medium,
                        format!("Runs on {} — fine to start, with a ceiling", h.name()),
                        format!(
                            "{} is the right place to launch from. Its ceiling: {ceiling}. The templates' \
                             production shape (image → compose → kustomize base + dev/prod overlays) runs \
                             anywhere, so the migration is a build step, not a rewrite.",
                            h.name()
                        ),
                        Fix::Manual {
                            description: format!(
                                "When the ceiling is close: {first_step}. Keel lays the containers scaffold \
                                 over this repository and the deploy pipeline with it."
                            ),
                        },
                    ));
                }
                Hosting::Supabase | Hosting::Firebase => {
                    let (what, path) = match h {
                        Hosting::Supabase => (
                            "the schema, row-level security policies, auth users and storage buckets live in Supabase's dashboard unless `supabase/migrations` captures them",
                            "keep Postgres — Supabase is Postgres — but own it: every schema change as a migration in the repository, auth through the `auth` template's sessions when the Supabase auth model stops fitting, and a compose Postgres for dev so nobody develops against production data",
                        ),
                        _ => (
                            "Firestore's document model, security rules and Firebase Auth are the application's shape, and they do not port",
                            "model the data as tables with migrations (the `fullstack` template), move auth to sessions you own (`auth` template), keep Firebase for push or analytics if you like it; migrate collection by collection with a dual-write period",
                        ),
                    };
                    let has_migrations = ctx
                        .files()
                        .any(|f| f.as_str().starts_with("supabase/migrations/"));
                    out.push(Finding::new(
                        "infra/backend-as-a-service",
                        Dimension::StatePlacement,
                        if has_migrations { Severity::Low } else { Severity::Medium },
                        format!("Data and auth live in {}", h.name()),
                        format!(
                            "{what}. That is fast to start and hard to leave: {}. Owning the data means \
                             the repository can recreate it, and a team can run dev, test and prod as \
                             three databases instead of one.",
                            if has_migrations { "migrations are captured here, which is the hard part done" } else { "nothing in the repository can recreate the database" }
                        ),
                        Fix::Manual {
                            description: format!("Migration path: {path}."),
                        },
                    ));
                }
                Hosting::Aws | Hosting::Gcp | Hosting::Azure => {
                    let around = match h {
                        Hosting::Aws => {
                            "images to ECR, Postgres on RDS (or Aurora) reached through a private subnet, secrets in Secrets Manager rendered into the cluster by External Secrets, EKS for the kustomize overlays — or ECS if there is no cluster and nobody wants one"
                        }
                        Hosting::Gcp => {
                            "images to Artifact Registry, Cloud SQL for Postgres with the auth proxy sidecar, Secret Manager, GKE Autopilot for the kustomize overlays — or Cloud Run for a single service that can live without a cluster"
                        }
                        _ => {
                            "images to ACR, Azure Database for PostgreSQL, Key Vault through the CSI driver, AKS for the kustomize overlays"
                        }
                    };
                    out.push(Finding::new(
                        "infra/cloud-present",
                        Dimension::Deployability,
                        Severity::Info,
                        format!("Already on {} — build around it", h.name()),
                        format!(
                            "The repository uses {}. Keel does not suggest leaving a cloud a team already \
                             runs; it fits the production shape to it: {around}.",
                            h.name()
                        ),
                        Fix::Manual {
                            description: format!(
                                "Set the registry and hosts in `k8s/overlays/{{dev,prod}}` to the {} \
                                 equivalents; the workflows only need the login step changed.",
                                h.name()
                            ),
                        },
                    ));
                }
                Hosting::Cloudflare => {
                    if !containerised {
                        out.push(Finding::new(
                            "infra/cloudflare-present",
                            Dimension::Deployability,
                            Severity::Info,
                            "Runs on Cloudflare Workers",
                            "Keel's golden path. Workers cannot run long-lived processes or arbitrary \
                             binaries; when the service needs a worker loop, a WebSocket server or a \
                             container, the `stack` scaffold beside it is the answer, with a service \
                             binding between the two.",
                            Fix::Manual {
                                description: "Nothing to do now. Watch D1's single-writer limit (~50 writes/s) \
                                              and KV's eventual consistency; Hyperdrive to Postgres past that."
                                    .to_string(),
                            },
                        ));
                    }
                }
                Hosting::Kubernetes | Hosting::Containers => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn vercel_plus_supabase_gets_the_ceiling_and_the_ownership_note() {
        let (_d, ctx) = fixture(&[(
            "package.json",
            r#"{"dependencies":{"next":"16","@supabase/supabase-js":"2","@vercel/otel":"1"}}"#,
        )]);
        let mut ids: Vec<&str> = HostingFit.run(&ctx).iter().map(|f| f.id).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["infra/backend-as-a-service", "infra/managed-platform"]
        );
    }

    #[test]
    fn a_cloud_is_built_around_not_left() {
        let (_d, ctx) = fixture(&[(
            "package.json",
            r#"{"dependencies":{"@aws-sdk/client-s3":"3","hono":"4"}}"#,
        )]);
        let f = HostingFit.run(&ctx);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "infra/cloud-present");
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].detail.contains("ECR"));
    }

    #[test]
    fn a_platform_already_moving_to_a_cluster_is_not_nagged() {
        let (_d, ctx) = fixture(&[
            ("package.json", r#"{"dependencies":{"next":"16"}}"#),
            ("vercel.json", "{}"),
            ("k8s/base/kustomization.yaml", ""),
        ]);
        assert!(HostingFit.run(&ctx).is_empty());
    }
}
