//! The production rules, as checks.
//!
//! Every generated project ships with a checklist (`docs/PRODUCTION.md`): a gate, reviewers,
//! separate environments, a deploy pipeline, an image that does not run as root, graceful
//! shutdown, migrations, validation at the boundary, rate limits, structured logs. An imported
//! project — a working app, vibe-coded or not — usually has none of it, and the old report
//! scored it 92. These checks are that checklist read against the repository, so the report
//! and the template agree on what "production" means.
//!
//! All heuristics: they read files and dependency names, never the network. Each one only
//! fires when the evidence is there (a service with routes, a Dockerfile, a database
//! dependency), so a docs site or a Rust CLI is left alone.

use crate::profile::{dependencies, package_jsons};
use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
use camino::Utf8Path;

pub struct ProductionPractices;

/// Source files that look like a server: routes, handlers, an app.
fn server_files(ctx: &RepoContext) -> Vec<&Utf8Path> {
    ctx.files()
        .filter(|p| {
            let s = p.as_str();
            !s.contains("node_modules")
                && (s.ends_with(".ts") || s.ends_with(".js") || s.ends_with(".mjs"))
                && !s.contains(".test.")
                && !s.contains(".spec.")
                && !s.contains("/__tests__/")
                && (s.contains("route.")
                    || s.contains("server")
                    || s.contains("/api/")
                    || s.contains("app.")
                    || s.contains("index."))
        })
        .collect()
}

fn any_file_contains(ctx: &RepoContext, files: &[&Utf8Path], needles: &[&str]) -> bool {
    files.iter().any(|p| {
        ctx.read(p.as_str())
            .is_some_and(|s| needles.iter().any(|n| s.contains(n)))
    })
}

impl Check for ProductionPractices {
    fn id(&self) -> &'static str {
        "practice/*"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Deployability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let pkgs = package_jsons(ctx);
        if pkgs.is_empty() {
            return Vec::new();
        }
        let deps = dependencies(ctx);
        let has_dep = |n: &str| deps.contains(n);
        let has_file = |n: &str| {
            ctx.files()
                .any(|p| p.file_name() == Some(n) && !p.as_str().contains("node_modules"))
        };
        let has_dir = |n: &str| ctx.files().any(|p| p.as_str().starts_with(n));
        let scripts: Vec<String> = pkgs
            .iter()
            .filter_map(|p| p.get("scripts").and_then(|s| s.as_object()))
            .flat_map(|m| m.keys().cloned())
            .collect();
        let has_script = |n: &str| scripts.iter().any(|s| s == n);
        let servers = server_files(ctx);
        let has_routes = !servers.is_empty()
            && (has_dep("hono")
                || has_dep("express")
                || has_dep("fastify")
                || has_dep("@nestjs/core")
                || has_dep("koa")
                || ctx
                    .files()
                    .any(|p| p.as_str().ends_with("route.ts") || p.as_str().ends_with("route.js")));
        let has_db = [
            "postgres",
            "pg",
            "@prisma/client",
            "prisma",
            "drizzle-orm",
            "mongoose",
            "mongodb",
            "mysql2",
            "better-sqlite3",
            "@supabase/supabase-js",
        ]
        .iter()
        .any(|d| has_dep(d));
        let dockerfiles: Vec<&Utf8Path> = ctx
            .files()
            .filter(|p| {
                p.file_name().is_some_and(|n| n.starts_with("Dockerfile"))
                    && !p.as_str().contains("node_modules")
            })
            .collect();
        let mut out = Vec::new();

        // ── the gate ────────────────────────────────────────────────────────────────────
        let makefile_check = ctx
            .read("Makefile")
            .is_some_and(|m| m.lines().any(|l| l.starts_with("check:")));
        let has_test = has_script("test") || has_script("test:unit");
        let has_typecheck = has_script("typecheck")
            || has_script("type-check")
            || has_script("tsc")
            || has_script("lint");
        if !(makefile_check || has_test && has_typecheck) {
            out.push(Finding::new(
                "verify/no-gate",
                Dimension::Verifiability,
                Severity::High,
                "No single command that says whether the project is good",
                "A gate is one command — typecheck, lint, tests — that Keel runs after every turn and \
                 CI runs before every build. Without it \"done\" is an opinion, and an agent's \
                 opinion of its own work is the least reliable one available.",
                Fix::Automatic {
                    description: "Add `make check` running the typecheck and the tests, and point \
                                  Keel's gate at it. The templates ship this Makefile."
                        .to_string(),
                },
            ));
        }

