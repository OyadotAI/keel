//! Creating a new project.
//!
//! A new project is scaffolded from the golden path rather than left empty: the whole point of Keel
//! is that a repository arrives already shippable, so a project it creates itself should start at a
//! passing readiness score rather than at the same findings every empty directory produces.
//!
//! # The three folders
//!
//! `frontend/` is Next.js on Workers, `backend/` is Go, `infra/` is how both of them ship. The
//! split is not organisational tidiness — it is the seam where the two halves have genuinely
//! different constraints, and putting them in one folder hides that.
//!
//! # Why the backend is its own Worker rather than Next.js API routes
//!
//! Two Workers cost almost nothing on Cloudflare and buy three things a single Next.js app cannot
//! have: the backend deploys on its own schedule, it is reachable by things that are not the
//! website, and the frontend talks to it over a service binding — an internal call with no public
//! route, no CORS and no second TLS hop.
//!
//! The seam is typed rather than documented. The backend exports the type of its route table, and
//! the frontend builds its client from that type, so a route that changes shape breaks the
//! frontend at compile time instead of at runtime. That is the whole argument for TypeScript on
//! both sides, and it is worth more than the language being the same.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

#[derive(Deserialize)]
pub struct NewProject {
    /// Directory to create the project in.
    pub parent: String,
    pub name: String,
    /// `app` for the full three-folder stack, `empty` for just the agent scaffolding.
    pub template: Option<String>,
}

#[derive(Serialize)]
pub struct Created {
    pub path: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Template {
    /// frontend (Next.js on Workers) + backend (Hono on Workers) + infra.
    App,
    /// Agent scaffolding and nothing else, for a project that brings its own stack.
    Empty,
}

fn expand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => std::env::var("HOME")
            .map(|h| format!("{h}{rest}"))
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

/// Names that are safe as a directory and as a Worker name.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewProject>,
) -> Result<Json<Created>, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());

    if !valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens — the name becomes a directory, two Worker \
             names.",
        ));
    }

    let parent = Utf8PathBuf::from(expand(&req.parent));
    if !parent.is_dir() {
        return Err(bad(&format!("no such directory: {parent}")));
    }
    let root = parent.join(&req.name);
    if root.exists() {
        return Err(bad(&format!("{root} already exists")));
    }

    std::fs::create_dir_all(&root).map_err(|e| bad(&e.to_string()))?;

    let template = match req.template.as_deref() {
        Some("empty") => Template::Empty,
        _ => Template::App,
    };
    for (rel, body) in scaffold(&req.name, template) {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(&e.to_string()))?;
        }
        std::fs::write(&path, body).map_err(|e| bad(&e.to_string()))?;
    }

    // The deploy script is the one file that is useless without the executable bit.
    #[cfg(unix)]
    if template == Template::App {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            root.join("infra/deploy.sh"),
            std::fs::Permissions::from_mode(0o755),
        );
    }

    // A repository from the start, so the diff view and the readiness scan both have a baseline.
    let _ = std::process::Command::new("git")
        .current_dir(&root)
        .arg("init")
        .arg("--quiet")
        .output();

    state.set_repo(root.clone());
    Ok(Json(Created {
        path: root.to_string(),
    }))
}

