//! What kind of project this is, and where it runs.
//!
//! A report that says "add kustomize manifests" to a Vercel-hosted Next.js app and to a Hono
//! service already in a cluster is the same report to both, and useful to neither. So the scan
//! first reads the shape of the repository — its frameworks and dependencies, its hosting — and
//! the checks and the plan speak to that shape. The closest Keel template is named so the
//! person can see what "the finished version of this" looks like, and the agent can read that
//! template's practices as the target.
//!
//! Detection is deterministic and offline: `package.json` dependencies and well-known files.

use crate::RepoContext;
use serde::Serialize;
use std::collections::BTreeSet;

/// Where the project runs today, as far as the repository says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Hosting {
    Vercel,
    Netlify,
    Supabase,
    Firebase,
    Render,
    Railway,
    Fly,
    Heroku,
    Cloudflare,
    Aws,
    Gcp,
    Azure,
    Kubernetes,
    Containers,
}

impl Hosting {
    pub fn name(self) -> &'static str {
        match self {
            Hosting::Vercel => "Vercel",
            Hosting::Netlify => "Netlify",
            Hosting::Supabase => "Supabase",
            Hosting::Firebase => "Firebase",
            Hosting::Render => "Render",
            Hosting::Railway => "Railway",
            Hosting::Fly => "Fly.io",
            Hosting::Heroku => "Heroku",
            Hosting::Cloudflare => "Cloudflare",
            Hosting::Aws => "AWS",
            Hosting::Gcp => "Google Cloud",
            Hosting::Azure => "Azure",
            Hosting::Kubernetes => "Kubernetes",
            Hosting::Containers => "containers",
        }
    }

    /// A managed platform that owns the runtime: fine to start on, with ceilings a growing
    /// service hits (cold starts, execution limits, egress, no long-lived processes, pricing
    /// that scales with the wrong number). The migration path is the containers stack.
    pub fn is_managed_platform(self) -> bool {
        matches!(
            self,
            Hosting::Vercel
                | Hosting::Netlify
                | Hosting::Render
                | Hosting::Railway
                | Hosting::Fly
                | Hosting::Heroku
        )
    }

    /// A backend-as-a-service that owns the data. The migration is about ownership: the
    /// schema, auth and storage move into the repository and a database the team runs.
    pub fn is_baas(self) -> bool {
        matches!(self, Hosting::Supabase | Hosting::Firebase)
    }

    /// A cloud the team already pays for. Keel builds around it rather than suggesting a move.
    pub fn is_cloud(self) -> bool {
        matches!(self, Hosting::Aws | Hosting::Gcp | Hosting::Azure)
    }
}

/// The repository's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// The closest Keel template id (`api`, `agent`, `fullstack`, …), or `blank` when nothing
    /// in the repository points anywhere.
    pub template: &'static str,
    pub template_title: &'static str,
    /// The open-source project that template is modelled on — the comparison the person can
    /// go and read.
    pub like: &'static str,
    /// 0–100. How much of the evidence points at that template rather than another.
    pub confidence: u32,
    /// What was seen: dependency and file names, so the guess can be checked.
    pub signals: Vec<String>,
    pub hosting: Vec<Hosting>,
    /// Frameworks and libraries that shape the advice: `next`, `hono`, `postgres`, `redis`…
    pub stack: Vec<&'static str>,
    /// True when `package.json` exists somewhere: a JavaScript/TypeScript service or app.
    pub is_js: bool,
}

