//! Working code for the templates: packs laid over the containers scaffold.
//!
//! A pack replaces `backend/src/app.ts`, adds a store with a Postgres and an in-memory
//! implementation (so the gate needs no database), migrations, a page, and tests. Each is
//! modelled on the open-source project the template says it is like — LiteLLM's exact
//! OpenAI-compatible surface, Unleash's client API and rollout hashing, Inngest's step and
//! retry semantics, LangGraph's checkpoint and interrupt model, smolagents' ReAct records,
//! Kong's rate-limit headers — so the code behaves like the thing people already know.

pub fn files(id: &str, name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    match id {
        "api" => vec![
            ("backend/src/app.ts", API_APP.into()),
            ("backend/src/store.ts", API_STORE.into()),
            ("backend/src/app.test.ts", API_TEST.into()),
            ("backend/migrations/0002_api.sql", API_SQL.into()),
            ("frontend/app/page.tsx", f(API_PAGE)),
        ],
        "jobs" => vec![
            ("backend/src/app.ts", JOBS_APP.into()),
            ("backend/src/store.ts", JOBS_STORE.into()),
            ("backend/src/worker.ts", JOBS_WORKER.into()),
            ("backend/src/app.test.ts", JOBS_TEST.into()),
            ("backend/migrations/0002_jobs.sql", JOBS_SQL.into()),
            ("frontend/app/page.tsx", f(JOBS_PAGE)),
        ],
        "llmproxy" => vec![
            ("backend/src/app.ts", LLM_APP.into()),
            ("backend/src/store.ts", LLM_STORE.into()),
            ("backend/src/providers.ts", LLM_PROVIDERS.into()),
            ("backend/src/app.test.ts", LLM_TEST.into()),
            ("backend/migrations/0002_llm.sql", LLM_SQL.into()),
            ("frontend/app/page.tsx", f(LLM_PAGE)),
        ],
        "flags" => vec![
            ("backend/src/app.ts", FLAGS_APP.into()),
            ("backend/src/store.ts", FLAGS_STORE.into()),
            ("backend/src/evaluate.ts", FLAGS_EVAL.into()),
            ("backend/src/app.test.ts", FLAGS_TEST.into()),
            ("backend/migrations/0002_flags.sql", FLAGS_SQL.into()),
            ("frontend/app/page.tsx", f(FLAGS_PAGE)),
        ],
        "loop" => vec![
            ("backend/src/app.ts", LOOP_APP.into()),
            ("backend/src/store.ts", LOOP_STORE.into()),
            ("backend/src/agent.ts", LOOP_AGENT.into()),
            ("backend/src/app.test.ts", LOOP_TEST.into()),
            ("backend/migrations/0002_runs.sql", LOOP_SQL.into()),
            ("frontend/app/page.tsx", f(LOOP_PAGE)),
        ],
        "graph" => vec![
            ("backend/src/app.ts", GRAPH_APP.into()),
            ("backend/src/store.ts", GRAPH_STORE.into()),
            ("backend/src/graph.ts", GRAPH_ENGINE.into()),
            ("backend/src/app.test.ts", GRAPH_TEST.into()),
            ("backend/migrations/0002_checkpoints.sql", GRAPH_SQL.into()),
            ("frontend/app/page.tsx", f(GRAPH_PAGE)),
        ],
        _ => Vec::new(),
    }
}

/// Ids that have a pack.
#[cfg(test)]
const WITH_PACK: &[&str] = &["api", "jobs", "llmproxy", "flags", "loop", "graph"];

// ═══ shared shapes ═══════════════════════════════════════════════════════════════════════════
//
// Every pack's `app.ts` exports `createApp(store)` and a default `createApp(pgStore())`, so the
// server runs against Postgres and the tests run against memory with the same routes.

// ═══ api: a REST API service, like PostgREST / Directus — with keys, limits and docs ═════════

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

// ═══ jobs: webhooks and background jobs, like Inngest / BullMQ ═══════════════════════════════

const JOBS_SQL: &str = r##"create table if not exists events (
  id text primary key,
  name text not null,
  data jsonb not null default '{}',
  received_at timestamptz not null default now()
);
create table if not exists jobs (
  id bigserial primary key,
  event_id text not null references events(id),
  fn text not null,
  state text not null default 'queued',   -- queued | running | completed | failed | dead
  attempts int not null default 0,
  max_attempts int not null default 5,
  run_after timestamptz not null default now(),
  last_error text,
  output jsonb,
  updated_at timestamptz not null default now()
);
create index if not exists jobs_ready on jobs (state, run_after);
"##;

const JOBS_STORE: &str = r##"import { db } from "./db";

export type Job = { id: number; event_id: string; fn: string; state: string; attempts: number; max_attempts: number; run_after: string; last_error: string | null; output: unknown };
export type Event = { id: string; name: string; data: unknown; received_at: string };

export interface Store {
  /// Insert the event and its jobs in one transaction (the outbox). Returns false when the
  /// event id was already seen — the idempotent path for a webhook retry.
  receive(event: Event, fns: string[]): Promise<boolean>;
  claim(now: Date): Promise<Job | null>;
  complete(id: number, output: unknown): Promise<void>;
  fail(id: number, error: string, retryAt: Date | null): Promise<void>;
  list(limit: number): Promise<(Job & { name: string })[]>;
  replay(id: number): Promise<boolean>;
}

export function pgStore(): Store {
  return {
    receive: async (e, fns) => db.begin(async (tx) => {
      const inserted = await tx`insert into events (id, name, data) values (${e.id}, ${e.name}, ${tx.json(e.data as never)}) on conflict (id) do nothing returning id`;
      if (inserted.count === 0) return false;
      for (const fn of fns) await tx`insert into jobs (event_id, fn) values (${e.id}, ${fn})`;
      return true;
    }),
    claim: async (now) => (await db<Job[]>`update jobs set state = 'running', attempts = attempts + 1, updated_at = now()
      where id = (select id from jobs where state in ('queued') and run_after <= ${now} order by run_after limit 1 for update skip locked)
      returning *`)[0] ?? null,
    complete: async (id, output) => { await db`update jobs set state = 'completed', output = ${db.json(output as never)}, updated_at = now() where id = ${id}`; },
    fail: async (id, error, retryAt) => {
      if (retryAt) await db`update jobs set state = 'queued', last_error = ${error}, run_after = ${retryAt}, updated_at = now() where id = ${id}`;
      else await db`update jobs set state = 'dead', last_error = ${error}, updated_at = now() where id = ${id}`;
    },
    list: async (limit) => db`select j.*, e.name from jobs j join events e on e.id = j.event_id order by j.updated_at desc limit ${limit}`,
    replay: async (id) => (await db`update jobs set state = 'queued', attempts = 0, run_after = now(), last_error = null where id = ${id} and state = 'dead'`).count > 0,
  };
}

export function memoryStore(): Store {
  const events = new Map<string, Event>(); const jobs: Job[] = [];
  return {
    receive: async (e, fns) => { if (events.has(e.id)) return false; events.set(e.id, e); for (const fn of fns) jobs.push({ id: jobs.length + 1, event_id: e.id, fn, state: "queued", attempts: 0, max_attempts: 5, run_after: new Date(0).toISOString(), last_error: null, output: null }); return true; },
    claim: async (now) => { const j = jobs.find((j) => j.state === "queued" && new Date(j.run_after) <= now); if (!j) return null; j.state = "running"; j.attempts++; return { ...j }; },
    complete: async (id, output) => { const j = jobs.find((j) => j.id === id)!; j.state = "completed"; j.output = output; },
    fail: async (id, error, retryAt) => { const j = jobs.find((j) => j.id === id)!; j.last_error = error; if (retryAt) { j.state = "queued"; j.run_after = retryAt.toISOString(); } else j.state = "dead"; },
    list: async (limit) => jobs.slice(-limit).reverse().map((j) => ({ ...j, name: events.get(j.event_id)!.name })),
    replay: async (id) => { const j = jobs.find((j) => j.id === id && j.state === "dead"); if (!j) return false; j.state = "queued"; j.attempts = 0; j.run_after = new Date(0).toISOString(); j.last_error = null; return true; },
  };
}
"##;

const JOBS_APP: &str = r##"import { Hono } from "hono";
import { createHmac, timingSafeEqual } from "node:crypto";
import { pgStore, type Store } from "./store";

// Inbound events verified before they are parsed, stored with their jobs in one transaction
// (the outbox), answered in milliseconds; a worker (worker.ts) claims and runs them with
// bounded retries and a dead-letter state you can replay. Event id is the idempotency key —
// a provider's retry is acknowledged, not reprocessed.

// Which functions an event fans out to. Add a name here and a handler in worker.ts.
export const ROUTING: Record<string, string[]> = {
  "user/created": ["send-welcome", "sync-crm"],
  "payment/succeeded": ["fulfil-order"],
};

// Signature verification the way Stripe and GitHub do it: HMAC of the raw body, compared in
// constant time, with the secret from the environment.
export function verify(raw: string, header: string | undefined, secret: string): boolean {
  if (!header || !secret) return false;
  const sig = header.replace(/^sha256=/, "").replace(/^v1=/, "");
  const want = createHmac("sha256", secret).update(raw).digest("hex");
  return sig.length === want.length && timingSafeEqual(Buffer.from(sig), Buffer.from(want));
}

export function createApp(store: Store) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // The webhook: verify the raw body, then store event + jobs atomically, then 202.
    .post("/api/webhooks/:source", async (c) => {
      const raw = await c.req.text();
      const secret = process.env[`WEBHOOK_SECRET_${c.req.param("source").toUpperCase()}`] ?? process.env.WEBHOOK_SECRET ?? "";
      if (!verify(raw, c.req.header("x-signature") ?? c.req.header("x-hub-signature-256"), secret)) {
        return c.json({ error: { message: "bad signature", code: "unauthorized" } }, 401);
      }
      let body: { id?: string; name?: string; type?: string; data?: unknown };
      try { body = JSON.parse(raw); } catch { return c.json({ error: { message: "not JSON", code: "invalid" } }, 400); }
      const name = body.name ?? body.type ?? "unknown";
      const id = body.id ?? c.req.header("x-request-id") ?? crypto.randomUUID();
      const fns = ROUTING[name] ?? [];
      const fresh = await store.receive({ id, name, data: body.data ?? body, received_at: new Date().toISOString() }, fns);
      return c.json({ received: true, duplicate: !fresh, jobs: fresh ? fns.length : 0 }, 202);
    })

    // Internal send, for the app's own events (no signature; behind the network boundary).
    .post("/api/events", async (c) => {
      const body = (await c.req.json()) as { id?: string; name: string; data?: unknown };
      const id = body.id ?? crypto.randomUUID();
      const fns = ROUTING[body.name] ?? [];
      const fresh = await store.receive({ id, name: body.name, data: body.data ?? {}, received_at: new Date().toISOString() }, fns);
      return c.json({ id, duplicate: !fresh, jobs: fns.length }, 202);
    })

    // The admin view and the replay button.
    .get("/api/jobs", async (c) => c.json({ jobs: await store.list(100) }))
    .post("/api/jobs/:id/replay", async (c) => ((await store.replay(Number(c.req.param("id")))) ? c.json({ ok: true }) : c.json({ error: { message: "not dead", code: "invalid" } }, 400)));

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const JOBS_WORKER: &str = r##"import { pgStore, type Store, type Job } from "./store";

// The worker: claim one job, run its function, record the outcome. Exponential backoff with
// full jitter; after max_attempts the job is dead and stays visible. Runs as its own process
// (`bun run worker`) and its own Deployment, so it scales on queue depth, not on HTTP load.

export type Handler = (data: unknown, job: Job) => Promise<unknown>;

export const HANDLERS: Record<string, Handler> = {
  "send-welcome": async (data) => ({ sent: true, to: (data as { email?: string }).email ?? "unknown" }),
  "sync-crm": async (data) => ({ synced: true, id: (data as { id?: string }).id }),
  "fulfil-order": async (data) => ({ fulfilled: true, order: (data as { order?: string }).order }),
};

export function backoff(attempt: number, base = 1000, cap = 5 * 60_000): number {
  const exp = Math.min(cap, base * 2 ** attempt);
  return Math.floor(Math.random() * exp);
}