        // ── reviewers ───────────────────────────────────────────────────────────────────
        let agents_dir = ctx
            .files()
            .filter(|p| p.as_str().starts_with(".claude/agents/"))
            .count();
        if agents_dir == 0 {
            out.push(Finding::new(
                "agent/no-reviewers",
                Dimension::AgentLegibility,
                Severity::Medium,
                "No reviewer subagents",
                "Every Keel template ships three reviewers under `.claude/agents/`: a code reviewer, \
                 a security reviewer that reads a change as an attacker, and a reliability reviewer \
                 that reads it as the on-call engineer. A repository without them relies on one \
                 agent grading its own work.",
                Fix::Automatic {
                    description: "Add `.claude/agents/{reviewer,security,reliability}.md` and the \
                                  production checklist they enforce (`docs/PRODUCTION.md`). Keel \
                                  writes these without touching anything else."
                        .to_string(),
                },
            ));
        }

        // ── environments and a deploy pipeline ──────────────────────────────────────────
        let workflows: Vec<String> = ctx
            .files()
            .filter(|p| p.as_str().starts_with(".github/workflows/"))
            .filter_map(|p| ctx.read(p.as_str()))
            .collect();
        let deploys = workflows.iter().any(|w| {
            let l = w.to_lowercase();
            l.contains("deploy")
                || l.contains("docker/build-push")
                || l.contains("kubectl")
                || l.contains("wrangler")
                || l.contains("vercel")
        });
        let has_envs = has_dir("k8s/overlays/")
            || has_dir("kustomize/overlays/")
            || has_dir("helm/")
            || has_file("wrangler.jsonc")
            || has_file("wrangler.toml")
            || has_file("wrangler.json")
            || ctx.files().any(|p| {
                p.as_str().contains("environments/")
                    || p.file_name().is_some_and(|n| {
                        n.contains("prod") && n.ends_with(".yml")
                            || n.contains("prod") && n.ends_with(".yaml")
                    })
            });
        if has_routes || !dockerfiles.is_empty() {
            if !deploys {
                out.push(Finding::new(
                    "deploy/no-pipeline",
                    Dimension::Deployability,
                    Severity::Medium,
                    "Nothing deploys this automatically",
                    "No workflow builds an image or deploys. Whatever gets to production gets there \
                     by hand, which means differently each time and by one person.",
                    Fix::Assisted {
                        description: "Add `.github/workflows/deploy-dev.yaml` (on push to main) and \
                                      `deploy-prod.yaml` (on tag): test, build the image to ghcr, \
                                      apply the overlay, roll only what changed. The templates ship both."
                            .to_string(),
                    },
                ));
            }
            if !has_envs {
                out.push(Finding::new(
                    "deploy/no-environments",
                    Dimension::EnvironmentHygiene,
                    Severity::Medium,
                    "Dev and prod are not separate anywhere",
                    "There is one shape of deployment, so every change is tried for the first time \
                     in production. The templates keep a `base` and `dev`/`prod` overlays that differ \
                     only in replicas, hosts and secrets — and never share a stateful binding.",
                    Fix::Assisted {
                        description: "Add kustomize overlays (`k8s/overlays/dev`, `k8s/overlays/prod`) \
                                      or the platform's equivalent, each with its own database, secrets \
                                      and hostname."
                            .to_string(),
                    },
                ));
            }
        }

        // ── the image ───────────────────────────────────────────────────────────────────
        for df in &dockerfiles {
            if let Some(text) = ctx.read(df.as_str()) {
                let runs_as_user = text
                    .lines()
                    .any(|l| l.trim_start().starts_with("USER ") && !l.contains("root"));
                if !runs_as_user {
                    out.push(
                        Finding::new(
                            "security/image-runs-as-root",
                            Dimension::Security,
                            Severity::Medium,
                            "The container runs as root",
                            "No `USER` instruction, so every process in the container is root. A \
                             dependency with a bug becomes root on the node. The templates create a \
                             system user and switch to it before `CMD`.",
                            Fix::Automatic {
                                description: "Add a non-root user (`addgroup --system nodejs && adduser \
                                              --system app`) and `USER app` before the final `CMD`; make \
                                              the working directory owned by it."
                                    .to_string(),
                            },
                        )
                        .at((*df).to_owned()),
                    );
                }
            }
            if !has_file(".dockerignore") {
                out.push(Finding::new(
                    "deploy/no-dockerignore",
                    Dimension::Deployability,
                    Severity::Low,
                    "No .dockerignore",
                    "Without one, `node_modules`, `.git` and `.env` files go into the build context — \
                     slow builds, and a secret one `COPY . .` away from the image.",
                    Fix::Automatic {
                        description: "Add `.dockerignore` with node_modules, .git, .env*, coverage, .next/cache."
                            .to_string(),
                    },
                ));
                break;
            }
        }

