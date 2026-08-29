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
        ("CLAUDE.md", f(API_CLAUDE)),
        ("AGENTS.md", f(API_AGENTS)),
        ("README.md", f(API_README)),
        (".claude/agents/api-contract.md", API_REVIEWER.into()),
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

const API_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a REST API with the operational surface of a Kong-fronted, Stripe-documented service:
a key per caller with scopes and a per-minute limit answered in `RateLimit-*` headers,
`Idempotency-Key` on every create, keyset pagination, and an OpenAPI document served from the
process. It is modelled on what PostgREST and Kong give you, reduced to one Hono app you own.
"Done" here means: the route exists in `app.ts`, `openapi()` describes it, `app.test.ts` exercises
it through `app.request` on the memory store, and `make check` is green.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | Every route, the auth + limit middleware on `/api/v1/*`, `openapi()`, the exported `AppType` |
| `backend/src/store.ts` | The `Store` interface and its two implementations: `pgStore()` (production) and `memoryStore()` (tests) |
| `backend/src/db.ts` | The one `postgres` pool, from `DATABASE_URL` |
| `backend/src/server.ts` | `@hono/node-server` on `PORT`, SIGTERM drain |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` in name order, once each |
| `backend/src/seed.ts` | Sample `notes` rows for the base scaffold; nothing the API serves |
| `backend/src/app.test.ts` | The gate for this service: auth, idempotency, pagination, limits, OpenAPI |
| `backend/migrations/0002_api.sql` | `api_keys`, `items`, `idempotency` |
| `frontend/app/page.tsx` | The docs page: how to get a key, and the routes read from `/api/openapi.json` |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client built from the backend's route table |

### The request path

1. `server.ts` hands the request to `app.fetch`.
2. `/api/health*`, `/api/keys` and `/api/openapi.json` are matched before the middleware and need no key (`/api/keys` needs `ADMIN_TOKEN`).
3. `.use("/api/v1/*")` reads `Authorization: Bearer sk_…` (or `?apikey=`), hashes it with SHA-256 and looks it up with `store.keyByHash`. No row: 401 with `{error:{code:"unauthorized"}}`.
4. The same middleware calls `store.hit("key:<id>", now)`, sets `RateLimit-Limit/Remaining/Reset`, and returns 429 + `Retry-After: 60` when the count exceeds `rpm`. The key's `name`, `scopes` and `rpm` go into `c.var.key`.
5. Write routes (`POST`, `DELETE`) check `scopes` includes `"write"`; 403 otherwise.
6. `POST /api/v1/items` reads the raw body text first, hashes it, and consults `store.getIdem` when `Idempotency-Key` is present: same hash replays the stored response with `Idempotent-Replayed: true`; a different hash is 422 `idempotency_mismatch`.
7. Only then is the body parsed with a `.strict()` zod schema; unknown fields are 400.
8. The item is written with `store.putItem`; if there was an idempotency key the `{status, body}` is stored under it; 201.

### Data model

| Table | Columns that matter | Why |
|---|---|---|
| `api_keys` | `key_hash text unique` | The plaintext is never stored; the response to `POST /api/keys` is the only place it exists |
| | `scopes text[]`, `rpm int` | Authorisation and the limit are properties of the key, not of the route |
| `items` | `id uuid`, `created_at` + index `(created_at, id)` | Keyset pagination orders on that pair and the index makes the `>` comparison a range scan |
| | `body jsonb` | The resource's payload; the schema is enforced at the boundary, not by the table |
| `idempotency` | `key text primary key`, `request_hash`, `status`, `body`, `created_at` | A retry must return the *first* answer; the hash is how a reuse with a different body is detected. `getIdem` only returns rows younger than 24h |

## Invariants

1. **Auth and the limit run before any `/api/v1` handler, in that order.** A handler that
   reads `c.get("key")` must be able to assume it is set. Guarded by `app.test.ts` "no key, no
   service" (401 with no key and with an unknown key).
2. **A key's plaintext exists exactly once, in the 201 from `POST /api/keys`.** Only
   `sha256(raw)` is stored, so a database read cannot recover a key. Guarded indirectly: the
   `beforeAll` in `app.test.ts` can only use the key it was handed, and `keyByHash` is the only
   lookup.
3. **`POST /api/keys` requires `ADMIN_TOKEN`.** Unset means nobody can mint keys, not that
   anybody can — the check is `!admin || auth !== admin`. Add the test when you touch it.
4. **The idempotency hash is of the raw body text, computed before parsing.** Two bodies that
   parse to the same object but differ in whitespace are different requests, which is the
   safe direction. Guarded by "idempotency: same key replays, different body is refused".
5. **A replay returns the stored status and body and sets `Idempotent-Replayed: true`.** The
   handler must not run again. Same test.
6. **`putIdem` never overwrites.** `on conflict (key) do nothing` in Postgres, `if (!idem.has)`
   in memory; the first response wins even under a concurrent retry. Same test.
7. **Pagination is keyset on `(created_at, id)`, capped at 100, and the cursor is opaque.**
   The cursor is base64url of `{created_at, id}` of the last row; `listItems` fetches
   `limit + 1` to know whether there is a next page. Guarded by "keyset pagination walks
   every item once".
8. **The limit answers in headers on every response and in 429 + `Retry-After` when
   exceeded.** `RateLimit-Remaining` is computed after the hit, so the first request on a
   `rpm: 2` key says `1`. Guarded by "the limit answers in headers and then in 429".
9. **Write scope is checked per route, not in the middleware.** A read-only key can list and
   get. Add a test with `scopes: ["read"]` when you add a write route.
10. **Bodies are parsed with `.strict()` schemas.** Unknown fields are 400 with
    `details: p.error.flatten()`. A new field is a schema change, not a passthrough.
11. **`openapi()` describes every public route.** It is hand-maintained; a route added without
    a `paths` entry is a bug. Guarded, minimally, by "openapi describes the routes" — extend
    that assertion when you add a route.
12. **Errors are `{error:{message, code, details?}}`.** Clients switch on `code`; keep the
    set stable (`unauthorized`, `forbidden`, `not_found`, `invalid`, `idempotency_mismatch`,
    `rate_limited`).

## Extending it

**Add a resource** (say `orders`):
1. `backend/migrations/0003_orders.sql` — table with `id uuid primary key`, `created_at`, and
   `create index … on orders (created_at, id)`.
2. `store.ts` — add `Order`, and `listOrders/getOrder/putOrder/deleteOrder` to `Store`, in both
   `pgStore` and `memoryStore`. Copy the `items` shape: `(created_at, id) > (…)` with
   `limit`, and the memory sort on `created_at + id`.
3. `app.ts` — routes under `/api/v1/orders`, inside the same chained expression (the seam is
   the inferred type of that chain). Reuse the cursor and idempotency code from `items`.
4. `openapi()` — a `paths` entry per route and a `components.schemas.Order`.
5. `app.test.ts` — at least: a 401 without a key, a replay with the same `Idempotency-Key`,
   a two-page walk.

**Add a scope** (say `admin`): pass it in `POST /api/keys` `scopes`, check
`c.get("key").scopes.includes("admin")` in the routes that need it, and add a test that a
`["read","write"]` key gets 403 there. No migration: `scopes` is `text[]`.

**Add a field to items**: `0003_….sql` adds the column (`add column … null` first; expand
before contract), `Item` in `store.ts` grows, the zod schema in `POST` grows, `putItem` in both
stores writes it, `openapi()`'s `Item` schema lists it. Test: a body with the field round-trips;
a body with a misspelt field is 400.

**Add an update route** (`PATCH /api/v1/items/:id`): check `write` scope, `.strict()` partial
schema, `putItem` upserts on `id` already. Add it to `openapi()`. Test: unknown field 400, then
`GET` shows the change.

**Move the limit to Redis** (needed at 2+ replicas): implement `hit` in `pgStore` with a Redis
`ZADD`/`ZREMRANGEBYSCORE`/`ZCARD` in a `MULTI` on `REDIS_URL` (the compose file and the
cluster both have Redis). The interface and the test do not change.

**Rotate or revoke a key**: there is no route yet. Add `DELETE /api/keys/:id` under
`ADMIN_TOKEN`, `deleteKey` on the store, and a test that a revoked key is 401 on the next call.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes (prod) | Postgres; defaults to the compose instance locally |
| `ADMIN_TOKEN` | yes, to mint keys | Bearer for `POST /api/keys`. Unset: that route is always 401 |
| `PORT` | no | Default `8000` |
| `API_URL` | frontend, server-side | Where the docs page fetches `/api/openapi.json`; default `http://127.0.0.1:8000` |