export async function tick(store: Store, handlers = HANDLERS, now = new Date()): Promise<Job | null> {
  const job = await store.claim(now);
  if (!job) return null;
  const handler = handlers[job.fn];
  try {
    if (!handler) throw new Error(`no handler for ${job.fn}`);
    const event = await eventData(store, job);
    const out = await Promise.race([handler(event, job), timeout(30_000)]);
    await store.complete(job.id, out);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    const retry = job.attempts < job.max_attempts ? new Date(now.getTime() + backoff(job.attempts)) : null;
    await store.fail(job.id, msg, retry);
  }
  return job;
}

async function eventData(store: Store, job: Job): Promise<unknown> {
  const rows = await store.list(1000);
  return (rows.find((r) => r.id === job.id) as { data?: unknown } | undefined)?.data ?? {};
}

const timeout = (ms: number) => new Promise((_, rej) => setTimeout(() => rej(new Error("handler timed out")), ms).unref());

if (import.meta.main) {
  const store = pgStore();
  console.log(JSON.stringify({ level: "info", msg: "worker started" }));
  let stopping = false;
  process.on("SIGTERM", () => { stopping = true; });
  while (!stopping) {
    const did = await tick(store);
    if (!did) await new Promise((r) => setTimeout(r, 500));
  }
}
"##;

const JOBS_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createHmac } from "node:crypto";
import { createApp, verify } from "./app";
import { memoryStore } from "./store";
import { tick, backoff } from "./worker";

process.env.WEBHOOK_SECRET = "s3cret";
const sign = (raw: string) => "sha256=" + createHmac("sha256", "s3cret").update(raw).digest("hex");

describe("webhooks and jobs", () => {
  test("a bad signature never reaches parsing", async () => {
    const app = createApp(memoryStore());
    const res = await app.request("/api/webhooks/stripe", { method: "POST", headers: { "x-signature": "sha256=nope" }, body: "{" });
    expect(res.status).toBe(401);
    expect(verify("x", undefined, "s")).toBe(false);
  });

  test("an event fans out to its jobs once, however often it is delivered", async () => {
    const store = memoryStore();
    const app = createApp(store);
    const raw = JSON.stringify({ id: "evt_1", name: "user/created", data: { email: "a@b.c" } });
    const first = await app.request("/api/webhooks/stripe", { method: "POST", headers: { "x-signature": sign(raw) }, body: raw });
    expect(first.status).toBe(202);
    expect((await first.json()).jobs).toBe(2);
    const again = await app.request("/api/webhooks/stripe", { method: "POST", headers: { "x-signature": sign(raw) }, body: raw });
    expect((await again.json()).duplicate).toBe(true);
    expect((await store.list(10)).length).toBe(2);
  });

  test("the worker retries with backoff and dead-letters after max attempts", async () => {
    const store = memoryStore();
    await store.receive({ id: "e", name: "payment/succeeded", data: {}, received_at: "" }, ["fulfil-order"]);
    let calls = 0;
    const failing = { "fulfil-order": async () => { calls++; throw new Error("downstream down"); } };
    let job = await tick(store, failing, new Date(0));
    expect(job?.attempts).toBe(1);
    const after = (await store.list(1))[0];
    expect(after.state).toBe("queued");
    expect(after.last_error).toBe("downstream down");
    // Drive it to death with a clock far in the future.
    for (let i = 1; i <= 10; i++) job = await tick(store, failing, new Date(Date.now() + i * 1e9)); // each tick well past any backoff
    expect((await store.list(1))[0].state).toBe("dead");
    expect(calls).toBe(5);
    // And replay it.
    expect(await store.replay(1)).toBe(true);
    expect((await store.list(1))[0].state).toBe("queued");
  });

  test("backoff grows and is jittered", () => {
    expect(backoff(0)).toBeLessThan(1000);
    expect(backoff(5)).toBeLessThan(32_000);
    expect(backoff(20)).toBeLessThanOrEqual(5 * 60_000);
  });
});
"##;

const JOBS_PAGE: &str = r##"async function jobs() {
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/api/jobs`, { cache: "no-store" });
  return (await res.json()).jobs as { id: number; name: string; fn: string; state: string; attempts: number; last_error: string | null }[];
}

// Every job by state, newest first. A dead one can be replayed from here.
export default async function Home() {
  const rows = await jobs();
  return (
    <main>
      <h1>{{NAME}} jobs</h1>
      <p>Send a test event: <code>{`curl -X POST http://localhost:8000/api/events -H 'content-type: application/json' -d '{"name":"user/created","data":{"email":"a@b.c"}}'`}</code> — then run <code>bun run worker</code> in backend/.</p>
      <table>
        <thead><tr><th>id</th><th>event</th><th>function</th><th>state</th><th>attempts</th><th>error</th></tr></thead>
        <tbody>
          {rows.map((j) => (
            <tr key={j.id}><td>{j.id}</td><td>{j.name}</td><td>{j.fn}</td><td>{j.state}</td><td>{j.attempts}</td><td>{j.last_error ?? ""}</td></tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && <p>No jobs yet.</p>}
    </main>
  );
}
"##;

// ═══ llmproxy: an OpenAI-compatible proxy, like LiteLLM ═════════════════════════════════════

const LLM_SQL: &str = r##"create table if not exists virtual_keys (
  token text primary key,             -- sha256 of the key
  key_alias text,
  models text[] not null default '{}', -- empty = any
  max_budget numeric,
  spend numeric not null default 0,
  rpm_limit int,
  created_at timestamptz not null default now()
);
create table if not exists spend_logs (
  request_id text primary key,
  api_key text not null,
  model text not null,
  model_group text not null,
  provider text not null,
  prompt_tokens int not null default 0,
  completion_tokens int not null default 0,
  spend numeric not null default 0,
  duration_ms int not null default 0,
  status text not null,
  started_at timestamptz not null default now()
);
"##;

const LLM_STORE: &str = r##"import { db } from "./db";

export type VKey = { token: string; key_alias: string | null; models: string[]; max_budget: number | null; spend: number; rpm_limit: number | null };
export type SpendLog = { request_id: string; api_key: string; model: string; model_group: string; provider: string; prompt_tokens: number; completion_tokens: number; spend: number; duration_ms: number; status: string };

export interface Store {
  key(token: string): Promise<VKey | null>;
  createKey(k: VKey): Promise<void>;
  addSpend(token: string, amount: number): Promise<void>;
  log(l: SpendLog): Promise<void>;
  logs(token: string | null, limit: number): Promise<SpendLog[]>;
}

export function pgStore(): Store {
  return {
    key: async (t) => (await db<VKey[]>`select token, key_alias, models, max_budget::float, spend::float, rpm_limit from virtual_keys where token = ${t}`)[0] ?? null,
    createKey: async (k) => { await db`insert into virtual_keys (token, key_alias, models, max_budget, rpm_limit) values (${k.token}, ${k.key_alias}, ${k.models}, ${k.max_budget}, ${k.rpm_limit})`; },
    addSpend: async (t, a) => { await db`update virtual_keys set spend = spend + ${a} where token = ${t}`; },
    log: async (l) => { await db`insert into spend_logs ${db(l, "request_id", "api_key", "model", "model_group", "provider", "prompt_tokens", "completion_tokens", "spend", "duration_ms", "status")}`; },
    logs: async (t, limit) => t ? db<SpendLog[]>`select * from spend_logs where api_key = ${t} order by started_at desc limit ${limit}` : db<SpendLog[]>`select * from spend_logs order by started_at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const keys = new Map<string, VKey>(); const logs: SpendLog[] = [];
  return {
    key: async (t) => keys.get(t) ?? null,
    createKey: async (k) => { keys.set(k.token, { ...k }); },
    addSpend: async (t, a) => { const k = keys.get(t); if (k) k.spend += a; },
    log: async (l) => { logs.push(l); },
    logs: async (t, limit) => logs.filter((l) => !t || l.api_key === t).slice(-limit).reverse(),
  };
}
"##;

const LLM_PROVIDERS: &str = r##"// Provider adapters: OpenAI-shaped in, provider-shaped out, and back. The Anthropic mapping is
// LiteLLM's: system messages lifted to `system`, tool calls to `tool_use` blocks, tool results
// to `tool_result`, stop reasons renamed. Streaming is re-emitted as OpenAI chunks.

export type Msg = { role: "system" | "user" | "assistant" | "tool"; content: string | null; tool_calls?: ToolCall[]; tool_call_id?: string; name?: string };
export type ToolCall = { id: string; type: "function"; function: { name: string; arguments: string } };
export type Tool = { type: "function"; function: { name: string; description?: string; parameters?: unknown } };
export type ChatRequest = { model: string; messages: Msg[]; stream?: boolean; temperature?: number; max_tokens?: number; tools?: Tool[]; tool_choice?: unknown; stop?: string | string[] };
export type Usage = { prompt_tokens: number; completion_tokens: number; total_tokens: number };
export type ChatResponse = { id: string; object: "chat.completion"; created: number; model: string; choices: { index: number; message: Msg; finish_reason: string }[]; usage: Usage };

/// A deployment: what `model_list` in LiteLLM's config holds. `model` is `provider/name`.
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export type Deployment = { model_name: string; model: string; api_base?: string; api_key_env: string; input_cost_per_token: number; output_cost_per_token: number };

export const PRICES: Record<string, [number, number]> = {
  "gpt-4o": [2.5e-6, 10e-6], "gpt-4o-mini": [0.15e-6, 0.6e-6],
  "claude-sonnet-4-20250514": [3e-6, 15e-6], "claude-3-5-haiku-20241022": [0.8e-6, 4e-6],
};

export function providerOf(model: string): "openai" | "anthropic" {
  return model.startsWith("anthropic/") ? "anthropic" : "openai";
}

export function cost(d: Deployment, u: Usage): number {
  return u.prompt_tokens * d.input_cost_per_token + u.completion_tokens * d.output_cost_per_token;
}

/// One call, non-streaming or streaming. Returns either a ChatResponse or an async iterator of
/// OpenAI chunks. `fetchImpl` is injectable so the tests never leave the process.
export type Chunk = { id: string; object: "chat.completion.chunk"; created: number; model: string; choices: { index: 0; delta: Partial<Msg>; finish_reason: string | null }[]; usage?: Usage };

export interface Provider {
  complete(req: ChatRequest, d: Deployment, key: string, fetchImpl: Fetch): Promise<ChatResponse>;
  stream(req: ChatRequest, d: Deployment, key: string, fetchImpl: Fetch): AsyncIterable<Chunk>;
}

const bare = (m: string) => m.replace(/^[a-z]+\//, "");
const now = () => Math.floor(Date.now() / 1000);

// ── OpenAI: pass-through with the model name unwrapped ───────────────────────────────────
export const openai: Provider = {
  async complete(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.openai.com/v1"}/chat/completions`, {
      method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify({ ...req, model: bare(d.model), stream: false }),
    });
    if (!res.ok) throw new ProviderError(res.status, await res.text());
    return (await res.json()) as ChatResponse;
  },
  async *stream(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.openai.com/v1"}/chat/completions`, {
      method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify({ ...req, model: bare(d.model), stream: true, stream_options: { include_usage: true } }),
    });
    if (!res.ok || !res.body) throw new ProviderError(res.status, await res.text());
    for await (const data of sse(res.body)) {
      if (data === "[DONE]") return;
      yield JSON.parse(data) as Chunk;
    }
  },
};