        // ── the server's behaviour ──────────────────────────────────────────────────────
        if has_routes {
            let srv: Vec<&Utf8Path> = servers.clone();
            if !any_file_contains(
                ctx,
                &srv,
                &[
                    "SIGTERM",
                    "beforeExit",
                    "gracefulShutdown",
                    "onShutdown",
                    "enableShutdownHooks",
                ],
            ) && !has_dep("next")
            {
                out.push(Finding::new(
                    "reliability/no-graceful-shutdown",
                    Dimension::RuntimeContract,
                    Severity::Medium,
                    "The server does not drain on SIGTERM",
                    "A rollout sends SIGTERM and waits; a process that exits at once drops the \
                     requests it was serving, and one that ignores it is killed after the grace \
                     period. Every deploy becomes a handful of 502s.",
                    Fix::Assisted {
                        description: "On SIGTERM stop accepting, finish in-flight requests, close \
                                      the pool, then exit — and give the Deployment a `preStop` \
                                      sleep so endpoints are removed first. `server.ts` in the \
                                      templates is the shape."
                            .to_string(),
                    },
                ));
            }
            let validators = [
                "zod",
                "valibot",
                "yup",
                "joi",
                "ajv",
                "class-validator",
                "@sinclair/typebox",
                "arktype",
                "superstruct",
            ];
            if !validators.iter().any(|v| has_dep(v)) {
                out.push(Finding::new(
                    "security/no-input-validation",
                    Dimension::Security,
                    Severity::High,
                    "Request bodies are not validated with a schema",
                    "Routes exist and no schema library does. Inputs go straight from JSON into \
                     queries and logic, which is where injection, mass assignment and 500s on bad \
                     input come from. Validate once at the boundary, reject unknown fields, bound \
                     sizes.",
                    Fix::Assisted {
                        description:
                            "Add zod; parse every body and query with `.strict()` schemas \
                                      shared with the frontend; return 400 with the issues."
                                .to_string(),
                    },
                ));
            }
            let limiters = [
                "rate-limiter-flexible",
                "express-rate-limit",
                "@fastify/rate-limit",
                "hono-rate-limiter",
                "@upstash/ratelimit",
                "@nestjs/throttler",
                "bottleneck",
            ];
            if !limiters.iter().any(|v| has_dep(v))
                && !any_file_contains(
                    ctx,
                    &srv,
                    &["RateLimit-", "rateLimit", "ratelimit", "rate_limit"],
                )
            {
                out.push(Finding::new(
                    "security/no-rate-limit",
                    Dimension::Security,
                    Severity::Medium,
                    "No rate limiting on the API",
                    "One caller in a loop is the whole capacity. Limits per principal — token bucket, \
                     `429` with `RateLimit-*` headers — are what keeps one bad client from being an \
                     outage, and auth endpoints need a lockout on top.",
                    Fix::Assisted {
                        description: "Add a per-key or per-IP limit in middleware (in-process to \
                                      start, Redis once there are replicas) answering RateLimit-* \
                                      and Retry-After; the `api` template has it."
                            .to_string(),
                    },
                ));
            }
            let loggers = [
                "pino",
                "winston",
                "bunyan",
                "@opentelemetry/api",
                "@vercel/otel",
                "@sentry/node",
                "@sentry/nextjs",
            ];
            if !loggers.iter().any(|v| has_dep(v))
                && any_file_contains(ctx, &srv, &["console.log("])
            {
                out.push(Finding::new(
                    "observability/no-structured-logs",
                    Dimension::Observability,
                    Severity::Low,
                    "Logs are console.log strings",
                    "A log line nobody can query is a log line nobody reads at 3am. One JSON object \
                     per line with `level`, `msg`, `request_id` is the difference between grep and \
                     a dashboard.",
                    Fix::Assisted {
                        description: "Log one JSON object per line (`{\"level\",\"msg\",\"request_id\"}`), \
                                      add a request id middleware, and send errors somewhere that pages."
                            .to_string(),
                    },
                ));
            }
        }

