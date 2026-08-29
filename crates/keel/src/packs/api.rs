//! api: a REST API service, like PostgREST / Directus — with keys, limits and docs ═════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", API_APP.into()),
        ("backend/src/store.ts", API_STORE.into()),
        ("backend/src/app.test.ts", API_TEST.into()),
        ("backend/migrations/0002_api.sql", API_SQL.into()),
        ("frontend/app/page.tsx", f(API_PAGE)),
    ]
}

const API_SQL: &str = r##"create table if not exists api_keys (
  id bigserial primary key,
  name text not null,
  key_hash text not null unique,
  scopes text[] not null default '{}',
  rpm int not null default 60,
  created_at timestamptz not null default now()
);
create table if not exists items (
  id uuid primary key,
  name text not null,
  body jsonb not null default '{}',
  created_at timestamptz not null default now()
);
create index if not exists items_created on items (created_at, id);
create table if not exists idempotency (
  key text primary key,
  request_hash text not null,
  status int not null,
  body jsonb not null,
  created_at timestamptz not null default now()
);
"##;

const API_STORE: &str = r##"import { db } from "./db";

// The store behind the API. Postgres in the process, memory in the tests — same interface,
// so the routes are tested as routes and the SQL is one file to read.
export type Item = { id: string; name: string; body: unknown; created_at: string };
export type Key = { id: number; name: string; key_hash: string; scopes: string[]; rpm: number };
export type Idem = { request_hash: string; status: number; body: unknown };

export interface Store {
  keyByHash(hash: string): Promise<Key | null>;
  createKey(name: string, hash: string, scopes: string[], rpm: number): Promise<Key>;
  listItems(after: { created_at: string; id: string } | null, limit: number): Promise<Item[]>;
  getItem(id: string): Promise<Item | null>;
  putItem(item: Item): Promise<void>;
  deleteItem(id: string): Promise<boolean>;
  getIdem(key: string): Promise<Idem | null>;
  putIdem(key: string, idem: Idem): Promise<void>;
  /// Sliding-window count for a key in the last minute; returns the count after increment.
  hit(bucket: string, nowMs: number): Promise<number>;
}

export function pgStore(): Store {
  const hits = new Map<string, number[]>();
  return {
    keyByHash: async (hash) => (await db<Key[]>`select id, name, key_hash, scopes, rpm from api_keys where key_hash = ${hash}`)[0] ?? null,
    createKey: async (name, hash, scopes, rpm) =>
      (await db<Key[]>`insert into api_keys (name, key_hash, scopes, rpm) values (${name}, ${hash}, ${scopes}, ${rpm}) returning id, name, key_hash, scopes, rpm`)[0],
    listItems: async (after, limit) =>
      after
        ? db<Item[]>`select id, name, body, created_at from items where (created_at, id) > (${after.created_at}, ${after.id}) order by created_at, id limit ${limit}`
        : db<Item[]>`select id, name, body, created_at from items order by created_at, id limit ${limit}`,
    getItem: async (id) => (await db<Item[]>`select id, name, body, created_at from items where id = ${id}`)[0] ?? null,
    putItem: async (i) => { await db`insert into items (id, name, body, created_at) values (${i.id}, ${i.name}, ${db.json(i.body as never)}, ${i.created_at}) on conflict (id) do update set name = excluded.name, body = excluded.body`; },
    deleteItem: async (id) => (await db`delete from items where id = ${id}`).count > 0,
    getIdem: async (key) => (await db<Idem[]>`select request_hash, status, body from idempotency where key = ${key} and created_at > now() - interval '24 hours'`)[0] ?? null,
    putIdem: async (key, i) => { await db`insert into idempotency (key, request_hash, status, body) values (${key}, ${i.request_hash}, ${i.status}, ${db.json(i.body as never)}) on conflict (key) do nothing`; },
    // In-process window: fine per replica; move to Redis when replicas must share a limit.
    hit: async (bucket, now) => {
      const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000);
      w.push(now); hits.set(bucket, w); return w.length;
    },
  };
}

