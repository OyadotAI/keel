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
//! # Why the Go service runs in a Container
//!
//! Workers run JavaScript, TypeScript, Python and Rust. Go is not on that list, and the community
//! WASM shim for it describes itself as experimental. So a Go backend has exactly two homes: a
//! Cloudflare Container, or a second cloud.
//!
//! This scaffold picks the Container, because a second cloud costs a second account, a second
//! token, a second dashboard and a second tracing story — and the point of being Cloudflare-only
//! was to delete all four. The price is real and is written down in `infra/README.md`: containers
//! do not autoscale, so instance count is a number a human sets, and their disk is wiped on every
//! restart, so nothing durable may live on it. Both are fine at the size this template is for, and
//! both are load-bearing enough that the generated project says so out loud.

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
    /// frontend (Next.js on Workers) + backend (Go in a Container) + infra.
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

/// Names that are safe as a directory, as a Worker name and as a Go module path.
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
             names and a Go module path.",
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
        ("backend/go.mod", f(GO_MOD)),
        ("backend/cmd/api/main.go", f(GO_MAIN)),
        ("backend/internal/api/router.go", GO_ROUTER.to_string()),
        (
            "backend/internal/api/router_test.go",
            GO_ROUTER_TEST.to_string(),
        ),
        ("backend/Dockerfile", DOCKERFILE.to_string()),
        ("backend/.dockerignore", DOCKERIGNORE.to_string()),
        ("backend/package.json", f(BACK_PACKAGE_JSON)),
        ("backend/wrangler.jsonc", f(BACK_WRANGLER)),
        ("backend/tsconfig.json", BACK_TSCONFIG.to_string()),
        ("backend/worker/index.ts", BACK_WORKER.to_string()),
    ]);

    files
}

// ═══ root ════════════════════════════════════════════════════════════════════════════════════

const CLAUDE_MD: &str = r#"# {{NAME}}

Three folders, because the two halves of this app have different constraints and hiding that
behind one folder is how the constraints get violated.

| Folder | What it is | Where it runs |
|---|---|---|
| `frontend/` | Next.js + React | A Cloudflare Worker, built by OpenNext |
| `backend/` | Go HTTP service | A Cloudflare Container, fronted by a Worker |
| `infra/` | Deploy scripts and the environment map | Nowhere — it is how the other two ship |

The frontend reaches the backend through a **service binding**, not a public URL. The call never
leaves Cloudflare's network, so the backend needs no public route and no CORS.

## Rules that are not style preferences

- **Nothing durable on the container's disk.** It is wiped on every restart. State goes in D1, R2,
  KV or a Durable Object.
- **Dev and prod never share a binding.** Each environment has its own Workers and its own
  resources. Pointing dev at a prod database is the failure this layout exists to prevent.
- **The container does not autoscale.** `max_instances` is a number a human chose. If you need it
  higher, raise it deliberately and say why.
- Explain *why* in comments, not what. The code already says what.

## Verification

`make check` is the gate: it typechecks the frontend and runs `go vet` and the Go tests. Every
change keeps it green.

## Running it

`make dev` runs the Next.js dev server. `cd backend && go run ./cmd/api` runs the API directly on
:8080 — no Docker needed for the inner loop.
"#;

const README_MD: &str = r#"# {{NAME}}

```
frontend/   Next.js + React, deployed as a Cloudflare Worker
backend/    Go service, deployed as a Cloudflare Container
infra/      how both of them reach dev and prod
```

## Develop

```
make dev                      # Next.js on :3000
cd backend && go run ./cmd/api # Go API on :8080
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
     # Go build output.\nbin/\n*.test\n\n\
     # Never commit real values.\n.env\n.env.*\n!.env.example\n\n\
     # Repository-supplied agent config, quarantined by `keel trust`.\n.keel/quarantine/\n";

// Recipes need real tabs, so this one is not a raw string.
const MAKEFILE: &str = "# The gate. Keel and CI both run `make check`, so there is exactly one\n\
     # command to keep green and one place to change what it means.\n\
     .PHONY: check check-frontend check-backend check-worker dev clean\n\n\
     check: check-frontend check-backend check-worker\n\n\
     check-frontend:\n\
     \tcd frontend && bun run typecheck\n\n\
     check-backend:\n\
     \tcd backend && go vet ./... && go test ./...\n\n\
     # The Worker that fronts the container is shipped code too. A type error there is a failed\n\
     # deploy rather than a broken page, which is the more expensive of the two.\n\
     check-worker:\n\
     \tcd backend && bun run typecheck\n\n\
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
      - uses: actions/setup-go@v5
        with:
          go-version-file: backend/go.mod
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
              │ Next.js, built by OpenNext│ bind  │ Worker ──▶ Go container    │
              └──────────────────────────┘        └───────────────────────────┘

              ┌──────────────────────────┐        ┌───────────────────────────┐
  internet ──▶│ {{NAME}}-web-prod        │──API──▶│ {{NAME}}-api-prod         │
              └──────────────────────────┘        └───────────────────────────┘
```

