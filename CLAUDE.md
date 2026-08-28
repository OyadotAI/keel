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
7. **Listing sessions never shows what was said.** `discover_sessions` runs constantly to populate
   the switcher and returns titles, counts and timestamps only — reading a transcript to render a
   list is not licence to display it. `transcript()` is the separate, explicit path for opening one
   session the user asked for by name, and it rejects any id that could climb out of the project
   directory. Both asserted by test.

## Layout

- `keel-scanner` — checks. Depends on nothing else in the workspace, touches no network. Keep it
  that way: it ships before any credential exists.
- `keel-harness` — `claude` supervision and trust quarantine.
- `keel-mcp` — the tool surface.
- `keel-providers` — GitHub, Cloudflare.
- `keel-generator` — golden-path templates and workload placement.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and never surfaces session message bodies.

## What a new project looks like

Three folders, because the halves have genuinely different constraints:

- `frontend/` — Next.js + React, compiled to a Worker by OpenNext.
- `backend/` — Go, in a Cloudflare Container behind a Worker that owns the Durable Object.
- `infra/` — the deploy script and the environment map.

The frontend reaches the backend through a **service binding**, so the call never leaves
Cloudflare and the backend needs no public route.

**Go is not a Workers language.** Workers run JS, TS, Python and Rust; the WASM shim for Go calls
itself experimental. So a Go backend is either a Container or a second cloud, and a second cloud
is a second account, token, dashboard and tracing backend — the four things dropping Kubernetes
was meant to delete. The Container wins on that, and the bill is stated in the generated
`infra/README.md` rather than buried: no autoscaling, ephemeral disk, cold start on wake. When a
service outgrows those, the honest answer is Cloud Run and a second credential, not a bigger
`max_instances`.

Generated projects are verified by generating one and running its own gate, not by asserting on
strings alone. Two bugs that only that catches: `NextConfig` dropped `eslint` in Next 16, and
`@cloudflare/containers` is on 0.3.x. Both would have shipped a project that fails its first
`make check`.

## The editor

Monaco is vendored in `ui/vendor` and embedded with `rust-embed`. It is the full `min/vs` bundle on
purpose: the AMD graph in `editor/editor.main.js` depends on `language/*`, and trimming those
modules makes the loader fail silently with a blank editor and nothing in the console. If you need
to shrink it, drop files under `assets/*.worker.js` (language services) — never `language/`.

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
