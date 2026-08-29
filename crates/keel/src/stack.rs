//! The production scaffold: containers, not Workers.
//!
//! Modelled on how A2ABase is built and deployed — bun builds a Next.js standalone bundle that a
//! slim Node image runs as a non-root user; Hono on Node behind the same shape; kustomize with a
//! `base` and `dev`/`prod` overlays; secrets generated from `backend/.env` and never committed;
//! GitHub Actions that test, build and push to ghcr, decrypt the env with `age`, apply the
//! overlay and roll the deployments. Rolling updates never take a pod away before its
//! replacement is ready, a `preStop` sleep outlives the endpoint-removal lag, and the HPA — not
//! the manifest — owns the replica count.
//!
//! The Cloudflare scaffold in `project.rs` is the golden path for a repository that wants no
//! cluster. This one is for teams that already run one, or that need to own the box.

/// Every file, with `{{NAME}}` substituted.
pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("Makefile", MAKEFILE.into()),
        ("docker-compose.yml", f(COMPOSE)),
        ("nginx/nginx.conf", NGINX.into()),
        (".gitignore", GITIGNORE.into()),
        (".env.example", ENV_EXAMPLE.into()),
        ("CHANGELOG.md", CHANGELOG.into()),
        ("AGENTS.md", f(AGENTS_MD)),
        (".github/workflows/deploy-dev.yaml", f(DEPLOY_DEV)),
        (".github/workflows/deploy-prod.yaml", f(DEPLOY_PROD)),
        // ── frontend ─────────────────────────────────────────────────────────────────────────
        ("frontend/package.json", f(FRONT_PACKAGE)),
        ("frontend/tsconfig.json", FRONT_TSCONFIG.into()),
        ("frontend/next.config.ts", NEXT_CONFIG.into()),
        ("frontend/Dockerfile", FRONT_DOCKERFILE.into()),
        ("frontend/.dockerignore", DOCKERIGNORE.into()),
        ("frontend/app/layout.tsx", f(APP_LAYOUT)),
        ("frontend/app/page.tsx", f(APP_PAGE)),
        ("frontend/app/globals.css", APP_CSS.into()),
        ("frontend/app/api/health/route.ts", HEALTH_ROUTE.into()),
        ("frontend/lib/api.ts", API_CLIENT.into()),
        // ── backend ──────────────────────────────────────────────────────────────────────────
        ("backend/package.json", f(BACK_PACKAGE)),
        ("backend/tsconfig.json", BACK_TSCONFIG.into()),
        ("backend/Dockerfile", BACK_DOCKERFILE.into()),
        ("backend/.dockerignore", DOCKERIGNORE.into()),
        ("backend/src/app.ts", BACK_APP.into()),
        ("backend/src/server.ts", BACK_SERVER.into()),
        ("backend/src/db.ts", BACK_DB.into()),
        ("backend/src/migrate.ts", BACK_MIGRATE.into()),
        ("backend/src/app.test.ts", BACK_TEST.into()),
        ("backend/migrations/0001_init.sql", MIGRATION.into()),
        // ── k8s ──────────────────────────────────────────────────────────────────────────────
        ("k8s/README.md", f(K8S_README)),
        ("k8s/base/kustomization.yaml", K8S_BASE_KUSTOMIZATION.into()),
        ("k8s/base/namespace.yaml", f(K8S_NAMESPACE)),
        ("k8s/base/backend.yaml", f(K8S_BACKEND)),
        ("k8s/base/frontend.yaml", f(K8S_FRONTEND)),
        (
            "k8s/overlays/dev/kustomization.yaml",
            f(K8S_DEV_KUSTOMIZATION),
        ),
        ("k8s/overlays/dev/ingress.yaml", f(K8S_DEV_INGRESS)),
        (
            "k8s/overlays/prod/kustomization.yaml",
            f(K8S_PROD_KUSTOMIZATION),
        ),
        ("k8s/overlays/prod/ingress.yaml", f(K8S_PROD_INGRESS)),
        ("k8s/overlays/prod/cert.yaml", f(K8S_CERT)),
        ("k8s/secrets.example.yaml", f(K8S_SECRETS_EXAMPLE)),
        ("k8s/scripts/env-to-secrets.sh", f(ENV_TO_SECRETS)),
    ]
}

/// The CLAUDE.md for this stack: the rules that came from running one for real.
pub fn claude_md(name: &str) -> String {
    CLAUDE_MD.replace("{{NAME}}", name)
}

const MAKEFILE: &str = r#".DEFAULT_GOAL := help
ENV ?= dev

help:
	@echo "check         the gate: typecheck + tests, both halves (Keel and CI run this)"
	@echo "dev           start postgres + redis in Docker, then: make backend / make frontend"
	@echo "backend       run the API with reload on :8000"
	@echo "frontend      run Next.js with reload on :3000"
	@echo "up            the whole stack in Docker behind nginx on :8080"
	@echo "migrate       apply backend/migrations to DATABASE_URL"
	@echo "k8s-secrets   backend/.env -> k8s/secrets.yaml for ENV=dev|prod (never committed)"
	@echo "encrypt-env   backend/.env -> backend/.env.age (committed; CI decrypts it)"
	@echo "release       tag vX.Y.Z; the prod workflow deploys the tag"

.PHONY: check check-frontend check-backend dev backend frontend up down migrate k8s-secrets encrypt-env decrypt-env release

check: check-frontend check-backend

check-frontend:
	cd frontend && bun run typecheck

check-backend:
	cd backend && bun run typecheck && bun test

dev:
	docker compose up -d postgres redis
	@echo "postgres on :5432, redis on :6379 — now: make backend (and make frontend)"

backend:
	cd backend && bun run dev

frontend:
	cd frontend && bun run dev

up:
	docker compose --profile app up --build

down:
	docker compose --profile app down

migrate:
	cd backend && bun run migrate

# Secrets are generated from the one env file, per environment, and applied by CI or by you.
# The output is gitignored; the encrypted env is what gets committed.
k8s-secrets:
	k8s/scripts/env-to-secrets.sh --env $(ENV)

encrypt-env:
	age -R .age-recipients -o backend/.env.age backend/.env

decrypt-env:
	age -d -i age-key.txt -o backend/.env backend/.env.age

release:
	@v=$${V:-$$(git describe --tags --abbrev=0 2>/dev/null | awk -F. '{printf "%s.%s.%d", $$1, $$2, $$3+1}')}; \
	v=$${v:-v0.1.0}; git tag -a "$$v" -m "$$v" && git push origin "$$v" && echo "released $$v"
"#;

