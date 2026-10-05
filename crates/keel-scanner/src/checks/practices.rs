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

use crate::profile::{dependencies, languages, other_dependencies, package_jsons};
use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
use camino::Utf8Path;

pub struct ProductionPractices;

/// The agent team every Keel project ships (`keel_generator::team::agents`).
const TEAM: &[&str] = &[
    "pm",
    "designer",
    "principal",
    "qa",
    "reviewer",
    "security",
    "reliability",
];
/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Present in the walk, or on disk. The walk honours `.gitignore`, and plenty of repositories
/// ignore `.claude/` wholesale — there the files adopt writes are invisible to the walk, and a
/// finding whose own fix cannot clear it is one nobody can ever leave.
fn on_disk(ctx: &RepoContext, rel: &str) -> bool {
    ctx.has(rel) || ctx.root().join(rel).is_file()
}

/// Lowercased `keel_generator::team::LIFECYCLE_HEADING`, without the `##`.
const LIFECYCLE_HEADING: &str = "how a request becomes a change";

/// Source files that look like a server: routes, handlers, an app — in any of the languages.
fn server_files(ctx: &RepoContext) -> Vec<&Utf8Path> {
    ctx.files()
        .filter(|p| {
            let s = p.as_str();
            if s.contains("node_modules")
                || s.contains("/vendor/")
                || s.contains("_test.go")
                || s.contains(".test.")
                || s.contains(".spec.")
                || s.contains("/__tests__/")
                || s.contains("/tests/")
            {
                return false;
            }
            let js = (s.ends_with(".ts") || s.ends_with(".js") || s.ends_with(".mjs"))
                && (s.contains("route.")
                    || s.contains("server")
                    || s.contains("/api/")
                    || s.contains("app.")
                    || s.contains("index."));
            let go = s.ends_with(".go")
                && (s.contains("main.go")
                    || s.contains("server")
                    || s.contains("handler")
                    || s.contains("router")
                    || s.contains("/api/")
                    || s.contains("http"));
            let py = s.ends_with(".py")
                && (s.contains("main.py")
                    || s.contains("app.py")
                    || s.contains("server")
                    || s.contains("routes")
                    || s.contains("views")
                    || s.contains("/api/"));
            let rs = s.ends_with(".rs")
                && (s.contains("main.rs")
                    || s.contains("server")
                    || s.contains("routes")
                    || s.contains("handlers"));
            js || go || py || rs
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
        self.evaluate(ctx).0
    }

    fn evaluate(&self, ctx: &RepoContext) -> (Vec<Finding>, Vec<&'static str>) {
        let langs = languages(ctx);
        if langs.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let pkgs = package_jsons(ctx);
        let mut deps = dependencies(ctx);
        deps.extend(other_dependencies(ctx));
        let go = langs.contains(&"go");
        let py = langs.contains(&"python");
        let rs = langs.contains(&"rust");
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
        let js_router = has_dep("hono")
            || has_dep("express")
            || has_dep("fastify")
            || has_dep("@nestjs/core")
            || has_dep("koa")
            || ctx
                .files()
                .any(|p| p.as_str().ends_with("route.ts") || p.as_str().ends_with("route.js"));
        let go_router = go
            && (has_dep("github.com/go-chi/chi/v5")
                || has_dep("github.com/gin-gonic/gin")
                || has_dep("github.com/labstack/echo/v4")
                || has_dep("github.com/gofiber/fiber/v2")
                || has_dep("github.com/gorilla/mux")
                || any_file_contains(
                    ctx,
                    &servers,
                    &["http.ListenAndServe", "http.Server{", "&http.Server"],
                ));
        let py_router = py
            && (has_dep("fastapi")
                || has_dep("flask")
                || has_dep("django")
                || has_dep("starlette"));
        let rs_router =
            rs && (has_dep("axum") || has_dep("actix-web") || has_dep("rocket") || has_dep("warp"));
        let has_routes = !servers.is_empty() && (js_router || go_router || py_router || rs_router);
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
            "github.com/jackc/pgx/v5",
            "github.com/lib/pq",
            "gorm.io/gorm",
            "github.com/jmoiron/sqlx",
            "go.mongodb.org/mongo-driver",
            "sqlalchemy",
            "psycopg",
            "psycopg2",
            "asyncpg",
            "django",
            "pymongo",
            "sqlx",
            "tokio-postgres",
            "diesel",
            "sea-orm",
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
        // What was judged here, pass or fail, pushed beside the condition that decides it.
        let mut checked = vec![
            "verify/no-gate",
            "agent/no-reviewers",
            "docs/no-readme",
            "docs/no-architecture",
        ];

        // ── the gate ────────────────────────────────────────────────────────────────────
        let makefile_check = ctx.read("Makefile").is_some_and(|m| {
            m.lines()
                .any(|l| l.starts_with("check:") || l.starts_with("test:") || l.starts_with("ci:"))
        });
        let has_test = has_script("test") || has_script("test:unit");
        let has_typecheck = has_script("typecheck")
            || has_script("type-check")
            || has_script("tsc")
            || has_script("lint");
        // Go, Rust and Python carry their test runner with the toolchain; a test file is the
        // evidence that anyone runs it.
        let native_tests = (go && ctx.files().any(|p| p.as_str().ends_with("_test.go")))
            || (rs && ctx.files().any(|p| p.as_str().contains("/tests/")))
            || (py
                && ctx.files().any(|p| {
                    p.file_name()
                        .is_some_and(|n| n.starts_with("test_") && n.ends_with(".py"))
                }));
        if !(makefile_check || has_test && has_typecheck || native_tests && pkgs.is_empty()) {
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

        // ── the agent team and the lifecycle that runs it ───────────────────────────────
        // The names are `keel_generator::team`'s, repeated because this crate depends on
        // nothing; `review::tests::adopting_clears_the_team_finding` fails if the two drift.
        let missing: Vec<&str> = TEAM
            .iter()
            .copied()
            .filter(|n| !on_disk(ctx, &format!(".claude/agents/{n}.md")))
            .collect();
        // Only asked of a CLAUDE.md that exists: a repository with none is `agent/no-instructions`.
        let no_lifecycle = ["CLAUDE.md", "AGENTS.md", ".claude/CLAUDE.md"]
            .iter()
            .filter_map(|f| ctx.read(f))
            .reduce(|a, b| a + &b)
            .is_some_and(|all| !all.to_lowercase().contains(LIFECYCLE_HEADING));
        // Any skill with "frontend" in its directory counts: a project that installed Anthropic's
        // `frontend-design` has made the choice this finding is about.
        let no_frontend_skill = crate::profile::has_frontend(ctx)
            && !ctx.files().any(|p| {
                p.as_str().starts_with(".claude/skills/")
                    && p.as_str().contains("frontend")
                    && p.file_name() == Some("SKILL.md")
            })
            && !on_disk(ctx, ".claude/skills/frontend/SKILL.md");
        if !missing.is_empty() || no_lifecycle || no_frontend_skill {
            let none = missing.len() == TEAM.len();
            let mut lacks = Vec::new();
            if !missing.is_empty() {
                lacks.push(format!("`{}`", missing.join("`, `")));
            }
            if no_lifecycle {
                lacks.push("the lifecycle in CLAUDE.md".to_string());
            }
            if no_frontend_skill {
                lacks.push("the frontend skill".to_string());
            }
            out.push(Finding::new(
                "agent/no-reviewers",
                Dimension::AgentLegibility,
                // A repository with some of the team has already chosen to delegate; the rest is
                // a smaller gap than one agent grading its own work.
                if none {
                    Severity::Medium
                } else {
                    Severity::Low
                },
                format!("Agent team incomplete: missing {}", and_list(&lacks)),
                "Every Keel project ships a team under `.claude/agents/` and a lifecycle in \
                 CLAUDE.md that runs it: `pm` scopes the ask, `designer` owns what people see, \
                 `principal` designs the change, `reviewer`, `security` and `reliability` read the \
                 diff, and `qa` runs the result. Without them one agent plans, builds and grades \
                 its own work.",
                Fix::Automatic {
                    description: "Add the missing agents under `.claude/agents/`, the production \
                                  checklist they enforce (`docs/PRODUCTION.md`), the lifecycle and \
                                  engineering rules in CLAUDE.md, and the `frontend` skill when \
                                  there is a frontend. Keel never overwrites a file that exists."
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
                // A desktop app, a CLI or a library reaches production by publishing a signed
                // artifact, not by rolling a deployment. Read only for the first list and every
                // one of them reports "nothing deploys this" at a repository whose release is
                // fully automated — the false positive this check exists to avoid.
                || l.contains("gh release create")
                || l.contains("gh release upload")
                || l.contains("action-gh-release")
                || l.contains("upload-release-asset")
                || l.contains("cargo publish")
                || l.contains("npm publish")
        });
        // Dev and prod apart: kustomize overlays, Helm values per env, Wrangler envs, Terraform
        // environments or per-env tfvars, or a deploy workflow per environment.
        let tf_envs = ctx.files().any(|p| {
            let s = p.as_str();
            (s.contains("terraform/")
                || s.contains("infra/")
                || s.ends_with(".tf")
                || s.ends_with(".tfvars"))
                && (s.contains("/environments/")
                    || s.contains("/envs/")
                    || s.contains("/env/")
                    || s.contains("/prod")
                    || s.contains("/production")
                    || s.contains("prod.tfvars")
                    || s.contains("production.tfvars")
                    || s.contains("/staging"))
        });
        let has_envs = has_dir("k8s/overlays/")
            || has_dir("kustomize/overlays/")
            || has_dir("helm/")
            || has_dir("charts/")
            || has_file("wrangler.jsonc")
            || has_file("wrangler.toml")
            || has_file("wrangler.json")
            || tf_envs
            || ctx.files().any(|p| {
                p.as_str().contains("environments/")
                    || p.file_name().is_some_and(|n| {
                        (n.contains("prod") || n.contains("staging"))
                            && (n.ends_with(".yml")
                                || n.ends_with(".yaml")
                                || n.ends_with(".env")
                                || n.ends_with(".tfvars"))
                    })
            });
        if has_routes || !dockerfiles.is_empty() {
            checked.extend(["deploy/no-pipeline", "deploy/no-environments"]);
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
        if !dockerfiles.is_empty() {
            checked.extend(["security/image-runs-as-root", "deploy/no-dockerignore"]);
        }
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
            checked.extend([
                "reliability/no-graceful-shutdown",
                "security/no-input-validation",
                "security/no-rate-limit",
                "observability/no-structured-logs",
            ]);
            let srv: Vec<&Utf8Path> = servers.clone();
            let drains = any_file_contains(
                ctx,
                &srv,
                &[
                    "SIGTERM",
                    "beforeExit",
                    "gracefulShutdown",
                    "onShutdown",
                    "enableShutdownHooks",
                    "signal.Notify",
                    ".Shutdown(",
                    "signal.NotifyContext",
                    "with_graceful_shutdown",
                    "tokio::signal",
                    "lifespan",
                    "on_event(\"shutdown\")",
                ],
            );
            if !drains && !has_dep("next") && !has_dep("fastapi") && !has_dep("django") {
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
                "github.com/go-playground/validator/v10",
                "github.com/go-ozzo/ozzo-validation/v4",
                "github.com/swaggo/swag",
                "pydantic",
                "fastapi",
                "django",
                "marshmallow",
                "validator",
                "garde",
                "serde_valid",
                // axum and actix deserialise bodies into typed structs: validation at the boundary.
                "serde",
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
                "golang.org/x/time",
                "github.com/go-chi/httprate",
                "github.com/ulule/limiter/v3",
                "github.com/didip/tollbooth/v7",
                "github.com/sethvargo/go-limiter",
                "slowapi",
                "django-ratelimit",
                "tower_governor",
                "governor",
            ];
            if !limiters.iter().any(|v| has_dep(v))
                && !any_file_contains(
                    ctx,
                    &srv,
                    &[
                        "RateLimit-",
                        "rateLimit",
                        "ratelimit",
                        "rate_limit",
                        "rate.NewLimiter",
                        "httprate.",
                    ],
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
                "github.com/rs/zerolog",
                "go.uber.org/zap",
                "github.com/sirupsen/logrus",
                "go.opentelemetry.io/otel",
                "github.com/getsentry/sentry-go",
                "structlog",
                "loguru",
                "python-json-logger",
                "tracing",
                "tracing-subscriber",
            ];
            let bare = any_file_contains(
                ctx,
                &srv,
                &[
                    "console.log(",
                    "log.Printf(",
                    "log.Println(",
                    "fmt.Println(",
                    "print(",
                    "println!(",
                ],
            ) && !any_file_contains(ctx, &srv, &["slog."]);
            if !loggers.iter().any(|v| has_dep(v)) && bare {
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
            checked.push("reliability/no-migrations");
            let has_migrations = has_dir("migrations/")
                || has_dir("alembic/")
                || ctx.files().any(|p| {
                    p.as_str().contains("/migrations/")
                        || p.as_str().contains("prisma/migrations")
                        || p.as_str().contains("drizzle/") && p.as_str().ends_with(".sql")
                        || p.as_str().starts_with("supabase/migrations")
                })
                || has_dep("github.com/golang-migrate/migrate/v4")
                || has_dep("github.com/pressly/goose/v3")
                || has_dep("ariga.io/atlas")
                || has_dep("alembic")
                || has_dep("sqlx-cli")
                || has_dep("refinery");
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

        // ── the instructions' substance ─────────────────────────────────────────────────
        // A CLAUDE.md that says "be careful" is not instructions. The templates' name the gate,
        // the architecture, the invariants and how to extend; a short one with none of that
        // leaves the agent guessing exactly as no file would.
        if let Some(md) = ctx
            .read("CLAUDE.md")
            .or_else(|| ctx.read("AGENTS.md"))
            .or_else(|| ctx.read(".claude/CLAUDE.md"))
        {
            checked.push("agent/thin-instructions");
            let l = md.to_lowercase();
            let names_gate = [
                "make check",
                "make test",
                "npm test",
                "bun test",
                "go test",
                "cargo test",
                "pytest",
                "typecheck",
                "## check",
                "## gate",
                "## test",
            ]
            .iter()
            .any(|k| l.contains(k));
            let has_layout = [
                "## architecture",
                "## layout",
                "## structure",
                "## key paths",
                "## files",
                "| area",
                "## where",
            ]
            .iter()
            .any(|k| l.contains(k));
            let has_rules = [
                "## rules",
                "## invariants",
                "## conventions",
                "## non-negotiable",
                "never ",
                "always ",
                "must ",
            ]
            .iter()
            .any(|k| l.contains(k));
            let lines = md.lines().filter(|l| !l.trim().is_empty()).count();
            if lines < 12 || !names_gate || !(has_layout || has_rules) {
                let mut missing = Vec::new();
                if !names_gate {
                    missing.push("the command that checks the project");
                }
                if !has_layout {
                    missing.push("where things are");
                }
                if !has_rules {
                    missing.push("the rules and invariants");
                }
                if lines < 12 {
                    missing.push("more than a few lines");
                }
                out.push(Finding::new(
                    "agent/thin-instructions",
                    Dimension::AgentLegibility,
                    Severity::Medium,
                    "The agent instructions are thin",
                    format!(
                        "CLAUDE.md exists but lacks {}. Every Keel template's CLAUDE.md names the gate, \
                         maps the files, states the invariants with the tests that guard them, and gives \
                         recipes for extending — so the agent works the way the team does.",
                        missing.join(", ")
                    ),
                    Fix::Assisted {
                        description: "Rewrite CLAUDE.md from the code: the gate, a file map, the request \
                                      path, the invariants (with the tests), how to extend, how to run. \
                                      The review does this when asked to fix the docs."
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

        // ── the map and the rules, where an agent will find them ────────────────────────
        // A CLAUDE.md with an architecture section is the same map in the file the agent
        // already reads; a second copy is a second thing to keep true.
        let mapped_in_instructions = ["CLAUDE.md", "AGENTS.md", ".claude/CLAUDE.md"]
            .iter()
            .filter_map(|f| ctx.read(f))
            .any(|md| {
                md.lines()
                    .any(|l| l.to_lowercase().starts_with("## architecture"))
            });
        if !on_disk(ctx, "ARCHITECTURE.md")
            && !on_disk(ctx, "docs/ARCHITECTURE.md")
            && !mapped_in_instructions
        {
            out.push(Finding::new(
                "docs/no-architecture",
                Dimension::AgentLegibility,
                Severity::Low,
                "No ARCHITECTURE.md",
                "The map an agent reads before it changes anything: the parts, what each one is \
                 for, how they talk, and the standards they share. Without it every session \
                 rebuilds the map from whichever files it opens first.",
                Fix::Assisted {
                    description: "Write ARCHITECTURE.md from the code: a table of the parts with \
                                  their paths and runtimes, how they talk, and the engineering \
                                  standards they share."
                        .to_string(),
                },
            ));
        }
        // The rules in CLAUDE.md are read; the linter's are enforced. Asked only of a JS/TS
        // project, because `tooling/eslint` is what Keel ships for it.
        let lint_wired = ctx.files().any(|p| {
            p.file_name()
                .is_some_and(|n| n.starts_with("eslint.config."))
                && !p.as_str().contains("node_modules")
                && ctx.read(p.as_str()).is_some_and(|c| {
                    c.contains("designRules") || c.contains("max-lines-per-function")
                })
        });
        if ctx.has("package.json") {
            checked.push("verify/lint-rules-unwired");
        }
        if ctx.has("package.json") && !lint_wired {
            out.push(Finding::new(
                "verify/lint-rules-unwired",
                Dimension::Verifiability,
                Severity::Low,
                "The engineering rules are not enforced by the linter",
                "CLAUDE.md asks for short functions, named constants and doc comments; nothing \
                 checks it, so the agent keeps them when it remembers to. The gate should fail \
                 when a rule is broken.",
                Fix::Assisted {
                    description:
                        "Import the rule sets from `tooling/eslint/index.js` (file header, \
                                  function and class length, no magic numbers, doc comments) into \
                                  the ESLint config, add `eslint-plugin-jsdoc`, and run the gate."
                            .to_string(),
                },
            ));
        }

        (out, checked)
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
            "docs/no-architecture",
            "verify/lint-rules-unwired",
        ] {
            assert!(got.contains(&want), "missing {want} in {got:?}");
        }
    }

    /// Wiring the shipped rule sets into the ESLint config, and writing the map, clear the two.
    #[test]
    fn the_map_and_wired_lint_rules_clear_their_findings() {
        let (_d, ctx) = fixture(&[
            ("package.json", r#"{"scripts":{"lint":"eslint ."}}"#),
            ("ARCHITECTURE.md", "# Architecture"),
            (
                "eslint.config.mjs",
                "import { designRules } from './tooling/eslint/index.js';",
            ),
        ]);
        let got = ids(&ctx);
        assert!(!got.contains(&"docs/no-architecture"), "{got:?}");
        assert!(!got.contains(&"verify/lint-rules-unwired"), "{got:?}");

        let (_d, go) = fixture(&[("go.mod", "module x")]);
        assert!(
            !ids(&go).contains(&"verify/lint-rules-unwired"),
            "lint rules are asked of JS/TS only"
        );
    }

    /// `files` plus what adopting writes and finishing setup wires: the whole agent team, the
    /// architecture map, and an ESLint config that imports the shipped rule sets.
    fn with_team(files: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut all = files.to_vec();
        all.push(("ARCHITECTURE.md", "# Architecture"));
        all.push((
            "eslint.config.mjs",
            "import { designRules } from './tooling/eslint/index.js';",
        ));
        for (path, _) in TEAM_FILES {
            all.push((path, ""));
        }
        all
    }

    const TEAM_FILES: [(&str, &str); 7] = [
        (".claude/agents/pm.md", ""),
        (".claude/agents/designer.md", ""),
        (".claude/agents/principal.md", ""),
        (".claude/agents/qa.md", ""),
        (".claude/agents/reviewer.md", ""),
        (".claude/agents/security.md", ""),
        (".claude/agents/reliability.md", ""),
    ];

    #[test]
    fn a_generated_project_is_silent() {
        let (_d, ctx) = fixture(&with_team(&[
            (
                "backend/package.json",
                r#"{"scripts":{"typecheck":"tsc","test":"bun test"},"dependencies":{"hono":"4","zod":"3","postgres":"3"}}"#,
            ),
            ("Makefile", "check: check-backend\n"),
            ("README.md", "# x"),
            (".dockerignore", "node_modules"),
            (".github/workflows/deploy-dev.yaml", "name: deploy"),
            ("k8s/overlays/prod/kustomization.yaml", ""),
            ("backend/Dockerfile", "FROM node\nUSER api\nCMD node"),
            ("backend/migrations/0001_init.sql", "create table x()"),
            (
                "backend/src/server.ts",
                "process.on('SIGTERM', () => {}); c.header('RateLimit-Limit', '1'); console.error(JSON.stringify({level:'info'}))",
            ),
            ("backend/src/app.ts", "app.get('/api/health')"),
        ]));
        assert!(ids(&ctx).is_empty(), "{:?}", ids(&ctx));
    }

    // Keel itself is the case: it ships a signed, notarised DMG from a tag and rolls nothing.
    #[test]
    fn publishing_a_signed_artifact_on_a_tag_is_a_pipeline() {
        let (_d, ctx) = fixture(&[
            ("Dockerfile", "FROM node\nUSER api\nCMD node"),
            (
                ".github/workflows/release.yml",
                "on:\n  push:\n    tags: ['v*']\njobs:\n  publish:\n    steps:\n \
                 - run: gh release create \"$GITHUB_REF_NAME\" dist/Keel.dmg -R o/releases\n",
            ),
        ]);
        let got = ids(&ctx);
        assert!(!got.contains(&"deploy/no-pipeline"), "{got:?}");
    }

    #[test]
    fn a_go_service_is_read_in_its_own_terms() {
        let (_d, ctx) = fixture(&[
            (
                "go.mod",
                "module x\n\nrequire (\n\tgithub.com/go-chi/chi/v5 v5.1.0\n\tgorm.io/gorm v1.25.12\n)\n",
            ),
            (
                "cmd/api/main.go",
                "package main\nfunc main() { r := chi.NewRouter(); log.Printf(\"up\"); http.ListenAndServe(\":8080\", r) }",
            ),
            ("Dockerfile", "FROM golang:1.25\nCOPY . .\nCMD [\"/app\"]"),
        ]);
        let got = ids(&ctx);
        for want in [
            "verify/no-gate",
            "reliability/no-graceful-shutdown",
            "security/no-input-validation",
            "security/no-rate-limit",
            "observability/no-structured-logs",
            "reliability/no-migrations",
            "security/image-runs-as-root",
        ] {
            assert!(got.contains(&want), "missing {want} in {got:?}");
        }
        let (_d2, ok) = fixture(&with_team(&[
            (
                "go.mod",
                "module x\n\nrequire (\n\tgithub.com/go-chi/chi/v5 v5.1.0\n\tgorm.io/gorm v1.25.12\n\tgithub.com/golang-migrate/migrate/v4 v4.18.1\n\tgithub.com/go-playground/validator/v10 v10.0.0\n\tgithub.com/go-chi/httprate v0.9.0\n\tgithub.com/rs/zerolog v1.33.0\n)\n",
            ),
            (
                "cmd/api/main.go",
                "package main\nfunc main() { signal.NotifyContext(ctx); srv.Shutdown(ctx) }",
            ),
            ("cmd/api/main_test.go", "package main"),
            ("Makefile", "test:\n\tgo test ./...\n"),
            ("README.md", "# x"),
            (
                "CLAUDE.md",
                "# x\n\n## How a request becomes a change\n\n## Gate\nmake test\n\n## Architecture\n| Area | Path |\n|---|---|\n| api | cmd/api |\n\n## Rules\n- never log secrets\n- always migrate first\n1\n2\n3\n4\n",
            ),
            (".dockerignore", ".git"),
            (".github/workflows/deploy.yml", "name: deploy"),
            ("terraform/environments/prod/main.tf", ""),
            ("Dockerfile", "FROM golang\nUSER app\nCMD [\"/app\"]"),
        ]));
        assert!(ids(&ok).is_empty(), "{:?}", ids(&ok));
    }

    #[test]
    fn a_static_site_is_left_alone() {
        let (_d, ctx) = fixture(&with_team(&[
            (
                "package.json",
                r#"{"scripts":{"build":"astro build","test":"vitest","typecheck":"astro check"},"dependencies":{"astro":"4"}}"#,
            ),
            ("README.md", "# site"),
            (".claude/skills/frontend/SKILL.md", ""),
        ]));
        assert!(ids(&ctx).is_empty(), "{:?}", ids(&ctx));
    }
}