**Scaling.** The API is stateless except for one thing: `pgStore().hit` keeps the sliding
window in a process `Map`. With N replicas a key gets N × `rpm`. That is the first thing to
move to Redis (`REDIS_URL` is already provisioned) when the HPA takes you past one pod.
Everything else — keys, items, idempotency — is in Postgres and shared.

**Failure modes.**
- Postgres down: readiness (`/api/health/ready`) still says `db: "ok"` — it does not probe the
  pool. Requests fail with a 500 from `postgres`. Make readiness run `select 1` before you rely
  on it in a rollout.
- Idempotency row exists but `putItem` failed: cannot happen in that order (the item is written
  first); the reverse — item written, `putIdem` failed — means a retry creates a second item.
  Wrap both in `db.begin` if that matters to you.
- A malformed `cursor` (not base64url JSON) throws in `JSON.parse` and surfaces as 500. Validate
  it with zod if clients construct cursors by hand.

**What to watch.** Per-route RED from the JSON logs (`request_id` is not yet stamped — add a
`.use` that sets one); count of 429 per key (`RateLimit-*` is the client's view, the log is
yours); `idempotency` row count (no pruning yet, see Ceilings).

## Ceilings

- **Rate limit is per replica** (`store.ts`, `// In-process window`). Upgrade: Redis sorted
  set keyed by `key:<id>`.