const COMPOSE: &str = r#"# Local infrastructure by default; the whole stack with `--profile app`.
#
#   docker compose up -d            postgres + redis, apps run on the host with reload
#   make up                         everything in containers behind nginx on :8080
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: app
      POSTGRES_PASSWORD: app
      POSTGRES_DB: {{NAME}}
    ports: ["5432:5432"]
    volumes: ["{{NAME}}_pgdata:/var/lib/postgresql/data"]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U app -d {{NAME}}"]
      interval: 5s
      timeout: 3s
      retries: 10

  redis:
    image: redis:7-alpine
    command: ["redis-server", "--appendonly", "yes", "--maxmemory", "256mb", "--maxmemory-policy", "allkeys-lru"]
    ports: ["6379:6379"]
    volumes: ["{{NAME}}_redis:/data"]
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 10

  backend:
    profiles: ["app"]
    build: ./backend
    environment:
      DATABASE_URL: postgres://app:app@postgres:5432/{{NAME}}
      REDIS_URL: redis://redis:6379
      PORT: "8000"
    depends_on:
      postgres: { condition: service_healthy }
      redis: { condition: service_healthy }
    healthcheck:
      test: ["CMD", "node", "-e", "fetch('http://127.0.0.1:8000/api/health/ready').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"]
      interval: 10s
      timeout: 3s
      retries: 6

  frontend:
    profiles: ["app"]
    build:
      context: ./frontend
      args:
        NEXT_PUBLIC_API_URL: /api
    environment:
      API_URL: http://backend:8000
    depends_on:
      backend: { condition: service_healthy }

  nginx:
    profiles: ["app"]
    image: nginx:1.27-alpine
    ports: ["8080:80"]
    volumes: ["./nginx/nginx.conf:/etc/nginx/nginx.conf:ro"]
    depends_on: [frontend, backend]

volumes:
  {{NAME}}_pgdata:
  {{NAME}}_redis:
"#;

const NGINX: &str = r#"# One origin for both halves: /api to the API, everything else to the app. In the cluster the
# ingress controller does this job with the same split; here it is for `make up` and for a VM.
worker_processes auto;
events { worker_connections 1024; }
http {
  include       mime.types;
  sendfile      on;
  client_max_body_size 50m;
  proxy_read_timeout 300s;
  gzip on;
  gzip_types text/plain text/css application/json application/javascript image/svg+xml;

  upstream frontend { server frontend:3000; }
  upstream backend  { server backend:8000; }

  server {
    listen 80;
    location /api/ {
      proxy_pass http://backend;
      proxy_set_header Host $host;
      proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
      proxy_set_header X-Forwarded-Proto $scheme;
    }
    location / {
      proxy_pass http://frontend;
      proxy_set_header Host $host;
      proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
      proxy_set_header X-Forwarded-Proto $scheme;
      proxy_http_version 1.1;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection "upgrade";
    }
  }
}
"#;

const GITIGNORE: &str = r#"node_modules/
.next/
dist/
coverage/
.env
.env.*
!.env.example
!backend/.env.age
backend/.env
k8s/secrets.yaml
age-key.txt
.DS_Store
"#;

const ENV_EXAMPLE: &str = r#"# Copy to backend/.env. Never commit backend/.env; commit backend/.env.age (make encrypt-env).
# Keys with a _DEV suffix win in the dev overlay, so one file serves both environments.
DATABASE_URL=postgres://app:app@localhost:5432/app
REDIS_URL=redis://localhost:6379
DOMAIN=example.com
DOMAIN_DEV=dev.example.com
SENTRY_DSN=
NEXT_PUBLIC_SENTRY_DSN=
POSTHOG_API_KEY=
POSTHOG_HOST=https://us.i.posthog.com
# FEATURE_* lines pass through to the cluster verbatim.
FEATURE_EXAMPLE=false
"#;

const CHANGELOG: &str = r#"# Changelog

Newest first. Every change lands here under the day it shipped, in past tense, with specifics —
what changed and why, never marketing. `### Added`, `### Fixed`, `### Changed`.

## Unreleased

### Added
- Scaffolded by Keel: Next.js and Hono in containers, kustomize overlays, CI to GHCR and GKE.
"#;

const DEPLOY_DEV: &str = r#"name: deploy-dev

on:
  push:
    branches: [main]

env:
  REGISTRY: ghcr.io
  ORG: ${{ github.repository_owner }}
  TAG: dev
  NAMESPACE: {{NAME}}-dev

jobs:
  changes:
    runs-on: ubuntu-latest
    outputs:
      backend: ${{ steps.f.outputs.backend }}
      frontend: ${{ steps.f.outputs.frontend }}
      k8s: ${{ steps.f.outputs.k8s }}
    steps:
      - uses: actions/checkout@v5
      - uses: dorny/paths-filter@v4
        id: f
        with:
          filters: |
            backend: ['backend/**']
            frontend: ['frontend/**']
            k8s: ['k8s/**']

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: oven-sh/setup-bun@v2
      - uses: actions/cache@v4
        with:
          path: |
            ~/.bun/install/cache
            frontend/node_modules
            backend/node_modules
          key: bun-${{ hashFiles('frontend/bun.lock', 'backend/bun.lock') }}
      - run: bun install --frozen-lockfile
        working-directory: frontend
      - run: bun install --frozen-lockfile
        working-directory: backend
      - run: make check

  build-backend:
    needs: [changes, test]
    if: needs.changes.outputs.backend == 'true'
    runs-on: ubuntu-latest
    permissions: { contents: read, packages: write }
    steps:
      - uses: actions/checkout@v5
      - uses: docker/login-action@v4
        with: { registry: ghcr.io, username: "${{ github.actor }}", password: "${{ secrets.GITHUB_TOKEN }}" }
      - uses: docker/setup-buildx-action@v4
      - uses: docker/build-push-action@v7
        with:
          context: backend
          platforms: linux/amd64
          push: true
          tags: ${{ env.REGISTRY }}/${{ env.ORG }}/{{NAME}}-backend:${{ env.TAG }}
          cache-from: type=gha,scope=backend-dev
          cache-to: type=gha,mode=max,scope=backend-dev

  build-frontend:
    needs: [changes, test]
    if: needs.changes.outputs.frontend == 'true'
    runs-on: ubuntu-latest
    permissions: { contents: read, packages: write }
    steps:
      - uses: actions/checkout@v5
      - uses: docker/login-action@v4
        with: { registry: ghcr.io, username: "${{ github.actor }}", password: "${{ secrets.GITHUB_TOKEN }}" }
      - uses: docker/setup-buildx-action@v4
      - uses: docker/build-push-action@v7
        with:
          context: frontend
          platforms: linux/amd64
          push: true
          tags: ${{ env.REGISTRY }}/${{ env.ORG }}/{{NAME}}-frontend:${{ env.TAG }}
          cache-from: type=gha,scope=frontend-dev
          cache-to: type=gha,mode=max,scope=frontend-dev
          build-args: |
            NEXT_PUBLIC_API_URL=/api
            NEXT_PUBLIC_SENTRY_DSN=${{ secrets.FRONTEND_SENTRY_DSN }}
          secrets: |
            SENTRY_AUTH_TOKEN=${{ secrets.SENTRY_AUTH_TOKEN }}

  deploy:
    needs: [changes, build-backend, build-frontend]
    if: always() && !failure() && !cancelled()
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: google-github-actions/auth@v3
        with: { credentials_json: "${{ secrets.GKE_SA_KEY }}" }
      - uses: google-github-actions/get-gke-credentials@v3
        with:
          cluster_name: ${{ secrets.GKE_CLUSTER }}
          location: ${{ secrets.GKE_ZONE }}
          project_id: ${{ secrets.GKE_PROJECT }}
      - run: kubectl create namespace $NAMESPACE || true
      # Secrets: the committed, age-encrypted env is decrypted for the length of this step,
      # rendered into Secret objects, applied, and removed. Nothing plaintext survives the job.
      - name: Apply secrets
        run: |
          sudo apt-get install -y age >/dev/null
          echo "${{ secrets.AGE_SECRET_KEY }}" > /tmp/age-key.txt
          age -d -i /tmp/age-key.txt -o backend/.env backend/.env.age
          make k8s-secrets ENV=dev
          kubectl apply -f k8s/secrets.yaml
          rm -f backend/.env k8s/secrets.yaml /tmp/age-key.txt
      - name: Registry pull secret
        run: |
          kubectl -n $NAMESPACE create secret docker-registry ghcr \
            --docker-server=ghcr.io --docker-username=${{ github.actor }} \
            --docker-password=${{ secrets.GHCR_PAT }} --dry-run=client -o yaml | kubectl apply -f -
      - run: kubectl apply -k k8s/overlays/dev
      # Mutable tags need a nudge: patch a rollout annotation on what changed.
      - name: Roll changed deployments
        run: |
          stamp=$(date +%s)
          [ "${{ needs.changes.outputs.backend }}" = "true" ] && kubectl -n $NAMESPACE patch deployment backend -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"rollout\":\"$stamp\"}}}}}" || true
          [ "${{ needs.changes.outputs.frontend }}" = "true" ] && kubectl -n $NAMESPACE patch deployment frontend -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"rollout\":\"$stamp\"}}}}}" || true
          kubectl -n $NAMESPACE rollout status deployment/backend --timeout=300s
          kubectl -n $NAMESPACE rollout status deployment/frontend --timeout=300s
      - run: kubectl -n $NAMESPACE get pods,svc,ingress