`API` is a service binding. The frontend calls the backend over Cloudflare's internal network, so
the backend has no public route, no CORS configuration and no second TLS hop.

## Why Go lives in a container

Workers support JavaScript, TypeScript, Python and Rust. Go is not on that list. A Go service can
therefore run in a Cloudflare Container, or on another cloud — and another cloud means another
account, another token, another dashboard and another tracing backend.

The container keeps it to one vendor. What that costs, honestly:

- **No autoscaling.** `max_instances` in `backend/wrangler.jsonc` is a number a human sets. There
  is no traffic-driven scaling to hide behind.
- **Ephemeral disk.** A container that sleeps and wakes comes back with the image's filesystem and
  nothing else. Uploads, caches and databases do not go there.
- **Cold starts on wake.** `sleepAfter` trades idle cost against first-request latency.

If the service outgrows those — sustained traffic that needs real autoscaling, or a workload that
wants local disk — the honest move is Cloud Run and a second credential, not a bigger container.
Say so when it happens instead of raising `max_instances` forever.

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
    "paths": { "@/*": ["./*"] }
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
  description: "Next.js on Workers, talking to a Go service over a service binding.",
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

// The page reads live state from the backend on every request, so there is nothing to prerender.
export const dynamic = "force-dynamic";

type Hello = { message: string; environment: string };

async function hello(): Promise<Hello | null> {
  const { env } = await getCloudflareContext({ async: true });

  // The hostname is ignored: a service binding routes by binding, not by DNS. It has to be a valid
  // URL, so it may as well say what it is.
  const res = await env.API.fetch("https://api.internal/api/hello");
  if (!res.ok) return null;
  return (await res.json()) as Hello;
}

export default async function Home() {
  const data = await hello();

  return (
    <main>
      <p className="eyebrow">{{NAME}}</p>
      <h1>The frontend is talking to the backend.</h1>
      <p className="lede">
        This page is a React server component running in a Cloudflare Worker. It called a Go service
        running in a container next to it, over a service binding that never left the network.
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
          The backend did not answer. In local development it is a separate process — run{" "}
          <code>go run ./cmd/api</code> in <code>backend/</code>.
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

const GO_MOD: &str = "module {{NAME}}\n\ngo 1.25\n";

const GO_MAIN: &str = r#"// Command api is the HTTP service. It runs in a Cloudflare Container in production and directly
// on the host during development — the same binary either way, configured only through the
// environment.
package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"{{NAME}}/internal/api"
)

func main() {
	log := slog.New(slog.NewJSONHandler(os.Stdout, nil))

	port := os.Getenv("PORT")
	if port == "" {
		port = "8080"
	}

	srv := &http.Server{
		Addr:    ":" + port,
		Handler: api.Router(log),
		// A container is reachable from a Worker, not from the open internet, but a slow-header
		// client is a resource leak regardless of who can reach it.
		ReadHeaderTimeout: 5 * time.Second,
	}

	// The platform stops a container with SIGTERM and waits before killing it. Draining in that
	// window is the difference between a clean deploy and a handful of dropped requests.
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGTERM, syscall.SIGINT)

	go func() {
		log.Info("listening", "port", port)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Error("server failed", "err", err)
			os.Exit(1)
		}
	}()

	<-stop
	log.Info("shutting down")

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if err := srv.Shutdown(ctx); err != nil {
		log.Error("shutdown did not finish cleanly", "err", err)
	}
}
"#;

const GO_ROUTER: &str = r#"// Package api holds the HTTP surface. It is separate from main so the routes can be tested
// without binding a port.
package api

import (
	"encoding/json"
	"log/slog"
	"net/http"
	"os"
)

// Header the fronting Worker sets. The container has no notion of which environment it is in on
// its own — it is one image deployed to both — so the Worker that routes to it says.
const environmentHeader = "X-Environment"

type health struct {
	OK          bool   `json:"ok"`
	Environment string `json:"environment"`
}

type hello struct {
	Message     string `json:"message"`
	Environment string `json:"environment"`
}

// Router returns the service's routes.
func Router(log *slog.Logger) http.Handler {
	mux := http.NewServeMux()

	// A readiness endpoint the platform can poll, kept separate from any business route so that
	// health checks never depend on application state.
	mux.HandleFunc("GET /health", func(w http.ResponseWriter, r *http.Request) {
		write(w, log, health{OK: true, Environment: environment(r)})
	})

	mux.HandleFunc("GET /api/hello", func(w http.ResponseWriter, r *http.Request) {
		write(w, log, hello{Message: "Hello from Go.", Environment: environment(r)})
	})

	return mux
}