/// The files a new project starts with.
fn scaffold(name: &str, template: Template) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);

    let mut files: Vec<(&'static str, String)> = vec![
        ("CLAUDE.md", f(CLAUDE_MD)),
        (".gitignore", GITIGNORE.to_string()),
        ("README.md", f(README_MD)),
    ];

    if template == Template::Empty {
        return files;
    }

    files.extend([
        ("Makefile", MAKEFILE.to_string()),
        (".github/workflows/deploy.yml", DEPLOY_YML.to_string()),
        // ── infra ────────────────────────────────────────────────────────────────────────────
        ("infra/README.md", f(INFRA_README)),
        ("infra/deploy.sh", DEPLOY_SH.to_string()),
        // ── the agent's own setup ────────────────────────────────────────────────────────────
        (".claude/agents/reviewer.md", REVIEWER_AGENT.to_string()),
        (".claude/agents/platform-limits.md", LIMITS_AGENT.to_string()),
        // ── frontend ─────────────────────────────────────────────────────────────────────────
        ("frontend/package.json", f(FRONT_PACKAGE_JSON)),
        ("frontend/wrangler.jsonc", f(FRONT_WRANGLER)),
        ("frontend/tsconfig.json", FRONT_TSCONFIG.to_string()),
        ("frontend/next.config.ts", NEXT_CONFIG.to_string()),
        ("frontend/open-next.config.ts", OPEN_NEXT_CONFIG.to_string()),
        (
            "frontend/cloudflare-env.d.ts",
            CLOUDFLARE_ENV_DTS.to_string(),
        ),
        ("frontend/app/layout.tsx", f(APP_LAYOUT)),
        ("frontend/app/page.tsx", f(APP_PAGE)),
        ("frontend/app/globals.css", APP_CSS.to_string()),
        // ── backend ──────────────────────────────────────────────────────────────────────────
        ("backend/package.json", f(BACK_PACKAGE_JSON)),
        ("backend/wrangler.jsonc", f(BACK_WRANGLER)),
        ("backend/tsconfig.json", BACK_TSCONFIG.to_string()),
        ("backend/src/index.ts", BACK_INDEX.to_string()),
        ("backend/src/index.test.ts", BACK_TEST.to_string()),
    ]);

    files
}

// ═══ the agent's own setup ═══════════════════════════════════════════════════════════════════
//
// Two subagents, not five. Each earns its place by doing something a fresh context does better
// than the agent that just wrote the code, which is the only reason to spend a delegation on it.
//
// What is deliberately absent: `.claude/settings.json` and `.mcp.json`. Both are repository-
// supplied agent configuration, both are quarantined by `keel-harness::trust` before any agent
// runs, and a scaffold that ships one would hand every new project a Critical finding on its first
// scan. Hooks are the same story and worse — a shell command that runs on somebody else's machine
// when they open the repo.

const REVIEWER_AGENT: &str = r#"---
name: reviewer
description: "Use before reporting a change as done, after the gate is green. Reads the diff against what was asked and reports only what is actually wrong."
tools: Read, Grep, Glob, Bash
---

You review a change you did not write.

That is the entire reason you exist: an author checking their own work confirms it rather than
verifies it, and the measurements on this are not close. You have a fresh context, so read the code
rather than a description of it.

## What to do

1. Read the diff — `git diff` for unstaged work, `git diff HEAD` to include staged.
2. Read enough of the surrounding files to know what the changed code is called by and what it
   calls. A diff alone hides every caller it broke.
3. Run the gate yourself. Green is a fact, not a claim, and you can check it in one command.

## What to report

Only things that are wrong. For each: the file and line, what breaks, and the input or state that
makes it break. If you cannot name the case that fails, you have found a preference, not a bug, and
it does not go in the list.

An empty list is a valid and common result. Say so plainly and stop. You are not scored on how much
you find, and a reviewer that always finds something is a reviewer nobody reads twice.

## What not to report

Style, naming, formatting, or how you would have written it. Anything the gate already checks.
Anything outside the diff — if the change is correct and the file around it was already wrong, that
is a different piece of work and saying so buries the answer to the question you were asked.
"#;

const LIMITS_AGENT: &str = r#"---
name: platform-limits
description: "Use when adding storage, a queue, a binding, or anything that holds state. Checks the design against Cloudflare's real ceilings before it is built on."
tools: Read, Grep, Glob
---

You check where state is about to live against what the platform actually does.

These limits are not obscure. They are the ones that are fine in development, fine at launch, and
then are not — which is the worst time to find them.

## The ceilings

- **D1** is single-writer at roughly 50 writes/second, 10 GB. Fine for application data with human
  write rates. Not fine for per-request logging, event ingestion, counters, or anything a queue
  feeds. Past that the answer is Hyperdrive to a managed Postgres, not a bigger D1.
- **KV** is eventually consistent, up to 60 seconds. Fine for config, flags and cached reads. Never
  for anything read back immediately after writing — a session you write then read on the next
  request will not be there.
- **Durable Objects** are single-writer per object and strongly consistent, 10 GB each. This is the
  answer for per-tenant state, and the wrong answer for anything global, because every request for
  that object queues behind the last one.
- **R2** for objects and files, with no egress cost. Anything over a few hundred KB belongs here
  rather than in a database row.
- **Workers** get 128 MB of memory. Reading a large file into a buffer is the usual way to find out.
- **Queues** for anything that does not have to finish inside the request.