"#;

const DEPLOY_PROD: &str = r#"name: deploy-prod

# Production deploys a tag, never a branch: `make release` makes the tag.
on:
  push:
    tags: ['v*']

env:
  REGISTRY: ghcr.io
  ORG: ${{ github.repository_owner }}
  TAG: ${{ github.ref_name }}
  NAMESPACE: {{NAME}}

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
        working-directory: frontend
      - run: bun install --frozen-lockfile
        working-directory: backend
      - run: make check

  build:
    needs: test
    runs-on: ubuntu-latest
    permissions: { contents: read, packages: write }
    strategy:
      matrix: { half: [backend, frontend] }
    steps:
      - uses: actions/checkout@v5
      - uses: docker/login-action@v4
        with: { registry: ghcr.io, username: "${{ github.actor }}", password: "${{ secrets.GITHUB_TOKEN }}" }
      - uses: docker/setup-buildx-action@v4
      - uses: docker/build-push-action@v7
        with:
          context: ${{ matrix.half }}
          platforms: linux/amd64
          push: true
          # Both the version and `latest`, so the overlay can pin either.
          tags: |
            ${{ env.REGISTRY }}/${{ env.ORG }}/{{NAME}}-${{ matrix.half }}:${{ env.TAG }}
            ${{ env.REGISTRY }}/${{ env.ORG }}/{{NAME}}-${{ matrix.half }}:latest
          cache-from: type=gha,scope=${{ matrix.half }}-prod
          cache-to: type=gha,mode=max,scope=${{ matrix.half }}-prod
          build-args: |
            NEXT_PUBLIC_API_URL=/api
            NEXT_PUBLIC_SENTRY_DSN=${{ secrets.FRONTEND_SENTRY_DSN }}
            SENTRY_RELEASE=${{ env.TAG }}
          secrets: |
            SENTRY_AUTH_TOKEN=${{ secrets.SENTRY_AUTH_TOKEN }}

  deploy:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: google-github-actions/auth@v3
        with: { credentials_json: "${{ secrets.GKE_SA_KEY }}" }
      - uses: google-github-actions/get-gke-credentials@v3
        with:
          cluster_name: ${{ secrets.GKE_CLUSTER }}
          location: ${{ secrets.GKE_ZONE }}
          project_id: ${{ secrets.GKE_PROJECT }}
      - run: kubectl create namespace $NAMESPACE || true
      - name: Apply secrets
        run: |
          sudo apt-get install -y age >/dev/null
          echo "${{ secrets.AGE_SECRET_KEY }}" > /tmp/age-key.txt
          age -d -i /tmp/age-key.txt -o backend/.env backend/.env.age
          make k8s-secrets ENV=prod
          sed -i "s/^  SENTRY_RELEASE: .*/  SENTRY_RELEASE: \"$TAG\"/" k8s/secrets.yaml
          kubectl apply -f k8s/secrets.yaml
          rm -f backend/.env k8s/secrets.yaml /tmp/age-key.txt
      - name: Registry pull secret
        run: |
          kubectl -n $NAMESPACE create secret docker-registry ghcr \
            --docker-server=ghcr.io --docker-username=${{ github.actor }} \
            --docker-password=${{ secrets.GHCR_PAT }} --dry-run=client -o yaml | kubectl apply -f -
      - run: kubectl apply -k k8s/overlays/prod
      - run: |
          stamp=$(date +%s)
          for d in backend frontend; do
            kubectl -n $NAMESPACE patch deployment $d -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"rollout\":\"$stamp\"}}}}}"
            kubectl -n $NAMESPACE rollout status deployment/$d --timeout=300s
          done
"#;

// ═══ frontend ════════════════════════════════════════════════════════════════════════════════

const FRONT_PACKAGE: &str = r#"{
  "name": "{{NAME}}-frontend",
  "private": true,
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "start": "next start",
    "typecheck": "tsc --noEmit",
    "test": "vitest run",
    "test:coverage": "vitest run --coverage"
  },
  "dependencies": {
    "next": "^16.0.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "hono": "^4.7.0"
  },
  "devDependencies": {
    "@types/node": "^22",
    "@types/react": "^19",
    "@types/react-dom": "^19",
    "typescript": "^5.7.0",
    "vitest": "^3"
  }
}
"#;

const FRONT_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["dom", "dom.iterable", "esnext"],
    "strict": true,
    "noEmit": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "jsx": "preserve",
    "incremental": true,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "plugins": [{ "name": "next" }],
    "paths": { "@/*": ["./*"], "@backend/*": ["../backend/src/*"] }
  },
  "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules"]
}
"#;

const NEXT_CONFIG: &str = r#"import type { NextConfig } from "next";

// Standalone output only inside the image: it is what `node server.js` runs, and it is not what
// `next dev` wants. Type errors fail `make check` in CI, not the image build. (`eslint` is not
// a NextConfig key in Next 16; a template that set it failed its own first typecheck.)
const inDocker = process.env.DOCKER_BUILD === "true";