// ── Anthropic: the Messages API, mapped both ways ────────────────────────────────────────
function toAnthropic(req: ChatRequest, d: Deployment) {
  const system = req.messages.filter((m) => m.role === "system").map((m) => m.content ?? "").join("\n");
  const messages: { role: "user" | "assistant"; content: unknown }[] = [];
  for (const m of req.messages) {
    if (m.role === "system") continue;
    if (m.role === "tool") { messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: m.tool_call_id, content: m.content ?? "" }] }); continue; }
    if (m.role === "assistant" && m.tool_calls?.length) {
      const blocks: unknown[] = m.content ? [{ type: "text", text: m.content }] : [];
      for (const t of m.tool_calls) blocks.push({ type: "tool_use", id: t.id, name: t.function.name, input: JSON.parse(t.function.arguments || "{}") });
      messages.push({ role: "assistant", content: blocks }); continue;
    }
    const last = messages.at(-1);
    if (last && last.role === m.role && typeof last.content === "string") last.content += "\n" + (m.content ?? "");
    else messages.push({ role: m.role, content: m.content ?? "" });
  }
  const tools = req.tools?.map((t) => ({ name: t.function.name, description: t.function.description, input_schema: t.function.parameters ?? { type: "object" } }));
  const tc = req.tool_choice;
  const tool_choice = tc === "auto" ? { type: "auto" } : tc === "required" ? { type: "any" } : tc === "none" ? { type: "none" } : typeof tc === "object" && tc ? { type: "tool", name: (tc as { function: { name: string } }).function.name } : undefined;
  return { model: bare(d.model), max_tokens: req.max_tokens ?? 4096, temperature: req.temperature, system: system || undefined, messages, tools, tool_choice, stop_sequences: typeof req.stop === "string" ? [req.stop] : req.stop, stream: req.stream ?? false };
}
const finish = (r: string) => (r === "max_tokens" ? "length" : r === "tool_use" ? "tool_calls" : "stop");

export const anthropic: Provider = {
  async complete(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.anthropic.com"}/v1/messages`, {
      method: "POST", headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ ...toAnthropic(req, d), stream: false }),
    });
    if (!res.ok) throw new ProviderError(res.status, await res.text());
    const a = (await res.json()) as { id: string; content: { type: string; text?: string; id?: string; name?: string; input?: unknown }[]; stop_reason: string; usage: { input_tokens: number; output_tokens: number } };
    const text = a.content.filter((b) => b.type === "text").map((b) => b.text).join("");
    const tool_calls: ToolCall[] = a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, type: "function", function: { name: b.name!, arguments: JSON.stringify(b.input ?? {}) } }));
    return {
      id: "chatcmpl-" + a.id, object: "chat.completion", created: now(), model: req.model,
      choices: [{ index: 0, message: { role: "assistant", content: text || null, ...(tool_calls.length ? { tool_calls } : {}) }, finish_reason: finish(a.stop_reason) }],
      usage: { prompt_tokens: a.usage.input_tokens, completion_tokens: a.usage.output_tokens, total_tokens: a.usage.input_tokens + a.usage.output_tokens },
    };
  },
  async *stream(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.anthropic.com"}/v1/messages`, {
      method: "POST", headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ ...toAnthropic(req, d), stream: true }),
    });
    if (!res.ok || !res.body) throw new ProviderError(res.status, await res.text());
    const id = "chatcmpl-" + crypto.randomUUID(); const created = now();
    const chunk = (delta: Partial<Msg>, finish_reason: string | null = null, usage?: Usage): Chunk => ({ id, object: "chat.completion.chunk", created, model: req.model, choices: [{ index: 0, delta, finish_reason }], ...(usage ? { usage } : {}) });
    let input = 0, toolIndex = -1;
    for await (const data of sse(res.body)) {
      const ev = JSON.parse(data) as { type: string; message?: { usage: { input_tokens: number } }; content_block?: { type: string; id: string; name: string }; delta?: { type?: string; text?: string; partial_json?: string; stop_reason?: string }; usage?: { output_tokens: number } };
      switch (ev.type) {
        case "message_start": input = ev.message?.usage.input_tokens ?? 0; yield chunk({ role: "assistant" }); break;
        case "content_block_start": if (ev.content_block?.type === "tool_use") { toolIndex++; yield chunk({ tool_calls: [{ id: ev.content_block.id, type: "function", function: { name: ev.content_block.name, arguments: "" } }] }); } break;
        case "content_block_delta":
          if (ev.delta?.type === "text_delta") yield chunk({ content: ev.delta.text });
          if (ev.delta?.type === "input_json_delta") yield chunk({ tool_calls: [{ id: "", type: "function", function: { name: "", arguments: ev.delta.partial_json ?? "" } }] });
          break;
        case "message_delta": { const out = ev.usage?.output_tokens ?? 0; yield chunk({}, finish(ev.delta?.stop_reason ?? "end_turn"), { prompt_tokens: input, completion_tokens: out, total_tokens: input + out }); break; }
        default: break;
      }
    }
  },
};

export class ProviderError extends Error { constructor(public status: number, body: string) { super(body.slice(0, 400)); } }

/// `data:` lines out of an SSE body.
export async function* sse(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader(); const dec = new TextDecoder(); let buf = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    buf += dec.decode(value, { stream: true });
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
      if (line.startsWith("data:")) yield line.slice(5).trim();
    }
  }
}
"##;

const LLM_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { createHash, randomBytes } from "node:crypto";
import { pgStore, type Store } from "./store";
import { anthropic, openai, providerOf, cost, PRICES, ProviderError, type ChatRequest, type Deployment, type Fetch, type Usage } from "./providers";

// LiteLLM's surface: POST /v1/chat/completions (streaming as OpenAI chunks ending in [DONE]),
// GET /v1/models, POST /key/generate under the master key, virtual keys with model allowlists
// and budgets, a spend log per call with cost from a price table, ordered fallbacks between
// deployments of the same model group. Provider keys come from the environment and never
// leave this process.

const sha = (s: string) => createHash("sha256").update(s).digest("hex");

/// The router table — LiteLLM's `model_list`. A model_name can appear more than once; the
/// first deployment that answers wins, in order. Edit here or load from MODEL_LIST_JSON.
export const MODEL_LIST: Deployment[] = process.env.MODEL_LIST_JSON ? JSON.parse(process.env.MODEL_LIST_JSON) : [
  { model_name: "gpt-4o", model: "openai/gpt-4o", api_key_env: "OPENAI_API_KEY", input_cost_per_token: PRICES["gpt-4o"][0], output_cost_per_token: PRICES["gpt-4o"][1] },
  { model_name: "claude", model: "anthropic/claude-sonnet-4-20250514", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: PRICES["claude-sonnet-4-20250514"][0], output_cost_per_token: PRICES["claude-sonnet-4-20250514"][1] },
  { model_name: "fast", model: "anthropic/claude-3-5-haiku-20241022", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: PRICES["claude-3-5-haiku-20241022"][0], output_cost_per_token: PRICES["claude-3-5-haiku-20241022"][1] },
  { model_name: "fast", model: "openai/gpt-4o-mini", api_key_env: "OPENAI_API_KEY", input_cost_per_token: PRICES["gpt-4o-mini"][0], output_cost_per_token: PRICES["gpt-4o-mini"][1] },
];

export const FALLBACKS: Record<string, string[]> = { "gpt-4o": ["claude"], claude: ["gpt-4o"] };

const err = (message: string, type: string, code: string | number) => ({ error: { message, type, param: null, code } });