        // ── the database ────────────────────────────────────────────────────────────────
        if has_db {
            let has_migrations = has_dir("migrations/")
                || ctx.files().any(|p| {
                    p.as_str().contains("/migrations/")
                        || p.as_str().contains("prisma/migrations")
                        || p.as_str().contains("drizzle/") && p.as_str().ends_with(".sql")
                        || p.as_str().starts_with("supabase/migrations")
                });
            if !has_migrations {
                out.push(Finding::new(
                    "reliability/no-migrations",
                    Dimension::StatePlacement,
                    Severity::High,
                    "A database, but no migrations",
                    "The schema lives in someone's head or in a dashboard. Nothing can recreate it, \
                     so there is no dev environment that matches prod and no rollback. Schema \
                     changes as SQL files, applied by an init container before each rollout, are \
                     the fix — expand/contract, never a rename in place.",
                    Fix::Assisted {
                        description: "Add `migrations/0001_init.sql` from the current schema (`pg_dump \
                                      --schema-only`), a `migrate` script that applies unapplied files \
                                      in a transaction, and run it before the server starts."
                            .to_string(),
                    },
                ));
            }
        }

        // ── the front door ──────────────────────────────────────────────────────────────
        if !has_file("README.md") {
            out.push(Finding::new(
                "docs/no-readme",
                Dimension::AgentLegibility,
                Severity::Low,
                "No README",
                "The first thing a new engineer, an auditor or an agent opens. Five minutes to \
                 running, how to check it, how it ships.",
                Fix::Automatic {
                    description:
                        "Write README.md: what it is, `make demo`, `make check`, how to deploy."
                            .to_string(),
                },
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    fn ids(ctx: &RepoContext) -> Vec<&'static str> {
        ProductionPractices
            .run(ctx)
            .into_iter()
            .map(|f| f.id)
            .collect()
    }

    #[test]
    fn a_vibe_coded_express_app_gets_the_whole_checklist() {
        let (_d, ctx) = fixture(&[
            (
                "package.json",
                r#"{"scripts":{"start":"node server.js"},"dependencies":{"express":"4","pg":"8"}}"#,
            ),
            (
                "server.js",
                "const app = require('express')(); app.get('/api/x', (req,res)=>{ console.log('hi'); res.json(db.query(req.body)) }); app.listen(3000)",
            ),
            ("Dockerfile", "FROM node:22\nCOPY . .\nCMD node server.js"),
        ]);
        let got = ids(&ctx);
        for want in [
            "verify/no-gate",
            "agent/no-reviewers",
            "deploy/no-pipeline",
            "deploy/no-environments",
            "security/image-runs-as-root",
            "deploy/no-dockerignore",
            "reliability/no-graceful-shutdown",
            "security/no-input-validation",
            "security/no-rate-limit",
            "observability/no-structured-logs",
            "reliability/no-migrations",
            "docs/no-readme",
        ] {
            assert!(got.contains(&want), "missing {want} in {got:?}");
        }
    }

    #[test]
    fn a_generated_project_is_silent() {
        let (_d, ctx) = fixture(&[
            (
                "backend/package.json",
                r#"{"scripts":{"typecheck":"tsc","test":"bun test"},"dependencies":{"hono":"4","zod":"3","postgres":"3"}}"#,
            ),
            ("Makefile", "check: check-backend\n"),
            ("README.md", "# x"),
            (".dockerignore", "node_modules"),
            (".claude/agents/reviewer.md", "---\nname: reviewer\n---"),
            (".github/workflows/deploy-dev.yaml", "name: deploy"),
            ("k8s/overlays/prod/kustomization.yaml", ""),
            ("backend/Dockerfile", "FROM node\nUSER api\nCMD node"),
            ("backend/migrations/0001_init.sql", "create table x()"),
            (
                "backend/src/server.ts",
                "process.on('SIGTERM', () => {}); c.header('RateLimit-Limit', '1'); console.error(JSON.stringify({level:'info'}))",
            ),
            ("backend/src/app.ts", "app.get('/api/health')"),
        ]);
        assert!(ids(&ctx).is_empty(), "{:?}", ids(&ctx));
    }

    #[test]
    fn a_static_site_is_left_alone() {
        let (_d, ctx) = fixture(&[
            (
                "package.json",
                r#"{"scripts":{"build":"astro build","test":"vitest","typecheck":"astro check"},"dependencies":{"astro":"4"}}"#,
            ),
            ("README.md", "# site"),
            (".claude/agents/reviewer.md", ""),
        ]);
        assert!(ids(&ctx).is_empty(), "{:?}", ids(&ctx));
    }
}