const nextConfig: NextConfig = {
  output: inDocker ? "standalone" : undefined,
  typescript: { ignoreBuildErrors: inDocker },
  // The API is reached through the same origin in every environment: nginx or the ingress
  // routes /api. In dev, Next proxies it to the local backend.
  async rewrites() {
    const api = process.env.API_URL ?? "http://127.0.0.1:8000";
    return inDocker ? [] : [{ source: "/api/:path*", destination: `${api}/api/:path*` }];
  },
};

export default nextConfig;
"#;

const FRONT_DOCKERFILE: &str = r#"# bun installs and builds; node runs. Four stages so a dependency change does not rebuild the
# app and an app change does not reinstall dependencies.
FROM oven/bun:1-slim AS base
WORKDIR /app

FROM base AS deps
COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile

FROM base AS builder
COPY --from=deps /app/node_modules ./node_modules
COPY . .
# Client-side env is baked at build time, so it arrives as build args.
ARG NEXT_PUBLIC_API_URL=/api
ARG NEXT_PUBLIC_SENTRY_DSN=
ARG SENTRY_RELEASE=
ENV NEXT_PUBLIC_API_URL=$NEXT_PUBLIC_API_URL NEXT_PUBLIC_SENTRY_DSN=$NEXT_PUBLIC_SENTRY_DSN \
    SENTRY_RELEASE=$SENTRY_RELEASE DOCKER_BUILD=true NEXT_TELEMETRY_DISABLED=1
RUN --mount=type=secret,id=SENTRY_AUTH_TOKEN \
    --mount=type=cache,target=/app/.next/cache \
    SENTRY_AUTH_TOKEN="$(cat /run/secrets/SENTRY_AUTH_TOKEN 2>/dev/null || echo '')" \
    bun run build

FROM node:22-slim AS runner
WORKDIR /app
ENV NODE_ENV=production PORT=3000 HOSTNAME=0.0.0.0 NEXT_TELEMETRY_DISABLED=1
RUN addgroup --gid 1001 nodejs && adduser --uid 1001 --ingroup nodejs --disabled-password --gecos "" nextjs
COPY --from=builder --chown=nextjs:nodejs /app/public ./public
COPY --from=builder --chown=nextjs:nodejs /app/.next/standalone ./
COPY --from=builder --chown=nextjs:nodejs /app/.next/static ./.next/static
USER nextjs
EXPOSE 3000
CMD ["node", "server.js"]
"#;

const DOCKERIGNORE: &str = r#"node_modules
.next
dist
coverage
.env
.env.*
*.md
Dockerfile
.git
"#;

const APP_LAYOUT: &str = r#"import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = { title: "{{NAME}}" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
"#;

const APP_PAGE: &str = r#"import { api } from "@/lib/api";

// A server component that reads the API through the typed client. If the API changes the shape
// of `/api/health/ready`, this stops compiling — that is the seam doing its job.
export default async function Home() {
  const res = await api.api.health.ready.$get();
  const health = await res.json();
  return (
    <main>
      <h1>{{NAME}}</h1>
      <p>
        API: <code>{health.status}</code> · db <code>{health.db}</code>
      </p>
    </main>
  );
}
"#;

const APP_CSS: &str = r#":root { color-scheme: light dark; font-family: system-ui, sans-serif; }
body { margin: 0; padding: 2rem; max-width: 60ch; }
code { font-family: ui-monospace, monospace; }
"#;

const HEALTH_ROUTE: &str = r#"// The frontend's own probe target. The cluster asks this, not the page.
export function GET() {
  return Response.json({ status: "ok" });
}
"#;

const API_CLIENT: &str = r#"import { hc } from "hono/client";
import type { AppType } from "@backend/app";

// Built from the API's route types — no generated SDK, no schema file. Server-side the call goes
// straight to the service; in the browser it goes through the same origin's /api.
const base =
  typeof window === "undefined"
    ? (process.env.API_URL ?? "http://127.0.0.1:8000")
    : "";

export const api = hc<AppType>(base);
"#;

// ═══ backend ═════════════════════════════════════════════════════════════════════════════════

const BACK_PACKAGE: &str = r#"{
  "name": "{{NAME}}-backend",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "bun --watch src/server.ts",
    "build": "bun build src/server.ts --target=node --outdir=dist",
    "start": "node dist/server.js",
    "typecheck": "tsc --noEmit",
    "test": "bun test",
    "migrate": "bun src/migrate.ts"
  },
  "dependencies": {
    "@hono/node-server": "^1.13.0",
    "hono": "^4.7.0",
    "postgres": "^3.4.0",
    "zod": "^3.24.0"
  },
  "devDependencies": {
    "@types/node": "^22",
    "bun-types": "latest",
    "typescript": "^5.7.0"
  }
}
"#;

const BACK_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "types": ["bun-types", "node"]
  },
  "include": ["src/**/*.ts"]
}
"#;

const BACK_DOCKERFILE: &str = r#"FROM oven/bun:1-slim AS base
WORKDIR /app

FROM base AS deps
COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile

FROM base AS builder
COPY --from=deps /app/node_modules ./node_modules
COPY . .
RUN bun run build

FROM node:22-slim AS runner
WORKDIR /app
ENV NODE_ENV=production PORT=8000
RUN addgroup --gid 1001 nodejs && adduser --uid 1001 --ingroup nodejs --disabled-password --gecos "" api
COPY --from=deps --chown=api:nodejs /app/node_modules ./node_modules
COPY --from=builder --chown=api:nodejs /app/dist ./dist
COPY --chown=api:nodejs migrations ./migrations
USER api
EXPOSE 8000
CMD ["node", "dist/server.js"]
"#;

const BACK_APP: &str = r#"import { Hono } from "hono";
import { z } from "zod";
import { db } from "./db";

// One chained expression. Hono infers the route table from it, and the frontend builds its
// client from that type. Routes assigned to `app` one at a time still run — and silently turn
// every frontend call into `any`.
const app = new Hono()
  .get("/api/health", (c) => c.json({ status: "ok" }))
  .get("/api/health/ready", async (c) => {
    // Ready means the database answers, which is what the cluster's readiness probe wants to
    // know before routing traffic here.
    try {
      await db`select 1`;
      return c.json({ status: "ok", db: "ok" as const });
    } catch {
      return c.json({ status: "degraded", db: "down" as const }, 503);
    }
  })
  .get("/api/notes", async (c) => {
    const rows = await db<{ id: number; body: string; created_at: string }[]>`
      select id, body, created_at from notes order by id desc limit 100`;
    return c.json({ notes: rows });
  })
  .post("/api/notes", async (c) => {
    const parsed = z.object({ body: z.string().min(1).max(2000) }).safeParse(await c.req.json());
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400);
    const [row] = await db<{ id: number }[]>`
      insert into notes (body) values (${parsed.data.body}) returning id`;
    return c.json({ id: row.id }, 201);
  });

export type AppType = typeof app;
export default app;
"#;

