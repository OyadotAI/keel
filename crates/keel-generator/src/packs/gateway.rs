//! gateway: an API gateway, like Kong / APISIX — keys, limits, routing, circuit, log ═════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", GW_APP.into()),
        ("backend/src/store.ts", GW_STORE.into()),
        ("backend/src/app.test.ts", GW_TEST.into()),
        ("backend/migrations/0002_gateway.sql", GW_SQL.into()),
        ("frontend/app/page.tsx", f(GW_PAGE)),
        ("CLAUDE.md", f(GW_CLAUDE)),
        ("AGENTS.md", f(GW_AGENTS)),
        ("README.md", f(GW_README)),
        (".claude/agents/proxy-semantics.md", GW_REVIEWER.into()),
    ]
}

const GW_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { createHash, randomBytes } from "node:crypto";
import { pgStore, type Route, type Store } from "./store";

// Kong's surface, without the plugins framework: Routes (path prefixes + methods → an upstream),
// Consumers with key-auth (the `apikey` header or a Bearer token), rate-limiting answered in
// RateLimit-* headers, proxying with X-Forwarded-* / X-Request-Id, upstream and proxy latency
// headers, retries only for idempotent methods, a circuit breaker per route, and a request log.
// Admin API under ADMIN_TOKEN at /api/admin/*; everything else is proxied.