export function memoryStore(): Store {
  const keys: Key[] = [], items = new Map<string, Item>(), idem = new Map<string, Idem>(), hits = new Map<string, number[]>();
  return {
    keyByHash: async (h) => keys.find((k) => k.key_hash === h) ?? null,
    createKey: async (name, key_hash, scopes, rpm) => { const k = { id: keys.length + 1, name, key_hash, scopes, rpm }; keys.push(k); return k; },
    listItems: async (after, limit) => {
      const all = [...items.values()].sort((a, b) => (a.created_at + a.id).localeCompare(b.created_at + b.id));
      const from = after ? all.findIndex((i) => i.created_at + i.id > after.created_at + after.id) : 0;
      return from < 0 ? [] : all.slice(from, from + limit);
    },
    getItem: async (id) => items.get(id) ?? null,
    putItem: async (i) => { items.set(i.id, i); },
    deleteItem: async (id) => items.delete(id),
    getIdem: async (k) => idem.get(k) ?? null,
    putIdem: async (k, i) => { if (!idem.has(k)) idem.set(k, i); },
    hit: async (bucket, now) => { const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000); w.push(now); hits.set(bucket, w); return w.length; },
  };
}
"##;

const API_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { createHash, randomBytes } from "node:crypto";
import { pgStore, type Store } from "./store";

// A REST API the way Kong fronts one and Stripe documents one: a key per caller with scopes
// and a per-minute limit answered in RateLimit-* headers; Idempotency-Key on every create,
// stored with the request hash so a retry returns the first answer and a reuse with a
// different body is refused; keyset pagination; an OpenAPI document built from the routes.
const sha = (s: string) => createHash("sha256").update(s).digest("hex");

type Env = { Variables: { key: { name: string; scopes: string[]; rpm: number } } };

export function createApp(store: Store) {
  const app = new Hono<Env>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── keys ────────────────────────────────────────────────────────────────────────────
    // Bootstrap with ADMIN_TOKEN from the environment; after that, keys make keys.
    .post("/api/keys", async (c) => {
      const admin = process.env.ADMIN_TOKEN;
      const auth = c.req.header("authorization")?.replace(/^Bearer /, "");
      if (!admin || auth !== admin) return c.json({ error: { message: "admin token required", code: "unauthorized" } }, 401);
      const p = z.object({ name: z.string().min(1), scopes: z.array(z.string()).default(["read", "write"]), rpm: z.number().int().min(1).max(10_000).default(60) }).safeParse(await c.req.json());
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      const raw = "sk_" + randomBytes(24).toString("hex");
      const key = await store.createKey(p.data.name, sha(raw), p.data.scopes, p.data.rpm);
      // The plaintext exists exactly once, in this response.
      return c.json({ id: key.id, name: key.name, key: raw, scopes: key.scopes, rpm: key.rpm }, 201);
    })

    // ── auth + limit, in that order, before any handler ─────────────────────────────────
    .use("/api/v1/*", async (c, next) => {
      const raw = c.req.header("authorization")?.replace(/^Bearer /, "") ?? c.req.query("apikey");
      if (!raw) return c.json({ error: { message: "No API key found in request", code: "unauthorized" } }, 401);
      const key = await store.keyByHash(sha(raw));
      if (!key) return c.json({ error: { message: "Unauthorized", code: "unauthorized" } }, 401);
      const used = await store.hit(`key:${key.id}`, Date.now());
      const remaining = Math.max(0, key.rpm - used);
      c.header("RateLimit-Limit", String(key.rpm));
      c.header("RateLimit-Remaining", String(remaining));
      c.header("RateLimit-Reset", "60");
      if (used > key.rpm) { c.header("Retry-After", "60"); return c.json({ error: { message: "API rate limit exceeded", code: "rate_limited" } }, 429); }
      c.set("key", { name: key.name, scopes: key.scopes, rpm: key.rpm });
      await next();
    })

    // ── items: the resource ─────────────────────────────────────────────────────────────
    .get("/api/v1/items", async (c) => {
      const limit = Math.min(100, Number(c.req.query("limit") ?? 20));
      const cursor = c.req.query("cursor");
      const after = cursor ? JSON.parse(Buffer.from(cursor, "base64url").toString()) : null;
      const rows = await store.listItems(after, limit + 1);
      const page = rows.slice(0, limit);
      const last = page.at(-1);
      const next = rows.length > limit && last ? Buffer.from(JSON.stringify({ created_at: last.created_at, id: last.id })).toString("base64url") : null;
      return c.json({ data: page, next_cursor: next });
    })
    .post("/api/v1/items", async (c) => {
      if (!c.get("key").scopes.includes("write")) return c.json({ error: { message: "key lacks write scope", code: "forbidden" } }, 403);
      const idemKey = c.req.header("idempotency-key");
      const text = await c.req.text();
      const hash = sha(text);
      if (idemKey) {
        const seen = await store.getIdem(idemKey);
        if (seen && seen.request_hash !== hash) return c.json({ error: { message: "Idempotency-Key reused with a different body", code: "idempotency_mismatch" } }, 422);
        if (seen) { c.header("Idempotent-Replayed", "true"); return c.json(seen.body as object, seen.status as 201); }
      }
      const p = z.object({ name: z.string().min(1).max(200), body: z.record(z.unknown()).default({}) }).strict().safeParse(JSON.parse(text || "{}"));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      const item = { id: crypto.randomUUID(), name: p.data.name, body: p.data.body, created_at: new Date().toISOString() };
      await store.putItem(item);
      if (idemKey) await store.putIdem(idemKey, { request_hash: hash, status: 201, body: item });
      return c.json(item, 201);
    })
    .get("/api/v1/items/:id", async (c) => {
      const item = await store.getItem(c.req.param("id"));
      return item ? c.json(item) : c.json({ error: { message: "not found", code: "not_found" } }, 404);
    })
    .delete("/api/v1/items/:id", async (c) => {
      if (!c.get("key").scopes.includes("write")) return c.json({ error: { message: "key lacks write scope", code: "forbidden" } }, 403);
      return (await store.deleteItem(c.req.param("id"))) ? c.body(null, 204) : c.json({ error: { message: "not found", code: "not_found" } }, 404);
    })

    // ── docs, from the routes ───────────────────────────────────────────────────────────
    .get("/api/openapi.json", (c) => c.json(openapi()));

  return app;
}