- **`idempotency` rows are filtered at 24h but never deleted.** Upgrade: a nightly
  `delete … where created_at < now() - interval '24 hours'`, or partition by day and drop.
- **`RateLimit-Reset` is always `60`.** A true value needs the oldest timestamp in the window;
  the sliding array has it — return it from `hit` when a client asks.
- **`openapi()` is hand-written**, not derived from the zod schemas. Upgrade:
  `@hono/zod-openapi`, which makes the schema the single source.
- **`?apikey=` in the query string is accepted** for browser convenience; it lands in access
  logs. Drop it when every client can send a header.
- **No body size limit** beyond what `node-server` allows. Add `bodyLimit` from `hono/body-limit`
  before accepting untrusted uploads.
- **Scopes are `read`/`write` across all resources.** Per-resource scopes are a string
  convention away (`items:write`) — the column already fits.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const API_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the API.

## Run

    make demo                       # postgres + redis, migrate, seed, backend :8000, frontend :3000
    make check                      # the gate: typecheck both halves, bun test in backend/
    cd backend && bun test --watch  # just this service's tests

`make demo` needs `ADMIN_TOKEN` in `backend/.env` (copy `.env.example`) or nothing can mint a key.

## Every route, by hand

Mint a key (the plaintext is in this response and nowhere else):

    curl -s -X POST localhost:8000/api/keys \
      -H "authorization: Bearer $ADMIN_TOKEN" -H "content-type: application/json" \
      -d '{"name":"cli","scopes":["read","write"],"rpm":120}'
    # 201 {"id":1,"name":"cli","key":"sk_3f9c…","scopes":["read","write"],"rpm":120}

    export KEY=sk_3f9c…

Create, idempotently:

    curl -si -X POST localhost:8000/api/v1/items \
      -H "authorization: Bearer $KEY" -H "content-type: application/json" \
      -H "idempotency-key: order-1234" \
      -d '{"name":"first","body":{"sku":"A1","qty":2}}'
    # HTTP/1.1 201  RateLimit-Limit: 120  RateLimit-Remaining: 119  RateLimit-Reset: 60
    # {"id":"9b2e…","name":"first","body":{"sku":"A1","qty":2},"created_at":"2026-…"}