/// A dependency name and the weight it lends to each template.
const DEP_SIGNALS: &[(&str, &[(&str, u32)])] = &[
    ("next", &[("fullstack", 3), ("internal", 1)]),
    ("react", &[("fullstack", 1)]),
    ("hono", &[("api", 2), ("fullstack", 1)]),
    ("express", &[("api", 2)]),
    ("fastify", &[("api", 2)]),
    ("@nestjs/core", &[("api", 3)]),
    ("koa", &[("api", 2)]),
    ("zod", &[("api", 1)]),
    ("swagger-ui-express", &[("api", 2)]),
    ("@hono/zod-openapi", &[("api", 3)]),
    ("bullmq", &[("jobs", 4)]),
    ("bull", &[("jobs", 3)]),
    ("inngest", &[("jobs", 4)]),
    ("@temporalio/worker", &[("dag", 3), ("jobs", 2)]),
    ("agenda", &[("jobs", 2)]),
    ("node-cron", &[("jobs", 2)]),
    ("ws", &[("realtime", 3)]),
    ("socket.io", &[("realtime", 4)]),
    ("@hono/node-ws", &[("realtime", 3)]),
    ("pusher", &[("realtime", 3)]),
    ("ably", &[("realtime", 3)]),
    ("openai", &[("agent", 3), ("llmproxy", 1)]),
    ("@anthropic-ai/sdk", &[("agent", 3)]),
    ("ai", &[("agent", 2)]),
    ("@ai-sdk/openai", &[("agent", 2)]),
    ("langchain", &[("agent", 2), ("graph", 1)]),
    ("@langchain/langgraph", &[("graph", 5)]),
    ("@langchain/core", &[("agent", 1)]),
    ("crewai", &[("orchestrator", 4)]),
    ("@mastra/core", &[("agent", 2), ("graph", 2)]),
    (
        "@modelcontextprotocol/sdk",
        &[("mcpgateway", 4), ("agent", 1)],
    ),
    ("litellm", &[("llmproxy", 4)]),
    ("portkey-ai", &[("aigateway", 4)]),
    ("promptfoo", &[("evals", 4)]),
    ("braintrust", &[("evals", 3)]),
    ("unleash-client", &[("flags", 4)]),
    ("@openfeature/server-sdk", &[("flags", 4)]),
    ("launchdarkly-node-server-sdk", &[("flags", 3)]),
    ("@launchdarkly/node-server-sdk", &[("flags", 3)]),
    ("stripe", &[("tenant", 3)]),
    ("better-auth", &[("auth", 4)]),
    ("lucia", &[("auth", 4)]),
    ("next-auth", &[("auth", 3)]),
    ("@auth/core", &[("auth", 3)]),
    ("passport", &[("auth", 3)]),
    ("jose", &[("auth", 1)]),
    ("dockerode", &[("sandbox", 4)]),
    ("isolated-vm", &[("sandbox", 3)]),
    ("@e2b/code-interpreter", &[("sandbox", 4)]),
    ("kafkajs", &[("pipeline", 3)]),
    ("@clickhouse/client", &[("pipeline", 3)]),
    ("duckdb", &[("pipeline", 3)]),
    ("@kubernetes/client-node", &[("controlplane", 4)]),
    ("http-proxy", &[("gateway", 3)]),
    ("http-proxy-middleware", &[("gateway", 3)]),
    ("@tanstack/react-table", &[("internal", 2)]),
    ("ag-grid-react", &[("internal", 2)]),
    ("react-admin", &[("internal", 3)]),
    ("@refinedev/core", &[("internal", 3)]),
    ("prisma", &[("fullstack", 1)]),
    ("@prisma/client", &[("fullstack", 1)]),
    ("drizzle-orm", &[("fullstack", 1)]),
];

/// Dependencies whose presence changes the advice, whatever the template.
const STACK_DEPS: &[(&str, &str)] = &[
    ("next", "next"),
    ("hono", "hono"),
    ("express", "express"),
    ("fastify", "fastify"),
    ("@nestjs/core", "nest"),
    ("postgres", "postgres"),
    ("pg", "postgres"),
    ("@prisma/client", "prisma"),
    ("prisma", "prisma"),
    ("drizzle-orm", "drizzle"),
    ("mongoose", "mongodb"),
    ("mongodb", "mongodb"),
    ("ioredis", "redis"),
    ("redis", "redis"),
    ("@upstash/redis", "redis"),
    ("zod", "zod"),
    ("pino", "pino"),
    ("winston", "winston"),
    ("@opentelemetry/api", "otel"),
    ("@sentry/node", "sentry"),
    ("@sentry/nextjs", "sentry"),
    ("stripe", "stripe"),
    ("openai", "openai"),
    ("@anthropic-ai/sdk", "anthropic"),
    ("@supabase/supabase-js", "supabase"),
    ("firebase", "firebase"),
    ("firebase-admin", "firebase"),
    ("@aws-sdk/client-s3", "aws-sdk"),
    ("aws-sdk", "aws-sdk"),
    ("@google-cloud/storage", "gcp-sdk"),
    ("@google-cloud/pubsub", "gcp-sdk"),
    ("@azure/storage-blob", "azure-sdk"),
    ("vitest", "vitest"),
    ("jest", "jest"),
    ("typescript", "typescript"),
];