const BACK_SERVER: &str = r#"import { serve } from "@hono/node-server";
import app from "./app";

const port = Number(process.env.PORT ?? 8000);
const server = serve({ fetch: app.fetch, port }, () => {
  console.log(JSON.stringify({ level: "info", msg: "listening", port }));
});

// Drain on SIGTERM: the cluster removes the pod from the Service, waits (preStop), then sends
// this. Finishing in-flight requests here is what makes a rollout invisible.
process.on("SIGTERM", () => {
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 10_000).unref();
});
"#;

const BACK_DB: &str = r#"import postgres from "postgres";

// One pool for the process. The URL comes from the environment — a compose service locally,
// a Secret in the cluster — and is never in the repository.
export const db = postgres(process.env.DATABASE_URL ?? "postgres://app:app@localhost:5432/app", {
  max: 10,
  idle_timeout: 20,
  connect_timeout: 10,
});
"#;

const BACK_MIGRATE: &str = r#"import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { db } from "./db";

// Plain SQL files, applied in name order, each recorded once. Run by `make migrate` locally and
// by a Job before a rollout in the cluster. No ORM decides the schema; the files do.
const dir = join(import.meta.dirname, "..", "migrations");
await db`create table if not exists schema_migrations (name text primary key, applied_at timestamptz default now())`;
const done = new Set((await db<{ name: string }[]>`select name from schema_migrations`).map((r) => r.name));
for (const file of (await readdir(dir)).filter((f) => f.endsWith(".sql")).sort()) {
  if (done.has(file)) continue;
  const sql = await readFile(join(dir, file), "utf8");
  await db.begin(async (tx) => {
    await tx.unsafe(sql);
    await tx`insert into schema_migrations (name) values (${file})`;
  });
  console.log(`applied ${file}`);
}
await db.end();
"#;

const BACK_TEST: &str = r#"import { describe, expect, test } from "bun:test";
import app from "./app";

// The routes that need no database. The gate runs these without infrastructure, which is what
// makes it something Keel can run after every turn.
describe("api", () => {
  test("health answers", async () => {
    const res = await app.request("/api/health");
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ status: "ok" });
  });

  test("a bad note is refused", async () => {
    const res = await app.request("/api/notes", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ body: "" }),
    });
    expect(res.status).toBe(400);
  });
});
"#;

const MIGRATION: &str = r#"create table if not exists notes (
  id bigserial primary key,
  body text not null,
  created_at timestamptz not null default now()
);
"#;

// ═══ k8s ═════════════════════════════════════════════════════════════════════════════════════

const K8S_README: &str = r#"# Kubernetes

kustomize: `base/` holds the workloads, `overlays/dev` and `overlays/prod` set the namespace,
image tags, ingress and certificate.

    kubectl apply -k k8s/overlays/dev
    kubectl apply -k k8s/overlays/prod

Secrets are not in kustomize. They are rendered from `backend/.env` by
`k8s/scripts/env-to-secrets.sh --env dev|prod` into `k8s/secrets.yaml` (gitignored) and applied
separately — by CI on every deploy, or by you. The workloads read them with `envFrom`.

What the manifests insist on, and why:
- **No `replicas:` on a Deployment.** The HPA owns it; an `apply` that sets it resets a scaled
  deployment mid-rollout.
- **`maxUnavailable: 0`.** A rollout never takes a pod away before its replacement is ready.
- **`preStop: sleep 15`.** The endpoint is removed from the Service before the pod gets SIGTERM,
  but not instantly; the sleep outlives that lag, so no request lands on a pod that is gone.
- **Startup probes with a long `failureThreshold`.** A slow first boot is not a crash loop.
- **`PodDisruptionBudget maxUnavailable: 1`**, not `minAvailable`, so a one-replica dev overlay
  can still drain a node.
"#;

const K8S_BASE_KUSTOMIZATION: &str = r#"apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - namespace.yaml
  - backend.yaml
  - frontend.yaml
"#;

const K8S_NAMESPACE: &str = r#"apiVersion: v1
kind: Namespace
metadata:
  name: {{NAME}}
"#;

const K8S_BACKEND: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: backend
  labels: { app: backend }
spec:
  # No replicas: the HPA below owns the count.
  selector:
    matchLabels: { app: backend }
  strategy:
    type: RollingUpdate
    rollingUpdate: { maxUnavailable: 0, maxSurge: 2 }
  template:
    metadata:
      labels: { app: backend }
    spec:
      terminationGracePeriodSeconds: 90
      imagePullSecrets: [{ name: ghcr }]
      initContainers:
        # Migrations run before the new version serves, from the same image, with the same env.
        - name: migrate
          image: ghcr.io/ORG/{{NAME}}-backend:latest
          command: ["node", "--experimental-strip-types", "src/migrate.ts"]
          envFrom: [{ secretRef: { name: app-secrets } }]
      containers:
        - name: backend
          image: ghcr.io/ORG/{{NAME}}-backend:latest
          imagePullPolicy: Always
          ports: [{ containerPort: 8000 }]
          envFrom: [{ secretRef: { name: app-secrets } }]
          lifecycle:
            preStop: { exec: { command: ["sh", "-c", "sleep 15"] } }
          startupProbe:
            httpGet: { path: /api/health/ready, port: 8000 }
            periodSeconds: 5
            failureThreshold: 36
          readinessProbe:
            httpGet: { path: /api/health/ready, port: 8000 }
            periodSeconds: 10
          livenessProbe:
            httpGet: { path: /api/health, port: 8000 }
            periodSeconds: 20
          resources:
            requests: { cpu: 300m, memory: 512Mi }
            limits: { cpu: 1500m, memory: 2Gi }
---
apiVersion: v1
kind: Service
metadata:
  name: backend
spec:
  selector: { app: backend }
  ports: [{ port: 8000, targetPort: 8000 }]
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: backend
spec:
  scaleTargetRef: { apiVersion: apps/v1, kind: Deployment, name: backend }
  minReplicas: 2
  maxReplicas: 5
  metrics:
    - type: Resource
      resource: { name: cpu, target: { type: Utilization, averageUtilization: 70 } }
  behavior:
    scaleUp:
      stabilizationWindowSeconds: 120
      policies: [{ type: Pods, value: 1, periodSeconds: 60 }]
    scaleDown:
      stabilizationWindowSeconds: 300
      policies: [{ type: Pods, value: 1, periodSeconds: 60 }]
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: backend
spec:
  maxUnavailable: 1
  selector:
    matchLabels: { app: backend }
"#;

const K8S_FRONTEND: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: frontend
  labels: { app: frontend }