Send it again — same key, same body — and you get the same `id` with `Idempotent-Replayed: true`.
Same key, different body:

    # HTTP/1.1 422 {"error":{"message":"Idempotency-Key reused with a different body","code":"idempotency_mismatch"}}

List, two at a time:

    curl -s "localhost:8000/api/v1/items?limit=2" -H "authorization: Bearer $KEY"
    # {"data":[{…},{…}],"next_cursor":"eyJjcmVhdGVkX2F0Ijoi…"}
    curl -s "localhost:8000/api/v1/items?limit=2&cursor=eyJjcmVhdGVkX2F0Ijoi…" -H "authorization: Bearer $KEY"
    # {"data":[{…}],"next_cursor":null}

Get and delete:

    curl -s localhost:8000/api/v1/items/9b2e… -H "authorization: Bearer $KEY"        # 200 the item, or 404
    curl -si -X DELETE localhost:8000/api/v1/items/9b2e… -H "authorization: Bearer $KEY"  # 204, no body

Unknown field is refused:

    curl -s -X POST localhost:8000/api/v1/items -H "authorization: Bearer $KEY" \
      -H "content-type: application/json" -d '{"name":"x","nmae":"typo"}'
    # 400 {"error":{"message":"invalid body","code":"invalid","details":{…}}}

Over the limit (a `rpm: 2` key, third call inside a minute):

    # HTTP/1.1 429  Retry-After: 60  {"error":{"message":"API rate limit exceeded","code":"rate_limited"}}

The document, and health:

    curl -s localhost:8000/api/openapi.json | jq .paths
    curl -s localhost:8000/api/health          # {"status":"ok"}
    curl -s localhost:8000/api/health/ready    # {"status":"ok","db":"ok"}

## How the tests work

`app.test.ts` builds the app once with `createApp(memoryStore())` and sets
`process.env.ADMIN_TOKEN = "admin"` before that. There is no database, no server and no port:
every test calls `app.request(path, init)` and reads a `Response`. `beforeAll` mints one
`rpm: 1000` key that the tests share; the limit test mints its own `rpm: 2` key so it cannot
starve the others.

The memory store is the specification of the Postgres store: same interface, same ordering
rule (`created_at + id`), same first-write-wins on idempotency. When you change `pgStore`,
change `memoryStore` to match and let the test say whether the behaviour survived.