func environment(r *http.Request) string {
	if env := r.Header.Get(environmentHeader); env != "" {
		return env
	}
	// Reached only outside a container — during local development there is no Worker in front.
	if env := os.Getenv("ENVIRONMENT"); env != "" {
		return env
	}
	return "local"
}

func write(w http.ResponseWriter, log *slog.Logger, body any) {
	w.Header().Set("Content-Type", "application/json")
	if err := json.NewEncoder(w).Encode(body); err != nil {
		// The status is already written by now, so this can only be logged, not reported.
		log.Error("encoding response failed", "err", err)
	}
}
"#;

const GO_ROUTER_TEST: &str = r#"package api

import (
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"testing"
)

func do(t *testing.T, method, target string, headers map[string]string) *http.Response {
	t.Helper()
	req := httptest.NewRequest(method, target, nil)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	Router(slog.New(slog.DiscardHandler)).ServeHTTP(rec, req)
	return rec.Result()
}

func TestHealthReportsTheEnvironmentTheWorkerNamed(t *testing.T) {
	res := do(t, http.MethodGet, "/health", map[string]string{environmentHeader: "prod"})
	if res.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200", res.StatusCode)
	}

	var got health
	body, _ := io.ReadAll(res.Body)
	if err := json.Unmarshal(body, &got); err != nil {
		t.Fatalf("body is not JSON: %v (%s)", err, body)
	}
	if !got.OK || got.Environment != "prod" {
		t.Fatalf("got %+v, want {true prod}", got)
	}
}

func TestEnvironmentFallsBackWhenNoWorkerIsInFront(t *testing.T) {
	res := do(t, http.MethodGet, "/health", nil)

	var got health
	body, _ := io.ReadAll(res.Body)
	if err := json.Unmarshal(body, &got); err != nil {
		t.Fatalf("body is not JSON: %v (%s)", err, body)
	}
	if got.Environment != "local" {
		t.Fatalf("environment = %q, want local", got.Environment)
	}
}

func TestUnknownRoutesAre404NotPanics(t *testing.T) {
	res := do(t, http.MethodGet, "/nope", nil)
	if res.StatusCode != http.StatusNotFound {
		t.Fatalf("status = %d, want 404", res.StatusCode)
	}
}
"#;

const DOCKERFILE: &str = r#"# Multi-stage so the shipped image is a single static binary and nothing else: no shell, no package
# manager, no Go toolchain. A container that cannot run a shell cannot be talked into running one.
FROM golang:1.25-alpine AS build

WORKDIR /src
COPY go.mod ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/api ./cmd/api

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/api /api
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/api"]
"#;

const DOCKERIGNORE: &str =
    "node_modules/\nworker/\n.wrangler/\n*.md\npackage.json\ntsconfig.json\nwrangler.jsonc\n";

const BACK_PACKAGE_JSON: &str = r#"{
  "name": "{{NAME}}-api",
  "private": true,
  "type": "module",
  "scripts": {
    "typecheck": "tsc --noEmit",
    "deploy:dev": "wrangler deploy --env dev",
    "deploy:prod": "wrangler versions upload --env prod"
  },
  "dependencies": {
    "@cloudflare/containers": "^0.3"
  },
  "devDependencies": {
    "@cloudflare/workers-types": "^4",
    "typescript": "^5",
    "wrangler": "^4"
  }
}
"#;

const BACK_WRANGLER: &str = r#"{
  // A Worker whose only job is to put the Go container behind a service binding. Cloudflare has no
  // way to route to a container directly — it is always reached through a Durable Object — so this
  // shim is a platform requirement, not indirection someone chose.
  "name": "{{NAME}}-api",
  "main": "worker/index.ts",
  "compatibility_date": "2026-08-01",
  "compatibility_flags": ["nodejs_compat"],
  "observability": { "enabled": true },

  // The container class is a Durable Object class, so it needs a migration tag the first time it
  // appears. Migrations are inherited by every environment; bindings below are not.
  "migrations": [{ "tag": "v1", "new_sqlite_classes": ["Backend"] }],

  // Named environments replace bindings rather than merging them, so each one is written out in
  // full. `max_instances` is a number a human chose: containers do not autoscale, and pretending
  // otherwise is how a launch turns into a queue.
  "env": {
    "dev": {
      "name": "{{NAME}}-api-dev",
      "vars": { "ENVIRONMENT": "dev" },
      "containers": [
        {
          "class_name": "Backend",
          "image": "./Dockerfile",
          "max_instances": 1,
          "instance_type": "dev"
        }
      ],
      "durable_objects": {
        "bindings": [{ "name": "BACKEND", "class_name": "Backend" }]
      }
    },
    "prod": {
      "name": "{{NAME}}-api-prod",
      "vars": { "ENVIRONMENT": "prod" },
      "containers": [
        {
          "class_name": "Backend",
          "image": "./Dockerfile",
          "max_instances": 3,
          "instance_type": "basic"
        }
      ],
      "durable_objects": {
        "bindings": [{ "name": "BACKEND", "class_name": "Backend" }]
      }
    }
  }
}
"#;