## What to report

Name the specific thing being stored, the write rate or size you expect, and whether the chosen
target holds. If it does, say so in one line and stop. If it does not, say which ceiling it hits
first and what to use instead.

Do not repeat the list above back. The person asking has it.
"#;

// ═══ root ════════════════════════════════════════════════════════════════════════════════════

const CLAUDE_MD: &str = r#"# {{NAME}}

Three folders, because the two halves of this app have different constraints and hiding that
behind one folder is how the constraints get violated.

| Folder | What it is | Where it runs |
|---|---|---|
| `frontend/` | Next.js + React | A Cloudflare Worker, built by OpenNext |
| `backend/` | Hono API | A Cloudflare Worker |
| `infra/` | Deploy scripts and the environment map | Nowhere — it is how the other two ship |

The frontend reaches the backend through a **service binding**, not a public URL. The call never
leaves Cloudflare's network, so the backend needs no public route and no CORS.

The seam between them is typed, not documented. `backend/src/index.ts` exports `AppType`, the
frontend builds its client from it, and a route that changes shape breaks the frontend at compile
time. There is no generated client to regenerate.

## Rules that are not style preferences

- **Routes stay chained** in `backend/src/index.ts`. Assigning them one at a time to `app` throws
  away the type the frontend depends on, and nothing fails loudly when it happens.
- **Dev and prod never share a binding.** Each environment has its own Workers and its own
  resources. Pointing dev at a prod database is the failure this layout exists to prevent.
- **Nothing is added to the frontend that the backend should own.** A Next.js route handler is the
  easy place to put an endpoint and the wrong one: it cannot be called by a cron trigger, a
  webhook or anything that is not the website.
- Explain *why* in comments, not what. The code already says what.

## Verification

`make check` is the gate. It typechecks both halves and runs the API's tests, and it is the whole
definition of "done" here — a change that has not been through it is not finished, whoever or
whatever wrote it.

Run it before saying a change works. Never report a result you have not seen: "the tests pass"
means you ran them and read the output.

## Subagents

Two, in `.claude/agents/`, and both exist because a fresh context does something the author of a
change cannot:

- **reviewer** — run before reporting work as done, after the gate is green. It reads the diff
  against what was asked. Self-review confirms rather than verifies; this is the mitigation.
- **platform-limits** — run before building on any new place state will live. Cloudflare's real
  ceilings (D1's write rate, KV's 60-second propagation, a Durable Object's single writer) are all
  fine right up until they are not.

## Running it

`make dev` runs the Next.js dev server. `cd backend && bun run dev` runs the API on its own.

## What not to add here

`.claude/settings.json`, `.claude/hooks/` and `.mcp.json` are repository-supplied agent
configuration: they execute on the machine of whoever opens this repo. Keel quarantines them before
any agent runs and the scan rates them Critical. If this project needs an MCP server or a
permission rule, it belongs in the user scope, not committed here.
"#;

const README_MD: &str = r#"# {{NAME}}

```
frontend/   Next.js + React, deployed as a Cloudflare Worker
backend/    Hono API, deployed as a Cloudflare Worker
infra/      how both of them reach dev and prod
```

## Develop

```
make dev                    # Next.js on :3000
cd backend && bun run dev   # the API on :8787
```

## Check

```
make check
```

## Ship

```
infra/deploy.sh dev    # deploy both halves to dev
infra/deploy.sh prod   # upload a prod version and print its preview URL
```

Promotion to production is a separate, gated step — see `infra/README.md`.
"#;

const GITIGNORE: &str = "node_modules/\n.next/\n.open-next/\n.wrangler/\ndist/\n.DS_Store\n\n\
     # Never commit real values.\n.env\n.env.*\n!.env.example\n\n\
     # Repository-supplied agent config, quarantined by `keel trust`.\n.keel/quarantine/\n";

// Recipes need real tabs, so this one is not a raw string.
const MAKEFILE: &str = "# The gate. Keel and CI both run `make check`, so there is exactly one\n\
     # command to keep green and one place to change what it means.\n\
     .PHONY: check check-frontend check-backend dev clean\n\n\
     check: check-frontend check-backend\n\n\
     # The frontend has no tests of its own yet — it is a page that renders what the API returns,\n\
     # and the typecheck already proves it agrees with the API about the shape of that.\n\
     check-frontend:\n\
     \tcd frontend && bun run typecheck\n\n\
     check-backend:\n\
     \tcd backend && bun run typecheck && bun test\n\n\
     dev:\n\
     \tcd frontend && bun run dev\n\n\
     clean:\n\
     \trm -rf frontend/.next frontend/.open-next\n";

const DEPLOY_YML: &str = r#"name: deploy

on:
  pull_request:
  push:
    branches: [main]

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
        working-directory: frontend
      - run: bun install --frozen-lockfile
        working-directory: backend
      - run: make check

  deploy-dev:
    if: github.ref == 'refs/heads/main'
    needs: verify
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
        working-directory: frontend
      - run: bun install --frozen-lockfile
        working-directory: backend
      - run: infra/deploy.sh dev
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}

  # Production is deliberately absent. Promotion happens through Keel, which shows the commit
  # diff, the binding diff and any pending migrations before a human approves it — and then
  # deploys the version that already passed dev rather than building a new one.
