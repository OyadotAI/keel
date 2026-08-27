# Keel — working agreement

Keel is a local IDE that takes an existing repository, reports how ready it is for agent work and
production deployment, fixes what's missing, provisions Cloudflare, and owns the ship-and-observe
loop. Cloudflare only. No Kubernetes.

## Non-negotiables

These are enforced by tests. Changing any of them is a deliberate decision, not a refactor.

1. **The agent never gets a shell.** `--permission-mode dontAsk` + `--strict-mcp-config`; built-in
   `Bash`/`Edit`/`Write` stay denied. Every effect passes through a `keel-mcp` tool.
2. **`--bare` is never passed.** It would break subscription auth ("OAuth and keychain are never
   read"). Because of that, repo `.claude/settings.json` hooks load — so `keel-harness::trust`
   quarantines them *before* the first invocation.
3. **Deploy tools take an explicit `env`, never a default.**
4. **Dev and prod never share a stateful binding.**
5. **Promotion redeploys the proven artifact**, never rebuilds.
6. **Stop sends SIGINT**, not SIGTERM. SIGTERM abandons the turn.
7. **Session transcripts are summarised, never displayed.** Titles, counts and timestamps only.
   A transcript holds everything the user ever said in that repo; reading one to render a list is
   not a licence to show it. Asserted by test.

## Layout

- `keel-scanner` — checks. Depends on nothing else in the workspace, touches no network. Keep it
  that way: it ships before any credential exists.
- `keel-harness` — `claude` supervision and trust quarantine.
- `keel-mcp` — the tool surface.
- `keel-providers` — GitHub, Cloudflare.
- `keel-generator` — golden-path templates and workload placement.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and never surfaces session message bodies.

## Conventions

- Rust 2024, `cargo fmt`, `clippy -D warnings`. `make check` is the gate.
- Every scanner finding must carry a `Fix`. A finding without one is a bug — it turns the report
  into a lint run nobody acts on.
- Check ids (`security/untrusted-agent-config`) are stable once shipped. Users and CI pin to them.
- Scoring is a plain total so it is predictable. Corpus tests assert exact scores; if you change
  penalties, that is a visible reviewed change.
- Comments explain *why*, especially where a platform constraint drove the design. The Cloudflare
  ceilings encoded here are the product's real asset.

## Platform facts that drive the design

- **Container disk is ephemeral** — resets to the image on every restart, `sleepAfter` 10 min
  default. Durable state never goes there.
- **D1 is single-writer at ~50 writes/sec.** Above that, Hyperdrive to a managed Postgres.
- **KV is eventually consistent**, up to 60s propagation.
- **Cloudflare has no OIDC/keyless deploy** as of Aug 2026 — scoped, rotated API tokens instead.

## Verification

`make check`. Dogfood with `make scan`, and against `../A2ABaseAI` for a repo with real CI and tests.
