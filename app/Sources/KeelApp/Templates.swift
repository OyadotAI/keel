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
        case control = "Control planes"
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

    /// The first prompt: the brief, with the architecture attached so the agent builds to it.
    var fullBrief: String {
        guard !brief.isEmpty else { return "" }
        return brief + "\n\nBuild to the architecture below, which is also in CLAUDE.md. Where the scaffold already provides a component, extend it rather than adding a second one.\n\n" + architecture
    }

    // The practices every stack-scaffolded template shares; listed once, appended to each.
    private static let base = [
        "Zero-downtime rollouts: maxUnavailable 0, preStop drain, startup/readiness/liveness probes, HPA owns replicas, PDB lets one drain",
        "Secrets only from the environment; committed only age-encrypted; images run as a non-root user",
        "Typed seam: the frontend client is built from the API's route types — a shape change fails typecheck, not production",
        "One JSON log line per event with a request id; errors to Sentry with context; cheap health endpoints",
        "make check is the gate — Keel runs it after every turn, CI before every image",
    ]
    private static func with(_ extra: [String]) -> [String] { extra + base }

    static let all: [Template] = [
        // ── backend & workers ─────────────────────────────────────────────────────────────
        Template(id: "api", category: .backend, title: "REST API service", icon: "point.3.connected.trianglepath.dotted",
                 blurb: "A typed HTTP API with keys, limits and generated docs — the service other things call.",
                 scaffold: "app",
                 components: [
                    .init(name: "API", role: "typed routes, validation, auth middleware", tech: "Hono on Node"),
                    .init(name: "Database", role: "resources, API keys (hashed), migrations", tech: "Postgres"),
                    .init(name: "Rate limiter", role: "per-key sliding window", tech: "Redis"),
                    .init(name: "Docs", role: "OpenAPI generated from the routes, rendered in the frontend", tech: "Next.js"),
                 ],
                 flow: ["Request arrives with an API key header", "Middleware hashes the key, loads its scopes and checks the Redis window", "zod validates the body against the route schema", "The handler runs one transaction and returns a typed response", "Errors return a stable shape with a request id; the same id is in the log line"],
                 practices: with(["Idempotency keys on every POST that creates something", "Cursor pagination, never offset", "Bounded body size and outbound timeouts on every route"]),
                 brief: "Make the backend a standalone API service: resources in the database with migrations and typed Hono routes, API-key auth with keys stored hashed, per-key rate limiting in Redis, request validation with zod, idempotency keys on creates, cursor pagination, an OpenAPI document generated from the routes, and a docs page in the frontend that renders it. Run the gate when done.",
                 wants: nil),
        Template(id: "jobs", category: .backend, title: "Webhooks & background jobs", icon: "arrow.triangle.2.circlepath.circle",
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
                 practices: with(["Idempotency on both the inbound event and the outbound side effect", "Workers scale on queue depth, separately from the API", "Every dead-lettered event is visible and replayable"]),
                 brief: "Build an event-processing backend: webhook endpoints that verify signatures (Stripe and GitHub as examples) and enqueue to Redis streams, a separate worker Deployment that consumes with idempotency keys in the database, exponential backoff and a dead-letter table, a leader-elected scheduler for cron jobs, and an admin page listing events by state with a replay action. Run the gate when done.",
                 wants: nil),
        Template(id: "pipeline", category: .backend, title: "Data pipeline", icon: "arrow.down.right.and.arrow.up.left",
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
                 practices: with(["Watermarks, not timestamps of now — reruns and late data are normal", "Raw is immutable; everything downstream is rebuildable from it", "Transforms are pure functions with tests"]),
                 brief: "Build a data pipeline: a batched ingest endpoint appending to an events table partitioned by day, a scheduled job with a watermark that transforms new partitions through pure, unit-tested functions and upserts daily aggregates, a backfill command for a date range, a metrics API over the aggregates, and a frontend page with a chart and a table. Seed sample data so it demos. Run the gate when done.",
                 wants: nil),
        Template(id: "realtime", category: .backend, title: "Realtime service", icon: "dot.radiowaves.left.and.right",
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
                 practices: with(["Sticky sessions at the ingress; reconnect with a cursor so a replica dying loses nothing", "Backpressure: slow consumers are dropped, not the room", "Tokens expire in seconds and are single-use"]),
                 brief: "Build a realtime service: a WebSocket endpoint with a room per channel, join tokens minted by an HTTP route from the session, ordered message history in the database with cursor-based resume, Redis pub/sub so rooms work across replicas, presence with TTL heartbeats, backpressure for slow clients, and a frontend page that joins a room and shows messages live. Run the gate when done.",
                 wants: nil),
        // ── full-stack ────────────────────────────────────────────────────────────────────
        Template(id: "fullstack", category: .fullstack, title: "Full-stack app", icon: "square.stack.3d.up",
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
                 brief: "",
                 wants: nil),
        Template(id: "auth", category: .fullstack, title: "Auth & users", icon: "person.badge.key",
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
                 practices: with(["Sessions rotate on privilege change; logout invalidates server-side", "Every auth event is in an audit table", "Rate limits on the magic-link endpoint"]),
                 brief: "Build the account layer: users and sessions in the database, email magic-link sign-in with signed single-use tokens stored hashed, HttpOnly session cookies with rotation, roles (owner, admin, member) enforced by middleware on Hono routes, personal API tokens hashed at rest, an audit table for auth events, rate limits on the sign-in endpoint, and account pages. No passwords stored, ever. Run the gate when done.",
                 wants: nil),
        Template(id: "tenant", category: .fullstack, title: "Multi-tenant SaaS", icon: "building.2",
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
                 practices: with(["Row-level scope by construction, tested with a cross-tenant read that must fail", "Invitations expire; membership changes are audited", "Billing state is derived from webhooks, never from the client"]),
                 brief: "Build a multi-tenant SaaS: organisations and memberships in the database, a scoped query layer where every tenant-table query requires the org id (with a test that a cross-tenant read fails), invitations that expire, plan limits enforced in middleware, Stripe webhook handling for subscription state, and org settings pages. Run the gate when done.",
                 wants: nil),
        Template(id: "internal", category: .fullstack, title: "Admin & internal tools", icon: "tablecells",
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
                 brief: "Build an internal tool: a data table with sorting, filtering, inline editing and CSV export over a database table, schema-driven forms with zod shared between frontend and backend, an approval workflow as an explicit state machine, an audit log written in the same transaction as every change, and roles (admin, editor, viewer). Run the gate when done.",
                 wants: nil),
        // ── agents ────────────────────────────────────────────────────────────────────────
        Template(id: "agent", category: .agents, title: "AI agent backend", icon: "sparkles",
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
                 practices: with(["Tool inputs validated; tools cannot reach the filesystem or shell", "Budgets before the call, usage after", "Every run is replayable from the stored messages"]),
                 brief: "Build an AI agent backend: a Hono endpoint that streams Claude API responses with tool use (a web-fetch tool and a notes tool backed by the database), a typed tool registry with per-tool timeouts, conversation history in the database with token counts, per-user daily budgets in Redis checked before each call, a system prompt editable from a settings page, and a minimal streaming chat UI. The API key comes from the environment, never the repository. Run the gate when done.",
                 wants: nil),
        Template(id: "orchestrator", category: .agents, title: "Multi-agent orchestrator", icon: "point.topleft.down.to.point.bottomright.curvepath",
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
                 practices: with(["Tasks are idempotent and resumable after a worker dies", "The judge never sees the worker's reasoning, only its output", "Budgets per run and per task"]),
                 brief: "Build a multi-agent orchestrator: a planner that turns a goal into a task graph stored in the database, a Redis-backed queue that releases tasks when their dependencies complete, a worker Deployment that runs tasks with scoped tools and budgets, a judge step with a cold context that checks each output against acceptance criteria and triggers re-planning of failed nodes, and a run viewer page showing the graph live. Run the gate when done.",
                 wants: nil),
        Template(id: "evals", category: .agents, title: "Evaluation harness", icon: "checklist",
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
                 brief: "Build an evaluation harness: versioned datasets of cases in the database, a worker that runs a prompt and model configuration over a dataset with a concurrency cap and retries, graders (exact match, rubric, and a cold-context model judge), per-case results with score, cost and latency, and a leaderboard page comparing runs with a per-case drill-down. Run the gate when done.",
                 wants: nil),
        // ── control planes ────────────────────────────────────────────────────────────────
        Template(id: "controlplane", category: .control, title: "Control plane", icon: "slider.horizontal.3",
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
                 practices: with(["Reconcile is idempotent and level-triggered — it can run any number of times", "Optimistic concurrency on writes (generation / etag)", "Every action is an event you can read"]),
                 brief: "Build a control plane: declarative resources with generations and optimistic concurrency in the database, a worker reconciler loop that diffs desired against observed state through provider adapters and requeues failures with backoff, status and event records per resource, and a console page showing resources, status and events. Include one real provider adapter (for example, managing records in a table) and a fake one for tests. Run the gate when done.",
                 wants: nil),
        Template(id: "flags", category: .control, title: "Feature flags & config", icon: "switch.2",
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
                 practices: with(["Evaluation is local and deterministic", "Every change is attributable and reversible", "Percentage rollouts hash a stable key"]),
                 brief: "Build a feature-flag and config service: flags with rules, segments and percentage rollouts stored versioned in the database with an audit table, a snapshot per environment published through Redis and streamed to clients over SSE, a small TypeScript SDK that evaluates in memory and keeps the last snapshot on disconnect, and a console page to edit, target, roll out and kill. Run the gate when done.",
                 wants: nil),
        // ── proxies & gateways ────────────────────────────────────────────────────────────
        Template(id: "gateway", category: .proxies, title: "API gateway", icon: "arrow.left.arrow.right",
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
                 practices: with(["Fail fast on a dead upstream; never queue behind it", "Retries only for idempotent methods", "Route changes are versioned and atomic"]),
                 brief: "Build an API gateway: a Hono service that matches requests to a versioned route table (stored in the database, snapshotted to Redis), enforces API-key auth and per-key/per-route rate limits before forwarding, forwards with timeouts, a request id and retries only for idempotent methods, opens a circuit on repeated upstream failures, logs every request with timing, and a console page for routes, keys and live traffic. Run the gate when done.",
                 wants: nil),
        Template(id: "llmproxy", category: .proxies, title: "LLM proxy", icon: "brain",
                 blurb: "One endpoint in front of every model provider: routing, caching, budgets, and logs you own.",
                 scaffold: "app",
                 components: [
                    .init(name: "Proxy", role: "OpenAI-compatible in, provider-specific out, streaming", tech: "Hono"),
                    .init(name: "Router", role: "model → provider, fallbacks, weights", tech: "config in the database"),
                    .init(name: "Cache", role: "exact and semantic hits", tech: "Redis"),
                    .init(name: "Budgets", role: "per-key spend, hard stops", tech: "Redis + Postgres"),
                    .init(name: "Logs", role: "every request, tokens, cost, latency", tech: "Postgres"),
                 ],
                 flow: ["A client calls the proxy as if it were a provider", "The router picks the provider; a cache hit returns immediately", "Budget checked; the call is streamed through with the provider key from the environment", "Usage and cost recorded; a provider error falls back to the next"],
                 practices: with(["Provider keys never leave the proxy", "Budgets stop spend before it happens", "Fallbacks are ordered and observable"]),
                 brief: "Build an LLM proxy: an OpenAI-compatible streaming endpoint that routes model names to providers (Anthropic and OpenAI adapters) with ordered fallbacks, exact-match response caching in Redis, per-key budgets with hard stops, request logs with tokens, cost and latency in the database, provider keys only from the environment, and a console page with spend by key and recent requests. Run the gate when done.",
                 wants: nil),
        // ── ides & harnesses ──────────────────────────────────────────────────────────────
        Template(id: "harness", category: .harness, title: "Agent harness", icon: "terminal",
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
                 practices: with(["The agent never gets a shell; every effect is a tool", "Approvals are questions the turn waits on", "A green diff is not done until the gate says so"]),
                 brief: "Build an agent harness: a worker that runs a coding agent per task in a checkout with only an MCP tool server (read, edit, run-gate) and no shell, a policy that holds named tool calls for approval, a trace table of every call and result, the repository's own check command run after every turn with its verdict stored, and a review page showing the diff beside the calls, the verdict, and approve/deny buttons. Run the gate when done.",
                 wants: nil),
        Template(id: "sandbox", category: .harness, title: "Code-runner service", icon: "cube.transparent",
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
                 brief: "Build a code-runner service: a Hono run API that queues code with limits, an executor worker that runs each job in an isolated container with CPU, memory and time limits and no network by default, captures stdout, stderr, exit code and timings, stores output files as expiring artifacts, and a playground page. Include a fake executor for tests so the gate needs no Docker. Run the gate when done.",
                 wants: nil),
        // ── agent builders ────────────────────────────────────────────────────────────────
        Template(id: "builder", category: .builders, title: "Agent builder platform", icon: "wand.and.stars",
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
                 brief: "Build an agent builder platform: versioned agent definitions (prompt, tools, skills) in the database with draft and published states, a worker runtime that executes a pinned version with its tools and a budget, triggers (schedules and webhooks) that enqueue runs, a run store with every message and tool call, and a builder UI to edit, test, publish and watch runs live. Run the gate when done.",
                 wants: nil),
        // ── migrations ────────────────────────────────────────────────────────────────────
        Template(id: "bubble", category: .migrate, title: "Port from Bubble.io", icon: "arrow.right.doc.on.clipboard",
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
                 brief: "The attached file is an export of a Bubble.io application. Read it fully first. Then: map every data type to a table with a migration, every page to a Next.js route with the same fields and layout intent, every workflow to a typed Hono route or a scheduled job, and every option set to a TypeScript enum. Write a MIGRATION.md listing each Bubble element and where it went, and anything you could not map. Work in small commits, one data type or page per commit. Run the gate after each.",
                 wants: "the Bubble export (.json)"),
        Template(id: "n8n", category: .migrate, title: "Port from n8n", icon: "arrow.triangle.branch",
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
                 brief: "The attached file is one or more exported n8n workflows. Read them fully first. Then rebuild each workflow as code: triggers become Hono routes or scheduled jobs, HTTP nodes become typed fetch calls, transforms become plain functions with unit tests, and credentials become environment variables that are named but never written. Keep the node names as function names so the two line up. Write a MIGRATION.md mapping every node to its code. Run the gate when done.",
                 wants: "the n8n workflow export (.json)"),
        // ── starting points ───────────────────────────────────────────────────────────────
        Template(id: "blank", category: .starting, title: "Blank", icon: "doc",
                 blurb: "Agent scaffolding only — a CLAUDE.md, a gate, the reviewer agents. No application code.",
                 scaffold: "empty", components: [], flow: [], practices: [], brief: "", wants: nil),
    ]
}
