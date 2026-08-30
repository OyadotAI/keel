# keel — production checklist

Derived by Keel from this repository on adoption; edit it to fit, and keep it true. The reviewers under `.claude/agents/` enforce it.

| | |
|---|---|
| Languages | rust |
| Stack | axum, tracing |
| Runs on | Cloudflare + containers |
| Gate | `make check` |
| Closest template | REST API service (like PostgREST, Directus) |

## The gate

`make check` is the one command that says whether the project is good. Keel runs it after every turn; CI runs it before every build; nobody says "done" without it. For Rust that is `cargo clippy --all-targets -- -D warnings && cargo test`.

## Where it stands

Readiness 78/100 when this file was written. In order:

1. **Make it safe** — Nothing else matters while someone can run code on your machine or read a secret from the repository.
   - 6 actions pinned to a tag, not a commit — Pin each `uses:` to a commit SHA with the version in a comment (`actions/checkout@<sha> # v4`), and let Dependabot's `github-actions` ecosystem keep them current.
2. **Make it checkable** — One command that says whether it is good. Every later step is verified by this one.
   - No reviewer subagents — Add `.claude/agents/{reviewer,security,reliability}.md` and the production checklist they enforce (`docs/PRODUCTION.md`). Keel writes these without touching anything else.
3. **Make it deployable the same way twice** — An image, a compose file for the laptop, manifests for the cluster, dev and prod apart, a pipeline that does it — so production is not a person.
   - Nothing deploys this automatically — Add `.github/workflows/deploy-dev.yaml` (on push to main) and `deploy-prod.yaml` (on tag): test, build the image to ghcr, apply the overlay, roll only what changed. The templates ship both.
   - No .dockerignore — Add `.dockerignore` with node_modules, .git, .env*, coverage, .next/cache.
4. **Make it survive a bad day** — Rollouts that drop nothing, a schema the repository can recreate, limits that keep one client from being an outage.
   - No rate limiting on the API — Add a per-key or per-IP limit in middleware (in-process to start, Redis once there are replicas) answering RateLimit-* and Retry-After; the `api` template has it.

## Change discipline

- Small commits, one thing each, conventional messages. A change appends to `CHANGELOG.md`: past tense, what and why.
- Prefer a dependency the project already has over a new one.
- Config comes from the environment and every variable is in `.env.example` with a comment. Secrets are never in the repository.
- Logs go through `tracing` with a JSON subscriber in production and `request_id` on every span.

## Production rules

Checked by the `reliability` and `security` reviewers; argue with the rule here, not in a PR.

- Handlers are stateless; session and rate-limit state live in Redis or a signed cookie, never in process memory once there are replicas.
- Every outbound call has a timeout shorter than its caller's, and the deadline travels in a header. Retries: bounded, exponential backoff with jitter, idempotent operations only.
- A mutating route a client may retry takes `Idempotency-Key`, stored with the request hash and the response. Exactly-once is at-least-once plus an idempotent consumer.
- "Write the row and publish the event" is one transaction through an outbox; every consumer has a dead-letter path and a replay tool.
- Pagination is keyset, capped at 100. IDs are UUIDs (v7 where the database sorts on them).
- Inputs are validated once at the boundary with a schema; unknown fields are rejected; bodies are bounded. Between services: short-lived signed tokens with an audience.
- Rate limits per principal (`429` + `RateLimit-*`); auth endpoints also lock out.
- Three probes with three meanings: startup, readiness (checks dependencies), liveness (never does). Drain on SIGTERM; a `preStop` sleep so endpoints are removed first.
- Circuit breakers and per-dependency concurrency limits on every external call; degrade with a typed "unavailable", never a 500.
- Images run as a non-root user, pinned base, `.dockerignore`, tagged by commit SHA — `latest` is an alias, never what a cluster pulls.
- CI: `permissions: contents: read` by default, actions pinned to SHAs, a concurrency group, a timeout on every job, and no deploy that does not `needs:` the gate. Production deploys from a tag or a dispatch into a protected environment.
- Alerts fire on SLO burn rate and link to a runbook. Backups are only real once restored.

## Deploying

Not yet described in the repository. Target: an image, a compose file for the laptop, manifests for the runtime, dev and prod apart, a pipeline that does it.