"#;

// ═══ infra ═══════════════════════════════════════════════════════════════════════════════════

const INFRA_README: &str = r#"# Infrastructure

Two Workers per environment, four in total.

```
              ┌──────────────────────────┐        ┌───────────────────────────┐
  internet ──▶│ {{NAME}}-web-dev         │──API──▶│ {{NAME}}-api-dev          │
              │ Next.js, built by OpenNext│ bind  │ Hono                       │
              └──────────────────────────┘        └───────────────────────────┘

              ┌──────────────────────────┐        ┌───────────────────────────┐
  internet ──▶│ {{NAME}}-web-prod        │──API──▶│ {{NAME}}-api-prod         │
              └──────────────────────────┘        └───────────────────────────┘
```

`API` is a service binding. The frontend calls the backend over Cloudflare's internal network, so
the backend has no public route, no CORS configuration and no second TLS hop.

## Why the API is its own Worker

A Next.js route handler is the easy place to put an endpoint. It is also only reachable by the
website: a cron trigger, a webhook, a mobile client or a partner integration cannot call it
without going through the frontend's rendering stack.

A second Worker costs almost nothing on Cloudflare and separates the two lifecycles. The API can
ship without rebuilding the site, and the site can ship without redeploying the API.

The seam is typed. `backend/src/index.ts` exports `AppType` — the type of its whole route table —
and the frontend builds its client from it. No generated SDK, no schema file, no drift: change a
route and the frontend stops compiling.

## Deploying

```
infra/deploy.sh dev
```

Backend first. That order is not cosmetic: the frontend holds a service binding to the backend
Worker, and a binding to a Worker that does not exist yet fails the deploy.

```
infra/deploy.sh prod
```

For prod this uploads a **version** and prints its preview URL. It does not route traffic. That is
the point: the artifact is built once, checked at its preview URL, and then the same artifact is
promoted. A promotion that rebuilds is how dev and prod silently diverge.

## Secrets

`wrangler secret put NAME --env dev` from inside `backend/` or `frontend/`. Secrets are per Worker
and per environment; there is no shared store, on purpose.
"#;

const DEPLOY_SH: &str = r#"#!/usr/bin/env bash
# Deploy one environment.
#
# Backend first, always: the frontend Worker declares a service binding to the backend Worker, and
# binding to a Worker that does not exist yet is a deploy-time failure, not a runtime one.
set -euo pipefail

env="${1:-}"
case "$env" in
  dev|prod) ;;
  *) echo "usage: infra/deploy.sh dev|prod" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"

if [ "$env" = "prod" ]; then
  # Production gets a version, not a deployment. Uploading gives the artifact a preview URL to
  # check against; promoting that exact artifact is a separate, human-approved step.
  (cd "$root/backend"  && bunx wrangler versions upload --env prod)
  (cd "$root/frontend" && bunx opennextjs-cloudflare build && bunx wrangler versions upload --env prod)
  echo
  echo "Uploaded prod versions. Check their preview URLs, then promote from Keel."
  exit 0
fi

(cd "$root/backend"  && bunx wrangler deploy --env dev)
(cd "$root/frontend" && bunx opennextjs-cloudflare build && bunx wrangler deploy --env dev)
"#;

// ═══ frontend ════════════════════════════════════════════════════════════════════════════════