spec:
  selector:
    matchLabels: { app: frontend }
  strategy:
    type: RollingUpdate
    rollingUpdate: { maxUnavailable: 0, maxSurge: 2 }
  template:
    metadata:
      labels: { app: frontend }
    spec:
      terminationGracePeriodSeconds: 40
      imagePullSecrets: [{ name: ghcr }]
      containers:
        - name: frontend
          image: ghcr.io/ORG/{{NAME}}-frontend:latest
          imagePullPolicy: Always
          ports: [{ containerPort: 3000 }]
          env:
            - { name: API_URL, value: "http://backend:8000" }
          envFrom: [{ secretRef: { name: frontend-secrets } }]
          lifecycle:
            preStop: { exec: { command: ["sh", "-c", "sleep 15"] } }
          startupProbe:
            httpGet: { path: /api/health, port: 3000 }
            periodSeconds: 5
            failureThreshold: 24
          readinessProbe:
            httpGet: { path: /api/health, port: 3000 }
            periodSeconds: 10
          livenessProbe:
            httpGet: { path: /api/health, port: 3000 }
            periodSeconds: 20
          resources:
            requests: { cpu: 250m, memory: 256Mi }
            limits: { cpu: 1000m, memory: 1Gi }
---
apiVersion: v1
kind: Service
metadata:
  name: frontend
spec:
  selector: { app: frontend }
  ports: [{ port: 3000, targetPort: 3000 }]
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: frontend
spec:
  scaleTargetRef: { apiVersion: apps/v1, kind: Deployment, name: frontend }
  minReplicas: 2
  maxReplicas: 5
  metrics:
    - type: Resource
      resource: { name: cpu, target: { type: Utilization, averageUtilization: 75 } }
  behavior:
    scaleUp:
      stabilizationWindowSeconds: 120
      policies: [{ type: Pods, value: 1, periodSeconds: 60 }]
    scaleDown:
      stabilizationWindowSeconds: 300
      policies: [{ type: Pods, value: 1, periodSeconds: 60 }]
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: frontend
spec:
  maxUnavailable: 1
  selector:
    matchLabels: { app: frontend }
"#;

const K8S_DEV_KUSTOMIZATION: &str = r#"apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: {{NAME}}-dev
resources:
  - ../../base
  - ingress.yaml
images:
  - name: ghcr.io/ORG/{{NAME}}-backend
    newTag: dev
  - name: ghcr.io/ORG/{{NAME}}-frontend
    newTag: dev
patches:
  # Dev is small: one pod each is enough, and the PDB's maxUnavailable lets it drain.
  - target: { kind: HorizontalPodAutoscaler, name: backend }
    patch: |
      - op: replace
        path: /spec/minReplicas
        value: 1
  - target: { kind: HorizontalPodAutoscaler, name: frontend }
    patch: |
      - op: replace
        path: /spec/minReplicas
        value: 1
  - target: { kind: Namespace, name: {{NAME}} }
    patch: |
      - op: replace
        path: /metadata/name
        value: {{NAME}}-dev
"#;

const K8S_DEV_INGRESS: &str = r#"apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: {{NAME}}
  annotations:
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/proxy-body-size: 50m
    nginx.ingress.kubernetes.io/proxy-read-timeout: "300"
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  ingressClassName: nginx
  tls:
    - hosts: [dev.{{NAME}}.example.com]
      secretName: {{NAME}}-dev-cert
  rules:
    - host: dev.{{NAME}}.example.com
      http:
        paths:
          - path: /api
            pathType: Prefix
            backend: { service: { name: backend, port: { number: 8000 } } }
          - path: /
            pathType: Prefix
            backend: { service: { name: frontend, port: { number: 3000 } } }
"#;

const K8S_PROD_KUSTOMIZATION: &str = r#"apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: {{NAME}}
resources:
  - ../../base
  - ingress.yaml
  - cert.yaml
images:
  - name: ghcr.io/ORG/{{NAME}}-backend
    newTag: latest
  - name: ghcr.io/ORG/{{NAME}}-frontend
    newTag: latest
"#;

const K8S_PROD_INGRESS: &str = r#"apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: {{NAME}}
  annotations:
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/proxy-body-size: 50m
    nginx.ingress.kubernetes.io/proxy-read-timeout: "300"
    nginx.ingress.kubernetes.io/affinity: cookie
    nginx.ingress.kubernetes.io/session-cookie-name: {{NAME}}-route
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  ingressClassName: nginx
  tls:
    - hosts: [{{NAME}}.example.com]
      secretName: {{NAME}}-cert
  rules:
    - host: {{NAME}}.example.com
      http:
        paths:
          - path: /api
            pathType: Prefix
            backend: { service: { name: backend, port: { number: 8000 } } }
          - path: /
            pathType: Prefix
            backend: { service: { name: frontend, port: { number: 3000 } } }
"#;

const K8S_CERT: &str = r#"# cert-manager with DNS-01 through Cloudflare: works before the ingress is reachable and for
# wildcards. The token lives in the `cloudflare-api-token` Secret, applied by hand once.
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: ops@example.com
    privateKeySecretRef: { name: letsencrypt-prod }
    solvers:
      - dns01:
          cloudflare:
            apiTokenSecretRef: { name: cloudflare-api-token, key: api-token }
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: {{NAME}}-cert
spec:
  secretName: {{NAME}}-cert
  issuerRef: { name: letsencrypt-prod, kind: ClusterIssuer }
  dnsNames: [{{NAME}}.example.com]
"#;

const K8S_SECRETS_EXAMPLE: &str = r#"# What k8s/scripts/env-to-secrets.sh renders. Never commit the real one.
apiVersion: v1
kind: Secret
metadata:
  name: app-secrets
  namespace: {{NAME}}
type: Opaque
stringData:
  DATABASE_URL: "postgres://..."
  REDIS_URL: "redis://..."
  SENTRY_DSN: ""
---
apiVersion: v1
kind: Secret
metadata:
  name: frontend-secrets
  namespace: {{NAME}}
type: Opaque
stringData:
  NEXT_PUBLIC_SENTRY_DSN: ""
"#;

const ENV_TO_SECRETS: &str = r##"#!/usr/bin/env bash
# backend/.env -> k8s/secrets.yaml, for one environment.
#
# One env file serves both environments: a key with a _DEV suffix wins in dev and falls back to
# the plain key. FEATURE_* lines pass through verbatim, so a new flag needs no edit here. The
# output is gitignored; CI renders it from the age-encrypted env and deletes it afterwards.
set -euo pipefail

ENV=dev
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENV="$2"; shift 2 ;;
    *) echo "usage: $0 --env dev|prod" >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "$0")/../.." && pwd)"
envfile="$root/backend/.env"
out="$root/k8s/secrets.yaml"
[ -f "$envfile" ] || { echo "no $envfile" >&2; exit 1; }

case "$ENV" in
  dev)  NAMESPACE="{{NAME}}-dev" ;;
  prod) NAMESPACE="{{NAME}}" ;;
  *) echo "env must be dev or prod" >&2; exit 2 ;;
esac

get_var() {
  grep -E "^$1=" "$envfile" 2>/dev/null | head -1 | sed -E 's/^[^=]+=//; s/#.*$//; s/^[[:space:]]*//; s/[[:space:]]*$//' | tr -d '"' | tr -d "'"
}
get_env_var() {
  local v=""
  [ "$ENV" = "dev" ] && v="$(get_var "$1_DEV")"
  [ -z "$v" ] && v="$(get_var "$1")"
  printf '%s' "$v"
}
yaml_val() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