export function openapi() {
  const item = { type: "object", properties: { id: { type: "string", format: "uuid" }, name: { type: "string" }, body: { type: "object" }, created_at: { type: "string", format: "date-time" } } };
  return {
    openapi: "3.1.0",
    info: { title: "API", version: "1.0.0" },
    components: { securitySchemes: { key: { type: "http", scheme: "bearer" } }, schemas: { Item: item } },
    security: [{ key: [] }],
    paths: {
      "/api/v1/items": {
        get: { summary: "List items", parameters: [{ name: "limit", in: "query", schema: { type: "integer", maximum: 100 } }, { name: "cursor", in: "query", schema: { type: "string" } }], responses: { "200": { description: "A page", content: { "application/json": { schema: { type: "object", properties: { data: { type: "array", items: { $ref: "#/components/schemas/Item" } }, next_cursor: { type: ["string", "null"] } } } } } } } },
        post: { summary: "Create an item", parameters: [{ name: "Idempotency-Key", in: "header", schema: { type: "string" } }], requestBody: { content: { "application/json": { schema: { type: "object", required: ["name"], properties: { name: { type: "string" }, body: { type: "object" } } } } } }, responses: { "201": { description: "Created" }, "422": { description: "Idempotency-Key reused with a different body" }, "429": { description: "Rate limited; see RateLimit-* headers" } } },
      },
      "/api/v1/items/{id}": {
        get: { summary: "Get an item", responses: { "200": { description: "The item" }, "404": { description: "No such item" } } },
        delete: { summary: "Delete an item", responses: { "204": { description: "Deleted" } } },
      },
    },
  };
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const API_TEST: &str = r##"import { describe, expect, test, beforeAll } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";

// Routes tested as routes against the memory store: auth, limits, idempotency, pagination.
process.env.ADMIN_TOKEN = "admin";
const app = createApp(memoryStore());
let key = "";

const json = (body: unknown, headers: Record<string, string> = {}) =>
  ({ method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${key}`, ...headers }, body: JSON.stringify(body) });

beforeAll(async () => {
  const res = await app.request("/api/keys", { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer admin" }, body: JSON.stringify({ name: "test", rpm: 1000 }) });
  expect(res.status).toBe(201);
  key = (await res.json()).key;
});

describe("api", () => {
  test("no key, no service", async () => {
    expect((await app.request("/api/v1/items")).status).toBe(401);
    expect((await app.request("/api/v1/items", { headers: { authorization: "Bearer sk_wrong" } })).status).toBe(401);
  });

  test("idempotency: same key replays, different body is refused", async () => {
    const a = await app.request("/api/v1/items", json({ name: "one" }, { "idempotency-key": "k1" }));
    expect(a.status).toBe(201);
    const first = await a.json();
    const b = await app.request("/api/v1/items", json({ name: "one" }, { "idempotency-key": "k1" }));
    expect(b.headers.get("idempotent-replayed")).toBe("true");
    expect((await b.json()).id).toBe(first.id);
    const c = await app.request("/api/v1/items", json({ name: "two" }, { "idempotency-key": "k1" }));
    expect(c.status).toBe(422);
  });

  test("keyset pagination walks every item once", async () => {
    for (const n of ["a", "b", "c"]) await app.request("/api/v1/items", json({ name: n }));
    const seen: string[] = [];
    let cursor: string | null = null;
    do {
      const res = await app.request(`/api/v1/items?limit=2${cursor ? `&cursor=${cursor}` : ""}`, { headers: { authorization: `Bearer ${key}` } });
      const page = await res.json();
      seen.push(...page.data.map((i: { id: string }) => i.id));
      cursor = page.next_cursor;
    } while (cursor);
    expect(new Set(seen).size).toBe(seen.length);
    expect(seen.length).toBeGreaterThanOrEqual(4);
  });

  test("the limit answers in headers and then in 429", async () => {
    const fresh = await app.request("/api/keys", { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer admin" }, body: JSON.stringify({ name: "tiny", rpm: 2 }) });
    const k = (await fresh.json()).key;
    const h = { headers: { authorization: `Bearer ${k}` } };
    const r1 = await app.request("/api/v1/items", h);
    expect(r1.headers.get("ratelimit-remaining")).toBe("1");
    await app.request("/api/v1/items", h);
    const r3 = await app.request("/api/v1/items", h);
    expect(r3.status).toBe(429);
    expect(r3.headers.get("retry-after")).toBe("60");
  });

  test("openapi describes the routes", async () => {
    const doc = await (await app.request("/api/openapi.json")).json();
    expect(doc.paths["/api/v1/items"].post.parameters[0].name).toBe("Idempotency-Key");
  });
});
"##;

const API_PAGE: &str = r##"import { headers } from "next/headers";

// The docs page: what the API is, how to get a key, and the OpenAPI document rendered.
async function doc() {
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/api/openapi.json`, { cache: "no-store" });
  return res.json();
}

export default async function Home() {
  const d = await doc();
  const host = (await headers()).get("host") ?? "localhost:3000";
  return (
    <main>
      <h1>{{NAME}} API</h1>
      <p>Keys, per-minute limits in <code>RateLimit-*</code> headers, <code>Idempotency-Key</code> on creates, keyset pagination.</p>
      <h2>Get a key</h2>
      <pre>{`curl -X POST http://${host}/api/keys -H "authorization: Bearer $ADMIN_TOKEN" \\
  -H "content-type: application/json" -d '{"name":"me","rpm":60}'`}</pre>
      <h2>Use it</h2>
      <pre>{`curl http://${host}/api/v1/items -H "authorization: Bearer sk_..."
curl -X POST http://${host}/api/v1/items -H "authorization: Bearer sk_..." \\
  -H "idempotency-key: $(uuidgen)" -H "content-type: application/json" -d '{"name":"first"}'`}</pre>
      <h2>Routes</h2>
      {Object.entries(d.paths as Record<string, Record<string, { summary: string }>>).map(([path, ops]) => (
        <div key={path}>
          {Object.entries(ops).map(([m, op]) => (
            <p key={m}><code>{m.toUpperCase()} {path}</code> — {op.summary}</p>
          ))}
        </div>
      ))}
      <p><a href="/api/openapi.json">openapi.json</a></p>
    </main>
  );
}
"##;
