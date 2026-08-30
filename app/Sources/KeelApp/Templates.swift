import Foundation

/// What people build, as architectures rather than as blurbs.
///
/// Backend and full-stack engineers choose by the shape of the system: which components, how a
/// request moves through them, what keeps it up and what keeps it safe. So a template is that
/// shape written down — and the same text goes three places: the detail pane before Create,
/// the `## Architecture` section of the generated CLAUDE.md, and the first brief the agent
/// gets. One source, so what was promised is what is built.
struct Template: Identifiable, Hashable {
    let id: String
    let category: Category
    let title: String
    /// Open-source projects this is the shape of — "like Kong, APISIX". Open source on purpose:
    /// the person can read the reference, and so can the agent.
    let like: String
    let icon: String
    let blurb: String
    /// The scaffold to lay down: `app` (the chosen stack) or `empty` (agent scaffolding only).
    let scaffold: String
    /// Components and what each is for.
    let components: [Component]
    /// How one request or event moves through them, in order.
    let flow: [String]
    /// What is built in for availability, safety and operability.
    let practices: [String]
    /// What the agent is asked to build, beyond the scaffold. Empty means open and say nothing.
    let brief: String
    /// Working code ships with the scaffold: the gate passes and the page does something
    /// before the agent has run once. The brief says "extend", not "build".
    var runnable: Bool { Template.runnableIds.contains(id) }
    static let runnableIds: Set<String> = [
        "api", "jobs", "llmproxy", "flags", "loop", "graph", "pipeline", "realtime", "fullstack", "auth", "tenant", "internal", "agent", "orchestrator", "evals", "dag", "controlplane", "dataplane", "gateway", "aigateway", "mcpgateway", "harness", "sandbox", "builder", "bubble", "n8n",
    ]
    /// A file the brief needs attached first, and what to call it.
    let wants: String?

    struct Component: Hashable {
        let name: String
        let role: String
        let tech: String
    }

    enum Category: String, CaseIterable, Identifiable {
        case backend = "Backend & workers"
        case fullstack = "Full-stack"
        case agents = "Agents"
        case control = "Control pfeatures"
        case proxies = "Proxies & gateways"
        case harness = "IDEs & harnesses"
        case builders = "Agent builders"
        case migrate = "Migrations"
        case starting = "Starting points"
        var id: String { rawValue }
        var icon: String {
            switch self {
            case .backend: "server.rack"
            case .fullstack: "square.stack.3d.up"
            case .agents: "sparkles"
            case .control: "slider.horizontal.3"
            case .proxies: "arrow.left.arrow.right"
            case .harness: "terminal"
            case .builders: "wand.and.stars"
            case .migrate: "arrow.right.doc.on.clipboard"
            case .starting: "doc"
            }
        }
    }

    /// The architecture as Markdown — for CLAUDE.md and the brief.
    var architecture: String {
        var out = "### \(title)\n\n\(blurb)\n\n**Components**\n"
        for c in components { out += "- **\(c.name)** (\(c.tech)) — \(c.role)\n" }
        out += "\n**Flow**\n"
        for (i, f) in flow.enumerated() { out += "\(i + 1). \(f)\n" }
        out += "\n**Built in**\n"
        for p in practices { out += "- \(p)\n" }
        return out
    }

    /// Every template's architecture in one document, for a blank project to learn from.
    static var patternsDoc: String {
        var out = "# Patterns\n\nThe architectures Keel's templates are built from. Each names the open-source projects it is the shape of, its components, how a request moves, and the rules that keep it up.\n\n"
        for c in Category.allCases {
            let ts = all.filter { $0.category == c && !$0.components.isEmpty }
            guard !ts.isEmpty else { continue }
            out += "## \(c.rawValue)\n\n"
            for t in ts {
                out += t.architecture.replacingOccurrences(of: "### \(t.title)", with: "### \(t.title)" + (t.like.isEmpty ? "" : " — like \(t.like)")) + "\n"
            }
        }
        return out
    }

    /// The first prompt: the brief, with the architecture attached so the agent builds to it.
    var fullBrief: String {
        guard !brief.isEmpty else { return "" }
        return brief + "\n\nBuild to the architecture below, which is also in CLAUDE.md. Where the scaffold already provides a component, extend it rather than adding a second one.\n\n" + architecture
    }

    // The practices every stack-scaffolded template shares; listed once, appended to each.
    // The rules every service gets, with the mechanism, so a reviewer can check them rather
    // than agree with them. From the system-design canon (DDIA, SRE, Well-Architected, Stripe).
    private static let base = [
        "Stateless handlers: session in a signed cookie or Redis, never process memory — any replica serves any request",
        "Timeouts on every outbound call, shorter than the caller's; deadline propagated in a header",
        "Retries: max 3, exponential backoff with full jitter, only on idempotent operations, under a retry budget",
        "Zero-downtime rollouts: maxUnavailable 0, preStop drain ≥ LB deregistration, three distinct probes (startup / readiness checks deps / liveness never does), HPA owns replicas",
        "Migrations expand/contract and N−1 compatible: add nullable → dual-write → backfill → switch reads → drop, each its own deploy",
        "Connection pool sized to the database: replicas × pool < max_connections; a pooler in between",
        "Cache entries get TTL + jitter and single-flight refresh, so a hot key never stampedes the origin",
        "Secrets only from the environment, rotated, never in an image; images run as a non-root user; lockfiles pinned and scanned",
        "One JSON log line per event with request_id and trace_id propagated through queues; errors to Sentry; RED metrics per route",
        "SLO per endpoint class with an error budget; alerts on burn rate, not CPU; every alert links to a runbook",
        "Typed seam: the frontend client is built from the API's route types — a shape change fails typecheck, not production",
        "make check is the gate — Keel runs it after every turn, CI before every image",
    ]
    private static func with(_ extra: [String]) -> [String] { extra + base }