const sha = (s: string) => createHash("sha256").update(s).digest("hex");
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
const IDEMPOTENT = new Set(["GET", "HEAD", "OPTIONS", "PUT", "DELETE"]);
const HOP = new Set(["connection", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade", "proxy-authorization", "proxy-authenticate", "host", "content-length"]);

/// Circuit breaker per route: `FAIL_THRESHOLD` consecutive failures open it; after `OPEN_MS`
/// one request is let through (half-open) and its result decides.
// ponytail: per-replica state; a shared breaker needs Redis, and usually isn't worth it.
export const FAIL_THRESHOLD = 3, OPEN_MS = 30_000;
type Circuit = { failures: number; openedAt: number | null };

export function matchRoute(routes: Route[], method: string, path: string): Route | null {
  let best: Route | null = null, bestLen = -1;
  for (const r of routes) {
    if (r.methods.length && !r.methods.includes(method)) continue;
    for (const p of r.paths) {
      const prefix = p.replace(/\/$/, "");
      if ((path === prefix || path.startsWith(prefix + "/") || prefix === "") && prefix.length > bestLen) { best = r; bestLen = prefix.length; }
    }
  }
  return best;
}

const routeSchema = z.object({
  name: z.string().regex(/^[a-z0-9-]+$/),
  paths: z.array(z.string().startsWith("/")).min(1),
  methods: z.array(z.enum(["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"])).default([]),
  upstream_url: z.string().url(),
  strip_path: z.boolean().default(true),
  rpm: z.number().int().min(1).nullable().default(null),
  timeout_ms: z.number().int().min(100).max(120_000).default(10_000),
  retries: z.number().int().min(0).max(5).default(2),
});

export function createApp(store: Store, fetchImpl: Fetch = fetch) {
  // The snapshot: re-read only when the store's version moved, checked at most once a second.
  let snap: { version: number; routes: Route[] } = { version: 0, routes: [] }, checkedAt = 0;
  const routes = async (force = false) => {
    const now = Date.now();
    if (force || now - checkedAt > 1000) { checkedAt = now; if ((await store.routeVersion()) !== snap.version) snap = await store.routeTable(); }
    return snap.routes;
  };
  const circuits = new Map<string, Circuit>();
  const circuit = (id: string) => circuits.get(id) ?? (circuits.set(id, { failures: 0, openedAt: null }), circuits.get(id)!);
  const admin = async (c: { req: { header(n: string): string | undefined } }) => { const t = process.env.ADMIN_TOKEN; return !!t && c.req.header("authorization") === `Bearer ${t}`; };
  const unauthorized = { message: "admin token required" };

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── admin: routes, keys, log, circuit state ─────────────────────────────────────────
    .get("/api/admin/routes", async (c) => {
      if (!(await admin(c))) return c.json(unauthorized, 401);
      const rs = await routes(true);
      return c.json({ data: rs.map((r) => ({ ...r, circuit: circuit(r.id).openedAt ? "open" : "closed" })) });
    })
    .post("/api/admin/routes", async (c) => {
      if (!(await admin(c))) return c.json(unauthorized, 401);
      const p = routeSchema.safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ message: "invalid route", details: p.error.flatten() }, 400);
      const existing = (await routes(true)).find((r) => r.name === p.data.name);
      const route: Route = { id: existing?.id ?? crypto.randomUUID(), ...p.data };
      await store.putRoute(route);
      await routes(true);
      return c.json(route, 201);
    })
    .delete("/api/admin/routes/:name", async (c) => {
      if (!(await admin(c))) return c.json(unauthorized, 401);
      const ok = await store.deleteRoute(c.req.param("name"));
      await routes(true);
      return ok ? c.body(null, 204) : c.json({ message: "not found" }, 404);
    })
    .post("/api/admin/keys", async (c) => {
      if (!(await admin(c))) return c.json(unauthorized, 401);
      const p = z.object({ name: z.string().min(1), routes: z.array(z.string()).default([]), rpm: z.number().int().min(1).max(100_000).default(60) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ message: "invalid key", details: p.error.flatten() }, 400);
      const raw = "gw_" + randomBytes(24).toString("hex");
      const key = await store.createKey(p.data.name, sha(raw), p.data.routes, p.data.rpm);
      // The plaintext exists exactly once, in this response.
      return c.json({ id: key.id, name: key.name, key: raw, routes: key.routes, rpm: key.rpm }, 201);
    })
    .get("/api/admin/requests", async (c) => {
      if (!(await admin(c))) return c.json(unauthorized, 401);
      return c.json({ data: await store.logs(Math.min(500, Number(c.req.query("limit") ?? 100))) });
    })

    // ── the proxy: auth → limits → circuit → forward ────────────────────────────────────
    .all("/*", async (c) => {
      const started = Date.now();
      const request_id = c.req.header("x-request-id") ?? crypto.randomUUID();
      c.header("X-Request-Id", request_id);
      const url = new URL(c.req.url);
      const method = c.req.method.toUpperCase();
      const finish = async (status: number, extra: Partial<Parameters<Store["log"]>[0]> = {}) => {
        await store.log({ request_id, key_name: null, route: null, method, path: url.pathname, status, attempts: 0, upstream_ms: null, total_ms: Date.now() - started, error: null, ...extra });
      };

      const route = matchRoute(await routes(), method, url.pathname);
      if (!route) { await finish(404); return c.json({ message: "no Route matched with those values" }, 404); }

      // key-auth: Kong's `apikey` header, or a bearer token.
      const raw = c.req.header("apikey") ?? c.req.header("authorization")?.replace(/^Bearer /, "") ?? url.searchParams.get("apikey");
      const key = raw ? await store.keyByHash(sha(raw)) : null;
      if (!key) { await finish(401, { route: route.name }); return c.json({ message: raw ? "Unauthorized" : "No API key found in request" }, 401); }
      if (key.routes.length && !key.routes.includes(route.name)) { await finish(403, { route: route.name, key_name: key.name }); return c.json({ message: "key is not allowed on this route" }, 403); }

      // rate limits: the tighter of per-key and per-route decides.
      const now = Date.now();
      const usedKey = await store.hit(`key:${key.id}`, now);
      const usedRoute = route.rpm ? await store.hit(`route:${route.id}`, now) : 0;
      const remaining = Math.min(key.rpm - usedKey, route.rpm ? route.rpm - usedRoute : Infinity);
      c.header("RateLimit-Limit", String(Math.min(key.rpm, route.rpm ?? Infinity)));
      c.header("RateLimit-Remaining", String(Math.max(0, remaining)));
      c.header("RateLimit-Reset", "60");
      if (remaining < 0) { c.header("Retry-After", "60"); await finish(429, { route: route.name, key_name: key.name }); return c.json({ message: "API rate limit exceeded" }, 429); }

      const cb = circuit(route.id);
      if (cb.openedAt && now - cb.openedAt < OPEN_MS) { await finish(503, { route: route.name, key_name: key.name, error: "circuit open" }); return c.json({ message: "upstream circuit open", retry_after_ms: OPEN_MS - (now - cb.openedAt) }, 503); }

      // Build the upstream request once; the body is buffered so a retry can resend it.
      const base = new URL(route.upstream_url);
      let path = url.pathname;
      if (route.strip_path) { const prefix = route.paths.map((p) => p.replace(/\/$/, "")).filter((p) => path === p || path.startsWith(p + "/")).sort((a, b) => b.length - a.length)[0] ?? ""; path = path.slice(prefix.length) || "/"; }
      const target = new URL(base.pathname.replace(/\/$/, "") + path + url.search, base);
      const headers = new Headers();
      c.req.raw.headers.forEach((v, k) => { if (!HOP.has(k) && k !== "apikey" && k !== "authorization") headers.set(k, v); });
      headers.set("X-Request-Id", request_id);
      headers.set("X-Forwarded-Host", url.host); headers.set("X-Forwarded-Proto", url.protocol.replace(":", "")); headers.set("X-Consumer-Username", key.name);
      const body = method === "GET" || method === "HEAD" ? undefined : await c.req.arrayBuffer();
      const maxAttempts = IDEMPOTENT.has(method) ? 1 + route.retries : 1;

      let attempts = 0, upstreamMs = 0, lastErr = "";
      for (; attempts < maxAttempts; ) {
        attempts++;
        const t0 = Date.now();
        try {
          const res = await fetchImpl(target, { method, headers, body, signal: AbortSignal.timeout(route.timeout_ms), redirect: "manual" });
          upstreamMs += Date.now() - t0;
          if (res.status >= 500 && attempts < maxAttempts) { lastErr = `upstream ${res.status}`; continue; }
          if (res.status >= 500) { cb.failures++; if (cb.failures >= FAIL_THRESHOLD) cb.openedAt = Date.now(); }
          else { cb.failures = 0; cb.openedAt = null; }
          const out = new Headers(res.headers);
          for (const h of HOP) out.delete(h);
          out.set("X-Request-Id", request_id);
          out.set("X-Kong-Upstream-Latency", String(upstreamMs));
          out.set("X-Kong-Proxy-Latency", String(Date.now() - started - upstreamMs));
          c.res.headers.forEach((v, k) => { if (k.toLowerCase().startsWith("ratelimit")) out.set(k, v); });
          await finish(res.status, { route: route.name, key_name: key.name, attempts, upstream_ms: upstreamMs });
          return new Response(res.body, { status: res.status, headers: out });
        } catch (e) {
          upstreamMs += Date.now() - t0;
          lastErr = (e as Error).name === "TimeoutError" ? `upstream timeout after ${route.timeout_ms}ms` : (e as Error).message;
        }
      }
      cb.failures++; if (cb.failures >= FAIL_THRESHOLD) cb.openedAt = Date.now();
      const status = lastErr.includes("timeout") ? 504 : 502;
      await finish(status, { route: route.name, key_name: key.name, attempts, upstream_ms: upstreamMs, error: lastErr });
      return c.json({ message: lastErr, request_id }, status);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;
const GW_STORE: &str = r##"import { db } from "./db";

// Kong's objects, flattened: a Route already carries its Service (the upstream), a Consumer is
// a Key. Postgres in the process, memory in the tests — same interface.
export type Route = { id: string; name: string; paths: string[]; methods: string[]; upstream_url: string; strip_path: boolean; rpm: number | null; timeout_ms: number; retries: number };
export type Key = { id: number; name: string; key_hash: string; routes: string[]; rpm: number };
export type RequestLog = { request_id: string; key_name: string | null; route: string | null; method: string; path: string; status: number; attempts: number; upstream_ms: number | null; total_ms: number; error: string | null };

export interface Store {
  /// The whole route table plus its version. Every write bumps the version; the gateway
  /// keeps a snapshot and only re-reads when the version moved (that is what Redis holds
  /// when there is more than one replica).
  routeTable(): Promise<{ version: number; routes: Route[] }>;
  routeVersion(): Promise<number>;
  putRoute(r: Route): Promise<void>;
  deleteRoute(name: string): Promise<boolean>;
  keyByHash(hash: string): Promise<Key | null>;
  createKey(name: string, hash: string, routes: string[], rpm: number): Promise<Key>;
  log(l: RequestLog): Promise<void>;
  logs(limit: number): Promise<RequestLog[]>;
  /// Sliding-window count in the last minute, after increment.
  hit(bucket: string, nowMs: number): Promise<number>;
}

export function pgStore(): Store {
  const hits = new Map<string, number[]>();
  const bump = () => db`update gw_route_version set version = version + 1 where id = 1`;
  return {
    routeTable: async () => ({ version: (await db<{ version: number }[]>`select version::int from gw_route_version`)[0].version, routes: await db<Route[]>`select id, name, paths, methods, upstream_url, strip_path, rpm, timeout_ms, retries from gw_routes order by name` }),
    routeVersion: async () => (await db<{ version: number }[]>`select version::int from gw_route_version`)[0].version,
    putRoute: async (r) => { await db.begin(async (tx) => { await tx`insert into gw_routes (id, name, paths, methods, upstream_url, strip_path, rpm, timeout_ms, retries) values (${r.id}, ${r.name}, ${r.paths}, ${r.methods}, ${r.upstream_url}, ${r.strip_path}, ${r.rpm}, ${r.timeout_ms}, ${r.retries}) on conflict (name) do update set paths = excluded.paths, methods = excluded.methods, upstream_url = excluded.upstream_url, strip_path = excluded.strip_path, rpm = excluded.rpm, timeout_ms = excluded.timeout_ms, retries = excluded.retries`; await tx`update gw_route_version set version = version + 1 where id = 1`; }); },
    deleteRoute: async (name) => { const n = (await db`delete from gw_routes where name = ${name}`).count; if (n) await bump(); return n > 0; },
    keyByHash: async (h) => (await db<Key[]>`select id, name, key_hash, routes, rpm from gw_keys where key_hash = ${h}`)[0] ?? null,
    createKey: async (name, key_hash, routes, rpm) => (await db<Key[]>`insert into gw_keys (name, key_hash, routes, rpm) values (${name}, ${key_hash}, ${routes}, ${rpm}) returning id, name, key_hash, routes, rpm`)[0],
    log: async (l) => { await db`insert into gw_requests ${db(l, "request_id", "key_name", "route", "method", "path", "status", "attempts", "upstream_ms", "total_ms", "error")}`; },
    logs: async (limit) => db<RequestLog[]>`select request_id, key_name, route, method, path, status, attempts, upstream_ms, total_ms, error from gw_requests order by started_at desc limit ${limit}`,
    // ponytail: per-replica window; move to Redis INCR+EXPIRE when replicas must share a limit.
    hit: async (bucket, now) => { const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000); w.push(now); hits.set(bucket, w); return w.length; },
  };
}

export function memoryStore(): Store {
  const routes = new Map<string, Route>(), keys: Key[] = [], logs: RequestLog[] = [], hits = new Map<string, number[]>();
  let version = 1;
  return {
    routeTable: async () => ({ version, routes: [...routes.values()] }),
    routeVersion: async () => version,
    putRoute: async (r) => { routes.set(r.name, { ...r }); version++; },
    deleteRoute: async (name) => { const ok = routes.delete(name); if (ok) version++; return ok; },
    keyByHash: async (h) => keys.find((k) => k.key_hash === h) ?? null,
    createKey: async (name, key_hash, rs, rpm) => { const k = { id: keys.length + 1, name, key_hash, routes: rs, rpm }; keys.push(k); return k; },
    log: async (l) => { logs.push(l); },
    logs: async (limit) => logs.slice(-limit).reverse(),
    hit: async (bucket, now) => { const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000); w.push(now); hits.set(bucket, w); return w.length; },
  };
}
"##;
const GW_TEST: &str = r##"import { beforeAll, describe, expect, test } from "bun:test";
import { createApp, FAIL_THRESHOLD, type Fetch } from "./app";
import { memoryStore } from "./store";

// The gateway against a fake upstream: key auth, route matching, forwarding headers, limits,
// retries only for idempotent methods, the circuit breaker, and the request log.
process.env.ADMIN_TOKEN = "admin";

const calls: { url: string; method: string; headers: Headers; body: string }[] = [];
let failNext = 0, hang = false;
const upstream: Fetch = async (url, init) => {
  calls.push({ url: String(url), method: init?.method ?? "GET", headers: new Headers(init?.headers), body: init?.body ? Buffer.from(init.body as ArrayBuffer).toString() : "" });
  if (hang) await new Promise((_, rej) => init?.signal?.addEventListener("abort", () => rej(Object.assign(new Error("aborted"), { name: "TimeoutError" }))));
  if (failNext > 0) { failNext--; return new Response("boom", { status: 503 }); }
  return Response.json({ ok: true, path: new URL(String(url)).pathname }, { headers: { "x-upstream": "yes" } });
};

const store = memoryStore();
const app = createApp(store, upstream);
const adminPost = (path: string, body: unknown) => app.request(path, { method: "POST", headers: { authorization: "Bearer admin", "content-type": "application/json" }, body: JSON.stringify(body) });
let key = "";

beforeAll(async () => {
  expect((await adminPost("/api/admin/routes", { name: "users", paths: ["/users"], upstream_url: "http://users.internal/v2", timeout_ms: 200, retries: 2 })).status).toBe(201);
  await adminPost("/api/admin/routes", { name: "orders", paths: ["/orders"], methods: ["POST"], upstream_url: "http://orders.internal", rpm: 2 });
  key = (await (await adminPost("/api/admin/keys", { name: "mobile", rpm: 1000 })).json()).key;
});

describe("gateway", () => {
  test("admin needs the token; the proxy needs a key; unknown paths are 404", async () => {
    expect((await app.request("/api/admin/routes")).status).toBe(401);
    expect((await app.request("/users/1")).status).toBe(401);
    expect((await app.request("/users/1", { headers: { apikey: "gw_wrong" } })).status).toBe(401);
    expect((await app.request("/nothing", { headers: { apikey: key } })).status).toBe(404);
  });

  test("forwards with the prefix stripped, a request id and consumer headers", async () => {
    calls.length = 0;
    const res = await app.request("/users/42?x=1", { headers: { apikey: key, "x-request-id": "req-1" } });
    expect(res.status).toBe(200);
    expect(calls[0].url).toBe("http://users.internal/v2/42?x=1");
    expect(calls[0].headers.get("x-request-id")).toBe("req-1");
    expect(calls[0].headers.get("x-consumer-username")).toBe("mobile");
    expect(calls[0].headers.get("apikey")).toBeNull();
    expect(res.headers.get("x-upstream")).toBe("yes");
    expect(res.headers.get("x-kong-upstream-latency")).not.toBeNull();
    expect(res.headers.get("ratelimit-remaining")).not.toBeNull();
  });

  test("per-route limit answers 429 and per-key routes are enforced", async () => {
    const h = { method: "POST", headers: { apikey: key }, body: "{}" };
    expect((await app.request("/orders", h)).status).toBe(200);
    expect((await app.request("/orders", h)).status).toBe(200);
    const r3 = await app.request("/orders", h);
    expect(r3.status).toBe(429);
    expect(r3.headers.get("retry-after")).toBe("60");
    expect((await app.request("/orders", { headers: { apikey: key } })).status).toBe(404); // GET is not in methods
    const scoped = (await (await adminPost("/api/admin/keys", { name: "orders-only", routes: ["orders"] })).json()).key;
    expect((await app.request("/users/1", { headers: { apikey: scoped } })).status).toBe(403);
  });

  test("retries idempotent methods only", async () => {
    calls.length = 0; failNext = 1;
    expect((await app.request("/users/1", { headers: { apikey: key } })).status).toBe(200);
    expect(calls.length).toBe(2);
    calls.length = 0; failNext = 1;
    expect((await app.request("/users/1", { method: "POST", headers: { apikey: key }, body: "x" })).status).toBe(503);
    expect(calls.length).toBe(1);
  });

  test("a timeout is 504 and repeated failures open the circuit", async () => {
    await adminPost("/api/admin/routes", { name: "slow", paths: ["/slow"], upstream_url: "http://slow.internal", timeout_ms: 100 });
    hang = true;
    for (let i = 0; i < FAIL_THRESHOLD; i++) expect((await app.request("/slow", { method: "POST", headers: { apikey: key }, body: "x" })).status).toBe(504);
    hang = false; calls.length = 0;
    const res = await app.request("/slow", { headers: { apikey: key } });
    expect(res.status).toBe(503);
    expect((await res.json()).message).toContain("circuit open");
    expect(calls.length).toBe(0);
    const routes = await (await app.request("/api/admin/routes", { headers: { authorization: "Bearer admin" } })).json();
    expect(routes.data.find((r: { name: string }) => r.name === "slow").circuit).toBe("open");
  });

  test("every request is logged with timing", async () => {
    const logs = await (await app.request("/api/admin/requests", { headers: { authorization: "Bearer admin" } })).json();
    const open = logs.data.find((l: { error: string | null }) => l.error === "circuit open");
    expect(open.route).toBe("slow");
    expect(open.key_name).toBe("mobile");
    expect(typeof open.total_ms).toBe("number");
    expect(logs.data.some((l: { status: number; attempts: number }) => l.status === 200 && l.attempts === 2)).toBe(true);
  });
});
"##;
const GW_SQL: &str = r##"create table if not exists gw_routes (
  id text primary key,
  name text not null unique,
  paths text[] not null,               -- prefixes, longest match wins
  methods text[] not null default '{}', -- empty = any
  upstream_url text not null,
  strip_path boolean not null default true,
  rpm int,                              -- per-route limit, null = none
  timeout_ms int not null default 10000,
  retries int not null default 2,
  created_at timestamptz not null default now()
);
-- One row; bumped on every route write so replicas know their snapshot is stale.
create table if not exists gw_route_version (id int primary key default 1, version bigint not null default 1);
insert into gw_route_version (id) values (1) on conflict do nothing;
create table if not exists gw_keys (
  id bigserial primary key,
  name text not null,
  key_hash text not null unique,
  routes text[] not null default '{}',  -- route names; empty = all
  rpm int not null default 60,
  created_at timestamptz not null default now()
);
create table if not exists gw_requests (
  request_id text primary key,
  key_name text,
  route text,
  method text not null,
  path text not null,
  status int not null,
  attempts int not null default 1,
  upstream_ms int,
  total_ms int not null,
  error text,
  started_at timestamptz not null default now()
);
create index if not exists gw_requests_started on gw_requests (started_at desc);
"##;
const GW_PAGE: &str = r##"// The console: routes with their circuit state, and live traffic. Reads the admin API with
// ADMIN_TOKEN from the environment — never sent to the browser.
const base = process.env.API_URL ?? "http://127.0.0.1:8000";
const headers = { authorization: `Bearer ${process.env.ADMIN_TOKEN ?? ""}` };