DOMAIN="$(get_env_var DOMAIN)"
DATABASE_URL="$(get_env_var DATABASE_URL)"
REDIS_URL="$(get_env_var REDIS_URL)"
[ -n "$DATABASE_URL" ] || { echo "DATABASE_URL is required" >&2; exit 1; }

{
  echo "# Generated from backend/.env for $ENV - DO NOT COMMIT"
  echo "apiVersion: v1"
  echo "kind: Secret"
  echo "metadata: { name: app-secrets, namespace: $NAMESPACE }"
  echo "type: Opaque"
  echo "stringData:"
  echo "  DATABASE_URL: \"$(yaml_val "$DATABASE_URL")\""
  echo "  REDIS_URL: \"$(yaml_val "$REDIS_URL")\""
  echo "  APP_BASE_URL: \"https://$DOMAIN\""
  echo "  SENTRY_DSN: \"$(yaml_val "$(get_env_var SENTRY_DSN)")\""
  echo "  SENTRY_ENVIRONMENT: \"$ENV\""
  echo "  SENTRY_RELEASE: \"latest\""
  echo "  POSTHOG_API_KEY: \"$(yaml_val "$(get_env_var POSTHOG_API_KEY)")\""
  echo "  POSTHOG_HOST: \"$(yaml_val "$(get_env_var POSTHOG_HOST)")\""
  grep -E '^FEATURE_[A-Z0-9_]+=' "$envfile" | sed -E 's/^([^=]+)=(.*)$/  \1: "\2"/' | tr -d "'" || true
  echo "---"
  echo "apiVersion: v1"
  echo "kind: Secret"
  echo "metadata: { name: frontend-secrets, namespace: $NAMESPACE }"
  echo "type: Opaque"
  echo "stringData:"
  echo "  NEXT_PUBLIC_SENTRY_DSN: \"$(yaml_val "$(get_env_var NEXT_PUBLIC_SENTRY_DSN)")\""
} > "$out"

echo "wrote $out for $ENV ($NAMESPACE)"
"##;

// ═══ the agent's rules ═══════════════════════════════════════════════════════════════════════

const CLAUDE_MD: &str = r#"# {{NAME}}

Next.js (frontend) and Hono on Node (backend), each in its own container, Postgres and Redis
beside them, kustomize to a cluster. Scaffolded by Keel; these are the rules that keep it sane.

## The gate

`make check` — typecheck and lint the frontend, typecheck and test the backend. Keel runs it after
every turn and CI runs it before every image build. Green is a fact, not a claim; run it yourself
before saying anything is done.

## The seam is a type

`backend/src/app.ts` exports `AppType`, and `frontend/lib/api.ts` builds its client from it. The
routes stay one chained expression on `app`; assigning them one at a time still runs and silently
turns every frontend call into `any`.

## Change discipline

- Conventional Commits. Small commits, one thing each.
- Every change appends to `CHANGELOG.md` under today's date: past tense, what and why, specifics,
  no marketing words.
- Prefer a dependency the project already has over a new one. `bun`, not npm.
- No `eslint-disable`; fix the cause.
- Config comes from the environment and is documented in `.env.example`. Secrets are never in the
  repository: `backend/.env` is ignored, `backend/.env.age` is what is committed.
- Schema changes are SQL files in `backend/migrations/`, applied by `make migrate` locally and by
  the migrate init container before a rollout. No ORM owns the schema.
- Logs are one JSON object per line: `{"level","msg",...}`. No bare `console.log` in the backend.

## Production rules

Checked by the `reliability` and `security` reviewers; argue with the rule here, not in a PR.

- Handlers are stateless; session and rate-limit state live in Redis or a signed cookie.
- Every outbound call has a timeout shorter than its caller's, and the deadline travels in a
  header. Retries: max 3, exponential backoff with full jitter, idempotent operations only.
- Every mutating route that a client may retry takes `Idempotency-Key`, stored with the request
  hash and the response for 24h. Exactly-once is at-least-once plus an idempotent consumer.
- "Write the row and publish the event" is one transaction through an outbox; consumers record
  the event id in the same transaction as their effect. Every consumer has a dead-letter path
  that pages when non-empty and a replay tool.
- Pagination is keyset on `(created_at, id)`, capped at 100. IDs are UUIDv7.
- Cache entries carry TTL + jitter and refresh single-flight. Authenticated responses are
  `Cache-Control: private, no-store`.
- Pools are sized to the database: replicas × pool < `max_connections`.
- Schema changes are expand/contract, N−1 compatible, one step per deploy; never a rename in
  place. Large append-only tables are partitioned by time and pruned by dropping partitions.
- Three probes with three meanings: startup (still loading), readiness (can serve; checks
  dependencies), liveness (alive; never checks dependencies). Drain on SIGTERM.
- Circuit breakers and per-dependency concurrency limits on every external call; degrade with
  a typed "unavailable" value rather than 500.
- Rate limits per principal (token bucket, `429` + `RateLimit-*`); auth endpoints also lock out.
- Logs are one JSON line with `request_id`, `trace_id` and `tenant_id`, propagated through
  queues. RED metrics per route. Alerts fire on SLO burn rate, never on CPU, and link to a
  runbook.
- Inputs are validated once at the boundary with a schema; unknown fields are rejected; bodies
  are bounded. Between services: short-lived signed tokens with an audience, verified per hop.
- Backups are only real once restored: PITR on, a restore drill on the calendar.

## Deploying

`git push` to `main` deploys dev; `make release` tags and deploys prod. Both build images to
ghcr, decrypt the env with `age`, render secrets, apply the overlay and roll only what changed.
See `k8s/README.md` for what the manifests insist on and why.

## Key paths

| Area | Path |
|---|---|
| API routes | `backend/src/app.ts` |
| API server, shutdown | `backend/src/server.ts` |
| Database pool | `backend/src/db.ts` |
| Migrations | `backend/migrations/`, `backend/src/migrate.ts` |
| Typed API client | `frontend/lib/api.ts` |
| Pages | `frontend/app/` |
| Local infra | `docker-compose.yml`, `nginx/nginx.conf` |
| Cluster | `k8s/base/`, `k8s/overlays/{dev,prod}/` |
| Secrets | `k8s/scripts/env-to-secrets.sh` |
| CI/CD | `.github/workflows/deploy-{dev,prod}.yaml` |
"#;

const AGENTS_MD: &str = r#"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run it.

## Run

    make dev          # postgres + redis in Docker
    make backend      # API on :8000 with reload
    make frontend     # app on :3000 with reload, /api proxied to :8000
    make check        # the gate

Whole stack in containers: `make up` (nginx on :8080).

## Migrations

| Do | Command |
|---|---|
| Add one | new `backend/migrations/NNNN_name.sql` |
| Apply locally | `make migrate` |
| In the cluster | the `migrate` init container, before each rollout |
"#;