    static let all: [Template] = [
        // ── backend & workers ─────────────────────────────────────────────────────────────
        Template(id: "api", category: .backend, title: "REST API service", like: "PostgREST, Directus", icon: "point.3.connected.trianglepath.dotted",
                 blurb: "A typed HTTP API with keys, limits and generated docs — the service other things call.",
                 scaffold: "app",
                 components: [
                    .init(name: "API", role: "typed routes, validation, auth middleware", tech: "Hono on Node"),
                    .init(name: "Database", role: "resources, API keys (hashed), migrations", tech: "Postgres"),
                    .init(name: "Rate limiter", role: "per-key sliding window", tech: "Redis"),
                    .init(name: "Docs", role: "OpenAPI generated from the routes, rendered in the frontend", tech: "Next.js"),
                 ],
                 flow: ["Request arrives with an API key header", "Middleware hashes the key, loads its scopes and checks the Redis window", "zod validates the body against the route schema", "The handler runs one transaction and returns a typed response", "Errors return a stable shape with a request id; the same id is in the log line"],
                 practices: with(["Idempotency-Key on every mutating route: stored with the request hash and response for 24h; same key with a different body is 422", "Keyset pagination on (created_at, id), page size capped at 100 — never OFFSET", "429 with RateLimit-* headers from a token bucket per key; bounded body size; unknown fields rejected"]),
                 brief: "The backend already runs: hashed API keys (POST /api/keys under ADMIN_TOKEN), per-key limits answered in RateLimit-* headers, Idempotency-Key on creates with request-hash mismatch detection, keyset pagination, and /api/openapi.json rendered by the frontend. Read backend/src/app.ts and its tests first. Then extend it: replace the `items` resource with the real ones for this product (migrations, zod schemas, routes, OpenAPI), move the rate-limit window to Redis so replicas share it, and keep every route covered in app.test.ts. Run the gate when done.",
                 wants: nil),
        Template(id: "jobs", category: .backend, title: "Webhooks & background jobs", like: "Inngest, BullMQ", icon: "arrow.triangle.2.circlepath.circle",
                 blurb: "Inbound events verified and queued; workers that retry with backoff; nothing processed twice, nothing lost.",
                 scaffold: "app",
                 components: [
                    .init(name: "Webhook endpoints", role: "verify signatures, bound replay window, enqueue", tech: "Hono"),
                    .init(name: "Queue", role: "durable work items with visibility timeouts", tech: "Redis streams"),
                    .init(name: "Worker", role: "consumes, idempotent by key, backoff, dead-letter", tech: "Node process, own Deployment"),
                    .init(name: "Scheduler", role: "cron-shaped jobs, one leader", tech: "Redis lock"),
                    .init(name: "Events table", role: "every event, its state, its attempts", tech: "Postgres"),
                 ],
                 flow: ["Provider POSTs; signature verified before the body is parsed", "Event stored with its idempotency key; duplicate keys are acknowledged, not reprocessed", "Enqueued; the endpoint returns 202 in milliseconds", "A worker claims it, runs the handler, records the attempt", "Failure → backoff → retry; after N, dead-letter with a replay button in the admin page"],
                 practices: with(["Transactional outbox: the event row and the enqueue happen in one transaction, so a crash never loses an event", "Exactly-once is at-least-once plus an idempotent consumer: processed event ids recorded in the same transaction as the effect", "Dead-letter after N attempts; DLQ depth > 0 pages; every dead event is replayable from the admin page", "Workers scale on queue depth, separately from the API; bounded queues shed with 503 + Retry-After"]),
                 brief: "The backend already runs: signed webhooks (HMAC, constant-time) stored with their jobs in one transaction, a worker (bun run worker) that claims with SKIP LOCKED, retries with jittered backoff, dead-letters after five attempts, and a page listing jobs with replay. Read backend/src/app.ts, worker.ts and their tests first. Then extend it: add the real event names to ROUTING and their handlers to HANDLERS, add a scheduled job (cron) with leader election, and keep the worker's retry path tested. Run the gate when done.",
                 wants: nil),
        Template(id: "pipeline", category: .backend, title: "Data pipeline", like: "Airflow, dbt", icon: "arrow.down.right.and.arrow.up.left",
                 blurb: "Ingest raw records, transform on a schedule, keep aggregates queryable — with backfill and replay from day one.",
                 scaffold: "app",
                 components: [
                    .init(name: "Ingest API", role: "append-only raw events, batched", tech: "Hono"),
                    .init(name: "Raw store", role: "immutable events, partitioned by day", tech: "Postgres (or object storage)"),
                    .init(name: "Transform job", role: "pure functions, unit-tested, scheduled", tech: "Worker + cron"),
                    .init(name: "Aggregates", role: "daily rollups, upserted, versioned", tech: "Postgres"),
                    .init(name: "Metrics API", role: "reads aggregates; never scans raw", tech: "Hono"),
                 ],
                 flow: ["Records arrive in batches and are appended, never updated", "The scheduled job picks up unprocessed partitions with a watermark", "Transforms are pure: input rows → output rows, tested without a database", "Aggregates are upserted so a rerun is safe", "A backfill command replays any date range through the same code"],
                 practices: with(["Watermarks, not wall-clock now — reruns and late data are normal; the DB clock is the only clock", "Raw is immutable and partitioned by day; retention is dropping a partition, never DELETE", "Transforms are pure functions with tests; aggregates are upserted so any rerun is safe", "IDs are ULID/UUIDv7 so indexes stay ordered under write load"]),
                 brief: "The backend already runs: POST /api/ingest batched and idempotent by batch_id into a day-partitioned events table (partition created on first sight), pure tested transforms, a watermark job (bun run worker) that recomputes touched days and upserts aggregates, backfill for a range, GET /api/metrics, seeded sample data and a page with the table. Read backend/src/transforms.ts, worker.ts and their tests first. Then extend it: replace the sample event shape and aggregates with this product's, keep every transform pure and unit-tested, and add the chart the page needs. Run the gate when done.",
                 wants: nil),
        Template(id: "realtime", category: .backend, title: "Realtime service", like: "Centrifugo, Soketi", icon: "dot.radiowaves.left.and.right",
                 blurb: "WebSockets that survive more than one replica: rooms, presence, ordered history.",
                 scaffold: "app",
                 components: [
                    .init(name: "WebSocket endpoint", role: "join with a short-lived token, per-room fan-out", tech: "Hono + ws"),
                    .init(name: "Pub/sub", role: "messages cross replicas", tech: "Redis"),
                    .init(name: "History", role: "ordered messages per room, cursor reads", tech: "Postgres"),
                    .init(name: "Presence", role: "who is here, with TTL heartbeats", tech: "Redis"),
                    .init(name: "Token issuer", role: "HTTP route mints join tokens from the session", tech: "Hono"),
                 ],
                 flow: ["Client asks HTTP for a join token (session checked)", "Connects with the token; the replica subscribes to the room channel", "A message is written to history, then published; every replica fans out", "Presence heartbeats refresh a TTL key; expiry is a leave", "Reconnect resumes from the last seen cursor"],
                 practices: with(["Sticky sessions at the ingress; reconnect with a cursor so a replica dying loses nothing", "Backpressure: bounded per-connection buffers; slow consumers are dropped, not the room", "Join tokens expire in seconds and are single-use; the socket carries no long-lived credential", "Presence is a TTL lease renewed at TTL/3 — a dead client is gone in one TTL"]),
                 brief: "The backend already runs: rooms with Centrifugo-shaped join tokens (HS256, minted from a signed session), GET /api/rooms/:room/events as SSE with gapless offsets, history resume with since=, presence via TTL heartbeats, slow-client disconnect, and cross-replica fan-out over Postgres LISTEN/NOTIFY — no extra dependency. Read backend/src/app.ts and its protocol tests first. Then extend it: add this product's message types and authorization per room, move fan-out to Redis if NOTIFY's 8 KB cap bites, and add WebSocket via @hono/node-ws only if SSE+POST is not enough. Run the gate when done.",
                 wants: nil),
        // ── full-stack ────────────────────────────────────────────────────────────────────
        Template(id: "fullstack", category: .fullstack, title: "Full-stack app", like: "create-next-app + Hono, deployable", icon: "square.stack.3d.up",
                 blurb: "Next.js in front, Hono behind, Postgres and Redis beside — the scaffold as-is, ready to deploy.",
                 scaffold: "app",
                 components: [
                    .init(name: "Frontend", role: "server components, typed client to the API", tech: "Next.js 16, standalone"),
                    .init(name: "API", role: "chained typed routes, health, drain on SIGTERM", tech: "Hono on Node"),
                    .init(name: "Database", role: "SQL migrations run by an init container", tech: "Postgres"),
                    .init(name: "Cache", role: "sessions, rate limits, pub/sub", tech: "Redis"),
                    .init(name: "Edge", role: "one origin: / to the app, /api to the API", tech: "nginx locally, ingress in the cluster"),
                 ],
                 flow: ["Browser hits one origin", "/ renders in Next; server components call the API service-to-service", "/api goes straight to Hono", "Both read config from the environment; neither holds a secret in the image"],
                 practices: with([]),
                 brief: "The app already runs: a posts resource with server-rendered list, sign-in, create and per-row edit/delete via server actions, signed HttpOnly sessions, one zod schema shared by both halves, and optimistic concurrency (PUT carries version; stale gets 409 with the current row). Read backend/src/schema.ts, app.ts and the tests first. Then extend it: replace posts with this product's resources one at a time — migration, schema, routes, page — in small commits, keeping the 409 path and author checks tested. Run the gate after each.",
                 wants: nil),
        Template(id: "auth", category: .fullstack, title: "Auth & users", like: "Auth.js, Lucia, Keycloak", icon: "person.badge.key",
                 blurb: "The account layer done once: magic links, sessions, roles, API tokens — no passwords stored, ever.",
                 scaffold: "app",
                 components: [
                    .init(name: "Users & sessions", role: "users, sessions, tokens, audit", tech: "Postgres"),
                    .init(name: "Magic links", role: "signed single-use tokens, short expiry", tech: "Hono + email"),
                    .init(name: "Session middleware", role: "cookie, rotation on login, constant-time compare", tech: "Hono"),
                    .init(name: "Roles", role: "owner / admin / member enforced per route", tech: "middleware"),
                    .init(name: "Account pages", role: "profile, sessions, API tokens", tech: "Next.js"),
                 ],
                 flow: ["Email submitted → a signed token is mailed, stored hashed", "Link clicked → token verified once, session created, cookie set HttpOnly/Secure/SameSite", "Every protected route reads the session and the role before the handler", "API tokens are hashed at rest and scoped; revocation is immediate"],
                 practices: with(["Sessions rotate on login and privilege change; logout invalidates server-side; cookies HttpOnly, Secure, SameSite", "Token comparison in constant time; error text never reveals whether an account exists", "Rate limits and lockouts on every auth endpoint", "Every auth event is in an append-only audit table"]),
                 brief: "The backend already runs: better-auth-shaped routes (magic-link sign-in, verify, get-session, sign-out), Lucia-style sessions (random cookie token, sha256 at rest, rotation), hashed single-use 5-minute links, owner > admin > member roles via requireRole, hashed personal API tokens, an auth_audit table, and sign-in rate limits per email and IP; the email sender is injected. Read backend/src/app.ts and its six tests first. Then extend it: wire the real email provider from the environment, add the account pages this product needs, and keep every path in the tests. Run the gate when done.",
                 wants: nil),
        Template(id: "tenant", category: .fullstack, title: "Multi-tenant SaaS", like: "Cal.com, Twenty CRM", icon: "building.2",
                 blurb: "Organisations, memberships, and a data layer that cannot forget the tenant.",
                 scaffold: "app",
                 components: [
                    .init(name: "Tenancy", role: "orgs, memberships, invitations", tech: "Postgres"),
                    .init(name: "Scoped query layer", role: "every query carries org id; there is no unscoped path", tech: "TypeScript module"),
                    .init(name: "Plan limits", role: "enforced in middleware from a plans table", tech: "Hono"),
                    .init(name: "Billing hooks", role: "subscription state from provider webhooks", tech: "Stripe webhooks"),
                    .init(name: "Org settings", role: "members, roles, billing, danger zone", tech: "Next.js"),
                 ],
                 flow: ["Session resolves the user and the current org", "The scoped layer is the only way to touch tenant tables; it takes the org id as a required argument", "Plan limits are checked before writes that count against them", "Provider webhooks update subscription state idempotently"],
                 practices: with(["Row-level scope by construction, tested with a cross-tenant read that must fail", "tenant_id is the partition key on every tenant table — chosen now, not re-sharded later", "Invitations expire; membership changes are audited; billing state is derived from idempotent webhooks, never from the client", "Per-tenant rate limits so one tenant cannot starve the rest (bulkheads)"]),
                 brief: "The backend already runs: organisations and memberships, a store.tenant(orgId) query layer that is the only way to reach tenant tables (composite keys; a test proves a cross-tenant read fails), invitations hashed with 7-day expiry, plan limits as middleware (402), and a Stripe webhook with signature verification, replay protection and subscription → plan mapping. Read backend/src/store.ts, app.ts and the tests first. Then extend it: add this product's tenant tables through the same scoped layer, the org settings pages, and swap the simple session for the auth pack's. Run the gate when done.",
                 wants: nil),
        Template(id: "internal", category: .fullstack, title: "Admin & internal tools", like: "Appsmith, ToolJet", icon: "tablecells",
                 blurb: "Tables, forms, approvals and an audit log — the back office your team actually uses.",
                 scaffold: "app",
                 components: [
                    .init(name: "Data tables", role: "sort, filter, inline edit, export", tech: "Next.js"),
                    .init(name: "Forms", role: "schema-driven, validated on both sides", tech: "zod shared between halves"),
                    .init(name: "Approvals", role: "state machine with transitions and reasons", tech: "Postgres"),
                    .init(name: "Audit log", role: "who changed what, before and after", tech: "Postgres"),
                    .init(name: "Roles", role: "admin / editor / viewer", tech: "middleware"),
                 ],
                 flow: ["A row edit posts a validated patch", "The handler writes the change and an audit row in one transaction", "Approvals move through explicit states; every transition is logged", "Exports stream CSV from the same query"],
                 practices: with(["Audit rows in the same transaction as the change", "Validation schemas shared, not duplicated", "Read-only role tested"]),
                 brief: "The app already runs: a zod schema that drives both validation and the form, a list with allowlisted sort, filter, search and RFC-4180 CSV export, an approval workflow as a state × action transition table applied with optimistic checks (409 on races), an audit log written in the same transaction as every change, and admin/editor/viewer roles. Read backend/src/schema.ts, app.ts and the tests first. Then extend it: replace the request schema and transitions with this product's, keep every transition and role gate in the tests. Run the gate when done.",
                 wants: nil),
        // ── agents ────────────────────────────────────────────────────────────────────────
        Template(id: "agent", category: .agents, title: "AI agent backend", like: "Open WebUI, LibreChat", icon: "sparkles",
                 blurb: "Streaming model calls with tools, a conversation store, budgets — and a chat UI to exercise it.",
                 scaffold: "app",
                 components: [
                    .init(name: "Agent loop", role: "model call → tool calls → repeat, streamed", tech: "Hono + Claude API"),
                    .init(name: "Tools", role: "typed, schema-described, sandboxed side effects", tech: "TypeScript registry"),
                    .init(name: "Conversations", role: "messages, tool results, token counts", tech: "Postgres"),
                    .init(name: "Budgets", role: "per-user tokens per day, rate limits", tech: "Redis"),
                    .init(name: "Chat UI", role: "streams tokens and tool events", tech: "Next.js"),
                 ],
                 flow: ["Client streams a message; budget checked first", "The loop calls the model with the history and tool schemas", "Each tool call is validated against its schema and run with a timeout", "Results go back to the model; every step is stored and streamed to the UI", "Token usage is recorded against the budget"],
                 practices: with(["Tool inputs validated; tools cannot reach the filesystem or shell; every tool result is untrusted data, never instructions", "Budgets checked before the call, usage recorded after; a run has a token, dollar and wall-clock cap", "Every run is a trace tree — prompt, tool calls, tokens, cost, latency — replayable from the store", "Context compaction at a threshold; task state lives in the store, not only in the window"]),
                 brief: "The backend already runs: POST /api/chat/completions streaming one turn as SSE (delta, tool_call, tool_result, done) against the Claude Messages API, a typed tool registry with per-tool timeouts (web_fetch with a public-address guard, notes on the store), conversations and messages with token counts, a per-user daily token budget, an editable system prompt, and a streaming chat page. Read backend/src/agent.ts and its tests first — the model is injected, so tests script it. Then extend it: add this product's tools and data, per-user auth, and move the budget to Redis when replicas share it. Run the gate when done.",
                 wants: nil),
        Template(id: "orchestrator", category: .agents, title: "Multi-agent orchestrator", like: "CrewAI, AutoGen", icon: "point.topleft.down.to.point.bottomright.curvepath",
                 blurb: "A planner that splits work, workers that run in parallel, a judge that checks — with every step durable.",
                 scaffold: "app",
                 components: [
                    .init(name: "Planner", role: "turns a goal into a task graph", tech: "model call"),
                    .init(name: "Task queue", role: "durable tasks with dependencies", tech: "Redis + Postgres"),
                    .init(name: "Workers", role: "run tasks with their own tools and budgets", tech: "worker Deployment"),
                    .init(name: "Judge", role: "verifies outputs against the task's acceptance", tech: "model call, cold context"),
                    .init(name: "Run viewer", role: "the graph, live, with every message", tech: "Next.js"),
                 ],
                 flow: ["Goal submitted → planner writes tasks and edges", "Workers claim tasks whose dependencies are done", "Each result is judged; failures re-plan that node, not the run", "The run completes when the graph does; every message is stored"],
                 practices: with(["Tasks are idempotent and resumable after a worker dies; (run_id, task_id) is the idempotency key for every tool effect", "The judge is a cold context that sees the output and the acceptance criteria, never the worker's reasoning", "Budgets per run and per task; handoffs are bounded — an infinite handoff loop is a stop condition", "A second agent exists only where contexts must be isolated or work is genuinely parallel"]),
                 brief: "The backend already runs: a planner that writes a CrewAI-shaped task graph, the store as the dependency queue (SKIP LOCKED claim of tasks whose deps are done), a worker (bun run worker) running each task in a ReAct loop with only its tools and budget, a cold-context judge per output, re-planning of failed nodes with a cap, and a run viewer page. Read backend/src/engine.ts and its tests first. Then extend it: add this product's tools and acceptance criteria, real inputs, and the scale knobs the workload needs. Run the gate when done.",
                 wants: nil),
        Template(id: "evals", category: .agents, title: "Evaluation harness", like: "promptfoo, DeepEval", icon: "checklist",
                 blurb: "Datasets, runners, graders and a leaderboard — so a prompt change is a measured change.",
                 scaffold: "app",
                 components: [
                    .init(name: "Datasets", role: "cases with inputs and expected outcomes, versioned", tech: "Postgres"),
                    .init(name: "Runner", role: "runs a prompt/model over a dataset, in parallel, with retries", tech: "worker"),
                    .init(name: "Graders", role: "exact, rubric, and model-judged", tech: "TypeScript + model"),
                    .init(name: "Results", role: "per-case scores, cost, latency", tech: "Postgres"),
                    .init(name: "Leaderboard", role: "compare runs, drill into cases", tech: "Next.js"),
                 ],
                 flow: ["A run is created for a dataset and a config", "The runner fans out cases to workers with a concurrency cap", "Each output is graded; scores and cost are stored per case", "The leaderboard compares runs; a case view shows the diff"],
                 practices: with(["Runs are reproducible: config and dataset versions pinned", "Model-judged grading uses a cold context and a fixed rubric", "Cost per run is visible before it finishes"]),
                 brief: "The backend already runs: promptfoo-shaped datasets ({{var}} prompts, cases with vars and assert: equals | contains | llm-rubric) stored as immutable versions, runs pinned to a version with a concurrency cap and jittered retries, a cold-context rubric judge, per-case score, cost, latency and tokens, a leaderboard ranked by score then cost, a drill-down, and a one-click sample dataset. Read backend/src/runner.ts and its tests first. Then extend it: add this product's datasets and graders, real model configs to compare, and CI that fails on a regression. Run the gate when done.",
                 wants: nil),
        Template(id: "loop", category: .agents, title: "Loop agent", like: "smolagents, OpenHands", icon: "arrow.trianglehead.2.clockwise.rotate.90",
                 blurb: "One agent, one loop: observe → think → act → check, with stop conditions and budgets so it always ends.",
                 scaffold: "app",
                 components: [
                    .init(name: "Loop runner", role: "model call → tool calls → repeat until a stop condition", tech: "worker + Claude API"),
                    .init(name: "Stop conditions", role: "goal check, max steps, token budget, wall clock, no-progress detector", tech: "TypeScript"),
                    .init(name: "Tools", role: "schema-validated, timeouts, sandboxed side effects", tech: "tool registry"),
                    .init(name: "Trace", role: "every step: input, output, tokens, decision", tech: "Postgres"),
                    .init(name: "Run API + UI", role: "start, watch live, stop, replay", tech: "Hono + Next.js"),
                 ],
                 flow: ["A run starts with a goal, a tool set and budgets", "Each iteration: the model sees the trace so far and picks an action", "The tool runs with a timeout; its result is appended", "Stop conditions are checked after every step, not only on success", "The run ends with a verdict: done, budget exhausted, stuck, or stopped by a person"],
                 practices: [
                    "A loop that cannot end is a bug: max steps, token/dollar and wall-clock budgets, a no-progress detector, and a `done` tool the model must call with a verifiable result",
                    "Every step is durable before the next begins — a crashed worker resumes, never restarts",
                    "Tool results are truncated with a note, never silently; the model is told what it did not see",
                 ] + base,
                 brief: "The backend already runs: a ReAct loop (backend/src/agent.ts) with a calculator, web search and final_answer, every step recorded with tokens and timing, max_steps with a best-answer call, tool errors as observations, runs streamed over SSE and stored, stop and concurrency caps, and a page that shows a run step by step. Read agent.ts and its tests first — the model is a function, so tests script it. Then extend it: add the tools this product needs, a per-user token budget in Redis, and a no-progress detector. Run the gate when done.",
                 wants: nil),
        Template(id: "graph", category: .agents, title: "Graph agent", like: "LangGraph, Mastra", icon: "point.3.filled.connected.trianglepath.dotted",
                 blurb: "A state machine of nodes and edges with checkpoints: pause for a human, resume, replay from any node.",
                 scaffold: "app",
                 components: [
                    .init(name: "Graph definition", role: "nodes (model, tool, code), edges with conditions, entry and end", tech: "TypeScript, typed state"),
                    .init(name: "Checkpointer", role: "state after every node, per thread", tech: "Postgres"),
                    .init(name: "Executor", role: "runs a node, evaluates edges, persists, continues", tech: "worker"),
                    .init(name: "Interrupts", role: "human-in-the-loop: pause at a node, resume with input", tech: "API + queue"),
                    .init(name: "Thread UI", role: "the graph with the current node lit; replay and branch", tech: "Next.js"),
                 ],
                 flow: ["A thread starts at the entry node with typed state", "Each node reads state, writes a patch; the checkpoint is saved", "Edges choose the next node from state; conditional edges are plain functions with tests", "An interrupt node parks the thread; a person resumes it with input", "Replay re-runs from any checkpoint; a branch forks a new thread from it"],
                 practices: [
                    "State is a typed object patched by nodes, never free text — schema-checked at every checkpoint",
                    "Every node is idempotent against its checkpoint: replay and time-travel re-execute nodes, so side effects carry (thread_id, step) keys",
                    "Cycles need a counter in the state and an edge that exits on it",
                 ] + base,
                 brief: "The backend already runs: a StateGraph (backend/src/graph.ts) with reducers, conditional edges, a Postgres checkpointer, interrupt() for a human step and resume through the API, history and fork-from-checkpoint, plus an example draft → review → publish graph and a page that drives it. Read graph.ts and its tests first. Then extend it: replace the example with this product's graph, add a worker so long nodes run off the request path, and keep every edge function unit-tested. Run the gate when done.",
                 wants: nil),
        Template(id: "dag", category: .agents, title: "DAG agent", like: "Airflow, Temporal", icon: "arrow.triangle.branch",
                 blurb: "Deterministic fan-out and fan-in: a DAG of LLM and code steps with per-node retries, caching and cost.",
                 scaffold: "app",
                 components: [
                    .init(name: "DAG definition", role: "nodes with typed inputs/outputs, dependencies, concurrency", tech: "TypeScript"),
                    .init(name: "Scheduler", role: "runs ready nodes in parallel up to a cap", tech: "worker + Redis"),
                    .init(name: "Node cache", role: "same inputs → same output, keyed by content hash", tech: "Redis + Postgres"),
                    .init(name: "Retries", role: "per-node policy, backoff, dead-letter", tech: "TypeScript"),
                    .init(name: "Run view", role: "the DAG coloured by state, cost per node", tech: "Next.js"),
                 ],
                 flow: ["A run materialises the DAG for its inputs", "Nodes whose dependencies are done run in parallel, capped", "A node's inputs are hashed; a cache hit skips the call", "A failed node retries by its policy; a dead node fails only its descendants", "The run is done when every node is; partial results are kept"],
                 practices: [
                    "Nodes are pure functions of their inputs — that is what makes caching and replay correct",
                    "Cost and latency are recorded per node, so the expensive step is a fact",
                    "Partial failure is a state, not an exception: the DAG shows what ran",
                 ] + base,
                 brief: "The backend already runs: a typed DAG definition validated for cycles, a scheduler running ready nodes in parallel up to a pool size, per-node retry with backoff, upstream_failed for descendants only, content-hash caching of outputs, per-node latency and cost, an example fetch → summarise ×3 → merge → judge DAG with an injected model, and a page coloured by state. Read backend/src/dag.ts and its tests first. Then extend it: replace the example with this product's DAG, move execution to a worker (runs are inline in the POST today, noted in the file), and keep every node pure and tested. Run the gate when done.",
                 wants: nil),
        // ── control planes ────────────────────────────────────────────────────────────────
        Template(id: "controlplane", category: .control, title: "Control plane", like: "Kubernetes controllers, Crossplane", icon: "slider.horizontal.3",
                 blurb: "Desired state in, reconciled state out: an API that owns a fleet of things and keeps them right.",
                 scaffold: "app",
                 components: [
                    .init(name: "Resource API", role: "declarative resources with generations", tech: "Hono"),
                    .init(name: "State store", role: "desired and observed state, versioned", tech: "Postgres"),
                    .init(name: "Reconciler", role: "diffs desired vs observed, acts, requeues", tech: "worker loop"),
                    .init(name: "Providers", role: "adapters that act on the world", tech: "TypeScript interfaces"),
                    .init(name: "Console", role: "resources, status, events", tech: "Next.js"),
                 ],
                 flow: ["A client PUTs desired state; the generation increments", "The reconciler picks up changed resources", "It reads observed state from the provider, computes the diff, applies it", "Status and events are written; failures requeue with backoff", "Convergence is a fact in the status, not an assumption"],
                 practices: with(["Reconcile is idempotent and level-triggered: compare spec to status on a timer and on change; it can run any number of times", "generation on the spec, observedGeneration in the status, resourceVersion on writes — a lost update is a 409, never a silent overwrite", "Changes roll out staged — canary → 10% → all — and halt on error-rate rise", "Static stability: nothing the data plane needs waits on this API being up"]),
                 brief: "The backend already runs: Kubernetes-shaped resources (spec, generation, resourceVersion with 409 on stale writes, status conditions, events, finalising deletes), a reconciler (bun run worker) that observes and applies through a Provider adapter with requeue and backoff, a real adapter managing rows in a records table, a fake with failNext for tests, and a console page. Read backend/src/reconciler.ts, providers.ts and the tests first. Then extend it: add this product's kinds and their providers, keep every reconcile path under the injected clock in the tests. Run the gate when done.",
                 wants: nil),
        Template(id: "dataplane", category: .control, title: "Data plane", like: "Envoy, Pingora", icon: "waveform.path",
                 blurb: "The hot path that keeps serving when the control plane is down: config pulled, cached, and applied locally.",
                 scaffold: "app",
                 components: [
                    .init(name: "Data plane service", role: "serves requests from local state only; no control-plane call on the hot path", tech: "Hono, horizontally scaled"),
                    .init(name: "Config snapshot", role: "pulled on an interval and on push, versioned, kept on disk", tech: "Redis + local file"),
                    .init(name: "Control plane API", role: "desired state, generations; separate deployment", tech: "Hono"),
                    .init(name: "Local cache", role: "hot entries with TTL and single-flight refresh", tech: "in-process LRU"),
                    .init(name: "Telemetry", role: "RED metrics per route, shipped async", tech: "logs + metrics"),
                 ],
                 flow: ["A request is served from the local snapshot and cache; nothing waits on the control plane", "A background loop pulls the snapshot by generation; a push notifies it early", "If the control plane is unreachable, the last snapshot keeps serving and a stale gauge rises", "Metrics leave the process on a separate path with backpressure"],
                 practices: [
                    "The data plane has no synchronous dependency on the control plane — tested by killing it",
                    "Snapshots are versioned; a bad push is rolled back by choosing the previous generation",
                    "Cache stampedes are prevented by single-flight refresh",
                ] + base,
                 brief: "Both halves already run in one process: a control-plane API that writes routes and bumps one generation in the same transaction and serves snapshots by generation (304 when unchanged), and a data plane that resolves from a local snapshot with a single-flight LRU, pulls in the background, accepts push notifications, exposes a stale gauge, and never calls the control plane on the request path — a test kills the control plane and proves it. Read backend/src/dataplane.ts and its tests first. Then extend it: split into two Deployments via CONTROL_PLANE_URL, put this product's config in the snapshot, and move the channel to Redis if pull latency matters. Run the gate when done.",
                 wants: nil),
        Template(id: "flags", category: .control, title: "Feature flags & config", like: "Unleash, Flagsmith, OpenFeature", icon: "switch.2",
                 blurb: "Flags, targeting, and config a fleet reads in microseconds — with an audit trail and a kill switch.",
                 scaffold: "app",
                 components: [
                    .init(name: "Flag API", role: "flags, rules, segments, versions", tech: "Hono"),
                    .init(name: "Store", role: "definitions and audit", tech: "Postgres"),
                    .init(name: "Distribution", role: "snapshot per environment, pushed on change", tech: "Redis + SSE"),
                    .init(name: "SDK", role: "in-memory evaluation, stale-safe", tech: "TypeScript package"),
                    .init(name: "Console", role: "edit, target, roll out, kill", tech: "Next.js"),
                 ],
                 flow: ["An edit writes a new version and an audit row", "A snapshot is built and published; clients holding an SSE stream get it", "The SDK evaluates locally against the snapshot", "If the service is down, clients keep the last snapshot — never fail closed by accident"],
                 practices: with(["Evaluation is local and deterministic; the SDK keeps the last snapshot when the service is down", "Every change is attributable and reversible; a flag has an owner and a removal date", "Percentage rollouts hash a stable key so a user's bucket never flips", "Snapshots are versioned and swapped atomically — never partially applied"]),
                 brief: "The backend already runs: Unleash's client API (GET /api/client/features with ETag, metrics, an SSE update stream), local evaluation with flexibleRollout (murmur3, sticky per user), constraints, variants, an audited admin API, and a frontend endpoint that evaluates for one context. Read backend/src/app.ts, evaluate.ts and their tests first. Then extend it: add segments, per-environment snapshots published through Redis, and a console page to edit, target and roll out — keeping evaluation pure and covered by the reference tests. Run the gate when done.",
                 wants: nil),
        // ── proxies & gateways ────────────────────────────────────────────────────────────
        Template(id: "gateway", category: .proxies, title: "API gateway", like: "Kong, APISIX, Envoy Gateway", icon: "arrow.left.arrow.right",
                 blurb: "One front door: auth, rate limits, routing and observability for services behind it.",
                 scaffold: "app",
                 components: [
                    .init(name: "Gateway", role: "auth, limits, routing by path/host, retries", tech: "Hono"),
                    .init(name: "Route table", role: "upstreams, policies, versioned", tech: "Postgres → Redis snapshot"),
                    .init(name: "Limiter", role: "per-key and per-route windows", tech: "Redis"),
                    .init(name: "Upstreams", role: "health-checked, circuit-broken", tech: "fetch with timeouts"),
                    .init(name: "Console", role: "routes, keys, live traffic", tech: "Next.js"),
                 ],
                 flow: ["Request matched to a route; policy loaded from the snapshot", "Auth and rate limit decided before any upstream call", "Forwarded with a timeout and a request id; retried only if idempotent", "Response and timing logged; circuit opens on repeated upstream failure"],
                 practices: with(["Circuit breaker per upstream: open after N failures in a window, half-open probe, fallback — never queue behind a dead one", "Retries only for idempotent methods, under a retry budget (≤ 10% of requests)", "Bulkheads: a concurrency limit per upstream so one slow service cannot take the others", "Route changes are versioned and atomic; the request id and traceparent are forwarded"]),
                 brief: "The backend already runs: a versioned route table, key auth (sha256 at rest), per-key and per-route limits in RateLimit-* headers, longest-prefix matching, forwarding with X-Request-Id and X-Forwarded-*, timeouts, retries only for idempotent methods, a per-route circuit breaker, Kong-style latency headers, a request log with timing, an admin API under ADMIN_TOKEN and a console page. Read backend/src/app.ts and its six tests first. Then extend it: add this product's routes and upstreams, share limits and circuits through Redis across replicas, and add the plugins the traffic needs. Run the gate when done.",
                 wants: nil),
        Template(id: "llmproxy", category: .proxies, title: "LLM proxy (LiteLLM-style)", like: "LiteLLM, Bifrost", icon: "brain",
                 blurb: "One OpenAI-compatible endpoint in front of every provider: routing, fallbacks, spend per key, logs you own.",
                 scaffold: "app",
                 components: [
                    .init(name: "Proxy", role: "OpenAI-compatible in, provider-specific out, streaming", tech: "Hono"),
                    .init(name: "Router", role: "model → provider, fallbacks, weights", tech: "config in the database"),
                    .init(name: "Cache", role: "exact and semantic hits", tech: "Redis"),
                    .init(name: "Budgets", role: "per-key spend, hard stops", tech: "Redis + Postgres"),
                    .init(name: "Logs", role: "every request, tokens, cost, latency", tech: "Postgres"),
                 ],
                 flow: ["A client calls the proxy as if it were a provider", "The router picks the provider; a cache hit returns immediately", "Budget checked; the call is streamed through with the provider key from the environment", "Usage and cost recorded; a provider error falls back to the next"],
                 practices: with(["Provider keys never leave the proxy; rotation is a config change", "Budgets stop spend before it happens; cost is recorded per request", "Fallbacks are ordered, timed out individually, and observable as events", "Cache hits are served with a header saying so"]),
                 brief: "The backend already runs: POST /v1/chat/completions (streaming as OpenAI chunks), GET /v1/models, POST /key/generate under MASTER_KEY, virtual keys with model allowlists and budgets, Anthropic and OpenAI adapters with the message mapping, ordered fallbacks, and a spend log with cost per call shown on the page. Read backend/src/app.ts, providers.ts and their tests first. Then extend it: add exact-match response caching in Redis, per-key RPM limits, and any providers this product needs — the tests use a fake fetch, keep it that way. Run the gate when done.",
                 wants: nil),
        Template(id: "aigateway", category: .proxies, title: "AI gateway", like: "Portkey gateway, Helicone", icon: "shield.lefthalf.filled",
                 blurb: "The policy layer for model traffic: virtual keys, guardrails, semantic cache, A/B routing, full observability.",
                 scaffold: "app",
                 components: [
                    .init(name: "Gateway", role: "virtual keys → real providers; policies applied per key", tech: "Hono, streaming"),
                    .init(name: "Guardrails", role: "PII redaction, prompt-injection and topic checks, output filters", tech: "TypeScript pipeline"),
                    .init(name: "Semantic cache", role: "embedding-similar prompts served from cache", tech: "Redis + pgvector"),
                    .init(name: "Router", role: "weights, A/B, canaries, fallbacks per model", tech: "config in the database"),
                    .init(name: "Observability", role: "every call: prompt hash, tokens, cost, latency, guardrail verdicts", tech: "Postgres + dashboards"),
                 ],
                 flow: ["A client calls with a virtual key; its policy bundle is loaded", "Input guardrails run; a block returns a stable error the client can show", "The semantic cache is checked; a hit skips the provider", "The router picks a provider by weight or experiment; a failure falls back", "Output guardrails run on the stream; the call is logged with its verdicts"],
                 practices: [
                    "Virtual keys never reveal provider keys; rotating a provider key changes nothing for clients",
                    "Guardrail verdicts are logged even when they pass, so a policy change is measurable",
                    "Experiments are assigned by a stable hash of the caller, not per request",
                 ] + base,
                 brief: "The backend already runs: OpenAI-compatible streaming keyed by virtual keys carrying a policy (models, budget, rpm, guardrails, experiment), input guardrails (PII redaction, injection heuristics, topic allowlist) before any provider call, output PII filtering on the stream, an exact-match cache, sticky weighted A/B routing by caller hash with fallbacks, Anthropic and OpenAI adapters, every call logged with prompt hash, tokens, cost, latency and verdicts, and a dashboard. Read backend/src/app.ts, guardrails.ts and the tests first. Then extend it: add the semantic cache (pgvector column is noted in the migration), real guardrail models where regexes are not enough, and this product's policies. Run the gate when done.",
                 wants: nil),
        Template(id: "mcpgateway", category: .proxies, title: "MCP gateway", like: "mcp-proxy, Metorial", icon: "cable.connector",
                 blurb: "One authenticated MCP endpoint that aggregates tool servers, scopes what each caller sees, and audits every call.",
                 scaffold: "app",
                 components: [
                    .init(name: "Gateway", role: "speaks MCP to clients; connects to upstream servers", tech: "Hono + MCP SDK"),
                    .init(name: "Registry", role: "upstream servers, their tools, health", tech: "Postgres"),
                    .init(name: "Scopes", role: "which caller may see and call which tools", tech: "policy table + middleware"),
                    .init(name: "Audit", role: "every tool call: who, what, args hash, result size, duration", tech: "Postgres"),
                    .init(name: "Console", role: "servers, tools, scopes, recent calls", tech: "Next.js"),
                 ],
                 flow: ["A client authenticates and lists tools; only its scope's tools appear", "A tool call is validated against the upstream schema, then forwarded", "The upstream's result is size-capped and returned; the call is audited", "An unhealthy upstream's tools disappear from the list rather than failing calls"],
                 practices: [
                    "Tool schemas are fetched from upstreams and cached; the client never talks to an upstream directly",
                    "Results are capped and args hashed in the audit, so the log holds no payloads",
                    "Scope denial is a normal response, logged the same as a success",
                 ] + base,
                 brief: "The backend already runs: /mcp speaking MCP streamable HTTP (initialize, ping, tools/list, tools/call, proper JSON-RPC errors, Mcp-Session-Id), an upstream MCP client with cached tool schemas, a registry with per-caller scopes (server:* / server:tool), tool names as server__tool, request and result size caps, timeouts, required-argument checks, hidden tools for unhealthy upstreams, an audit with args hash and result size, and a console page — tested against a fake upstream MCP server. Read backend/src/app.ts, upstream.ts and the tests first. Then extend it: register this product's upstreams, add caller auth, and share session and health state through Redis across replicas. Run the gate when done.",
                 wants: nil),
        // ── ides & harnesses ──────────────────────────────────────────────────────────────
        Template(id: "harness", category: .harness, title: "Agent harness", like: "OpenHands, SWE-agent, Keel", icon: "terminal",
                 blurb: "Run coding agents against repositories with the tools you allow, the gate you define, and a record of every call.",
                 scaffold: "app",
                 components: [
                    .init(name: "Runner", role: "spawns the agent per task with a locked tool set", tech: "worker + child process"),
                    .init(name: "Tool server", role: "the only way the agent touches files or commands", tech: "MCP server"),
                    .init(name: "Gate", role: "the repo's own checks after every turn", tech: "make check"),
                    .init(name: "Trace store", role: "every tool call and result", tech: "Postgres"),
                    .init(name: "Review UI", role: "diffs, calls, verdicts, approve/deny", tech: "Next.js"),
                 ],
                 flow: ["A task names a repository and a goal", "The runner spawns the agent with only the tool server and no shell", "Each tool call is recorded and, if policy says so, held for approval", "The gate runs after each turn; its verdict is stored", "The review page shows the diff beside the calls and the verdict"],
                 practices: with(["The agent never gets a shell; every effect is a narrow typed tool with an audit record; secrets are injected per call, never into the context", "Approvals are questions the turn waits on; a question belongs to exactly one run", "A green diff is not done until the project's own gate says so — verification, not self-report", "Evals in CI: a golden task set graded by assertions first, judge second; a regression fails the build"]),
                 brief: "The backend already runs: a coding-agent run per task in a git checkout with only an in-process tool server (list, read, edit, run_gate, finish) and no shell, a policy that holds named tool calls for approval through the API (nobody answering is a denial), a trace of every call, the repository's check command run after every turn with the verdict and diff stored, and a review page with approve/deny beside the diff. Read backend/src/harness.ts, tools.ts and the tests first. Then extend it: add the tools this product's tasks need, real task sources, and the approval policy the team wants. Run the gate when done.",
                 wants: nil),
        Template(id: "sandbox", category: .harness, title: "Code-runner service", like: "E2B, Firecracker", icon: "cube.transparent",
                 blurb: "Execute untrusted code with limits, isolation and a clean result — the piece every agent product needs.",
                 scaffold: "app",
                 components: [
                    .init(name: "Run API", role: "submit code + inputs, get stdout/exit/timings", tech: "Hono"),
                    .init(name: "Executor", role: "isolated process per run, CPU/memory/time limits, no network by default", tech: "container per run"),
                    .init(name: "Queue", role: "runs wait their turn; concurrency capped", tech: "Redis"),
                    .init(name: "Artifacts", role: "outputs and files, expiring", tech: "object storage / Postgres"),
                    .init(name: "Playground", role: "try it, see limits", tech: "Next.js"),
                 ],
                 flow: ["A run is queued with a language, code and limits", "An executor claims it and starts an isolated container with the limits applied", "stdout, stderr, exit code and timings are captured; files are stored as artifacts", "The result is returned; the container is gone"],
                 practices: with(["No network unless a run asks and policy allows", "Limits enforced by the runtime, not the code", "Every run is logged with its limits and outcome"]),
                 brief: "The backend already runs: a Piston-shaped run API (POST /api/v2/execute and a queued POST /api/runs), an executor that builds a docker run argv with no network, memory, CPU, pids and read-only limits and a non-root user, captures stdout, stderr, exit code and timings, stores /work/out files as expiring artifacts, a fake executor so the gate needs no Docker, and a playground page. Read backend/src/executor.ts and its tests first. Then extend it: add the languages and base images this product needs, per-caller quotas, and move to gVisor or Firecracker when isolation must be stronger than a container. Run the gate when done.",
                 wants: nil),
        // ── agent builders ────────────────────────────────────────────────────────────────
        Template(id: "builder", category: .builders, title: "Agent builder platform", like: "Dify, Flowise, Langflow", icon: "wand.and.stars",
                 blurb: "Define agents from prompts, tools and skills; run them on schedules and channels; see every run.",
                 scaffold: "app",
                 components: [
                    .init(name: "Definitions", role: "agents, prompts, tools, skills, versions", tech: "Postgres"),
                    .init(name: "Runtime", role: "runs an agent version with its tools and budget", tech: "worker"),
                    .init(name: "Triggers", role: "schedules, webhooks, channels", tech: "cron + Hono"),
                    .init(name: "Run store", role: "every message, tool call, cost", tech: "Postgres"),
                    .init(name: "Builder UI", role: "edit, test, publish, watch runs", tech: "Next.js"),
                 ],
                 flow: ["An agent is edited as a draft and tested in place", "Publishing pins a version; triggers reference the version", "A trigger enqueues a run; the runtime executes it with the pinned tools", "Every step is stored; the UI shows runs live and lets you replay one"],
                 practices: with(["Versions are immutable; rollback is choosing an older one", "Tools are declared with schemas and tested in isolation", "Budgets per agent and per run"]),
                 brief: "The backend already runs: agent definitions with drafts and pinned published versions (runs snapshot the definition at enqueue), a runtime with a tool registry (now, calculator, http_get with a public-address guard), skills folded into the system prompt, step and token budgets, schedule and webhook triggers that fire once across workers, a run store with every message and tool call, and a builder page to edit, test, publish and watch runs. Read backend/src/runtime.ts and its tests first. Then extend it: add this product's tools and skills, per-user auth and workspaces, and the trigger sources the users need. Run the gate when done.",
                 wants: nil),
        // ── migrations ────────────────────────────────────────────────────────────────────
        Template(id: "bubble", category: .migrate, title: "Port from Bubble.io", like: "", icon: "arrow.right.doc.on.clipboard",
                 blurb: "Attach the Bubble export; data types, pages and workflows become code you own, one commit at a time.",
                 scaffold: "app",
                 components: [
                    .init(name: "Data types → tables", role: "one migration each, option sets as enums", tech: "Postgres"),
                    .init(name: "Pages → routes", role: "same fields, same intent", tech: "Next.js"),
                    .init(name: "Workflows → routes/jobs", role: "typed handlers or scheduled jobs", tech: "Hono"),
                    .init(name: "MIGRATION.md", role: "every element and where it went", tech: "Markdown"),
                 ],
                 flow: ["The export is read in full first", "Data types land as migrations, one commit each", "Pages and workflows follow, one commit each, gate green between", "Anything unmappable is listed, not guessed"],
                 practices: with(["Small commits, one Bubble element each", "Nothing silently dropped: the map is complete"]),
                 brief: "The attached file is an export of a Bubble.io application. The importer already runs: bun src/import/bubble.ts <file> parses the export (types, fields, option sets, pages, API workflows) and writes migrations/0003_bubble_types.sql, src/import/bubble.schemas.ts and MIGRATION.md; POST /api/import does the same over HTTP; and a Bubble Data-API-shaped CRUD (/api/obj/:type) serves the imported types immediately over a jsonb landing table with runtime validation. Run the importer on the attachment first and read MIGRATION.md. Then port what the importer cannot: each page to a Next.js route with the same fields and layout intent, each workflow to a typed Hono route or scheduled job, and move data from the landing table to the generated tables. Work in small commits, one type or page per commit; keep MIGRATION.md current. Run the gate after each.",
                 wants: "the Bubble export (.json)"),
        Template(id: "n8n", category: .migrate, title: "Port from n8n", like: "", icon: "arrow.triangle.branch",
                 blurb: "Attach n8n workflow JSON; every node becomes a function with a test, every trigger a route or a schedule.",
                 scaffold: "app",
                 components: [
                    .init(name: "Triggers → routes/jobs", role: "webhooks and schedules", tech: "Hono + cron"),
                    .init(name: "Nodes → functions", role: "named like the node, unit-tested", tech: "TypeScript"),
                    .init(name: "Credentials → env", role: "named, never written", tech: "environment"),
                    .init(name: "MIGRATION.md", role: "node → code map", tech: "Markdown"),
                 ],
                 flow: ["Workflows are read in full first", "Each trigger becomes an entry point", "Each node becomes a pure function with a test", "The map is written; nothing is guessed"],
                 practices: with(["Node names survive as function names", "Every transform has a test"]),
                 brief: "The attached file is one or more exported n8n workflows. The importer already runs: bun src/import/n8n.ts <file> parses nodes and connections, writes src/workflows/<name>.ts with one exported function per node (named for the node; unsupported ones throw, with the original source kept as a comment) and MIGRATION.md; POST /api/import (under ADMIN_TOKEN) registers each Webhook node at its n8n path and each schedule as a cron job; and the engine executes webhook, schedule, set, if, httpRequest, noOp and merge with the expression subset it supports — anything else fails loudly naming the node. Code nodes are never evaluated. Run the importer on the attachment first and read MIGRATION.md. Then port the unsupported nodes by hand into tested functions, set the named environment variables, and delete the engine once every workflow is plain code. Run the gate when done.",
                 wants: "the n8n workflow export (.json)"),
        // ── starting points ───────────────────────────────────────────────────────────────
        Template(id: "blank", category: .starting, title: "Blank", like: "", icon: "doc",
                 blurb: "Agent scaffolding only — a CLAUDE.md, a gate, the reviewer agents. No application code.",
                 scaffold: "empty", components: [], flow: [], practices: [], brief: "", wants: nil),
    ]
}