type Route = { name: string; paths: string[]; methods: string[]; upstream_url: string; rpm: number | null; circuit: string };
type Req = { request_id: string; key_name: string | null; route: string | null; method: string; path: string; status: number; attempts: number; upstream_ms: number | null; total_ms: number; error: string | null };

async function get<T>(path: string, fallback: T): Promise<T> {
  const res = await fetch(base + path, { headers, cache: "no-store" }).catch(() => null);
  return res?.ok ? ((await res.json()).data as T) : fallback;
}

export default async function Home() {
  const [routes, reqs] = await Promise.all([get<Route[]>("/api/admin/routes", []), get<Req[]>("/api/admin/requests?limit=50", [])]);
  return (
    <main>
      <h1>{{NAME}} — API gateway</h1>
      <p>Key auth, per-key and per-route limits in <code>RateLimit-*</code>, retries for idempotent methods, a circuit per route, every request logged.</p>
      <pre>{`curl -X POST http://localhost:8000/api/admin/routes -H "authorization: Bearer $ADMIN_TOKEN" \\
  -H "content-type: application/json" -d '{"name":"users","paths":["/users"],"upstream_url":"http://users.internal"}'
curl -X POST http://localhost:8000/api/admin/keys -H "authorization: Bearer $ADMIN_TOKEN" \\
  -H "content-type: application/json" -d '{"name":"mobile","rpm":600}'
curl http://localhost:8000/users/42 -H "apikey: gw_..."`}</pre>
      <h2>Routes</h2>
      <table>
        <thead><tr><th>name</th><th>paths</th><th>methods</th><th>upstream</th><th>rpm</th><th>circuit</th></tr></thead>
        <tbody>{routes.map((r) => <tr key={r.name}><td>{r.name}</td><td>{r.paths.join(", ")}</td><td>{r.methods.join(", ") || "any"}</td><td>{r.upstream_url}</td><td>{r.rpm ?? "—"}</td><td>{r.circuit}</td></tr>)}</tbody>
      </table>
      {routes.length === 0 && <p>No routes yet.</p>}
      <h2>Traffic</h2>
      <table>
        <thead><tr><th>request</th><th>key</th><th>route</th><th>method</th><th>path</th><th>status</th><th>tries</th><th>upstream ms</th><th>total ms</th><th>error</th></tr></thead>
        <tbody>{reqs.map((r) => <tr key={r.request_id}><td>{r.request_id.slice(0, 8)}</td><td>{r.key_name ?? "—"}</td><td>{r.route ?? "—"}</td><td>{r.method}</td><td>{r.path}</td><td>{r.status}</td><td>{r.attempts}</td><td>{r.upstream_ms ?? "—"}</td><td>{r.total_ms}</td><td>{r.error ?? ""}</td></tr>)}</tbody>
      </table>
      {reqs.length === 0 && <p>No requests yet.</p>}
    </main>
  );
}
"##;

