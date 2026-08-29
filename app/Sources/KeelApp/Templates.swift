import Foundation

/// What people start most, as a card each.
///
/// Every template is the same golden-path scaffold — Next.js and Hono on Cloudflare, two
/// environments, green from the first commit — plus a **brief**: the first thing the agent is
/// asked, already written. The scaffold is what makes it deployable; the brief is what makes it
/// the thing you wanted. Two of them start from an export file rather than a description,
/// because the audience is teams leaving Bubble and n8n with their data in hand.
struct Template: Identifiable, Hashable {
    let id: String
    let title: String
    let icon: String
    let blurb: String
    /// The scaffold to lay down: `app` (full stack) or `empty` (agent scaffolding only).
    let scaffold: String
    /// The first prompt, sent once the project is open. Empty means open it and say nothing.
    let brief: String
    /// A file the brief needs attached first — an export, a spec — and what to call it.
    let wants: String?

    static let all: [Template] = [
        Template(id: "fullstack", title: "Full-stack app", icon: "square.stack.3d.up",
                 blurb: "Next.js front, Hono API, D1, two environments. The golden path as-is.",
                 scaffold: "app", brief: "", wants: nil),
        Template(id: "api", title: "REST API service", icon: "point.3.connected.trianglepath.dotted",
                 blurb: "Typed Hono routes over D1, API keys, rate limits, OpenAPI docs.",
                 scaffold: "app",
                 brief: "Make the backend a standalone API service: resources in D1 with migrations and typed Hono routes, API-key auth with keys stored hashed, per-key rate limiting in KV, request validation with zod, an OpenAPI document generated from the routes, and a docs page in the frontend that renders it. Keep the route table one chained expression so the frontend client stays typed. Run the gate when done.",
                 wants: nil),
        Template(id: "jobs", title: "Webhooks & background jobs", icon: "arrow.triangle.2.circlepath.circle",
                 blurb: "Inbound webhooks verified and queued, workers that retry, cron schedules.",
                 scaffold: "app",
                 brief: "Build an event-processing backend: webhook endpoints that verify signatures (Stripe and GitHub as examples) and enqueue to Cloudflare Queues, a consumer Worker with idempotency keys in D1 and exponential retry, a cron-triggered job runner, a dead-letter table, and an admin page listing events and their state. Run the gate when done.",
                 wants: nil),
        Template(id: "auth", title: "Auth & users", icon: "person.badge.key",
                 blurb: "Magic links, sessions, roles, API tokens — the account layer done once.",
                 scaffold: "app",
                 brief: "Build the account layer: users and sessions in D1, email magic-link sign-in with signed single-use tokens, session cookies with rotation, roles (owner, admin, member) with a middleware that enforces them on Hono routes, personal API tokens, and account pages in the frontend. No passwords stored, ever. Run the gate when done.",
                 wants: nil),
        Template(id: "tenant", title: "Multi-tenant SaaS backend", icon: "building.2",
                 blurb: "Organisations, memberships, per-tenant data isolation, billing hooks.",
                 scaffold: "app",
                 brief: "Build a multi-tenant SaaS backend: organisations and memberships in D1, every table scoped by org id with a query layer that cannot forget the scope, invitations, plan limits enforced in middleware, Stripe webhook handling for subscription state, and an org settings page. Run the gate when done.",
                 wants: nil),
        Template(id: "pipeline", title: "Data pipeline", icon: "arrow.down.right.and.arrow.up.left",
                 blurb: "Ingest to R2, transform on a schedule, aggregate into D1, expose an API.",
                 scaffold: "app",
                 brief: "Build a data pipeline: an ingest endpoint that writes raw records to R2 as newline-delimited JSON, a cron Worker that transforms new objects and upserts daily aggregates into D1, a backfill command, a metrics API over the aggregates, and a frontend page with a chart and a table. Make transforms pure functions with unit tests. Run the gate when done.",
                 wants: nil),
        Template(id: "agent", title: "AI agent backend", icon: "sparkles",
                 blurb: "Streaming Claude API calls with tools, conversation store, a chat UI to test it.",
                 scaffold: "app",
                 brief: "Build an AI agent backend: a Hono endpoint that streams Claude API responses with tool use (a web-fetch tool and a D1-backed notes tool), conversation history in D1, a system prompt editable from a settings page, per-user rate limits, and a minimal streaming chat UI to exercise it. The API key is an environment binding, never in the repository. Run the gate when done.",
                 wants: nil),
        Template(id: "realtime", title: "Realtime service", icon: "dot.radiowaves.left.and.right",
                 blurb: "WebSockets on Durable Objects: rooms, presence, ordered messages.",
                 scaffold: "app",
                 brief: "Build a realtime service on Durable Objects: a room object per channel with WebSocket hibernation, presence, ordered message history persisted in the object's SQLite storage, a Hono route that issues short-lived join tokens, and a frontend page that joins a room and shows messages live. Run the gate when done.",
                 wants: nil),
        Template(id: "bubble", title: "Port from Bubble.io", icon: "arrow.right.doc.on.clipboard",
                 blurb: "Attach the Bubble export; data types, pages and workflows become code you own.",
                 scaffold: "app",
                 brief: "The attached file is an export of a Bubble.io application. Read it fully first. Then: map every data type to a D1 table with a migration, every page to a Next.js route with the same fields and layout intent, every workflow to a typed Hono route or a scheduled Worker, and every option set to a TypeScript enum. Write a MIGRATION.md listing each Bubble element and where it went, and anything you could not map. Work in small commits, one data type or page per commit. Run the gate after each.",
                 wants: "the Bubble export (.json)"),
        Template(id: "n8n", title: "Port from n8n", icon: "arrow.triangle.branch",
                 blurb: "Attach n8n workflow JSON; every node becomes a function you can test.",
                 scaffold: "app",
                 brief: "The attached file is one or more exported n8n workflows. Read them fully first. Then rebuild each workflow as code: triggers become Hono routes or cron-scheduled Workers, HTTP nodes become typed fetch calls, transforms become plain functions with unit tests, and credentials become environment bindings that are named but never written. Keep the node names as function names so the two line up. Write a MIGRATION.md mapping every node to its code. Run the gate when done.",
                 wants: "the n8n workflow export (.json)"),
        Template(id: "blank", title: "Blank", icon: "doc",
                 blurb: "Agent scaffolding only — a CLAUDE.md, a gate, no application code.",
                 scaffold: "empty", brief: "", wants: nil),
    ]
}
