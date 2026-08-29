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