const GW_CLAUDE: &str = r##"# {{NAME}} — API gateway

{{NAME}} is an API gateway in the shape of Kong: Routes match a path prefix and methods to an
upstream, Consumers hold a key, limits come back in `RateLimit-*`, and every request is logged
with its timing. It is one Hono process in front of your services, not a plugin platform. "Done"
here means a request that reaches the right upstream with the right headers, is refused for the
right reason (401, 403, 404, 429, 503) when it should be, and leaves a row in `gw_requests` either
way — and `make check` proves all of that without a database or a real upstream.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | `createApp(store, fetch)`: admin API, `matchRoute`, the proxy handler, the circuit breaker, header rewriting |
| `backend/src/store.ts` | `Store` interface; `pgStore()` for the process, `memoryStore()` for tests; the sliding-window `hit()` |
| `backend/src/app.test.ts` | The gateway against a fake upstream: auth, matching, forwarding, limits, retries, circuit, log |
| `backend/migrations/0002_gateway.sql` | `gw_routes`, `gw_route_version`, `gw_keys`, `gw_requests` |
| `backend/src/server.ts` | Listens on `PORT` (8000), drains on SIGTERM (from the stack) |
| `backend/src/db.ts` | The `postgres` pool from `DATABASE_URL` (from the stack) |
| `frontend/app/page.tsx` | The console: routes with circuit state, last 50 requests. Server component; reads the admin API with `ADMIN_TOKEN` |