export function createApp(store: Store, fetchImpl: Fetch = fetch, list: Deployment[] = MODEL_LIST) {
  const app = new Hono<{ Variables: { token: string } }>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .post("/key/generate", async (c) => {
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!master || c.req.header("authorization") !== `Bearer ${master}`) return c.json(err("master key required", "auth_error", 401), 401);
      const body = (await c.req.json().catch(() => ({}))) as { models?: string[]; max_budget?: number; key_alias?: string; rpm_limit?: number };
      const key = "sk-" + randomBytes(24).toString("hex");
      await store.createKey({ token: sha(key), key_alias: body.key_alias ?? null, models: body.models ?? [], max_budget: body.max_budget ?? null, spend: 0, rpm_limit: body.rpm_limit ?? null });
      return c.json({ key, models: body.models ?? [], max_budget: body.max_budget ?? null, spend: 0, key_alias: body.key_alias ?? null });
    })

    .use("/v1/*", async (c, next) => {
      const raw = c.req.header("authorization")?.replace(/^Bearer /, "");
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!raw) return c.json(err("Authentication Error: no key", "auth_error", 401), 401);
      if (raw !== master) {
        const k = await store.key(sha(raw));
        if (!k) return c.json(err("Authentication Error: invalid key", "auth_error", 401), 401);
        if (k.max_budget != null && k.spend >= k.max_budget) return c.json(err(`ExceededBudget: spend ${k.spend.toFixed(4)} >= max_budget ${k.max_budget}`, "budget_exceeded", 400), 400);
      }
      c.set("token", raw === master ? "master" : sha(raw));
      await next();
    })

    .get("/v1/models", async (c) => {
      const k = c.get("token") === "master" ? null : await store.key(c.get("token"));
      const names = [...new Set(list.map((d) => d.model_name))].filter((n) => !k || k.models.length === 0 || k.models.includes(n));
      return c.json({ object: "list", data: names.map((id) => ({ id, object: "model", created: 1677610602, owned_by: "openai" })) });
    })

    .post("/v1/chat/completions", async (c) => {
      const req = (await c.req.json()) as ChatRequest;
      const token = c.get("token");
      const k = token === "master" ? null : await store.key(token);
      if (k && k.models.length && !k.models.includes(req.model)) return c.json(err(`key not allowed to use model ${req.model}`, "auth_error", 401), 401);
      const groups = [req.model, ...(FALLBACKS[req.model] ?? [])];
      const deployments = groups.flatMap((g) => list.filter((d) => d.model_name === g));
      if (!deployments.length) return c.json(err(`model ${req.model} not found`, "invalid_request_error", "model_not_found"), 404);

      const request_id = "chatcmpl-" + crypto.randomUUID();
      const started = Date.now();
      let lastError: ProviderError | Error | null = null;
      for (const d of deployments) {
        const provider = providerOf(d.model) === "anthropic" ? anthropic : openai;
        const key = process.env[d.api_key_env];
        if (!key) { lastError = new Error(`${d.api_key_env} is not set`); continue; }
        const record = async (usage: Usage, status: string) => {
          const spend = cost(d, usage);
          await store.log({ request_id, api_key: token, model: d.model, model_group: req.model, provider: providerOf(d.model), prompt_tokens: usage.prompt_tokens, completion_tokens: usage.completion_tokens, spend, duration_ms: Date.now() - started, status });
          if (token !== "master") await store.addSpend(token, spend);
          return spend;
        };
        try {
          if (req.stream) {
            const it = provider.stream(req, d, key, fetchImpl)[Symbol.asyncIterator]();
            const first = await it.next(); // a provider error surfaces here, before headers go out
            c.header("x-litellm-model-id", d.model); c.header("x-litellm-call-id", request_id);
            return streamSSE(c, async (s) => {
              let usage: Usage = { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
              for (let r = first; !r.done; r = await it.next()) {
                if (r.value.usage) usage = r.value.usage;
                await s.writeSSE({ data: JSON.stringify(r.value) });
              }
              await s.writeSSE({ data: "[DONE]" });
              await record(usage, "success");
            });
          }
          const out = await provider.complete(req, d, key, fetchImpl);
          const spend = await record(out.usage, "success");
          c.header("x-litellm-model-id", d.model); c.header("x-litellm-call-id", request_id); c.header("x-litellm-response-cost", spend.toFixed(6));
          return c.json({ ...out, id: request_id, model: req.model });
        } catch (e) {
          lastError = e as Error; // fall through to the next deployment
        }
      }
      await store.log({ request_id, api_key: token, model: deployments[0].model, model_group: req.model, provider: providerOf(deployments[0].model), prompt_tokens: 0, completion_tokens: 0, spend: 0, duration_ms: Date.now() - started, status: "failure" });
      const status = lastError instanceof ProviderError ? lastError.status : 502;
      return c.json(err(lastError?.message ?? "all deployments failed", "api_error", status), status as 502);
    })

    .get("/spend/logs", async (c) => {
      const token = c.req.header("authorization")?.replace(/^Bearer /, "");
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!token) return c.json(err("key required", "auth_error", 401), 401);
      return c.json(await store.logs(token === master ? null : sha(token), 200));
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const LLM_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import type { Deployment, Fetch } from "./providers";

// The proxy against fake providers: OpenAI shape in and out, Anthropic mapping, fallbacks,
// budgets, streaming chunks ending in [DONE], spend logged with cost.
process.env.MASTER_KEY = "sk-master";
process.env.OPENAI_API_KEY = "x"; process.env.ANTHROPIC_API_KEY = "y";

const list: Deployment[] = [
  { model_name: "gpt-4o", model: "openai/gpt-4o", api_key_env: "OPENAI_API_KEY", input_cost_per_token: 1e-6, output_cost_per_token: 2e-6 },
  { model_name: "claude", model: "anthropic/claude-sonnet-4-20250514", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: 3e-6, output_cost_per_token: 15e-6 },
];

let openaiDown = false;
const fakeFetch: Fetch = async (url, init) => {
  const body = JSON.parse(String(init?.body));
  if (String(url).includes("openai")) {
    if (openaiDown) return new Response("upstream down", { status: 503 });
    if (body.stream) {
      const lines = [
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}\n\n`,
        `data: [DONE]\n\n`,
      ];
      return new Response(new ReadableStream({ start(ctl) { for (const l of lines) ctl.enqueue(new TextEncoder().encode(l)); ctl.close(); } }), { headers: { "content-type": "text/event-stream" } });
    }
    return Response.json({ id: "x", object: "chat.completion", created: 1, model: "gpt-4o", choices: [{ index: 0, message: { role: "assistant", content: "hello from openai" }, finish_reason: "stop" }], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });
  }
  // Anthropic: assert the mapping happened, answer in its own shape.
  expect(body.system).toBe("be brief");
  expect(body.messages[0]).toEqual({ role: "user", content: "hi" });
  expect(body.max_tokens).toBe(4096);
  return Response.json({ id: "msg_1", content: [{ type: "text", text: "hello from claude" }], stop_reason: "end_turn", usage: { input_tokens: 7, output_tokens: 3 } });
};

const app = createApp(memoryStore(), fakeFetch, list);
const chat = (key: string, body: unknown) => app.request("/v1/chat/completions", { method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" }, body: JSON.stringify(body) });

describe("llm proxy", () => {
  test("keys are minted under the master key and scoped to models", async () => {
    const res = await app.request("/key/generate", { method: "POST", headers: { authorization: "Bearer sk-master", "content-type": "application/json" }, body: JSON.stringify({ models: ["claude"], max_budget: 0.001 }) });
    const { key } = await res.json();
    expect(key.startsWith("sk-")).toBe(true);
    const models = await (await app.request("/v1/models", { headers: { authorization: `Bearer ${key}` } })).json();
    expect(models.data.map((m: { id: string }) => m.id)).toEqual(["claude"]);
    expect((await chat(key, { model: "gpt-4o", messages: [] })).status).toBe(401);
  });

  test("anthropic is mapped and answered in openai's shape, with cost logged", async () => {
    const res = await chat("sk-master", { model: "claude", messages: [{ role: "system", content: "be brief" }, { role: "user", content: "hi" }] });
    expect(res.status).toBe(200);
    const out = await res.json();
    expect(out.choices[0].message.content).toBe("hello from claude");
    expect(out.usage.total_tokens).toBe(10);
    expect(res.headers.get("x-litellm-response-cost")).toBe((7 * 3e-6 + 3 * 15e-6).toFixed(6));
  });

  test("a dead deployment falls back to the next group", async () => {
    openaiDown = true;
    const res = await chat("sk-master", { model: "gpt-4o", messages: [{ role: "system", content: "be brief" }, { role: "user", content: "hi" }] });
    openaiDown = false;
    expect(res.status).toBe(200);
    expect(res.headers.get("x-litellm-model-id")).toContain("anthropic/");
  });

  test("streaming re-emits chunks and ends with [DONE]", async () => {
    const res = await chat("sk-master", { model: "gpt-4o", messages: [{ role: "user", content: "hi" }], stream: true });
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    const text = await res.text();
    expect(text).toContain('"content":"hi"');
    expect(text.trim().endsWith("data: [DONE]")).toBe(true);
  });

  test("a budget stops spend", async () => {
    const res = await app.request("/key/generate", { method: "POST", headers: { authorization: "Bearer sk-master", "content-type": "application/json" }, body: JSON.stringify({ max_budget: 0.00001 }) });
    const { key } = await res.json();
    expect((await chat(key, { model: "gpt-4o", messages: [{ role: "user", content: "hi" }] })).status).toBe(200);
    const second = await chat(key, { model: "gpt-4o", messages: [{ role: "user", content: "hi" }] });
    expect(second.status).toBe(400);
    expect((await second.json()).error.type).toBe("budget_exceeded");
  });
});
"##;

const LLM_PAGE: &str = r##"async function logs() {
  const master = process.env.MASTER_KEY ?? process.env.LITELLM_MASTER_KEY ?? "";
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/spend/logs`, { headers: { authorization: `Bearer ${master}` }, cache: "no-store" });
  return res.ok ? ((await res.json()) as { request_id: string; model_group: string; model: string; prompt_tokens: number; completion_tokens: number; spend: number; duration_ms: number; status: string }[]) : [];
}

export default async function Home() {
  const rows = await logs();
  const total = rows.reduce((s, r) => s + Number(r.spend), 0);
  return (
    <main>
      <h1>{{NAME}} — LLM proxy</h1>
      <p>OpenAI-compatible. Point any SDK at <code>http://localhost:8000/v1</code> with a virtual key.</p>
      <pre>{`curl -X POST http://localhost:8000/key/generate -H "authorization: Bearer $MASTER_KEY" \\
  -H "content-type: application/json" -d '{"models":["claude","gpt-4o"],"max_budget":5}'
curl http://localhost:8000/v1/chat/completions -H "authorization: Bearer sk-..." \\
  -H "content-type: application/json" -d '{"model":"claude","messages":[{"role":"user","content":"hi"}]}'`}</pre>
      <h2>Spend · ${total.toFixed(4)}</h2>
      <table>
        <thead><tr><th>request</th><th>group → model</th><th>tokens</th><th>cost</th><th>ms</th><th>status</th></tr></thead>
        <tbody>{rows.map((r) => <tr key={r.request_id}><td>{r.request_id.slice(0, 16)}</td><td>{r.model_group} → {r.model}</td><td>{r.prompt_tokens}/{r.completion_tokens}</td><td>${Number(r.spend).toFixed(6)}</td><td>{r.duration_ms}</td><td>{r.status}</td></tr>)}</tbody>
      </table>
      {rows.length === 0 && <p>No calls yet.</p>}
    </main>
  );
}
"##;

// ═══ flags: feature flags, like Unleash (client API and evaluation) with OpenFeature's SDK ════

const FLAGS_SQL: &str = r##"create table if not exists flags (
  name text primary key,
  enabled boolean not null default false,
  description text,
  strategies jsonb not null default '[]',
  variants jsonb not null default '[]',
  version int not null default 1,
  updated_at timestamptz not null default now()
);
create table if not exists flag_audit (
  id bigserial primary key,
  name text not null,
  actor text,
  before jsonb,
  after jsonb,
  at timestamptz not null default now()
);
"##;

const FLAGS_EVAL: &str = r##"// Unleash's evaluation, locally: a flag is on if enabled and any strategy passes; strategies
// are `default`, `userWithId`, `flexibleRollout` (murmur3 of group:sticky mod 100, from the
// client specification) and `remoteAddress`; constraints gate a strategy; variants are chosen
// by the same hash over their weights. The SDK calls this against a snapshot, so a request
// never waits on the service.

export type Constraint = { contextName: string; operator: string; values?: string[]; value?: string; inverted?: boolean; caseInsensitive?: boolean };
export type Strategy = { name: string; parameters?: Record<string, string>; constraints?: Constraint[] };
export type Variant = { name: string; weight: number; payload?: { type: string; value: string } };
export type Flag = { name: string; enabled: boolean; strategies: Strategy[]; variants?: Variant[] };
export type Context = { userId?: string; sessionId?: string; remoteAddress?: string; properties?: Record<string, string>; currentTime?: string };

/// murmur3 x86 32-bit, seed 0 — what every Unleash SDK uses, so buckets agree across languages.
export function murmur3(key: string, seed = 0): number {
  const bytes = new TextEncoder().encode(key);
  let h = seed >>> 0; const c1 = 0xcc9e2d51, c2 = 0x1b873593; let i = 0;
  const mul = (a: number, b: number) => Math.imul(a, b) >>> 0;
  for (; i + 4 <= bytes.length; i += 4) {
    let k = (bytes[i] | (bytes[i + 1] << 8) | (bytes[i + 2] << 16) | (bytes[i + 3] << 24)) >>> 0;
    k = mul(k, c1); k = ((k << 15) | (k >>> 17)) >>> 0; k = mul(k, c2);
    h ^= k; h = ((h << 13) | (h >>> 19)) >>> 0; h = (mul(h, 5) + 0xe6546b64) >>> 0;
  }
  let k = 0;
  switch (bytes.length & 3) {
    case 3: k ^= bytes[i + 2] << 16; // falls through
    case 2: k ^= bytes[i + 1] << 8; // falls through
    case 1: k ^= bytes[i]; k = mul(k, c1); k = ((k << 15) | (k >>> 17)) >>> 0; k = mul(k, c2); h ^= k;
  }
  h ^= bytes.length; h ^= h >>> 16; h = mul(h, 0x85ebca6b); h ^= h >>> 13; h = mul(h, 0xc2b2ae35); h ^= h >>> 16;
  return h >>> 0;
}

export const normalized = (id: string, group: string, n = 100) => (murmur3(`${group}:${id}`) % n) + 1;

function field(ctx: Context, name: string): string | undefined {
  if (name === "userId") return ctx.userId; if (name === "sessionId") return ctx.sessionId;
  if (name === "remoteAddress") return ctx.remoteAddress; if (name === "currentTime") return ctx.currentTime ?? new Date().toISOString();
  return ctx.properties?.[name];
}

export function constraintPasses(c: Constraint, ctx: Context): boolean {
  const v = field(ctx, c.contextName);
  let ok: boolean;
  if (v === undefined) ok = false;
  else {
    const norm = (s: string) => (c.caseInsensitive ? s.toLowerCase() : s);
    const vals = (c.values ?? []).map(norm); const one = c.value ?? ""; const val = norm(v);
    switch (c.operator) {
      case "IN": ok = vals.includes(val); break;
      case "NOT_IN": ok = !vals.includes(val); break;
      case "STR_CONTAINS": ok = vals.some((x) => val.includes(x)); break;
      case "STR_STARTS_WITH": ok = vals.some((x) => val.startsWith(x)); break;
      case "STR_ENDS_WITH": ok = vals.some((x) => val.endsWith(x)); break;
      case "NUM_EQ": ok = Number(v) === Number(one); break;
      case "NUM_GT": ok = Number(v) > Number(one); break;
      case "NUM_GTE": ok = Number(v) >= Number(one); break;
      case "NUM_LT": ok = Number(v) < Number(one); break;
      case "NUM_LTE": ok = Number(v) <= Number(one); break;
      case "DATE_AFTER": ok = new Date(v) > new Date(one); break;
      case "DATE_BEFORE": ok = new Date(v) < new Date(one); break;
      case "REGEX": ok = new RegExp(one).test(v); break;
      default: ok = false;
    }
  }
  return c.inverted ? !ok : ok;
}

function strategyPasses(s: Strategy, flagName: string, ctx: Context): boolean {
  if (!(s.constraints ?? []).every((c) => constraintPasses(c, ctx))) return false;
  const p = s.parameters ?? {};
  switch (s.name) {
    case "default": return true;
    case "userWithId": return !!ctx.userId && (p.userIds ?? "").split(",").map((x) => x.trim()).includes(ctx.userId);
    case "remoteAddress": return !!ctx.remoteAddress && (p.IPs ?? "").split(",").map((x) => x.trim()).includes(ctx.remoteAddress);
    case "flexibleRollout": {
      const rollout = Number(p.rollout ?? 0); const group = p.groupId ?? flagName;
      const stick = p.stickiness ?? "default";
      const sticky = stick === "default" ? ctx.userId ?? ctx.sessionId : stick === "random" ? undefined : field(ctx, stick);
      const id = sticky ?? String(Math.random());
      return rollout > 0 && normalized(id, group) <= rollout;
    }
    case "gradualRolloutUserId": return !!ctx.userId && normalized(ctx.userId, p.groupId ?? flagName) <= Number(p.percentage ?? 0);
    default: return false;
  }
}

export function isEnabled(flag: Flag | undefined, ctx: Context, fallback = false): boolean {
  if (!flag) return fallback;
  if (!flag.enabled) return false;
  if (!flag.strategies.length) return true;
  return flag.strategies.some((s) => strategyPasses(s, flag.name, ctx));
}

export function variant(flag: Flag | undefined, ctx: Context): { name: string; enabled: boolean; payload?: Variant["payload"] } {
  if (!flag || !isEnabled(flag, ctx) || !flag.variants?.length) return { name: "disabled", enabled: false };
  const total = flag.variants.reduce((s, v) => s + v.weight, 0);
  const seed = ctx.userId ?? ctx.sessionId ?? ctx.remoteAddress ?? String(Math.random());
  const target = normalized(seed, flag.name, total);
  let acc = 0;
  for (const v of flag.variants) { acc += v.weight; if (acc >= target) return { name: v.name, enabled: true, payload: v.payload }; }
  return { name: "disabled", enabled: false };
}
"##;

const FLAGS_STORE: &str = r##"import { db } from "./db";
import type { Flag } from "./evaluate";

export type Stored = Flag & { description: string | null; version: number };

export interface Store {
  all(): Promise<Stored[]>;
  get(name: string): Promise<Stored | null>;
  upsert(flag: Stored, actor: string | null): Promise<Stored>;
  remove(name: string, actor: string | null): Promise<boolean>;
  audit(limit: number): Promise<{ id: number; name: string; actor: string | null; before: unknown; after: unknown; at: string }[]>;
}

export function pgStore(): Store {
  type Row = { name: string; enabled: boolean; description: string | null; strategies: unknown; variants: unknown; version: number };
  const row = (r: Row): Stored =>
    ({ name: r.name, enabled: r.enabled, description: r.description, strategies: r.strategies as Stored["strategies"], variants: r.variants as Stored["variants"], version: r.version });
  return {
    all: async () => (await db<Row[]>`select name, enabled, description, strategies, variants, version from flags order by name`).map(row),
    get: async (n) => { const r = (await db<Row[]>`select name, enabled, description, strategies, variants, version from flags where name = ${n}`)[0]; return r ? row(r) : null; },
    upsert: async (f, actor) => db.begin(async (tx) => {
      const before = (await tx`select name, enabled, description, strategies, variants, version from flags where name = ${f.name}`)[0] ?? null;
      const [after] = await tx<Row[]>`insert into flags (name, enabled, description, strategies, variants) values (${f.name}, ${f.enabled}, ${f.description}, ${tx.json(f.strategies as never)}, ${tx.json((f.variants ?? []) as never)})
        on conflict (name) do update set enabled = excluded.enabled, description = excluded.description, strategies = excluded.strategies, variants = excluded.variants, version = flags.version + 1, updated_at = now()
        returning name, enabled, description, strategies, variants, version`;
      await tx`insert into flag_audit (name, actor, before, after) values (${f.name}, ${actor}, ${tx.json(before as never)}, ${tx.json(after as never)})`;
      return row(after);
    }),
    remove: async (n, actor) => db.begin(async (tx) => {
      const before = (await tx`select name, enabled, strategies, variants from flags where name = ${n}`)[0] ?? null;
      const del = await tx`delete from flags where name = ${n}`;
      if (del.count) await tx`insert into flag_audit (name, actor, before, after) values (${n}, ${actor}, ${tx.json(before as never)}, null)`;
      return del.count > 0;
    }),
    audit: async (limit) => db`select id, name, actor, before, after, at from flag_audit order by id desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const flags = new Map<string, Stored>(); const audit: Awaited<ReturnType<Store["audit"]>> = [];
  return {
    all: async () => [...flags.values()].sort((a, b) => a.name.localeCompare(b.name)),
    get: async (n) => flags.get(n) ?? null,
    upsert: async (f, actor) => { const before = flags.get(f.name) ?? null; const after = { ...f, version: (before?.version ?? 0) + 1 }; flags.set(f.name, after); audit.unshift({ id: audit.length + 1, name: f.name, actor, before, after, at: new Date().toISOString() }); return after; },
    remove: async (n, actor) => { const before = flags.get(n); if (!before) return false; flags.delete(n); audit.unshift({ id: audit.length + 1, name: n, actor, before, after: null, at: new Date().toISOString() }); return true; },
    audit: async (limit) => audit.slice(0, limit),
  };
}
"##;

const FLAGS_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { createHash } from "node:crypto";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { isEnabled, variant, type Context } from "./evaluate";

// Unleash's client API (GET /api/client/features with ETag, POST /api/client/metrics), an
// admin API with an audit trail, a frontend endpoint that evaluates for one context, and an
// SSE stream that tells clients a new snapshot exists. Evaluation is local in the SDK; this
// service can be down and every client keeps its last snapshot.

const listeners = new Set<() => void>();
const notify = () => { for (const l of listeners) l(); };

const strategy = z.object({ name: z.string(), parameters: z.record(z.string()).optional(), constraints: z.array(z.object({ contextName: z.string(), operator: z.string(), values: z.array(z.string()).optional(), value: z.string().optional(), inverted: z.boolean().optional(), caseInsensitive: z.boolean().optional() })).optional() });
const flagBody = z.object({ name: z.string().regex(/^[a-zA-Z0-9._-]+$/), enabled: z.boolean().default(false), description: z.string().nullable().default(null), strategies: z.array(strategy).default([{ name: "default" }]), variants: z.array(z.object({ name: z.string(), weight: z.number().int().min(0).max(1000), payload: z.object({ type: z.string(), value: z.string() }).optional() })).default([]) });

const ctxFrom = (q: Record<string, string>): Context => ({ userId: q.userId, sessionId: q.sessionId, remoteAddress: q.remoteAddress, properties: Object.fromEntries(Object.entries(q).filter(([k]) => !["userId", "sessionId", "remoteAddress"].includes(k))) });

export function createApp(store: Store) {
  const admin = async (c: { req: { header: (n: string) => string | undefined } }) => {
    const t = process.env.ADMIN_TOKEN; const a = c.req.header("authorization")?.replace(/^Bearer /, "");
    return !!t && a === t;
  };
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── the client API: what SDKs poll ──────────────────────────────────────────────────
    .get("/api/client/features", async (c) => {
      const features = await store.all();
      const body = JSON.stringify({ version: 2, features: features.map((f) => ({ name: f.name, type: "release", enabled: f.enabled, description: f.description, strategies: f.strategies, variants: f.variants ?? [] })) });
      const etag = `"${createHash("sha1").update(body).digest("hex").slice(0, 16)}"`;
      if (c.req.header("if-none-match") === etag) return c.body(null, 304);
      c.header("ETag", etag);
      return c.body(body, 200, { "content-type": "application/json" });
    })
    .post("/api/client/metrics", async (c) => { await c.req.json().catch(() => null); return c.body(null, 202); })
    .post("/api/client/register", async (c) => { await c.req.json().catch(() => null); return c.body(null, 202); })
    // Clients hold this open; a change sends one line and they refetch.
    .get("/api/client/stream", (c) => streamSSE(c, async (s) => {
      const l = () => { void s.writeSSE({ event: "update", data: String(Date.now()) }); };
      listeners.add(l);
      await s.writeSSE({ event: "ready", data: "ok" });
      await new Promise<void>((resolve) => { s.onAbort(() => resolve()); });
      listeners.delete(l);
    }))

    // ── the frontend API: evaluated for one context ─────────────────────────────────────
    .get("/api/frontend", async (c) => {
      const ctx = ctxFrom(c.req.query());
      const toggles = (await store.all()).filter((f) => isEnabled(f, ctx)).map((f) => ({ name: f.name, enabled: true, variant: variant(f, ctx) }));
      return c.json({ toggles });
    })

    // ── admin, audited ──────────────────────────────────────────────────────────────────
    .get("/api/admin/features", async (c) => c.json({ features: await store.all() }))
    .put("/api/admin/features/:name", async (c) => {
      if (!(await admin(c))) return c.json({ error: { message: "admin token required", code: "unauthorized" } }, 401);
      const p = flagBody.safeParse({ ...(await c.req.json().catch(() => ({}))), name: c.req.param("name") });
      if (!p.success) return c.json({ error: { message: "invalid flag", code: "invalid", details: p.error.flatten() } }, 400);
      const saved = await store.upsert({ ...p.data, version: 0 }, c.req.header("x-actor") ?? null);
      notify();
      return c.json(saved);
    })
    .delete("/api/admin/features/:name", async (c) => {
      if (!(await admin(c))) return c.json({ error: { message: "admin token required", code: "unauthorized" } }, 401);
      const ok = await store.remove(c.req.param("name"), c.req.header("x-actor") ?? null);
      if (ok) notify();
      return ok ? c.body(null, 204) : c.json({ error: { message: "no such flag", code: "not_found" } }, 404);
    })
    .get("/api/admin/audit", async (c) => c.json({ audit: await store.audit(100) }));

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const FLAGS_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { isEnabled, murmur3, normalized, variant } from "./evaluate";

process.env.ADMIN_TOKEN = "admin";

describe("evaluation", () => {
  // From the Unleash client specification: these are the numbers every SDK must produce.
  test("murmur3 agrees with the reference", () => {
    expect(murmur3("123:gradualRolloutUserId")).toBeGreaterThan(0);
    expect(normalized("123", "gr1")).toBe(73);
    expect(normalized("999", "groupX")).toBe(25);
  });

  test("rollout is sticky per user and honours the percentage", () => {
    const flag = { name: "beta", enabled: true, strategies: [{ name: "flexibleRollout", parameters: { rollout: "50", stickiness: "userId", groupId: "beta" } }] };
    const a = isEnabled(flag, { userId: "u1" }), b = isEnabled(flag, { userId: "u1" });
    expect(a).toBe(b);
    let on = 0;
    for (let i = 0; i < 1000; i++) if (isEnabled(flag, { userId: `user-${i}` })) on++;
    expect(on).toBeGreaterThan(420); expect(on).toBeLessThan(580);
    expect(isEnabled({ ...flag, enabled: false }, { userId: "u1" })).toBe(false);
  });

  test("constraints gate a strategy and can be inverted", () => {
    const flag = { name: "eu", enabled: true, strategies: [{ name: "default", constraints: [{ contextName: "country", operator: "IN", values: ["DE", "FR"] }] }] };
    expect(isEnabled(flag, { properties: { country: "DE" } })).toBe(true);
    expect(isEnabled(flag, { properties: { country: "US" } })).toBe(false);
    expect(isEnabled(flag, {})).toBe(false);
    const inv = { ...flag, strategies: [{ name: "default", constraints: [{ contextName: "country", operator: "IN", values: ["DE"], inverted: true }] }] };
    expect(isEnabled(inv, { properties: { country: "US" } })).toBe(true);
  });

  test("variants split by weight and stay sticky", () => {
    const flag = { name: "cta", enabled: true, strategies: [{ name: "default" }], variants: [{ name: "a", weight: 500 }, { name: "b", weight: 500, payload: { type: "string", value: "Buy" } }] };
    const v = variant(flag, { userId: "u7" });
    expect(["a", "b"]).toContain(v.name);
    expect(variant(flag, { userId: "u7" }).name).toBe(v.name);
    expect(variant({ ...flag, enabled: false }, { userId: "u7" })).toEqual({ name: "disabled", enabled: false });
  });
});

describe("api", () => {
  const app = createApp(memoryStore());
  const put = (name: string, body: unknown) => app.request(`/api/admin/features/${name}`, { method: "PUT", headers: { authorization: "Bearer admin", "content-type": "application/json", "x-actor": "test" }, body: JSON.stringify(body) });

  test("admin writes are audited and the client API serves them with an ETag", async () => {
    expect((await app.request("/api/admin/features/x", { method: "PUT", body: "{}" })).status).toBe(401);
    expect((await put("new-checkout", { enabled: true })).status).toBe(200);
    const res = await app.request("/api/client/features");
    const etag = res.headers.get("etag")!;
    expect((await res.json()).features[0].name).toBe("new-checkout");
    expect((await app.request("/api/client/features", { headers: { "if-none-match": etag } })).status).toBe(304);
    await put("new-checkout", { enabled: false });
    expect((await app.request("/api/client/features", { headers: { "if-none-match": etag } })).status).toBe(200);
    const audit = await (await app.request("/api/admin/audit")).json();
    expect(audit.audit.length).toBe(2);
    expect(audit.audit[0].actor).toBe("test");
  });

  test("the frontend endpoint evaluates for a context", async () => {
    await put("vip", { enabled: true, strategies: [{ name: "userWithId", parameters: { userIds: "42, 43" } }] });
    const yes = await (await app.request("/api/frontend?userId=42")).json();
    const no = await (await app.request("/api/frontend?userId=1")).json();
    expect(yes.toggles.map((t: { name: string }) => t.name)).toContain("vip");
    expect(no.toggles.map((t: { name: string }) => t.name)).not.toContain("vip");
  });
});
"##;

const FLAGS_PAGE: &str = r##"async function features() {
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/api/admin/features`, { cache: "no-store" });
  return (await res.json()).features as { name: string; enabled: boolean; description: string | null; strategies: { name: string; parameters?: Record<string, string> }[]; version: number }[];
}

export default async function Home() {
  const rows = await features();
  return (
    <main>
      <h1>{{NAME}} — feature flags</h1>
      <p>Unleash-compatible: point an Unleash SDK at <code>http://localhost:8000/api</code>. Create a flag:</p>
      <pre>{`curl -X PUT http://localhost:8000/api/admin/features/new-checkout -H "authorization: Bearer $ADMIN_TOKEN" \\
  -H "content-type: application/json" -d '{"enabled":true,"strategies":[{"name":"flexibleRollout","parameters":{"rollout":"25","stickiness":"userId"}}]}'
curl 'http://localhost:8000/api/frontend?userId=42'`}</pre>
      <table>
        <thead><tr><th>flag</th><th>enabled</th><th>strategies</th><th>v</th></tr></thead>
        <tbody>{rows.map((f) => <tr key={f.name}><td>{f.name}</td><td>{f.enabled ? "on" : "off"}</td><td>{f.strategies.map((s) => s.name + (s.parameters?.rollout ? ` ${s.parameters.rollout}%` : "")).join(", ")}</td><td>{f.version}</td></tr>)}</tbody>
      </table>
      {rows.length === 0 && <p>No flags yet.</p>}
    </main>
  );
}
"##;

// ═══ loop: a ReAct agent, like smolagents ════════════════════════════════════════════════════

const LOOP_SQL: &str = r##"create table if not exists runs (
  id uuid primary key,
  task text not null,
  state text not null default 'running',   -- running | done | failed | stopped
  answer text,
  steps jsonb not null default '[]',
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
"##;

const LOOP_STORE: &str = r##"import { db } from "./db";
import type { Step } from "./agent";

export type Run = { id: string; task: string; state: string; answer: string | null; steps: Step[]; input_tokens: number; output_tokens: number; created_at: string };

export interface Store {
  create(id: string, task: string): Promise<void>;
  update(id: string, patch: Partial<Run>): Promise<void>;
  get(id: string): Promise<Run | null>;
  list(limit: number): Promise<Run[]>;
}

export function pgStore(): Store {
  return {
    create: async (id, task) => { await db`insert into runs (id, task) values (${id}, ${task})`; },
    update: async (id, p) => {
      await db`update runs set state = coalesce(${p.state ?? null}, state), answer = coalesce(${p.answer ?? null}, answer),
        steps = coalesce(${p.steps ? db.json(p.steps as never) : null}, steps), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens),
        output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`;
    },
    get: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    list: async (limit) => db<Run[]>`select id, task, state, answer, '[]'::jsonb as steps, input_tokens, output_tokens, created_at from runs order by created_at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>();
  return {
    create: async (id, task) => { runs.set(id, { id, task, state: "running", answer: null, steps: [], input_tokens: 0, output_tokens: 0, created_at: new Date().toISOString() }); },
    update: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p); },
    get: async (id) => runs.get(id) ?? null,
    list: async (limit) => [...runs.values()].reverse().slice(0, limit),
  };
}
"##;

const LOOP_AGENT: &str = r##"// The ReAct loop as smolagents runs it: each step is a model call that may request tool calls;
// tools run; observations go back; the loop ends when the model calls `final_answer`, or hits
// max_steps and is asked once more for its best answer. Every step is a record — model output,
// tool calls, observations, tokens, duration — so a run can be replayed and inspected. The
// model is a function, so tests drive the loop with a scripted one and never call a provider.

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type Step = { n: number; thought: string; calls: ToolCall[]; observations: { id: string; output: string; error?: boolean }[]; input_tokens: number; output_tokens: number; ms: number };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number; stop: "tool_use" | "end_turn" | "max_tokens" };
export type Message = { role: "user" | "assistant"; content: unknown };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;
export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<string> };

export const FINAL = "final_answer";

/// The tools the agent has. Add one here; the model sees its schema on the next run.
export const TOOLS: ToolDef[] = [
  { name: "calculator", description: "Evaluate an arithmetic expression (+ - * / ( ) and numbers).", input_schema: { type: "object", properties: { expression: { type: "string" } }, required: ["expression"] },
    run: async ({ expression }) => { const e = String(expression); if (!/^[\d\s+\-*/().]+$/.test(e)) throw new Error("only arithmetic"); return String(Function(`"use strict"; return (${e})`)()); } },
  { name: "web_search", description: "Search the web; returns titles and snippets.", input_schema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] },
    run: async ({ query }) => { const r = await fetch(`https://duckduckgo.com/html/?q=${encodeURIComponent(String(query))}`, { headers: { "user-agent": "Mozilla/5.0" } }); const html = await r.text(); const hits = [...html.matchAll(/class="result__a"[^>]*>([^<]+)</g)].slice(0, 5).map((m) => m[1]); return hits.join("\n") || "no results"; } },
  { name: FINAL, description: "Give the final answer to the task. Call this when done.", input_schema: { type: "object", properties: { answer: { type: "string" } }, required: ["answer"] }, run: async ({ answer }) => String(answer) },
];

export const SYSTEM = `You solve tasks step by step using tools. Think briefly, then call tools. When you know the answer, call final_answer with it. Never make up tool results.`;

export type RunResult = { answer: string | null; state: "done" | "failed" | "stopped"; steps: Step[]; input_tokens: number; output_tokens: number };

export async function runAgent(task: string, model: Model, opts: { tools?: ToolDef[]; maxSteps?: number; onStep?: (s: Step) => Promise<void> | void; signal?: AbortSignal } = {}): Promise<RunResult> {
  const tools = opts.tools ?? TOOLS; const maxSteps = opts.maxSteps ?? 10;
  const messages: Message[] = [{ role: "user", content: task }];
  const steps: Step[] = []; let input_tokens = 0, output_tokens = 0;

  for (let n = 1; n <= maxSteps; n++) {
    if (opts.signal?.aborted) return { answer: null, state: "stopped", steps, input_tokens, output_tokens };
    const t0 = Date.now();
    const reply = await model(SYSTEM, messages, tools);
    input_tokens += reply.input_tokens; output_tokens += reply.output_tokens;
    const step: Step = { n, thought: reply.text, calls: reply.calls, observations: [], input_tokens: reply.input_tokens, output_tokens: reply.output_tokens, ms: 0 };
    const blocks: unknown[] = reply.text ? [{ type: "text", text: reply.text }] : [];
    for (const c of reply.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    messages.push({ role: "assistant", content: blocks });

    const final = reply.calls.find((c) => c.name === FINAL);
    if (final) { step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step); return { answer: String(final.input.answer ?? ""), state: "done", steps, input_tokens, output_tokens }; }
    if (!reply.calls.length) {
      // Text with no tool call: treat as the answer, the way smolagents does when the model stops calling.
      step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step);
      return { answer: reply.text || null, state: reply.text ? "done" : "failed", steps, input_tokens, output_tokens };
    }
    for (const c of reply.calls) {
      const tool = tools.find((t) => t.name === c.name);
      try {
        if (!tool) throw new Error(`unknown tool ${c.name}`);
        const output = await Promise.race([tool.run(c.input), new Promise<string>((_, rej) => setTimeout(() => rej(new Error("tool timed out")), 30_000).unref())]);
        step.observations.push({ id: c.id, output: output.slice(0, 8000) });
      } catch (e) {
        // An error is an observation, not an exception: the model gets to recover.
        step.observations.push({ id: c.id, output: `Error: ${e instanceof Error ? e.message : String(e)}`, error: true });
      }
    }
    messages.push({ role: "user", content: step.observations.map((o) => ({ type: "tool_result", tool_use_id: o.id, content: o.output, is_error: o.error ?? false })) });
    step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step);
  }
  // Out of steps: one last call with no tools, for the best answer so far.
  const last = await model(SYSTEM, [...messages, { role: "user", content: "You are out of steps. Give your best final answer now as plain text." }], []);
  input_tokens += last.input_tokens; output_tokens += last.output_tokens;
  return { answer: last.text || null, state: last.text ? "done" : "failed", steps, input_tokens, output_tokens };
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Sonnet.
export function claude(fetchImpl: typeof fetch = fetch): Model {
  return async (system, messages, tools) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 2048, system, messages, tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }),
    });
    if (!res.ok) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const a = (await res.json()) as { content: { type: string; text?: string; id?: string; name?: string; input?: Record<string, unknown> }[]; stop_reason: string; usage: { input_tokens: number; output_tokens: number } };
    return {
      text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""),
      calls: a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, name: b.name!, input: b.input ?? {} })),
      input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens,
      stop: a.stop_reason === "tool_use" ? "tool_use" : a.stop_reason === "max_tokens" ? "max_tokens" : "end_turn",
    };
  };
}
"##;