const FRONT_PACKAGE_JSON: &str = r#"{
  "name": "{{NAME}}-web",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "typecheck": "tsc --noEmit",
    "preview": "opennextjs-cloudflare build && opennextjs-cloudflare preview",
    "deploy:dev": "opennextjs-cloudflare build && wrangler deploy --env dev",
    "deploy:prod": "opennextjs-cloudflare build && wrangler versions upload --env prod"
  },
  "dependencies": {
    "hono": "^4",
    "next": "^16",
    "react": "^19",
    "react-dom": "^19"
  },
  "devDependencies": {
    "@opennextjs/cloudflare": "^1",
    "@types/node": "^24",
    "@types/react": "^19",
    "@types/react-dom": "^19",
    "typescript": "^5",
    "wrangler": "^4"
  }
}
"#;

const FRONT_WRANGLER: &str = r#"{
  // Next.js on Workers. OpenNext compiles the app into a Worker; `main` and `assets` point at its
  // output, so both only exist after a build.
  "name": "{{NAME}}-web",
  "main": ".open-next/worker.js",
  "compatibility_date": "2026-08-01",
  "compatibility_flags": ["nodejs_compat"],
  "assets": { "directory": ".open-next/assets", "binding": "ASSETS" },

  // Traces come from Cloudflare's native Workers tracing, not an SDK inside the bundle.
  "observability": { "enabled": true },

  // Bindings and vars are NOT inherited by named environments — wrangler replaces them wholesale.
  // Anything that must exist in dev and prod is written out twice, on purpose: a binding that
  // silently falls through to the top level is how dev ends up talking to prod.
  "env": {
    "dev": {
      "name": "{{NAME}}-web-dev",
      "vars": { "ENVIRONMENT": "dev" },
      "services": [{ "binding": "API", "service": "{{NAME}}-api-dev" }]
    },
    "prod": {
      "name": "{{NAME}}-web-prod",
      "vars": { "ENVIRONMENT": "prod" },
      "services": [{ "binding": "API", "service": "{{NAME}}-api-prod" }]
    }
  }
}
"#;

const FRONT_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "jsx": "preserve",
    "strict": true,
    "noEmit": true,
    "allowJs": true,
    "esModuleInterop": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "incremental": true,
    "skipLibCheck": true,
    "plugins": [{ "name": "next" }],
    // `@backend/*` reaches the API's source for its types only. The import in app/page.tsx is an
    // `import type`, so it is erased before anything is bundled — no backend code ships here.
    "paths": { "@/*": ["./*"], "@backend/*": ["../backend/src/*"] }
  },
  "include": ["next-env.d.ts", "cloudflare-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules", ".open-next"]
}
"#;

const NEXT_CONFIG: &str = r#"import type { NextConfig } from "next";

const config: NextConfig = {
  // Fail the build on a type error rather than shipping one. The default already does this; it is
  // written down because turning it off is a tempting five-second fix under deadline.
  typescript: { ignoreBuildErrors: false },
};

export default config;
"#;

const OPEN_NEXT_CONFIG: &str = r#"import { defineCloudflareConfig } from "@opennextjs/cloudflare";

// No incremental cache configured yet. Adding one means choosing a Cloudflare resource to hold it
// (KV for eventual consistency, R2 for size, a Durable Object for strong consistency) — a decision
// worth making with a real access pattern in hand rather than by default.
export default defineCloudflareConfig({});
"#;

const CLOUDFLARE_ENV_DTS: &str = r#"// The bindings this Worker is given, as types. Keep it in step with the `env` block in
// wrangler.jsonc — nothing checks that they agree except a runtime failure.
declare global {
  interface CloudflareEnv {
    ENVIRONMENT: string;
    /** Service binding to the backend Worker. Internal to Cloudflare; never a public URL. */
    API: Fetcher;
  }
}

export {};
"#;

const APP_LAYOUT: &str = r#"import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "{{NAME}}",
  description: "Next.js and Hono, two Workers talking over a service binding.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
"#;

const APP_PAGE: &str = r#"import { getCloudflareContext } from "@opennextjs/cloudflare";
import { hc } from "hono/client";
import type { AppType } from "@backend/index";

// The page reads live state from the API on every request, so there is nothing to prerender.
export const dynamic = "force-dynamic";