// ═══ the reviewers every scaffold ships ══════════════════════════════════════════════════════
//
// Two more subagents beside `reviewer`, each a fresh context with one question. A security
// review by the agent that just wrote the code finds what that agent already believed; a
// separate one reads the diff cold.

pub const SECURITY_AGENT: &str = r#"---
name: security
description: Run on any change that touches auth, sessions, secrets, input parsing, file or shell access, SQL, or an external call. Reads the diff cold and reports only exploitable problems with the line and the fix.
tools: Read, Grep, Glob
---

You are reviewing a change for security. You did not write it and you do not trust it.

Read the diff and the files it touches. Report only what an attacker could use, each as:
`path:line — what — how it is exploited — the fix`. No style, no theory, no "consider".

Check, in this order:
1. Trust boundaries: every value from a request, a file, an env var or a webhook is untrusted
   until validated with a schema. Look for `any`, unchecked JSON, string-built SQL or shell.
2. Secrets: anything that looks like a key, token or password in code, logs, error messages,
   test fixtures or the repository. Config comes from the environment; check `.gitignore`.
3. AuthN/AuthZ: every route that reads or writes user data checks the session *and* the
   ownership/tenant scope. A query without the tenant id in a multi-tenant table is a finding.
4. Sessions and tokens: signed, expiring, single-use where they should be, rotated on login,
   compared in constant time, never in URLs.
5. Webhooks: signature verified before parsing, replay window bounded, idempotent handling.
6. Injection and traversal: SQL via parameters only; paths resolved and confined; no `eval`;
   `dangerouslySetInnerHTML` only with sanitised input.
7. Headers and CORS: explicit origins, credentials only where needed, no wildcard with cookies.
8. Rate limits on anything that costs money or sends mail; bounded body sizes; timeouts on
   outbound calls.
9. Dependencies: a new one needs a reason; a known-vulnerable one is a finding.

End with one line: `security: N findings` and, if N is 0, what you checked so the reader knows
it was not skipped.
"#;

pub const RELIABILITY_AGENT: &str = r#"---
name: reliability
description: Run before a change that affects deployment, startup, shutdown, probes, migrations, queues, retries, or resource limits ships. Reads the change as an on-call engineer and reports what will page someone at 3am.
tools: Read, Grep, Glob, Bash
---

You are the person who gets paged. Read the change as that person.

Report only what causes an outage, a stuck rollout, data loss, or a silent failure — each as
`path:line — what breaks — when — the fix`.

Check:
1. Rollouts: `maxUnavailable: 0`; a `preStop` that outlives endpoint removal; readiness that
   means "can serve", liveness that means "is alive" and nothing stricter; startup probes with
   room for a slow boot; no `replicas:` in a Deployment the HPA owns.
2. Shutdown: SIGTERM drains in-flight work and exits; the grace period covers the drain.
3. Migrations: forward-only, backward-compatible with the version still running during the
   rollout (add column then use it; never rename in one step), run once, before serving.
4. Queues and retries: idempotency keys, bounded retries with backoff, a dead-letter path,
   and a way to replay. A retry without idempotency is a duplicate charge.
5. Resources: requests set from measurement, limits that leave headroom, connection pools
   sized to the database's limit across all replicas.
6. Failure modes: every outbound call has a timeout; every cache miss has a source of truth;
   a dependency being down degrades, not crashes (the readiness probe says so).
7. Observability: one JSON line per event with a request id; errors reach Sentry with
   context; health endpoints exist and are cheap.
8. Config: dev and prod never share a stateful resource; secrets are not in the image.

End with one line: `reliability: N findings`, and if 0, what you checked.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(files: &'a [(&str, String)], p: &str) -> &'a str {
        files
            .iter()
            .find(|(f, _)| *f == p)
            .map(|(_, b)| b.as_str())
            .unwrap_or_else(|| panic!("no {p}"))
    }

    /// The manifests insist on the things that keep a rollout invisible.
    #[test]
    fn the_manifests_carry_the_rollout_rules() {
        let f = files("demo");
        for half in ["k8s/base/backend.yaml", "k8s/base/frontend.yaml"] {
            let y = get(&f, half);
            assert!(
                y.contains("maxUnavailable: 0"),
                "{half}: a rollout never removes a pod early"
            );
            assert!(
                y.contains("sleep 15"),
                "{half}: preStop outlives endpoint removal"
            );
            assert!(
                y.contains("startupProbe"),
                "{half}: slow boot is not a crash loop"
            );
            assert!(y.contains("PodDisruptionBudget"));
            assert!(
                !y.contains("\n  replicas:"),
                "{half}: the HPA owns the count"
            );
        }
        assert!(
            get(&f, "k8s/base/backend.yaml").contains("initContainers"),
            "migrations before serving"
        );
    }

    /// Nothing generated commits a secret, and everything that would is ignored.
    #[test]
    fn secrets_never_land_in_the_repository() {
        let f = files("demo");
        let ignore = get(&f, ".gitignore");
        for p in ["backend/.env", "k8s/secrets.yaml", "age-key.txt"] {
            assert!(ignore.contains(p), "{p} must be ignored");
        }
        assert!(
            ignore.contains("!backend/.env.age"),
            "the encrypted env is what is committed"
        );
        assert!(!f.iter().any(|(p, _)| *p == "k8s/secrets.yaml"));
        assert!(
            get(&f, ".github/workflows/deploy-dev.yaml")
                .contains("rm -f backend/.env k8s/secrets.yaml")
        );
    }

    /// Images run as somebody, not root, and bun builds what node runs.
    #[test]
    fn images_are_non_root_and_two_runtime() {
        let f = files("demo");
        for p in ["frontend/Dockerfile", "backend/Dockerfile"] {
            let d = get(&f, p);
            assert!(d.contains("USER "), "{p} runs as a user");
            assert!(
                d.contains("oven/bun") && d.contains("node:22-slim"),
                "{p}: bun builds, node runs"
            );
        }
        assert!(get(&f, "frontend/next.config.ts").contains("standalone"));
    }

    /// The seam is a type here too.
    #[test]
    fn the_frontend_client_is_built_from_the_api_type() {
        let f = files("demo");
        assert!(get(&f, "backend/src/app.ts").contains("export type AppType = typeof app;"));
        assert!(get(&f, "frontend/lib/api.ts").contains("hc<AppType>"));
        assert!(get(&f, "frontend/tsconfig.json").contains("@backend/*"));
    }

    /// kustomize agrees the overlays build, when kubectl is on the machine.
    #[test]
    fn the_overlays_build() {
        if std::process::Command::new("kubectl")
            .arg("version")
            .arg("--client")
            .output()
            .is_err()
        {
            eprintln!("kubectl not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files("demo") {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        for overlay in ["dev", "prod"] {
            let out = std::process::Command::new("kubectl")
                .args(["kustomize", &format!("k8s/overlays/{overlay}")])
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{overlay}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let y = String::from_utf8_lossy(&out.stdout);
            assert!(y.contains("kind: Deployment") && y.contains("kind: Ingress"));
        }
    }
}