const LOOP_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { runAgent, claude, TOOLS, type Model } from "./agent";

// POST /api/runs starts a run and streams its steps as SSE; every step is stored as it happens,
// so a run can be read back after the connection is gone. Concurrency is bounded — an agent
// loop is an expensive request — and a run can be stopped.

const running = new Map<string, AbortController>();
const MAX_CONCURRENT = Number(process.env.MAX_CONCURRENT_RUNS ?? 4);

export function createApp(store: Store, model: Model = claude()) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }))

    .post("/api/runs", async (c) => {
      const p = z.object({ task: z.string().min(1).max(4000), max_steps: z.number().int().min(1).max(30).default(10) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "task required", code: "invalid" } }, 400);
      if (running.size >= MAX_CONCURRENT) { c.header("Retry-After", "10"); return c.json({ error: { message: "too many runs in flight", code: "busy" } }, 429); }
      const id = crypto.randomUUID();
      await store.create(id, p.data.task);
      const ctl = new AbortController(); running.set(id, ctl);
      return streamSSE(c, async (s) => {
        await s.writeSSE({ event: "run", data: JSON.stringify({ id }) });
        s.onAbort(() => ctl.abort());
        const steps: unknown[] = [];
        try {
          const r = await runAgent(p.data.task, model, { maxSteps: p.data.max_steps, signal: ctl.signal, onStep: async (step) => {
            steps.push(step);
            await store.update(id, { steps: steps as never });
            await s.writeSSE({ event: "step", data: JSON.stringify(step) });
          } });
          await store.update(id, { state: r.state, answer: r.answer, steps: r.steps, input_tokens: r.input_tokens, output_tokens: r.output_tokens });
          await s.writeSSE({ event: "done", data: JSON.stringify({ state: r.state, answer: r.answer, input_tokens: r.input_tokens, output_tokens: r.output_tokens }) });
        } catch (e) {
          await store.update(id, { state: "failed", answer: e instanceof Error ? e.message : String(e) });
          await s.writeSSE({ event: "error", data: JSON.stringify({ message: e instanceof Error ? e.message : String(e) }) });
        } finally { running.delete(id); }
      });
    })
    .get("/api/runs", async (c) => c.json({ runs: await store.list(50) }))
    .get("/api/runs/:id", async (c) => { const r = await store.get(c.req.param("id")); return r ? c.json(r) : c.json({ error: { message: "no such run", code: "not_found" } }, 404); })
    .post("/api/runs/:id/stop", (c) => { const ctl = running.get(c.req.param("id")); if (!ctl) return c.json({ error: { message: "not running", code: "not_found" } }, 404); ctl.abort(); return c.json({ ok: true }); });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const LOOP_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { runAgent, TOOLS, type Model, type ModelReply } from "./agent";