pub(crate) const TEMPLATES: &[(&str, &str, &str)] = &[
    ("api", "REST API service", "PostgREST, Directus"),
    ("jobs", "Webhooks and background jobs", "Inngest, BullMQ"),
    ("pipeline", "Data pipeline", "Airbyte, dbt"),
    ("realtime", "Realtime service", "Centrifugo"),
    ("fullstack", "Full-stack app", "Next.js + Hono"),
    ("auth", "Accounts and sessions", "better-auth, Lucia"),
    ("tenant", "Multi-tenant SaaS", "Cal.com"),
    ("internal", "Internal tool", "Appsmith, Retool"),
    ("agent", "AI agent backend", "Open WebUI"),
    ("orchestrator", "Multi-agent orchestrator", "CrewAI"),
    ("evals", "Evaluation harness", "promptfoo"),
    ("loop", "Loop agent", "smolagents"),
    ("graph", "Graph agent", "LangGraph"),
    ("dag", "DAG runtime", "Airflow, Dagster"),
    ("controlplane", "Control plane", "Kubernetes controllers"),
    ("dataplane", "Control plane + data plane", "Envoy xDS"),
    ("flags", "Feature flags", "Unleash, OpenFeature"),
    ("gateway", "API gateway", "Kong, APISIX"),
    ("llmproxy", "LLM proxy", "LiteLLM"),
    ("aigateway", "AI gateway", "Portkey, Helicone"),
    ("mcpgateway", "MCP gateway", "mcp-proxy"),
    ("harness", "Agent harness", "OpenHands, SWE-agent"),
    ("sandbox", "Code runner", "Piston, E2B"),
    ("builder", "Agent builder", "Dify, Flowise"),
    ("blank", "Blank project", ""),
];

/// Every `package.json` in the tree that is not under `node_modules`, root first.
pub(crate) fn package_jsons(ctx: &RepoContext) -> Vec<serde_json::Value> {
    let mut paths: Vec<_> = ctx
        .files()
        .filter(|p| p.file_name() == Some("package.json") && !p.as_str().contains("node_modules"))
        .collect();
    paths.sort_by_key(|p| p.components().count());
    paths
        .into_iter()
        .filter_map(|p| ctx.read(p.as_str()))
        .filter_map(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .collect()
}

/// Every dependency name across every package.json.
pub(crate) fn dependencies(ctx: &RepoContext) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for pkg in package_jsons(ctx) {
        for key in ["dependencies", "devDependencies"] {
            if let Some(map) = pkg.get(key).and_then(|v| v.as_object()) {
                out.extend(map.keys().cloned());
            }
        }
    }
    out
}

pub fn detect(ctx: &RepoContext) -> Profile {
    let deps = dependencies(ctx);
    let is_js = ctx
        .files()
        .any(|p| p.file_name() == Some("package.json") && !p.as_str().contains("node_modules"));
    let mut signals = Vec::new();
    let mut scores: std::collections::BTreeMap<&str, u32> = Default::default();
    for (dep, weights) in DEP_SIGNALS {
        if deps.contains(*dep) {
            signals.push(dep.to_string());
            for (t, w) in *weights {
                *scores.entry(t).or_default() += w;
            }
        }
    }
    // Files say things dependencies do not.
    let file_signals: &[(&str, &str, u32)] = &[
        ("Dockerfile", "sandbox", 0),
        ("openapi.yaml", "api", 2),
        ("openapi.json", "api", 2),
        ("workflows.json", "n8n", 0),
    ];
    for (name, t, w) in file_signals {
        if ctx.files().any(|p| p.file_name() == Some(name)) && *w > 0 {
            signals.push(name.to_string());
            *scores.entry(t).or_default() += w;
        }
    }
    if ctx
        .files()
        .any(|p| p.as_str().starts_with("app/") && p.as_str().ends_with("route.ts"))
    {
        signals.push("app/**/route.ts".into());
        *scores.entry("fullstack").or_default() += 2;
        *scores.entry("api").or_default() += 1;
    }

    let total: u32 = scores.values().sum();
    let (template, best) = scores
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(t, s)| (*t, *s))
        .unwrap_or(("blank", 0));
    let confidence = if total == 0 {
        0
    } else {
        (best * 100 / total).clamp(25, 95)
    };
    let (template, title, like) = TEMPLATES
        .iter()
        .find(|(id, _, _)| *id == template)
        .copied()
        .unwrap_or(("blank", "Blank project", ""));

    let stack: Vec<&'static str> = {
        let mut v: Vec<&'static str> = STACK_DEPS
            .iter()
            .filter(|(d, _)| deps.contains(*d))
            .map(|(_, tag)| *tag)
            .collect();
        v.sort();
        v.dedup();
        v
    };

    Profile {
        template,
        template_title: title,
        like,
        confidence,
        signals,
        hosting: hosting(ctx, &deps),
        stack,
        is_js,
    }
}