## Adding a test

    test("a read-only key cannot write", async () => {
      const res = await app.request("/api/keys", { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer admin" }, body: JSON.stringify({ name: "ro", scopes: ["read"] }) });
      const k = (await res.json()).key;
      const w = await app.request("/api/v1/items", { method: "POST", headers: { authorization: `Bearer ${k}`, "content-type": "application/json" }, body: JSON.stringify({ name: "x" }) });
      expect(w.status).toBe(403);
    });

Name the test for the behaviour, not the route. Use the shared `json()` helper for
authenticated POSTs. If the test needs the clock (the limit window), mint a fresh key instead
of sleeping.

## Migrations

New table or column: `backend/migrations/0003_name.sql`, `make migrate` locally; in the cluster
the `migrate` init container applies it before the new pods serve. Expand first (nullable
column), contract in a later deploy.
"##;

const API_README: &str = r##"# {{NAME}}

A REST API with keys, limits, idempotent creates, keyset pagination and an OpenAPI document —
the parts of PostgREST and Kong you would otherwise bolt together, as one Hono service you can
read in an afternoon.

## What you get

- `POST /api/keys` mints `sk_…` keys with scopes and a per-minute limit; only the SHA-256 is stored.
- `Authorization: Bearer` on `/api/v1/*`, checked before any handler.
- `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset` on every response; `429` + `Retry-After` when exceeded.
- `Idempotency-Key` on `POST /api/v1/items`: a retry returns the first response (`Idempotent-Replayed: true`); a reuse with a different body is `422`.
- Keyset pagination on `(created_at, id)` with an opaque cursor, capped at 100.
- Strict input schemas: unknown fields are `400` with details.
- `GET /api/openapi.json`, and a docs page at `/` rendered from it.
- Tests that run without a database, against the same store interface production uses.
- A typed client (`frontend/lib/api.ts`) built from the route table — no generated SDK.

## Five minutes

    cp backend/.env.example backend/.env && echo ADMIN_TOKEN=change-me >> backend/.env
    make demo

Then, in another shell:

    KEY=$(curl -s -X POST localhost:8000/api/keys -H "authorization: Bearer change-me" \
      -H "content-type: application/json" -d '{"name":"me"}' | jq -r .key)

    curl -s -X POST localhost:8000/api/v1/items -H "authorization: Bearer $KEY" \
      -H "content-type: application/json" -H "idempotency-key: demo-1" -d '{"name":"first"}'
    # {"id":"…","name":"first","body":{},"created_at":"…"}

    curl -si localhost:8000/api/v1/items -H "authorization: Bearer $KEY" | grep -i ratelimit
    # RateLimit-Limit: 60 / RateLimit-Remaining: 58 / RateLimit-Reset: 60

    curl -s localhost:8000/api/openapi.json | jq '.paths | keys'
    # ["/api/v1/items", "/api/v1/items/{id}"]

Open http://localhost:3000 for the docs page.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness |
| POST | `/api/keys` | `ADMIN_TOKEN` | Mint a key: `{name, scopes?, rpm?}` → `{id, name, key, scopes, rpm}` |
| GET | `/api/v1/items?limit=&cursor=` | key | A page: `{data, next_cursor}` |
| POST | `/api/v1/items` | key, `write` | Create; honours `Idempotency-Key` |
| GET | `/api/v1/items/:id` | key | One item or 404 |
| DELETE | `/api/v1/items/:id` | key, `write` | 204 or 404 |
| GET | `/api/openapi.json` | none | OpenAPI 3.1 |

Errors are always `{"error":{"message","code","details?"}}` with codes `unauthorized`,
`forbidden`, `not_found`, `invalid`, `idempotency_mismatch`, `rate_limited`.

## Compared with PostgREST and Kong

**Same shapes, so their docs and client habits apply**

- Bearer keys in `Authorization`, and the IETF `RateLimit-*` header set Kong's rate-limiting plugin emits.
- Stripe-style `Idempotency-Key` semantics: replay on match, 422 on mismatch, 24-hour window.
- Cursor pagination returning `next_cursor: null` at the end.
- An OpenAPI 3.1 document at a well-known path, so Swagger UI, Redoc and codegen tools work unchanged.

**Better here**

- Typed end to end: the frontend client is derived from the route table, and a field the API does not return fails `tsc` in the frontend.
- The whole behaviour is ~150 lines of TypeScript you own, not a plugin chain you configure; a rule you disagree with is an edit, not a ticket.
- Tests run in-process against a memory store — no Postgres, no container — and cover auth, idempotency, pagination and limits.
- Input is validated with strict schemas at the boundary; PostgREST exposes whatever the table accepts.
- Ships with Dockerfile, compose, kustomize overlays, migrations and a CI pipeline; the OSS tools give you the binary and leave the rest to you.

**Not here yet**

- No automatic CRUD from the schema: PostgREST serves every table for free, here each resource is written by hand (`CLAUDE.md` › Extending it).
- No filtering, sorting or embedding syntax (`?select=`, `?order=`, `?name=eq.x`).
- No row-level security via Postgres roles; authorisation is `scopes` on the key.
- No key revocation, rotation or listing route.
- No `PUT`/`PATCH` on items.
- The limit is per replica until it moves to Redis.
- Kong's plugin ecosystem — OIDC, request transformation, caching, gRPC — is not reproduced.

## Production

- **Environments.** `DATABASE_URL`, `ADMIN_TOKEN`, `PORT`; from `backend/.env` locally, from the `app-secrets` Secret in the cluster (rendered by `k8s/scripts/env-to-secrets.sh`, committed as `backend/.env.age`).
- **Scaling.** The HPA in `k8s/base/backend.yaml` scales 2–5 pods on CPU. Keys, items and idempotency are in Postgres and shared; the rate-limit window is per pod until it moves to Redis.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness and startup. Readiness does not yet query the database.
- **Migrations.** `backend/migrations/*.sql`, applied by the `migrate` init container before each rollout; expand/contract.
- **Secrets.** Never in the image or the repository. `ADMIN_TOKEN` should be long and rotated by editing the env and re-rendering.
- **Overlay.** `kubectl apply -k k8s/overlays/prod`; `k8s/README.md` says what the manifests insist on.
- **What pages you.** 5xx rate per route from the JSON logs; a rise in `401`s from one key (leaked or rotated badly); `idempotency` table growth.

## Roadmap

- Rate limiting in Redis, shared across replicas.
- Readiness that checks the pool.
- Prune `idempotency` nightly.
- OpenAPI derived from the zod schemas (`@hono/zod-openapi`).
- Key revocation and listing under `ADMIN_TOKEN`.
- `PATCH /api/v1/items/:id`.
- A `request_id` on every log line.
"##;

const API_REVIEWER: &str = r##"---
name: api-contract
description: "Run on any change to backend/src/app.ts, store.ts or the migrations of this API. Checks the public contract — auth order, scopes, idempotency, pagination, rate-limit headers, the OpenAPI document — as an integrator who will pin an SDK to it."
tools: Read, Grep, Glob, Bash
---

You are the engineer whose client library will break if this API changes shape. Read the diff
as that person. Report only contract breaks and security gaps, each as
`path:line — what — how a client sees it — the fix`.

Check:
1. **Order of the middleware.** `.use("/api/v1/*")` still runs auth then the limit before any
   handler; no new `/api/v1` route is registered above it in the chain.
2. **Key handling.** `keyByHash(sha(raw))` is the only lookup; no route stores, logs or returns
   a plaintext key after `POST /api/keys` 201.
3. **Admin bootstrap.** `POST /api/keys` refuses when `ADMIN_TOKEN` is unset
   (`!admin || auth !== admin`), never falls open.
4. **Scopes.** Every mutating route checks `c.get("key").scopes.includes("write")` (or a
   stricter scope) and answers 403 with `code: "forbidden"`.
5. **Idempotency.** The hash is of `c.req.text()` before parsing; a hash mismatch is 422
   `idempotency_mismatch`; a match replays the stored status and body with
   `Idempotent-Replayed: true`; `putIdem` still does nothing on conflict in both stores.
6. **Pagination.** Ordering is `(created_at, id)` in SQL and `created_at + id` in memory;
   `limit` is capped at 100; `limit + 1` rows are fetched; the cursor encodes the last row of
   the page and nothing else. A `findIndex` returning `-1` yields `[]`, not the whole list.
7. **Rate-limit headers.** `RateLimit-Limit`, `-Remaining`, `-Reset` on every `/api/v1`
   response including the 429; `Retry-After` on the 429; `Remaining` never negative.
8. **Schemas.** Every parsed body uses `.strict()`; string lengths are bounded; numbers have
   `min`/`max`.
9. **Error envelope.** `{error:{message, code}}` on every non-2xx; no new `code` string
   without adding it to `CLAUDE.md` › Invariants 12.
10. **OpenAPI.** Every route under `/api/v1` has a `paths` entry with its parameters
    (`Idempotency-Key`, `limit`, `cursor`) and its error responses; `components.schemas`
    matches the `Item` type in `store.ts`.
11. **Status codes.** Create is 201, delete is 204 with no body, unknown id is 404.
12. **Store parity.** A method added to `Store` is implemented in both `pgStore` and
    `memoryStore` with the same semantics, and `app.test.ts` exercises it.
13. **Migration safety.** New columns are nullable or defaulted; no rename in place; any new
    ordering column has an index.

End with one line: `api-contract: N findings`, and if 0, what you checked.
"##;