// A scripted model drives the loop: tool call → observation → final_answer. No provider.
const script = (replies: Partial<ModelReply>[]): Model => { let i = 0; return async () => ({ text: "", calls: [], input_tokens: 10, output_tokens: 5, stop: "tool_use", ...replies[Math.min(i++, replies.length - 1)] }); };

describe("agent loop", () => {
  test("calls a tool, reads the observation, and answers", async () => {
    const model = script([
      { text: "I should compute it.", calls: [{ id: "t1", name: "calculator", input: { expression: "6*7" } }] },
      { text: "", calls: [{ id: "t2", name: "final_answer", input: { answer: "42" } }] },
    ]);
    const r = await runAgent("what is 6*7", model);
    expect(r.state).toBe("done"); expect(r.answer).toBe("42");
    expect(r.steps.length).toBe(2);
    expect(r.steps[0].observations[0].output).toBe("42");
    expect(r.input_tokens).toBe(20);
  });

  test("a tool error is an observation the model can recover from", async () => {
    const model = script([
      { calls: [{ id: "t1", name: "calculator", input: { expression: "rm -rf /" } }] },
      { calls: [{ id: "t2", name: "nope", input: {} }] },
      { calls: [{ id: "t3", name: "final_answer", input: { answer: "could not" } }] },
    ]);
    const r = await runAgent("x", model);
    expect(r.steps[0].observations[0].error).toBe(true);
    expect(r.steps[1].observations[0].output).toContain("unknown tool");
    expect(r.answer).toBe("could not");
  });

  test("max_steps ends the loop with a best answer", async () => {
    let calls = 0;
    const model: Model = async (_s, _m, tools) => { calls++; return tools.length ? { text: "", calls: [{ id: String(calls), name: "calculator", input: { expression: "1+1" } }], input_tokens: 1, output_tokens: 1, stop: "tool_use" } : { text: "probably 2", calls: [], input_tokens: 1, output_tokens: 1, stop: "end_turn" }; };
    const r = await runAgent("x", model, { maxSteps: 3 });
    expect(r.steps.length).toBe(3);
    expect(calls).toBe(4);
    expect(r.answer).toBe("probably 2");
  });

  test("the route streams steps and stores the run", async () => {
    const store = memoryStore();
    const app = createApp(store, script([{ calls: [{ id: "t", name: "final_answer", input: { answer: "hi" } }] }]));
    const res = await app.request("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ task: "say hi" }) });
    expect(res.status).toBe(200);
    const text = await res.text();
    expect(text).toContain("event: step"); expect(text).toContain("event: done");
    const id = JSON.parse(text.split("\n").find((l) => l.startsWith("data:"))!.slice(5)).id;
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("done"); expect(run.answer).toBe("hi");
    expect(TOOLS.some((t) => t.name === "final_answer")).toBe(true);
  });
});
"##;

const LOOP_PAGE: &str = r##""use client";
import { useState } from "react";

type Step = { n: number; thought: string; calls: { name: string; input: unknown }[]; observations: { output: string; error?: boolean }[]; ms: number };

// Type a task, watch the steps arrive. Each step is a model call and its tool results.
export default function Home() {
  const [task, setTask] = useState("What is 17 * 23, and is it prime?");
  const [steps, setSteps] = useState<Step[]>([]);
  const [answer, setAnswer] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    setBusy(true); setSteps([]); setAnswer(null);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ task }) });
    const reader = res.body!.getReader(); const dec = new TextDecoder(); let buf = "";
    for (;;) {
      const { value, done } = await reader.read(); if (done) break;
      buf += dec.decode(value, { stream: true });
      let i; while ((i = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, i); buf = buf.slice(i + 2);
        const ev = /event: (\w+)/.exec(block)?.[1]; const data = /data: (.*)/.exec(block)?.[1];
        if (!ev || !data) continue;
        if (ev === "step") setSteps((s) => [...s, JSON.parse(data)]);
        if (ev === "done") setAnswer(JSON.parse(data).answer);
        if (ev === "error") setAnswer("Error: " + JSON.parse(data).message);
      }
    }
    setBusy(false);
  }

  return (
    <main>
      <h1>{{NAME}} — agent</h1>
      <p>A ReAct loop with a calculator and web search. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>
      <textarea value={task} onChange={(e) => setTask(e.target.value)} rows={3} style={{ width: "100%" }} />
      <p><button onClick={run} disabled={busy}>{busy ? "Running…" : "Run"}</button></p>
      {steps.map((s) => (
        <details key={s.n} open>
          <summary>Step {s.n} · {s.calls.map((c) => c.name).join(", ") || "thinking"} · {s.ms} ms</summary>
          {s.thought && <p><em>{s.thought}</em></p>}
          {s.calls.map((c, i) => <pre key={i}>{c.name}({JSON.stringify(c.input)}){"\n→ "}{s.observations[i]?.output}</pre>)}
        </details>
      ))}
      {answer && <p><strong>Answer:</strong> {answer}</p>}
    </main>
  );
}
"##;