async function hello() {
  const { env } = await getCloudflareContext({ async: true });

  // The hostname is ignored — a service binding routes by binding, not by DNS — but it has to be a
  // valid URL, so it may as well say what it is.
  //
  // The client is built from the API's own exported type. There is no generated SDK here and
  // nothing to keep in step: rename a route or change what it returns and this file stops
  // compiling, which is the reason both halves are TypeScript.
  const api = hc<AppType>("https://api.internal", {
    fetch: env.API.fetch.bind(env.API),
  });

  const res = await api.api.hello.$get();
  if (!res.ok) return null;
  return await res.json();
}

export default async function Home() {
  const data = await hello();

  return (
    <main>
      <p className="eyebrow">{{NAME}}</p>
      <h1>The frontend is talking to the backend.</h1>
      <p className="lede">
        This page is a React server component running in a Cloudflare Worker. It called the API
        Worker next to it over a service binding — an internal call with no public route, no CORS
        and no second TLS hop — using a client built from the API's own types.
      </p>

      {data ? (
        <dl className="panel">
          <dt>message</dt>
          <dd>{data.message}</dd>
          <dt>environment</dt>
          <dd>{data.environment}</dd>
        </dl>
      ) : (
        <p className="panel error">
          The API did not answer. In local development it is a separate Worker — run{" "}
          <code>bun run dev</code> in <code>backend/</code>.
        </p>
      )}
    </main>
  );
}
"#;

const APP_CSS: &str = r#":root {
  --bg: #fbfaf8;
  --fg: #16150f;
  --muted: #6c6a60;
  --rule: #e3e0d8;
  --accent: #1f6f4a;
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg: #12120f;
    --fg: #f2f0e9;
    --muted: #9a978c;
    --rule: #2a2924;
    --accent: #63c493;
  }
}

* { box-sizing: border-box; }

body {
  margin: 0;
  background: var(--bg);
  color: var(--fg);
  font: 400 16px/1.6 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  -webkit-font-smoothing: antialiased;
}

main { max-width: 62ch; margin: 0 auto; padding: 14vh 24px 10vh; }

.eyebrow {
  margin: 0;
  font: 500 12px/1 ui-monospace, SFMono-Regular, Menlo, monospace;
  letter-spacing: .12em;
  text-transform: uppercase;
  color: var(--accent);
}

h1 {
  margin: 18px 0 0;
  font-size: clamp(2rem, 5vw, 2.9rem);
  font-weight: 600;
  letter-spacing: -.03em;
  line-height: 1.1;
}

.lede { margin: 20px 0 0; color: var(--muted); }

.panel {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 8px 20px;
  margin: 40px 0 0;
  padding: 20px 22px;
  border: 1px solid var(--rule);
  border-radius: 8px;
  font: 400 14px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace;
}

.panel dt { color: var(--muted); }
.panel dd { margin: 0; }
.panel.error { display: block; color: var(--muted); }
code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
"#;

// ═══ backend ═════════════════════════════════════════════════════════════════════════════════

const BACK_PACKAGE_JSON: &str = r#"{
  "name": "{{NAME}}-api",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "wrangler dev --env dev",
    "test": "bun test",
    "typecheck": "tsc --noEmit",
    "deploy:dev": "wrangler deploy --env dev",
    "deploy:prod": "wrangler versions upload --env prod"
  },
  "dependencies": {
    "hono": "^4"
  },
  "devDependencies": {
    "@cloudflare/workers-types": "^4",
    "@types/bun": "^1",
    "typescript": "^5",
    "wrangler": "^4"
  }
}
"#;

const BACK_WRANGLER: &str = r#"{
  // The API, as its own Worker. It deploys on its own schedule and is reachable by things that
  // are not the website — a cron trigger, a webhook, a mobile client — which is the reason it is
  // not a folder of Next.js route handlers.
  "name": "{{NAME}}-api",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-01",
  "compatibility_flags": ["nodejs_compat"],

  // Traces come from Cloudflare's native Workers tracing, not an SDK inside the bundle.
  "observability": { "enabled": true },

  // Bindings and vars are NOT inherited by named environments — wrangler replaces them wholesale.
  // Anything that must exist in both is written out twice, on purpose: a binding that silently
  // falls through to the top level is how dev ends up writing to prod. When this Worker gains a
  // D1 database or an R2 bucket, each environment gets its own, with its own id.
  "env": {
    "dev":  { "name": "{{NAME}}-api-dev",  "vars": { "ENVIRONMENT": "dev" } },
    "prod": { "name": "{{NAME}}-api-prod", "vars": { "ENVIRONMENT": "prod" } }
  }
}
"#;