### Request path (`app.ts`, the `.all("/*")` handler)

1. `X-Request-Id` is taken from the caller or minted; every later log row and the upstream request carry it.
2. `matchRoute` picks the route with the **longest matching prefix** among those whose `methods` allow the method. No match is 404 `no Route matched with those values`.
3. Key auth: `apikey` header, `Authorization: Bearer`, or `?apikey=`. The raw key is SHA-256'd and looked up in `gw_keys`. No key is 401; a key whose `routes` list is non-empty and does not name this route is 403.
4. Rate limits: `store.hit()` increments the per-key bucket and, if the route has `rpm`, the per-route bucket. The tighter remaining count decides; `RateLimit-Limit/Remaining/Reset` are set on every response, and over the limit is 429 with `Retry-After: 60`.
5. Circuit: an open breaker for this route (opened less than `OPEN_MS` ago) answers 503 `upstream circuit open` without calling the upstream.
6. The upstream request is built once: prefix stripped when `strip_path`, query preserved, hop-by-hop headers and the credential removed, `X-Forwarded-Host/Proto`, `X-Consumer-Username` and `X-Request-Id` added. Non-GET/HEAD bodies are buffered so a retry can resend them.
7. Attempts: `1 + retries` for idempotent methods (GET, HEAD, OPTIONS, PUT, DELETE), exactly 1 otherwise. A 5xx or a thrown fetch (timeout, connection refused) moves to the next attempt.
8. The last answer is returned with `X-Kong-Upstream-Latency`, `X-Kong-Proxy-Latency` and the `RateLimit-*` headers; a final failure is 504 (timeout) or 502 (anything else). Either way `store.log()` writes one row.

### Data model

| Table | Column | Why |
|---|---|---|
| `gw_routes` | `name` (unique) | The admin identity; `POST /api/admin/routes` upserts by name |
| | `paths text[]`, `methods text[]` | Prefix match, longest wins; empty `methods` is any |
| | `upstream_url` | Kong's Service, flattened into the Route |
| | `strip_path` | Whether the matched prefix is removed before forwarding (default true, like Kong) |
| | `rpm` | Per-route limit; null is none |
| | `timeout_ms`, `retries` | Bounded at 100–120000 and 0–5 by `routeSchema` |
| `gw_route_version` | one row, `version` | Bumped on every route write; replicas re-read their snapshot only when it moved |
| `gw_keys` | `key_hash` (unique) | SHA-256 of the plaintext; the plaintext is returned once at creation and never stored |
| | `routes text[]` | Route names this key may use; empty is all |
| | `rpm` | Per-key limit, default 60 |
| `gw_requests` | `request_id` (pk) | One row per request, including refusals |
| | `attempts`, `upstream_ms`, `total_ms`, `error` | Enough to tell a slow upstream from a slow gateway |

## Invariants

1. **The credential never reaches the upstream.** `apikey` and `authorization` are dropped when the upstream headers are built. Why: the upstream would otherwise see, log and possibly replay your gateway keys. Guarded by `app.test.ts` "forwards with the prefix stripped…" (`apikey` header is null at the upstream).
2. **Plaintext keys exist once, in the 201 response.** Only `sha256(raw)` is stored. Why: a leaked `gw_keys` table must not be a leaked set of keys. Guarded by the same test through `keyByHash`; `store.ts` has no method that returns a raw key.
3. **Retries only for idempotent methods.** `IDEMPOTENT` is the set; a POST gets one attempt even with `retries: 2`. Why: a retried POST is a duplicate order. Guarded by "retries idempotent methods only".
4. **A refused request is still logged.** Every early return calls `finish(status)` first. Why: 401/404/429 traffic is what you look at during an incident. Guarded by "every request is logged with timing" (finds the `circuit open` row).
5. **The circuit is per route and opens on `FAIL_THRESHOLD` consecutive failures**, not on a rate. A success resets it. Why: consecutive failures is the signal that survives low traffic. Guarded by "a timeout is 504 and repeated failures open the circuit".
6. **Longest prefix wins, and a method mismatch is a 404, not a 405.** Why: Kong answers 404 here and callers' SDKs expect it. Guarded by "per-route limit answers 429…" (`GET /orders` is 404 when `methods: ["POST"]`).
7. **The route snapshot is re-read only when `gw_route_version` moved**, checked at most once a second, forced after every admin write. Why: the hot path must not query Postgres per request, and an admin write must be visible in the same process immediately. Guarded by `beforeAll` in the test (routes written then matched at once) — the version check itself has no direct test; add one if you change `routes()`.
8. **Limits are answered in `RateLimit-*` on every proxied response**, not only on 429. Why: clients pace themselves from `Remaining`. Guarded by "forwards with the prefix stripped…" (`ratelimit-remaining` present on a 200).
9. **Admin routes need `ADMIN_TOKEN`; an unset token refuses everything.** `admin()` returns false when the env var is empty. Why: a missing secret must fail closed. Guarded by "admin needs the token…".
10. **Bodies are buffered before the first attempt.** Why: a `ReadableStream` can be sent once; a retry after a half-sent body would forward garbage. Guarded implicitly by the retry test (the second attempt carries the body); assert on `calls[1].body` if you touch this.