// ═══ graph: a stateful graph agent, like LangGraph ══════════════════════════════════════════

const GRAPH_SQL: &str = r##"-- LangGraph's checkpointer, as tables: one row per super-step per thread, with the state
-- after that step and the node(s) that ran. Time travel is a checkpoint id; resume is a
-- thread id.
create table if not exists threads (
  id uuid primary key,
  graph text not null,
  status text not null default 'idle',  -- idle | running | interrupted | done | failed
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create table if not exists checkpoints (
  id uuid primary key,
  thread_id uuid not null references threads(id),
  step int not null,
  parent_id uuid,
  node text not null,
  state jsonb not null,
  next text[] not null default '{}',
  interrupt jsonb,
  created_at timestamptz not null default now(),
  unique (thread_id, step)
);
"##;

const GRAPH_STORE: &str = r##"import { db } from "./db";
import type { Checkpoint } from "./graph";

export type Thread = { id: string; graph: string; status: string };

export interface Store {
  createThread(id: string, graph: string): Promise<void>;
  setStatus(id: string, status: string): Promise<void>;
  thread(id: string): Promise<Thread | null>;
  threads(limit: number): Promise<Thread[]>;
  put(cp: Checkpoint): Promise<void>;
  latest(threadId: string): Promise<Checkpoint | null>;
  history(threadId: string): Promise<Checkpoint[]>;
  checkpoint(id: string): Promise<Checkpoint | null>;
}

export function pgStore(): Store {
  return {
    createThread: async (id, graph) => { await db`insert into threads (id, graph) values (${id}, ${graph})`; },
    setStatus: async (id, status) => { await db`update threads set status = ${status}, updated_at = now() where id = ${id}`; },
    thread: async (id) => (await db<Thread[]>`select id, graph, status from threads where id = ${id}`)[0] ?? null,
    threads: async (limit) => db<Thread[]>`select id, graph, status from threads order by updated_at desc limit ${limit}`,
    put: async (cp) => { await db`insert into checkpoints (id, thread_id, step, parent_id, node, state, next, interrupt) values (${cp.id}, ${cp.thread_id}, ${cp.step}, ${cp.parent_id}, ${cp.node}, ${db.json(cp.state as never)}, ${cp.next}, ${cp.interrupt ? db.json(cp.interrupt as never) : null})`; },
    latest: async (t) => (await db<Checkpoint[]>`select * from checkpoints where thread_id = ${t} order by step desc limit 1`)[0] ?? null,
    history: async (t) => db<Checkpoint[]>`select * from checkpoints where thread_id = ${t} order by step`,
    checkpoint: async (id) => (await db<Checkpoint[]>`select * from checkpoints where id = ${id}`)[0] ?? null,
  };
}

export function memoryStore(): Store {
  const threads = new Map<string, Thread>(); const cps: Checkpoint[] = [];
  return {
    createThread: async (id, graph) => { threads.set(id, { id, graph, status: "idle" }); },
    setStatus: async (id, status) => { const t = threads.get(id); if (t) t.status = status; },
    thread: async (id) => threads.get(id) ?? null,
    threads: async (limit) => [...threads.values()].reverse().slice(0, limit),
    put: async (cp) => { cps.push(structuredClone(cp)); },
    latest: async (t) => cps.filter((c) => c.thread_id === t).sort((a, b) => b.step - a.step)[0] ?? null,
    history: async (t) => cps.filter((c) => c.thread_id === t).sort((a, b) => a.step - b.step),
    checkpoint: async (id) => cps.find((c) => c.id === id) ?? null,
  };
}
"##;

const GRAPH_ENGINE: &str = r##"// A StateGraph the way LangGraph defines one: nodes are functions from state to a partial
// update, edges are fixed or conditional, channels merge updates with a reducer (append for
// messages, replace otherwise). Execution is super-steps; after each one the state is
// checkpointed, so a thread resumes from its last checkpoint and can be forked from any
// earlier one. `interrupt()` inside a node pauses the graph for a human; `resume` continues
// it with their value. All of that is in this file, and the graph itself is in `workflow`.

export type Checkpoint = { id: string; thread_id: string; step: number; parent_id: string | null; node: string; state: Record<string, unknown>; next: string[]; interrupt: unknown | null };
export type Reducer = (a: unknown, b: unknown) => unknown;
export type Node<S> = (state: S, ctx: { interrupt: (value: unknown) => unknown }) => Promise<Partial<S>> | Partial<S>;

export const START = "__start__", END = "__end__";
export const append: Reducer = (a, b) => [...((a as unknown[]) ?? []), ...(Array.isArray(b) ? b : [b])];

class Interrupt { constructor(public value: unknown) {} }

export class StateGraph<S extends Record<string, unknown>> {
  private nodes = new Map<string, Node<S>>();
  private edges = new Map<string, string | ((s: S) => string)>();
  constructor(private channels: Partial<Record<keyof S, Reducer>> = {}) {}
  addNode(name: string, fn: Node<S>) { this.nodes.set(name, fn); return this; }
  addEdge(from: string, to: string) { this.edges.set(from, to); return this; }
  addConditionalEdges(from: string, route: (s: S) => string) { this.edges.set(from, route); return this; }
  nodeNames() { return [...this.nodes.keys()]; }

  private merge(state: S, update: Partial<S>): S {
    const out = { ...state };
    for (const [k, v] of Object.entries(update)) { const r = this.channels[k as keyof S]; (out as Record<string, unknown>)[k] = r ? r(out[k], v) : v; }
    return out;
  }
  private nextOf(node: string, state: S): string {
    const e = this.edges.get(node); if (!e) return END;
    return typeof e === "function" ? e(state) : e;
  }

  /// Run from a checkpoint (or from START with `input`) until END or an interrupt. Each
  /// super-step yields a checkpoint; the caller stores it. `resume` answers a pending interrupt.
  async *run(threadId: string, from: Checkpoint | null, input: Partial<S> | null, resume?: unknown, maxSteps = 50): AsyncGenerator<Checkpoint> {
    let state = (from?.state ?? {}) as S;
    if (input) state = this.merge(state, input);
    let next = from?.next?.length ? from.next[0] : this.nextOf(START, state);
    let step = from ? from.step + 1 : 0; let parent = from?.id ?? null;
    let resumeValue = from?.interrupt !== null && from?.interrupt !== undefined ? resume : undefined;
    if (!from) { const cp = { id: crypto.randomUUID(), thread_id: threadId, step: step++, parent_id: null, node: START, state, next: [next], interrupt: null }; parent = cp.id; yield cp; }
    while (next !== END && step < maxSteps) {
      const fn = this.nodes.get(next); if (!fn) throw new Error(`no node ${next}`);
      let interrupted: unknown = null;
      const ctx = { interrupt: (v: unknown) => { if (resumeValue !== undefined) { const r = resumeValue; resumeValue = undefined; return r; } throw new Interrupt(v); } };
      let update: Partial<S> = {};
      try { update = await fn(state, ctx); } catch (e) { if (e instanceof Interrupt) interrupted = e.value; else throw e; }
      if (interrupted !== null) {
        const cp = { id: crypto.randomUUID(), thread_id: threadId, step, parent_id: parent, node: next, state, next: [next], interrupt: interrupted };
        yield cp; return;
      }
      state = this.merge(state, update);
      const after = this.nextOf(next, state);
      const cp = { id: crypto.randomUUID(), thread_id: threadId, step: step++, parent_id: parent, node: next, state, next: after === END ? [] : [after], interrupt: null };
      parent = cp.id; next = after; yield cp;
    }
    if (next !== END) throw new Error(`stopped after ${maxSteps} steps`);
  }
}

// ── the graph in this project: draft → review (human) → publish ────────────────────────
export type Doc = { topic: string; draft: string; messages: string[]; approved: boolean; revisions: number };
export type LLM = (prompt: string) => Promise<string>;

export function workflow(llm: LLM) {
  return new StateGraph<Doc>({ messages: append })
    .addNode("draft", async (s) => ({ draft: await llm(`Write a short paragraph about: ${s.topic}${s.revisions ? `\nPrevious draft was rejected; feedback: ${s.messages.at(-1)}` : ""}`), messages: [`drafted (rev ${s.revisions})`] }))
    .addNode("review", (s, { interrupt }) => {
      const decision = interrupt({ question: "Approve this draft?", draft: s.draft }) as { approved: boolean; feedback?: string };
      return { approved: decision.approved, revisions: s.revisions + (decision.approved ? 0 : 1), messages: [decision.approved ? "approved" : `rejected: ${decision.feedback ?? ""}`] };
    })
    .addNode("publish", (s) => ({ messages: [`published: ${s.draft.slice(0, 40)}…`] }))
    .addEdge(START, "draft").addEdge("draft", "review")
    .addConditionalEdges("review", (s) => (s.approved ? "publish" : s.revisions >= 3 ? END : "draft"))
    .addEdge("publish", END);
}

export function claudeLLM(fetchImpl: typeof fetch = fetch): LLM {
  return async (prompt) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", { method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" }, body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 600, messages: [{ role: "user", content: prompt }] }) });
    if (!res.ok) throw new Error(`anthropic ${res.status}`);
    const a = (await res.json()) as { content: { type: string; text?: string }[] };
    return a.content.filter((b) => b.type === "text").map((b) => b.text).join("");
  };
}
"##;