const BACK_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "lib": ["ES2022"],
    "types": ["@cloudflare/workers-types", "bun"],
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    // Hono infers the route table from the chained calls in src/index.ts. Without this the
    // inference blows past the default depth on a real API and every route degrades to `any`.
    "jsx": "react-jsx",
    "jsxImportSource": "hono/jsx"
  },
  "include": ["src"]
}
"#;

const BACK_INDEX: &str = r#"import { Hono } from "hono";

type Bindings = {
  ENVIRONMENT: string;
};

// Routes are chained rather than declared one at a time on `app`. That is not a style choice:
// the chained expression is what carries every route's path, method and response type, and it is
// what the frontend's client is built from.
const app = new Hono<{ Bindings: Bindings }>()
  // A readiness endpoint the platform can poll, kept apart from any business route so that a
  // health check never depends on application state.
  .get("/health", (c) => c.json({ ok: true, environment: c.env.ENVIRONMENT }))

  .get("/api/hello", (c) =>
    c.json({ message: "Hello from the API.", environment: c.env.ENVIRONMENT }),
  );

/**
 * The shape of this API, as a type.
 *
 * The frontend imports it and builds a client from it, so there is no generated SDK to regenerate
 * and no schema to keep in step. Change a route here and the frontend stops compiling — which is
 * the point of both halves being TypeScript.
 */
export type AppType = typeof app;

export default app;
"#;

const BACK_TEST: &str = r#"import { expect, test } from "bun:test";
import app from "./index";

// Bindings are passed in rather than mocked: `app.request` takes the env a Worker would get, so
// these tests exercise the same code path production does.
//
// Passing that third argument drops the typed-response overload, so `json()` widens here in a way
// it does not for the frontend — annotate the body rather than trusting the matcher to narrow it.
const env = { ENVIRONMENT: "test" };

test("health reports the environment it is running in", async () => {
  const res = await app.request("/health", {}, env);
  expect(res.status).toBe(200);
  const body = (await res.json()) as { ok: boolean; environment: string };
  expect(body).toEqual({ ok: true, environment: "test" });
});

test("hello answers with the environment attached", async () => {
  const res = await app.request("/api/hello", {}, env);
  expect(res.status).toBe(200);
  const body = (await res.json()) as { message: string; environment: string };
  expect(body.environment).toBe("test");
});