const BACK_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "lib": ["ES2022"],
    "types": ["@cloudflare/workers-types"],
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["worker"]
}
"#;

const BACK_WORKER: &str = r#"import { Container, getContainer } from "@cloudflare/containers";

/**
 * The Go service, as a Durable Object. Every request to this Worker is forwarded into the
 * container over its own port; nothing else about the process is visible from here.
 */
export class Backend extends Container<Env> {
  // Matches the port the Go server listens on. They are two separate declarations of the same
  // number, which is exactly the kind of thing to check first when the container answers 502.
  defaultPort = 8080;

  // Containers bill for the time they are awake, so an idle one should stop. Ten minutes is long
  // enough that a burst of traffic pays the cold start once rather than repeatedly.
  sleepAfter = "10m";
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    // Answered by the Worker, not the container: the health of the edge and the health of the
    // service are different questions, and a check that wakes a sleeping container is a bill.
    if (url.pathname === "/health/edge") {
      return Response.json({ ok: true, environment: env.ENVIRONMENT });
    }

    // A fixed id keeps every request on one instance. Give it a tenant or session id instead when
    // one instance stops being enough — and remember `max_instances` bounds how many can exist.
    const container = getContainer(env.BACKEND, env.ENVIRONMENT);

    // The image is identical in both environments, so the environment travels with the request.
    const headers = new Headers(request.headers);
    headers.set("X-Environment", env.ENVIRONMENT);

    return container.fetch(new Request(request, { headers }));
  },
} satisfies ExportedHandler<Env>;

interface Env {
  ENVIRONMENT: string;
  BACKEND: DurableObjectNamespace<Backend>;
}
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

    /// The one binding that makes the layout worth having: the frontend reaches Go without leaving
    /// Cloudflare, and it reaches a *different* Worker in each environment.
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

    /// A Go container is reachable only through a Durable Object, and a DO class is only usable
    /// with a migration tag. Ship one without the other and the first deploy fails.
    #[test]
    fn the_container_is_wired_to_a_durable_object_with_a_migration() {
        let wrangler = get(&files(Template::App), "backend/wrangler.jsonc");
        assert!(wrangler.contains(r#""class_name": "Backend""#));
        assert!(wrangler.contains("new_sqlite_classes"));
        assert!(wrangler.contains(r#""name": "BACKEND""#));
        assert!(
            get(&files(Template::App), "backend/worker/index.ts").contains("export class Backend")
        );
    }

    /// The port is declared twice — once in Go, once in the container class — and a mismatch is a
    /// 502 with nothing useful in the logs.
    #[test]
    fn the_worker_and_the_go_server_agree_on_the_port() {
        let files = files(Template::App);
        assert!(get(&files, "backend/worker/index.ts").contains("defaultPort = 8080"));
        assert!(get(&files, "backend/cmd/api/main.go").contains(r#"port = "8080""#));
        assert!(get(&files, "backend/Dockerfile").contains("EXPOSE 8080"));
    }

    /// `make check` is the gate Keel runs after every turn. It has to cover both languages, or half
    /// the project ships unverified.
    #[test]
    fn the_gate_covers_both_languages() {
        let makefile = get(&files(Template::App), "Makefile");
        assert!(makefile.contains("cd frontend && bun run typecheck"));
        assert!(makefile.contains("go test ./..."));
        // The container's fronting Worker is TypeScript that ships; it is part of the gate too.
        assert!(makefile.contains("cd backend && bun run typecheck"));
        // Recipes are tab-indented or make refuses to parse them at all.
        assert!(makefile.contains("\n\tcd frontend"));
        assert!(makefile.contains("\n\tcd backend"));
    }

    #[test]
    fn a_new_project_starts_with_something_that_can_fail() {
        let files = files(Template::App);
        assert!(files.iter().any(|(p, _)| p.ends_with("_test.go")));
        // And with instructions for the agent that will work in it.
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
    }

    /// The Go module path is the name, so every import has to be rewritten with it.
    #[test]
    fn the_go_module_path_matches_its_imports() {
        let files = files(Template::App);
        assert!(get(&files, "backend/go.mod").starts_with("module demo\n"));
        assert!(get(&files, "backend/cmd/api/main.go").contains(r#""demo/internal/api""#));
    }

    #[test]
    fn an_empty_project_still_gets_agent_scaffolding() {
        let files = files(Template::Empty);
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
        assert!(!files.iter().any(|(p, _)| p.starts_with("backend/")));
    }
}