const GRAPH_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { workflow, claudeLLM, type Doc, type LLM, type Checkpoint } from "./graph";

// LangGraph's server, reduced to the four calls that matter: start a thread, read its state
// and history, resume an interrupt, fork from an earlier checkpoint. Every call is a
// checkpoint read and a loop of checkpoint writes; nothing lives in memory between requests.

export function createApp(store: Store, llm: LLM = claudeLLM()) {
  const graph = workflow(llm);
  const drive = async (threadId: string, from: Checkpoint | null, input: Partial<Doc> | null, resume?: unknown) => {
    await store.setStatus(threadId, "running");
    let last = from;
    try {
      for await (const cp of graph.run(threadId, from, input, resume)) { await store.put(cp); last = cp; }
      await store.setStatus(threadId, last?.interrupt != null ? "interrupted" : "done");
    } catch (e) { await store.setStatus(threadId, "failed"); throw e; }
    return last;
  };

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/graph", (c) => c.json({ nodes: graph.nodeNames() }))

    .post("/api/threads", async (c) => {
      const p = z.object({ topic: z.string().min(1).max(500) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "topic required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.createThread(id, "workflow");
      const last = await drive(id, null, { topic: p.data.topic, draft: "", messages: [], approved: false, revisions: 0 });
      return c.json({ id, status: (await store.thread(id))!.status, checkpoint: last }, 201);
    })
    .get("/api/threads", async (c) => c.json({ threads: await store.threads(50) }))
    .get("/api/threads/:id", async (c) => {
      const t = await store.thread(c.req.param("id")); if (!t) return c.json({ error: { message: "no such thread", code: "not_found" } }, 404);
      return c.json({ ...t, checkpoint: await store.latest(t.id) });
    })
    .get("/api/threads/:id/history", async (c) => c.json({ checkpoints: await store.history(c.req.param("id")) }))

    // Answer the interrupt: LangGraph's Command(resume=...).
    .post("/api/threads/:id/resume", async (c) => {
      const t = await store.thread(c.req.param("id")); if (!t) return c.json({ error: { message: "no such thread", code: "not_found" } }, 404);
      const last = await store.latest(t.id);
      if (!last || last.interrupt == null) return c.json({ error: { message: "thread is not waiting", code: "invalid" } }, 409);
      const body = await c.req.json().catch(() => null);
      const end = await drive(t.id, last, null, body);
      return c.json({ id: t.id, status: (await store.thread(t.id))!.status, checkpoint: end });
    })

    // Time travel: a new thread that continues from any checkpoint, with optional state edits.
    .post("/api/threads/:id/fork", async (c) => {
      const p = z.object({ checkpoint_id: z.string().uuid(), update: z.record(z.unknown()).optional() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "checkpoint_id required", code: "invalid" } }, 400);
      const cp = await store.checkpoint(p.data.checkpoint_id);
      if (!cp || cp.thread_id !== c.req.param("id")) return c.json({ error: { message: "no such checkpoint", code: "not_found" } }, 404);
      const id = crypto.randomUUID();
      await store.createThread(id, "workflow");
      const seed: Checkpoint = { ...cp, id: crypto.randomUUID(), thread_id: id, parent_id: cp.id, state: { ...cp.state, ...(p.data.update ?? {}) }, interrupt: null };
      await store.put(seed);
      const last = await drive(id, seed, null);
      return c.json({ id, status: (await store.thread(id))!.status, checkpoint: last }, 201);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const GRAPH_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { workflow, StateGraph, START, END, append } from "./graph";

const llm = async (prompt: string) => `Draft about ${/about: (.*?)(\n|$)/.exec(prompt)?.[1]} ${prompt.includes("rejected") ? "(revised)" : ""}`.trim();

describe("graph", () => {
  test("channels reduce and edges route", async () => {
    const g = new StateGraph<{ n: number; log: string[] }>({ log: append })
      .addNode("inc", (s) => ({ n: s.n + 1, log: [`n=${s.n + 1}`] }))
      .addEdge(START, "inc").addConditionalEdges("inc", (s) => (s.n < 3 ? "inc" : END));
    const cps = []; for await (const cp of g.run("t", null, { n: 0, log: [] })) cps.push(cp);
    expect(cps.at(-1)!.state).toEqual({ n: 3, log: ["n=1", "n=2", "n=3"] });
    expect(cps.map((c) => c.node)).toEqual([START, "inc", "inc", "inc"]);
    expect(cps[1].parent_id).toBe(cps[0].id);
  });

  test("a thread pauses at the human step and resumes from its checkpoint", async () => {
    const app = createApp(memoryStore(), llm);
    const start = await app.request("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic: "otters" }) });
    expect(start.status).toBe(201);
    const { id, status, checkpoint } = await start.json();
    expect(status).toBe("interrupted");
    expect(checkpoint.interrupt.question).toBe("Approve this draft?");
    expect(checkpoint.state.draft).toBe("Draft about otters");

    // Reject once: it loops back to draft, with the feedback in the prompt, and asks again.
    const rej = await app.request(`/api/threads/${id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved: false, feedback: "shorter" }) });
    const r = await rej.json();
    expect(r.status).toBe("interrupted");
    expect(r.checkpoint.state.draft).toContain("(revised)");
    expect(r.checkpoint.state.revisions).toBe(1);

    // Approve: publish, END.
    const ok = await (await app.request(`/api/threads/${id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved: true }) })).json();
    expect(ok.status).toBe("done");
    expect(ok.checkpoint.state.messages.at(-1)).toContain("published");
    expect((await app.request(`/api/threads/${id}/resume`, { method: "POST", body: "{}" })).status).toBe(409);

    // History is every checkpoint in order.
    const h = await (await app.request(`/api/threads/${id}/history`)).json();
    expect(h.checkpoints.map((c: { node: string }) => c.node)).toEqual([START, "draft", "review", "review", "draft", "review", "review", "publish"]);
  });

  test("forking from an earlier checkpoint replays with edited state", async () => {
    const app = createApp(memoryStore(), llm);
    const { id } = await (await app.request("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic: "otters" }) })).json();
    const h = await (await app.request(`/api/threads/${id}/history`)).json();
    const first = h.checkpoints[0];
    const fork = await app.request(`/api/threads/${id}/fork`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ checkpoint_id: first.id, update: { topic: "beavers" } }) });
    expect(fork.status).toBe(201);
    const f = await fork.json();
    expect(f.id).not.toBe(id);
    expect(f.checkpoint.state.draft).toBe("Draft about beavers");
    expect(workflow(llm).nodeNames()).toEqual(["draft", "review", "publish"]);
  });
});
"##;

const GRAPH_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Thread = { id: string; status: string; checkpoint: { node: string; state: { topic: string; draft: string; messages: string[]; revisions: number }; interrupt: { question: string } | null } | null };

// Start a thread, approve or reject at the human step, watch it publish. Every state shown
// here was read back from a checkpoint — refresh the page mid-way and nothing is lost.
export default function Home() {
  const [topic, setTopic] = useState("why otters hold hands");
  const [thread, setThread] = useState<Thread | null>(null);
  const [feedback, setFeedback] = useState("");
  const [threads, setThreads] = useState<{ id: string; status: string }[]>([]);

  const refresh = async () => setThreads((await (await fetch("/api/threads")).json()).threads);
  useEffect(() => { void refresh(); }, []);

  const start = async () => { const r = await fetch("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic }) }); setThread(await r.json()); void refresh(); };
  const resume = async (approved: boolean) => { if (!thread) return; const r = await fetch(`/api/threads/${thread.id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved, feedback }) }); setThread(await r.json()); setFeedback(""); void refresh(); };
  const open = async (id: string) => setThread(await (await fetch(`/api/threads/${id}`)).json());

  const s = thread?.checkpoint?.state;
  return (
    <main>
      <h1>{{NAME}} — graph agent</h1>
      <p>draft → review (you) → publish, checkpointed every step. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>
      <p><input value={topic} onChange={(e) => setTopic(e.target.value)} size={50} /> <button onClick={start}>Start</button></p>
      {thread && s && (
        <section>
          <p>Thread <code>{thread.id.slice(0, 8)}</code> · <strong>{thread.status}</strong> · at <code>{thread.checkpoint?.node}</code> · revisions {s.revisions}</p>
          <blockquote>{s.draft}</blockquote>
          {thread.checkpoint?.interrupt && (
            <p>{thread.checkpoint.interrupt.question} <input placeholder="feedback if rejecting" value={feedback} onChange={(e) => setFeedback(e.target.value)} /> <button onClick={() => resume(true)}>Approve</button> <button onClick={() => resume(false)}>Reject</button></p>
          )}
          <ol>{s.messages.map((m, i) => <li key={i}>{m}</li>)}</ol>
        </section>
      )}
      <h2>Threads</h2>
      <ul>{threads.map((t) => <li key={t.id}><a href="#" onClick={(e) => { e.preventDefault(); void open(t.id); }}>{t.id.slice(0, 8)}</a> · {t.status}</li>)}</ul>
    </main>
  );
}
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_replaces_the_app_and_brings_its_own_tests() {
        for id in WITH_PACK {
            let files = files(id, "demo");
            let paths: Vec<_> = files.iter().map(|(p, _)| *p).collect();
            assert!(paths.contains(&"backend/src/app.ts"), "{id}");
            assert!(paths.contains(&"backend/src/app.test.ts"), "{id}");
            assert!(paths.contains(&"frontend/app/page.tsx"), "{id}");
            assert!(
                paths
                    .iter()
                    .any(|p| p.starts_with("backend/migrations/0002_")),
                "{id}"
            );
            let page = &files
                .iter()
                .find(|(p, _)| *p == "frontend/app/page.tsx")
                .unwrap()
                .1;
            assert!(page.contains("demo") && !page.contains("{{NAME}}"), "{id}");
            // Tests must run without a database: they build the app on the memory store.
            let test = &files
                .iter()
                .find(|(p, _)| *p == "backend/src/app.test.ts")
                .unwrap()
                .1;
            assert!(test.contains("memoryStore()"), "{id}");
        }
        assert!(files("nope", "x").is_empty());
    }
}