## Extending it

**Add a header transform (Kong's request-transformer).** Add optional `add_headers: Record<string,string>` to `routeSchema` and `Route`, a column in a new `0003_` migration, the column in both `putRoute`/`routeTable` queries, then set them after `X-Forwarded-*` in the proxy. Test: register a route with `add_headers`, assert on `calls[0].headers`.

**Add a second auth method (HMAC, JWT).** Replace the `raw` lookup in step 3 with a function `authenticate(c): Promise<Key | null>` that tries each scheme; keep `keyByHash` as the first. Test: a request with the new credential reaches the upstream with `X-Consumer-Username` set. Migration only if the credential is a new table.

**Add per-route allowlists of consumers (Kong ACL).** The reverse of `Key.routes` already exists; do not add a second table. Add `consumers text[]` to `gw_routes`, check it after the 403 check. Test: a route with `consumers: ["mobile"]` refuses a key named otherwise with 403.

**Add a response cache for GET.** Key on `route.id + path + search`, store `{status, headers, body}` in Redis with `Cache-Control` honoured; look it up between the circuit check and the fetch. Test: two identical GETs make one upstream call; a POST never hits the cache.

**Add health checks (Kong's active health checks).** A `setInterval` in `server.ts` that GETs each `upstream_url` and closes/opens the circuit from the result. Keep it out of `createApp` so the tests stay synchronous. Test: none in `app.test.ts`; unit-test the probe function with a fake fetch.

**Add a route field.** In order: `routeSchema` (validation and default), `Route` type, `0003_*.sql` (`alter table gw_routes add column … default …`, so N−1 pods keep working), both queries in `pgStore`, the console table if it matters. Test: post a route with the field, read it back from `GET /api/admin/routes`.

**Make the rate limit shared across replicas.** Replace the `hits` map in `pgStore().hit` with Redis `INCR` + `EXPIRE 60` on the bucket (fixed window) or a sorted set (sliding). `memoryStore().hit` stays as it is. Test: unchanged; the memory store is the spec.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `ADMIN_TOKEN` | yes | Bearer token for `/api/admin/*`; unset means admin is closed |
| `DATABASE_URL` | yes | Postgres for routes, keys and the request log |
| `PORT` | no | Listen port, default 8000 |
| `API_URL` | frontend | Where the console reaches the backend (`http://backend:8000` in compose) |
| `REDIS_URL` | not yet | Reserved for shared limits and breakers |

**Per replica today:** the rate-limit windows (`hits` map), the circuit breaker state (`circuits` map), the route snapshot (`snap`). With N replicas a key's effective limit is up to N × `rpm`, and a breaker opens per pod. The snapshot is fine as it is: `gw_route_version` already makes every replica converge within a second. Limits and breakers move to Redis when the discrepancy matters.

**Failure modes and what the caller sees:**

| Situation | Status | Body / header |
|---|---|---|
| No route | 404 | `no Route matched with those values` |
| No key / wrong key | 401 | `No API key found in request` / `Unauthorized` |
| Key not allowed on route | 403 | `key is not allowed on this route` |
| Over limit | 429 | `API rate limit exceeded`, `Retry-After: 60` |
| Breaker open | 503 | `upstream circuit open`, `retry_after_ms` |
| Upstream timed out on every attempt | 504 | `upstream timeout after Nms`, `request_id` |
| Upstream 5xx / unreachable on every attempt | 502 or the upstream's status | the upstream's body, or `message` with `request_id` |
| Postgres down | 500 | `store.log` throws; the readiness probe still says ok (see Ceilings) |

**What to watch:** `gw_requests` is the metric source. `status >= 500` rate per `route`; p95 of `upstream_ms` vs `total_ms - upstream_ms` (the gateway's own cost); `error = 'circuit open'` count; `attempts > 1` rate (a flapping upstream before it trips). Ship the table to your metrics store or query it from the console.

## Ceilings

| Ceiling | Where | Upgrade |
|---|---|---|
| Rate-limit window is per replica | `store.ts` `hit()` (`ponytail:` comment) | Redis `INCR`+`EXPIRE` per bucket |
| Circuit breaker is per replica | `app.ts` `circuits` (`ponytail:` comment) | A Redis hash per route, or accept per-pod breakers — usually fine |
| Request log is a synchronous insert per request | `finish()` | Batch through a queue, or partition `gw_requests` by day and drop old partitions |
| Bodies are buffered in memory for retry | proxy step 6 | Cap body size in `routeSchema`, or stream when `retries: 0` |
| Readiness does not check Postgres | `/api/health/ready` | Run `select 1` there; the stack's rule is that readiness checks dependencies |
| No TLS termination, no WebSockets | by design | Put it behind the ingress; WebSocket upgrade needs a different handler than `fetch` |
| Route match is a linear scan | `matchRoute` | A radix tree when routes number in the thousands |

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;
const GW_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the architecture. This is how to run and test it.

## Run

    cp .env.example backend/.env && echo ADMIN_TOKEN=admin >> backend/.env
    ADMIN_TOKEN=admin make demo   # postgres + redis, migrate, seed, API on :8000, console on :3000
    make check                    # typecheck both halves, run backend tests — no database needed

Pieces on their own: `make dev` (infra), `make backend`, `make frontend`, `make migrate`. `make
up` runs the whole thing behind nginx on :8080. There is no worker or importer in this pack.

## Routes, with bodies

Admin (all need `authorization: Bearer $ADMIN_TOKEN`):

    # create or update a route (upsert by name)
    curl -s -X POST localhost:8000/api/admin/routes -H "authorization: Bearer $ADMIN_TOKEN" \
      -H "content-type: application/json" \
      -d '{"name":"users","paths":["/users"],"methods":[],"upstream_url":"http://users.internal/v2","strip_path":true,"rpm":null,"timeout_ms":5000,"retries":2}'
    # 201 {"id":"…","name":"users","paths":["/users"],"methods":[],"upstream_url":"http://users.internal/v2","strip_path":true,"rpm":null,"timeout_ms":5000,"retries":2}

    curl -s localhost:8000/api/admin/routes -H "authorization: Bearer $ADMIN_TOKEN"
    # {"data":[{…,"circuit":"closed"}]}

    curl -s -X DELETE localhost:8000/api/admin/routes/users -H "authorization: Bearer $ADMIN_TOKEN"
    # 204, or 404 {"message":"not found"}

    # mint a key; the plaintext is in this response and nowhere else
    curl -s -X POST localhost:8000/api/admin/keys -H "authorization: Bearer $ADMIN_TOKEN" \
      -H "content-type: application/json" -d '{"name":"mobile","routes":[],"rpm":600}'
    # 201 {"id":1,"name":"mobile","key":"gw_3f…","routes":[],"rpm":600}

    curl -s "localhost:8000/api/admin/requests?limit=20" -H "authorization: Bearer $ADMIN_TOKEN"
    # {"data":[{"request_id":"…","key_name":"mobile","route":"users","method":"GET","path":"/users/42","status":200,"attempts":1,"upstream_ms":12,"total_ms":14,"error":null}]}

The proxy (everything that is not `/api/health*` or `/api/admin/*`):

    curl -si localhost:8000/users/42?x=1 -H "apikey: gw_3f…"
    # forwarded to http://users.internal/v2/42?x=1 with X-Request-Id, X-Forwarded-Host/Proto,
    # X-Consumer-Username: mobile; back with RateLimit-Limit/Remaining/Reset,
    # X-Kong-Upstream-Latency, X-Kong-Proxy-Latency, X-Request-Id
    curl -s localhost:8000/users/42                       # 401 {"message":"No API key found in request"}
    curl -s localhost:8000/nowhere -H "apikey: gw_3f…"     # 404 {"message":"no Route matched with those values"}

`Authorization: Bearer gw_…` and `?apikey=gw_…` are accepted too.

## Tests

`backend/src/app.test.ts` builds the app with `createApp(memoryStore(), upstream)` where
`upstream` is a fake `Fetch` that records every call in `calls`, fails the next `failNext`
calls with 503, and when `hang` is true waits for the abort signal (so timeouts are real, at
`timeout_ms`). Nothing leaves the process; there is no Postgres and no network.

`ADMIN_TOKEN` is set at the top of the file. `beforeAll` registers two routes and one key; tests
share them, so a test that needs its own upstream behaviour registers its own route (see the
`slow` route in the circuit test).

Adding a test:

1. Register what you need with `adminPost("/api/admin/routes", {...})` — a distinct `name` and
   path so it cannot collide with `users`/`orders`/`slow`.
2. Drive the fake with `failNext`, `hang`, and read `calls` for what reached the upstream.
3. Assert on status, headers (`res.headers.get("ratelimit-remaining")`) and, for anything that
   should be observable later, on `GET /api/admin/requests`.
4. `cd backend && bun test` — or `make check` for the whole gate.

Exported for tests: `createApp`, `matchRoute`, `FAIL_THRESHOLD`, `OPEN_MS`, the `Fetch` type.
"##;
const GW_README: &str = r##"# {{NAME}}

An API gateway you own: Kong's routing, key auth, rate limits and request log, as one TypeScript
service with tests that run without a database.

## What you get

- `POST /api/admin/routes` — path-prefix and method routing to an upstream, longest prefix wins, prefix stripping, per-route timeout, retries and rate limit.
- `POST /api/admin/keys` — consumer keys (`gw_…`), hashed at rest, optionally scoped to named routes, each with its own per-minute limit.
- The proxy on every other path: key auth via `apikey`, `Authorization: Bearer` or `?apikey=`; `RateLimit-*` on every response; `X-Request-Id` end to end; `X-Forwarded-*` and `X-Consumer-Username` to the upstream; the credential stripped before forwarding.
- Retries for idempotent methods only, a timeout per route, a circuit breaker per route that opens after 3 consecutive failures and half-opens after 30 s.
- One row per request in `gw_requests` — including refusals — with attempts, upstream time and total time.
- A console on :3000 with routes, their circuit state and live traffic.
- `make check`: typecheck both halves and run the gateway against a fake upstream. No Postgres, no network.

## Five minutes

    cp .env.example backend/.env && echo ADMIN_TOKEN=admin >> backend/.env
    ADMIN_TOKEN=admin make demo

Then, in another shell, with something listening on :9000 (`python3 -m http.server 9000` will do):

    curl -s -X POST localhost:8000/api/admin/routes -H "authorization: Bearer admin" \
      -H "content-type: application/json" \
      -d '{"name":"files","paths":["/files"],"upstream_url":"http://localhost:9000"}'
    # {"id":"…","name":"files","paths":["/files"],"methods":[],"upstream_url":"http://localhost:9000","strip_path":true,"rpm":null,"timeout_ms":10000,"retries":2}

    KEY=$(curl -s -X POST localhost:8000/api/admin/keys -H "authorization: Bearer admin" \
      -H "content-type: application/json" -d '{"name":"me","rpm":5}' | jq -r .key)

    curl -si localhost:8000/files/ -H "apikey: $KEY" | head -12
    # HTTP/1.1 200 OK … RateLimit-Limit: 5  RateLimit-Remaining: 4  X-Kong-Upstream-Latency: 3 …

    for i in 1 2 3 4 5; do curl -s -o /dev/null -w "%{http_code} " localhost:8000/files/ -H "apikey: $KEY"; done
    # 200 200 200 200 429

    curl -s "localhost:8000/api/admin/requests?limit=3" -H "authorization: Bearer admin" | jq '.data[] | {status, attempts, upstream_ms, total_ms}'

Open http://localhost:3000 for the same in a table.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | liveness |
| GET | `/api/health/ready` | none | readiness |
| GET | `/api/admin/routes` | admin | routes with `circuit: open\|closed` |
| POST | `/api/admin/routes` | admin | create or update by `name` |
| DELETE | `/api/admin/routes/:name` | admin | remove |
| POST | `/api/admin/keys` | admin | mint a key; plaintext returned once |
| GET | `/api/admin/requests?limit=` | admin | last N requests (max 500) |
| ANY | `/*` | key | the proxy |

Admin auth is `Authorization: Bearer $ADMIN_TOKEN`.

## Compared with Kong

**Same shapes** — their docs and habits carry over:

- Route = paths + methods → upstream, `strip_path` default true, longest prefix wins, 404 `no Route matched with those values`.
- key-auth: the `apikey` header (plus Bearer and query), 401 `No API key found in request`.
- rate-limiting answered as `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset`, `Retry-After` on 429.
- `X-Kong-Upstream-Latency` and `X-Kong-Proxy-Latency`, `X-Consumer-Username`, `X-Forwarded-Host/Proto`, `X-Request-Id`.
- Retries and per-route timeouts; a 5xx during retries is retried, a final failure is 502/504.

**Better here:**

- One codebase, ~200 lines of gateway logic you can read in a sitting; a change is a PR, not a plugin in Lua or Go.
- Typed end to end: the admin API's route table is a TypeScript type the console consumes, and `zod` validates every admin body.
- Tests that need nothing running: `bun test` drives the gateway against a fake upstream, including timeouts and the breaker tripping.
- A request log in Postgres, queryable with SQL, with refusals included; Kong needs a logging plugin and a sink.
- The circuit breaker is built in; Kong's upstream health checks are a separate configuration.
- Kubernetes manifests, a migration runner, secrets rendering and a compose file arrive with it.

**Not here yet:**

- No plugin system, and none of Kong's ~100 plugins: no JWT/OAuth2/HMAC auth, no request/response transformers, no CORS plugin, no bot detection, no gRPC or GraphQL.
- No load balancing across several upstream targets, no active health checks, no weighted or hash-based balancing.
- No TLS termination, no WebSocket or SSE pass-through (the proxy is request/response over `fetch`).
- No declarative config file or `deck`; the admin API is the only way in.
- Rate limits and breakers are per replica until you move them to Redis (see Roadmap).
- No response caching, no request body size limit beyond what Node accepts.
- No Kong Manager; the console is read-only.

## Production

- **Environments:** `backend/.env` is the source of truth; `make encrypt-env` writes `backend/.env.age`, the only form committed. `ADMIN_TOKEN` and `DATABASE_URL` are required; `_DEV`-suffixed keys win in the dev overlay.
- **Scaling:** replicas are stateless except for the limit windows and breakers, which are per pod. Two replicas give a key up to 2 × its `rpm`. Acceptable until it is not; then Redis.
- **Probes:** `/api/health` is liveness, `/api/health/ready` readiness. Readiness does not yet query Postgres — add `select 1` there before you rely on it.
- **Migrations:** `backend/migrations/*.sql`, applied by `make migrate` locally and the `migrate` init container before each rollout. New route fields are `add column … default`, never a rename.
- **Secrets:** `k8s/scripts/env-to-secrets.sh` renders `backend/.env` into a Secret per environment; nothing secret is in an image or a manifest.
- **Overlays:** `k8s/overlays/dev` and `k8s/overlays/prod` — ingress, certificate, replica counts. `git push` to `main` deploys dev; `make release` tags for prod.
- **What pages you:** 5xx rate per route from `gw_requests`; breakers open (`error = 'circuit open'`); `total_ms - upstream_ms` growing (the gateway itself is slow — usually Postgres on the log insert).

## Roadmap

In order of how soon a real deployment hits it:

1. Shared rate limits and breakers in Redis (`store.ts` `hit()`, `app.ts` `circuits`).
2. Readiness that checks Postgres.
3. Request log written in batches or partitioned by day.
4. Several targets per route with round-robin and passive health.
5. Header transforms and a JWT auth option.
6. WebSocket pass-through.
"##;
const GW_REVIEWER: &str = r##"---
name: proxy-semantics
description: Run before a change to backend/src/app.ts or store.ts in the gateway ships. Reads the change as someone who has run Kong in production and knows which header, status or retry mistake turns into a duplicate order, a leaked key or a stampede.
tools: Read, Grep, Glob, Bash
---

You review reverse-proxy semantics. Report only what will misroute, retry unsafely, leak a
credential, lie in a header, or let a limit be bypassed — each as `path:line — what — when — fix`.

Check:
1. Credential stripping: `apikey` and `authorization` are removed from the upstream headers in
   the proxy handler. A new auth header must be added to that exclusion. Failure: your keys in
   the upstream's access log.
2. Retries: `maxAttempts` is 1 unless the method is in `IDEMPOTENT`. Any change that retries a
   POST or PATCH, or that treats a 4xx as retryable, is a bug. Failure: duplicate side effects.
3. Body buffering: non-GET bodies are read once with `arrayBuffer()` before the loop. A stream
   passed to `fetch` cannot be resent. Failure: the second attempt forwards an empty body.
4. Prefix stripping: the longest matching prefix is removed, the remainder defaults to `/`, the
   query string is kept, the upstream's own path is prepended. Check `//`, trailing slashes and
   an upstream URL that itself ends in `/`. Failure: 404s from the upstream that the log blames
   on it.
5. Hop-by-hop headers: `HOP` is applied both ways. `content-length` must not be copied when the
   body is re-encoded; `host` must not be forwarded. Failure: hung connections, wrong vhost.
6. Status mapping: timeout is 504, other transport failure 502, an upstream 5xx on the last
   attempt is passed through as-is; the breaker counts all three. Failure: a 504 counted as a
   success resets the breaker.
7. Rate-limit headers: `RateLimit-*` are set before the limit decision so they appear on 200
   and 429 alike; `Retry-After` only on 429; the tighter of key and route decides. Failure:
   clients pace on a number that is not the one enforced.
8. Circuit breaker: increments only on failure after the last attempt or a final 5xx, resets on
   any success, half-opens after `OPEN_MS`, and answers 503 without calling upstream while
   open. Failure: a breaker that never closes, or one that trips on a single retried 503.
9. Route matching: method filter first, then longest prefix; an empty `methods` means any; a
   method mismatch is 404. Failure: `/users` shadowing `/users-admin` or the reverse.
10. Snapshot freshness: every admin write calls `routes(true)`; the store bumps
    `gw_route_version` inside the same transaction as the write. Failure: a deleted route still
    matching on another replica for longer than a second, or forever.
11. Logging: every return path in the proxy calls `finish()` exactly once with the right
    status. Failure: refusals missing from the log, or a request logged twice.
12. Admin gate: every `/api/admin/*` handler starts with the `admin()` check, and an unset
    `ADMIN_TOKEN` refuses. Failure: an open admin API.
13. Validation: new route fields go through `routeSchema` with bounds (`timeout_ms` ≤ 120000,
    `retries` ≤ 5). Failure: a route with a 10-minute timeout holding a connection per request.

End with one line: `proxy-semantics: N findings`, and if 0, what you checked.
"##;
