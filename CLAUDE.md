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

## The one hook Keel ships

`keel-harness::trust` quarantines a repository's `.claude/settings.json` and the scanner rates it
Critical, because a hook there is a shell command that runs on the machine of whoever opens the
repo. Keel then passes a `PreToolUse` hook of its own in `--settings`, and the two are not in
tension:

- Keel's hook is in the settings Keel writes and passes on the command line. It is never read from
  the working tree, so nothing a repository contains can change it.
- It points at Keel's own binary and does one thing: ask the running Keel whether the person
  approves this call, and block until they answer.
- It fails open. Keel unreachable, socket dropped, nobody at the keyboard — every path prints
  nothing and exits 0, which defers to the allowlist. A guardrail that can wedge the agent is one
  people turn off.

This is what makes an approval a *question* rather than a notification. Before it, a refused
command came back as an error, the turn carried on without it, and the person's click added a rule
and asked the agent to retry — by which point it had usually worked around the gap. Verified end to
end: the call blocks, approving lets that same call proceed, denying returns a reason and the agent
stops rather than substituting.

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
- `backend/` — Hono, its own Worker.
- `infra/` — the deploy script and the environment map.

The frontend reaches the backend through a **service binding**, so the call never leaves
Cloudflare and the backend needs no public route.

**The seam is a type, not a document.** The API exports the type of its route table and the
frontend builds its client from it — no generated SDK, no schema file. That is the whole reason
both halves are TypeScript, and it is verified by deliberately asking for a field the API does not
return and checking that the frontend stops compiling.

That puts a shape requirement on the API: Hono infers the route table from one chained expression.
Assigning routes to `app` one at a time still runs, still passes the API's own tests, and silently
degrades every frontend call to `any`. It is a rule in the generated `CLAUDE.md` and a test here.

**Go was tried and dropped.** Workers run JS, TS, Python and Rust, so a Go backend needs a
Cloudflare Container or a second cloud. The container worked, but it brought manual instance
counts, ephemeral disk and cold starts on wake — and none of that buys anything a Hono Worker
does not already do for this template.

Generated projects are verified by generating one and running its own gate, not by asserting on
strings alone. Three bugs only that catches: `NextConfig` dropped `eslint` in Next 16; passing
bindings to `app.request` drops Hono's typed-response overload, so `json()` widens in tests in a
way it does not in the frontend; and, from the container version, `@cloudflare/containers` was on
0.3.x rather than the version first written. Each would have shipped a project that fails its own
first `make check`.

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