test("unknown routes are a 404, not a 500", async () => {
  const res = await app.request("/nope", {}, env);
  expect(res.status).toBe(404);
});
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn files(template: Template) -> Vec<(&'static str, String)> {
        scaffold("demo", template)
    }

    fn get(files: &[(&'static str, String)], name: &str) -> String {
        files
            .iter()
            .find(|(p, _)| *p == name)
            .unwrap_or_else(|| panic!("scaffold is missing {name}"))
            .1
            .clone()
    }

    #[test]
    fn rejects_names_that_are_not_safe_as_a_directory() {
        assert!(valid_name("my-app"));
        assert!(valid_name("api2"));
        assert!(!valid_name("../escape"));
        assert!(!valid_name("My App"));
        assert!(!valid_name(""));
        assert!(!valid_name("-leading"));
    }

    #[test]
    fn a_new_project_has_the_three_folders() {
        let files = files(Template::App);
        for prefix in ["frontend/", "backend/", "infra/"] {
            assert!(
                files.iter().any(|(p, _)| p.starts_with(prefix)),
                "no {prefix} in the scaffold"
            );
        }
    }

    /// The one binding that makes the layout worth having: the frontend reaches the API without
    /// leaving Cloudflare, and it reaches a *different* Worker in each environment.
    #[test]
    fn the_frontend_binds_to_its_own_environments_backend() {
        let wrangler = get(&files(Template::App), "frontend/wrangler.jsonc");
        assert!(wrangler.contains(r#""service": "demo-api-dev""#));
        assert!(wrangler.contains(r#""service": "demo-api-prod""#));
    }

    /// Non-negotiable 4: dev and prod never share a stateful binding. Wrangler replaces rather than
    /// merges bindings across named environments, so each one has to be written out in full — and
    /// the two must not end up naming the same Worker.
    #[test]
    fn dev_and_prod_are_separate_workers_on_both_halves() {
        let files = files(Template::App);
        for (config, base) in [
            ("frontend/wrangler.jsonc", "demo-web"),
            ("backend/wrangler.jsonc", "demo-api"),
        ] {
            let body = get(&files, config);
            assert!(
                body.contains(&format!(r#""name": "{base}-dev""#)),
                "{config}"
            );
            assert!(
                body.contains(&format!(r#""name": "{base}-prod""#)),
                "{config}"
            );
        }
    }

    /// The seam is a type, and it only exists if the API exports it and the frontend can resolve
    /// it. Break either half and the two sides drift silently — which is the failure this layout
    /// was chosen to prevent.
    #[test]
    fn the_api_type_reaches_the_frontend() {
        let files = files(Template::App);
        assert!(get(&files, "backend/src/index.ts").contains("export type AppType = typeof app;"));
        assert!(
            get(&files, "frontend/app/page.tsx")
                .contains(r#"import type { AppType } from "@backend/index""#)
        );
        assert!(
            get(&files, "frontend/tsconfig.json").contains(r#""@backend/*": ["../backend/src/*"]"#)
        );
        // hc() is what turns that type into calls, so the frontend has to actually depend on hono.
        assert!(get(&files, "frontend/package.json").contains(r#""hono""#));
    }

    /// Hono infers the route table from one chained expression. Assigning routes to `app` one at a
    /// time still runs, still passes the API's own tests, and silently degrades every frontend
    /// call to `any` — so the shape of the file is the contract, not a preference.
    #[test]
    fn the_api_routes_are_chained_into_one_expression() {
        let index = get(&files(Template::App), "backend/src/index.ts");
        assert!(index.contains("const app = new Hono<{ Bindings: Bindings }>()"));
        assert!(index.contains(".get(\"/health\""));
        assert!(index.contains(".get(\"/api/hello\""));
        // A route added as its own statement is the mistake this guards against.
        assert!(!index.contains("app.get("));
    }

    /// `make check` is the gate Keel runs after every turn. It has to cover both languages, or half
    /// the project ships unverified.
    #[test]
    fn the_gate_covers_both_halves() {
        let makefile = get(&files(Template::App), "Makefile");
        assert!(makefile.contains("cd frontend && bun run typecheck"));
        assert!(makefile.contains("cd backend && bun run typecheck && bun test"));
        // Recipes are tab-indented or make refuses to parse them at all.
        assert!(makefile.contains("\n\tcd frontend"));
        assert!(makefile.contains("\n\tcd backend"));
    }

    /// A repository that is good to work on with an agent is not one that merely runs. It says
    /// what the gate is, ships the reviewers worth delegating to, and does not carry the two files
    /// that would fail its own first scan.
    #[test]
    fn a_new_project_is_ready_for_an_agent() {
        let files = files(Template::App);

        let claude = get(&files, "CLAUDE.md");
        assert!(claude.contains("make check"), "the gate is named");
        assert!(claude.contains("Never report a result you have not seen"));

        for agent in [".claude/agents/reviewer.md", ".claude/agents/platform-limits.md"] {
            let body = get(&files, agent);
            assert!(body.starts_with("---\nname: "), "{agent} needs frontmatter");
            assert!(body.contains("description: \""), "{agent}: a description with a colon in it \
                    must be quoted or YAML swallows it");
        }

        // Shipping either of these would hand every new project a Critical finding on its first
        // scan, from the scaffold that is supposed to start it at 100.
        for forbidden in [".claude/settings.json", ".mcp.json"] {
            assert!(
                !files.iter().any(|(p, _)| *p == forbidden),
                "{forbidden} is quarantined by trust and rated Critical by the scanner"
            );
        }
    }

    #[test]
    fn a_new_project_starts_with_something_that_can_fail() {
        let files = files(Template::App);
        assert!(files.iter().any(|(p, _)| p.ends_with(".test.ts")));
        // And with instructions for the agent that will work in it.
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
    }

    #[test]
    fn an_empty_project_still_gets_agent_scaffolding() {
        let files = files(Template::Empty);
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
        assert!(!files.iter().any(|(p, _)| p.starts_with("backend/")));
    }
}