fn hosting(ctx: &RepoContext, deps: &BTreeSet<String>) -> Vec<Hosting> {
    let has_file = |n: &str| ctx.files().any(|p| p.file_name() == Some(n));
    let has_dir = |n: &str| ctx.files().any(|p| p.as_str().starts_with(n));
    let has_dep = |n: &str| deps.contains(n);
    let mut out = BTreeSet::new();
    if has_file("vercel.json")
        || has_dir(".vercel/")
        || has_dep("@vercel/otel")
        || has_dep("@vercel/analytics")
    {
        out.insert(Hosting::Vercel);
    }
    if has_file("netlify.toml") {
        out.insert(Hosting::Netlify);
    }
    if has_dir("supabase/") || has_dep("@supabase/supabase-js") || has_dep("@supabase/ssr") {
        out.insert(Hosting::Supabase);
    }
    if has_file("firebase.json") || has_dep("firebase") || has_dep("firebase-admin") {
        out.insert(Hosting::Firebase);
    }
    if has_file("render.yaml") {
        out.insert(Hosting::Render);
    }
    if has_file("railway.json") || has_file("railway.toml") {
        out.insert(Hosting::Railway);
    }
    if has_file("fly.toml") {
        out.insert(Hosting::Fly);
    }
    if has_file("Procfile") || has_file("app.json") && !has_file("next.config.ts") {
        out.insert(Hosting::Heroku);
    }
    if has_file("wrangler.toml") || has_file("wrangler.json") || has_file("wrangler.jsonc") {
        out.insert(Hosting::Cloudflare);
    }
    if has_file("serverless.yml")
        || has_file("samconfig.toml")
        || has_file("template.yaml") && has_dep("aws-sdk")
        || has_file("cdk.json")
        || has_dir("terraform/aws")
        || deps.iter().any(|d| d.starts_with("@aws-sdk/"))
        || has_dep("aws-sdk")
    {
        out.insert(Hosting::Aws);
    }
    if has_file("cloudbuild.yaml")
        || has_file("app.yaml")
        || has_file("skaffold.yaml")
        || deps.iter().any(|d| d.starts_with("@google-cloud/"))
    {
        out.insert(Hosting::Gcp);
    }
    if has_file("azure-pipelines.yml") || deps.iter().any(|d| d.starts_with("@azure/")) {
        out.insert(Hosting::Azure);
    }
    if has_dir("k8s/")
        || has_dir("kubernetes/")
        || has_dir("helm/")
        || ctx
            .files()
            .any(|p| p.file_name() == Some("kustomization.yaml"))
    {
        out.insert(Hosting::Kubernetes);
    }
    if ctx
        .files()
        .any(|p| p.file_name().is_some_and(|n| n.starts_with("Dockerfile")))
    {
        out.insert(Hosting::Containers);
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn a_next_app_on_vercel_with_supabase_is_read_as_such() {
        let (_d, ctx) = fixture(&[
            (
                "package.json",
                r#"{"dependencies":{"next":"16","react":"19","@supabase/supabase-js":"2","@vercel/otel":"1"}}"#,
            ),
            (
                "app/api/health/route.ts",
                "export const GET = () => new Response('ok')",
            ),
        ]);
        let p = detect(&ctx);
        assert_eq!(p.template, "fullstack");
        assert!(p.hosting.contains(&Hosting::Vercel));
        assert!(p.hosting.contains(&Hosting::Supabase));
        assert!(p.stack.contains(&"next"));
        assert!(p.confidence >= 25);
    }

    #[test]
    fn a_langgraph_service_in_a_cluster_is_a_graph_agent() {
        let (_d, ctx) = fixture(&[
            (
                "backend/package.json",
                r#"{"dependencies":{"hono":"4","@langchain/langgraph":"0.2","postgres":"3"}}"#,
            ),
            ("backend/Dockerfile", "FROM node"),
            ("k8s/base/kustomization.yaml", ""),
        ]);
        let p = detect(&ctx);
        assert_eq!(p.template, "graph");
        assert_eq!(p.like, "LangGraph");
        assert_eq!(p.hosting, vec![Hosting::Kubernetes, Hosting::Containers]);
    }

    #[test]
    fn nothing_recognisable_is_blank_with_no_confidence() {
        let (_d, ctx) = fixture(&[("Cargo.toml", ""), ("src/main.rs", "")]);
        let p = detect(&ctx);
        assert_eq!(p.template, "blank");
        assert_eq!(p.confidence, 0);
        assert!(!p.is_js);
    }
}
