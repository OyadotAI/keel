//! flags: feature flags, like Unleash (client API and evaluation) with OpenFeature's SDK ════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", FLAGS_APP.into()),
        ("backend/src/store.ts", FLAGS_STORE.into()),
        ("backend/src/evaluate.ts", FLAGS_EVAL.into()),
        ("backend/src/app.test.ts", FLAGS_TEST.into()),
        ("backend/migrations/0002_flags.sql", FLAGS_SQL.into()),
        ("frontend/app/page.tsx", f(FLAGS_PAGE)),
        ("CLAUDE.md", f(FLAGS_CLAUDE_MD)),
        ("AGENTS.md", f(FLAGS_AGENTS_MD)),
        ("README.md", f(FLAGS_README_MD)),
        (
            ".claude/agents/evaluation-parity.md",
            FLAGS_AGENT_EVAL.into(),
        ),
    ]
}

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

const FLAGS_CLAUDE_MD: &str = r##"# {{NAME}} — feature flags

A feature-flag service modelled on Unleash: the same client API (`GET /api/client/features`
with an ETag, `POST /api/client/metrics`, `POST /api/client/register`), the same strategies,
constraints and variants, and the same murmur3 bucketing, so an Unleash SDK in any language
points at this service and evaluates locally against the snapshot it fetched. An admin API with
an audit trail writes the flags; an SSE stream tells clients a new snapshot exists; a frontend
endpoint evaluates for one context. "Done" here means: an SDK that worked against Unleash
produces the same answers against this service, every write is in `flag_audit`, and the gate
is green with no database.

## Architecture

| File | Owns |
|---|---|
| `backend/src/evaluate.ts` | Pure evaluation: `murmur3`, `normalized`, constraints, strategies, `isEnabled`, `variant`. Imports nothing from the store. |
| `backend/src/store.ts` | `Store` interface; `pgStore` (flags + audit in one transaction) and `memoryStore` (tests). |
| `backend/src/app.ts` | Routes: client API, SSE stream, frontend evaluation, admin with `ADMIN_TOKEN`, audit. `AppType` is exported here. |
| `backend/src/app.test.ts` | Evaluation vectors from the Unleash client specification, and the API against `memoryStore`. |
| `backend/migrations/0002_flags.sql` | `flags` and `flag_audit`. |
| `backend/src/db.ts`, `server.ts`, `migrate.ts` | Pool, listener with SIGTERM drain, migration runner — unchanged from the stack. |
| `frontend/app/page.tsx` | Server-rendered table of flags from `/api/admin/features`. |

Request path for an SDK:

1. SDK calls `GET /api/client/features` with `If-None-Match`.
2. `store.all()` reads every flag; the body is serialised in Unleash's `{version: 2, features}` shape.
3. ETag is the sha1 of that body; a match returns `304`, otherwise the body with `ETag`.
4. The SDK evaluates locally, using the same algorithm as `evaluate.ts`, for every `isEnabled` call.
5. Meanwhile it may hold `GET /api/client/stream`; an admin write calls `notify()` and every open stream gets one `update` event, and the SDK refetches.
6. `POST /api/client/metrics` and `/register` are accepted (`202`) and discarded.

Request path for an admin write: `PUT /api/admin/features/:name` → bearer check → zod `flagBody`
(the URL name overrides the body) → `store.upsert` (row + audit row, one transaction, `version + 1`)
→ `notify()` → the saved row.

Data model:

| Table | Column | Why |
|---|---|---|
| `flags` | `name` (pk) | The identity SDKs use; validated `^[a-zA-Z0-9._-]+$` so it is safe in URLs and `groupId`s. |
| | `enabled` | The kill switch. Off wins over every strategy. |
| | `strategies jsonb` | Unleash's shape verbatim, so the client API can serve it untouched. |
| | `variants jsonb` | Weighted variants with optional payload. |
| | `version` | Increments on every upsert; the UI shows it, and it is how you tell "did my write land". |
| `flag_audit` | `name, actor, before, after, at` | Who changed what, from the `x-actor` header; `after` is null on delete. Append-only. |

## Invariants

1. **Evaluation is pure and offline.** `evaluate.ts` imports no store, no db, no env. An SDK
   holding a snapshot keeps answering while this service is down. Guarded by every test in
   `describe("evaluation")` running with no app and no store.
2. **Bucketing is Unleash's murmur3 x86 32-bit, seed 0, over `group:id`, `% 100 + 1`.** Change
   any of it and a user who is in the 25% here is out of it in the Go SDK. Guarded by
   `murmur3 agrees with the reference` (`normalized("123","gr1") === 73`, `("999","groupX") === 25`).
3. **A disabled flag is off regardless of strategies, and a flag with no strategies is on.**
   `isEnabled` short-circuits before touching strategies. Guarded by
   `rollout is sticky per user and honours the percentage` (last assertion) and
   `variants split by weight and stay sticky` (disabled → `{name:"disabled"}`).
4. **A missing context field fails its constraint**, and `inverted` flips that too. Guarded by
   `constraints gate a strategy and can be inverted` (`isEnabled(flag, {})` is false).
5. **Every admin write produces exactly one audit row, in the same transaction.** No path
   writes `flags` without `flag_audit`; `memoryStore` mirrors it. Guarded by
   `admin writes are audited and the client API serves them with an ETag` (two writes, two rows,
   actor recorded).
6. **No `ADMIN_TOKEN`, no writes.** `admin()` is `!!t && a === t`; an unset token refuses every
   `PUT`/`DELETE` rather than opening them. Guarded by the `401` assertion in the same test.
7. **The ETag is the content.** Any change to any flag changes the body, so a stale
   `If-None-Match` gets `200`. Guarded by the `304` then `200` sequence in the same test.
8. **Every successful write calls `notify()` after the store returns**, so streaming clients
   refetch a committed snapshot, never one in flight. No test yet — see "add a test for the
   stream" below.
9. **Variants are chosen by the same hash over the sum of weights, seeded by userId, then
   sessionId, then remoteAddress**, so the same person sees the same variant. Guarded by
   `variants split by weight and stay sticky`.
10. **`store.ts` is the only file that knows SQL.** Logic in SQL is logic the tests never run,
    because the suite uses `memoryStore`. A behaviour that exists only in `pgStore` is a bug.
11. **The frontend endpoint only returns enabled toggles**, in Unleash's frontend shape
    `{toggles:[{name, enabled:true, variant}]}`. Guarded by
    `the frontend endpoint evaluates for a context`.

## Extending it

**Add a strategy** (e.g. `applicationHostname`): add a `case` in `strategyPasses` in
`evaluate.ts`; read parameters from `s.parameters`; add a test in `describe("evaluation")` with a
context that passes and one that fails. No migration — strategies are JSON. Keep the name and
parameter keys identical to Unleash's, or SDKs that evaluate locally will disagree with
`/api/frontend`.

**Add a constraint operator** (e.g. `SEMVER_GT`): a `case` in `constraintPasses`; a test with
`inverted: true` as well, since inversion is applied after. Same names as Unleash.

**Add a context field**: `field()` in `evaluate.ts` for the well-known ones; anything else already
arrives through `properties`. On the frontend endpoint every query parameter that is not
`userId`/`sessionId`/`remoteAddress` is already a property, so a new field is a new query key.

**Add a flag attribute** (e.g. `type: "experiment"`, `impressionData`): column in a new
`backend/migrations/0003_*.sql` with a default (expand only); `Row` and `row()` in `pgStore`;
`Stored` type; `flagBody` in `app.ts`; the client body in `/api/client/features`; a test that
puts it and reads it back from the client API.

**Add an admin route** (e.g. `PATCH .../toggle`): one more link in the chained `app` in `app.ts`
— never `app.patch(...)` on its own line, or `AppType` loses it. Check `admin(c)` first, go
through `store.upsert` so the audit row exists, call `notify()`, then test `401` without the
token and the audit row with it.

**Add a test for the stream**: `app.request("/api/client/stream")` returns a streaming response;
read the first chunk for `event: ready`, write a flag, read the next chunk for `event: update`.
Put it in `describe("api")`.

**Add a second store** (e.g. Redis-backed snapshot): implement `Store`; keep `upsert` atomic with
its audit; run the whole `describe("api")` against it by parametrising `createApp(store)`.

## Operating it

| Env | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes in the cluster (defaults to the compose Postgres) | The pool in `db.ts`. |
| `ADMIN_TOKEN` | yes, or nothing can be written | Shared bearer for `PUT`/`DELETE /api/admin/features/*`. |
| `PORT` | no (8000) | Listener. |
| `API_URL` | no (`http://127.0.0.1:8000`) | The frontend's server-side base URL for `/api/admin/features`. |

Scaling: the service is read-heavy and the reads are one `select` plus a serialise. Replicas are
free for `/api/client/features` and `/api/frontend`. What is per-replica today:

- **`listeners` (the SSE fan-out).** `notify()` reaches the streams open on the replica that took
  the write. Clients on other replicas still poll and still get the new ETag; they just do not
  get the push. With more than one replica, move `notify()` to Postgres `NOTIFY flags` (the pool
  is already there) or Redis pub/sub (already in compose), and have each replica listen.

Nothing else is in memory. Redis is provisioned by the stack and unused by this service.

Failure modes:

| What fails | What the SDK sees | What the admin sees |
|---|---|---|
| Postgres down | `/api/client/features` 500; the SDK keeps its last snapshot and evaluates as before | writes fail; `/api/health/ready` still says `ok` (see Ceilings) |
| This service down | same: last snapshot, no pushes | nothing |
| `ADMIN_TOKEN` unset | nothing | every write is `401` |
| Stream dropped | the SDK reconnects or falls back to polling | nothing |

Watch: the rate of `304` versus `200` on `/api/client/features` (a low `304` share means SDKs are
not sending `If-None-Match` or something is churning flags), `PUT`/`DELETE` counts by `x-actor`,
open stream count per replica, `flag_audit` growth. Logs are the stack's one-JSON-line format.

## Ceilings

- **SSE fan-out is per process.** Upgrade: `NOTIFY`/`LISTEN` on the existing pool, or Redis.
- **One shared admin token.** No per-user identity, no SDK tokens, no read scoping; `x-actor` is
  self-reported. Upgrade: put the admin API behind the stack's auth and derive `actor` from the
  session; add an `api_tokens` table checked on the client routes.
- **`GET /api/admin/features` and `/api/admin/audit` are unauthenticated.** Flag definitions are
  usually not secret, but audit actors may be. Upgrade: the same `admin(c)` check, or a read token.
- **`/api/health/ready` does not touch the database.** It reports `db: "ok"` unconditionally.
  Upgrade: `await db\`select 1\`` with a 1s timeout, `503` on failure.
- **The client body is rebuilt on every request.** Fine to thousands of flags. Upgrade: cache
  `body` and `etag` in memory and invalidate in `notify()`.
- **Metrics are discarded.** `POST /api/client/metrics` is `202` and forgotten, so there is no
  "last seen" or per-flag usage. Upgrade: a `flag_metrics` table keyed by `(name, hour)`.
- **`flag_audit` grows without bound.** Upgrade: partition by month and drop old partitions.
- **`stickiness: "random"` and a missing sticky id use `Math.random()`**, which is what Unleash
  specifies — the flag is on for a fresh fraction of requests each time. Not a bug; know it.
- **The token comparison is `===`**, not constant-time. Upgrade: `timingSafeEqual` over equal-length buffers.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const FLAGS_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the flag service.

## Run

    make demo                    # postgres + redis, migrate, seed, backend :8000, frontend :3000
    make check                   # the gate: typecheck both halves, bun test the backend
    make backend                 # API only, with reload
    ADMIN_TOKEN=admin make backend   # or put ADMIN_TOKEN=admin in backend/.env (bun loads it)

Without `ADMIN_TOKEN` every write is `401`. The examples below assume `admin`.

## Routes, with bodies

Create or replace a flag (the URL name wins over any `name` in the body):

    curl -s -X PUT localhost:8000/api/admin/features/new-checkout \
      -H 'authorization: Bearer admin' -H 'x-actor: mk' -H 'content-type: application/json' \
      -d '{"enabled":true,"description":"new checkout flow","strategies":[{"name":"flexibleRollout","parameters":{"rollout":"25","stickiness":"userId","groupId":"new-checkout"}}]}'
    # {"name":"new-checkout","enabled":true,"description":"new checkout flow","strategies":[...],"variants":[],"version":1}

A flag for a list of users, with a constraint:

    curl -s -X PUT localhost:8000/api/admin/features/vip \
      -H 'authorization: Bearer admin' -H 'content-type: application/json' \
      -d '{"enabled":true,"strategies":[{"name":"userWithId","parameters":{"userIds":"42, 43"},"constraints":[{"contextName":"country","operator":"IN","values":["DE","FR"]}]}]}'

Variants:

    curl -s -X PUT localhost:8000/api/admin/features/cta \
      -H 'authorization: Bearer admin' -H 'content-type: application/json' \
      -d '{"enabled":true,"variants":[{"name":"a","weight":500},{"name":"b","weight":500,"payload":{"type":"string","value":"Buy now"}}]}'

What SDKs fetch, and the conditional request:

    curl -si localhost:8000/api/client/features | grep -i etag        # ETag: "3f9c..."
    curl -si localhost:8000/api/client/features -H 'if-none-match: "3f9c..."' | head -1   # HTTP/1.1 304

Evaluated for one context (every extra query key is a `properties` field):

    curl -s 'localhost:8000/api/frontend?userId=42&country=DE'
    # {"toggles":[{"name":"vip","enabled":true,"variant":{"name":"disabled","enabled":false}}, ...]}

Push notifications (hold it open in one terminal, write in another):

    curl -N localhost:8000/api/client/stream
    # event: ready / data: ok … then event: update / data: 1724900000000 after a PUT

Delete, list, audit:

    curl -s -X DELETE localhost:8000/api/admin/features/cta -H 'authorization: Bearer admin'   # 204, or 404
    curl -s localhost:8000/api/admin/features                                                  # {"features":[...]}
    curl -s localhost:8000/api/admin/audit                                                     # {"audit":[{"id":3,"name":"cta","actor":null,"before":{...},"after":null,"at":"..."}, ...]}

Metrics and register are accepted and dropped:

    curl -si -X POST localhost:8000/api/client/metrics -d '{}' | head -1    # HTTP/1.1 202

## Tests

`backend/src/app.test.ts`, run by `bun test` (part of `make check`). No database:

- `describe("evaluation")` calls `evaluate.ts` directly with literal flags and contexts. The
  murmur3 vectors are from the Unleash client specification; the 1000-user rollout asserts a
  50% flag lands between 42% and 58%.
- `describe("api")` builds `createApp(memoryStore())` and drives it with Hono's `app.request`,
  which never opens a port. `process.env.ADMIN_TOKEN = "admin"` at the top of the file is the
  only setup.

To add a test: put evaluation tests next to the strategy or operator they cover, with one passing
and one failing context; put route tests in `describe("api")` and go through `put()` so the audit
assertions stay true. If a test needs Postgres, the thing under test is in the wrong file.

## Migrations

New columns go in `backend/migrations/0003_*.sql`, additive with a default; `make migrate`
locally, the `migrate` init container in the cluster. `pgStore.row()` and `Stored` must both
change, and the client body in `app.ts` if SDKs should see it.
"##;

const FLAGS_README_MD: &str = r##"# {{NAME}}

Unleash-compatible feature flags you own: one Hono service, one Postgres, and every Unleash SDK
already works against it.

## What you get

- `GET /api/client/features` in Unleash's `version: 2` shape, with `ETag`/`If-None-Match`, so
  the official SDKs (Node, Go, Java, Python, .NET, the browser proxy client) poll it unchanged.
- `GET /api/client/stream` — an SSE push when any flag changes; clients refetch instead of waiting for the next poll.
- `GET /api/frontend?userId=…&anything=…` — evaluated toggles and variants for one context, for
  code that does not want an SDK.
- Strategies `default`, `userWithId`, `remoteAddress`, `flexibleRollout` (with `stickiness` and
  `groupId`), `gradualRolloutUserId`; constraints with `IN`, `NOT_IN`, `STR_*`, `NUM_*`,
  `DATE_*`, `REGEX`, `inverted`, `caseInsensitive`; weighted variants with payloads.
- Bucketing is murmur3 over `group:id`, exactly as the Unleash client specification defines it,
  so a user in your 10% here is the same user in the Go SDK's 10%.
- An admin API behind one bearer token, and `flag_audit`: every write, who, before, after.
- Tests that run with no database and no network (`make check`).
- Docker Compose for local, kustomize `base` + `dev`/`prod` overlays for a cluster.

## Five minutes

    echo ADMIN_TOKEN=admin >> backend/.env
    make demo

Then, in another terminal:

    curl -s -X PUT localhost:8000/api/admin/features/new-checkout \
      -H 'authorization: Bearer admin' -H 'x-actor: you' -H 'content-type: application/json' \
      -d '{"enabled":true,"strategies":[{"name":"flexibleRollout","parameters":{"rollout":"50","stickiness":"userId"}}]}'
    # → {"name":"new-checkout","enabled":true,...,"version":1}

    curl -s 'localhost:8000/api/frontend?userId=42'
    # → {"toggles":[{"name":"new-checkout","enabled":true,"variant":{"name":"disabled","enabled":false}}]}   (or [] — 42 may be in the other half)

    curl -si localhost:8000/api/client/features | grep -iE '^(HTTP|etag)'
    # → HTTP/1.1 200, ETag: "…"

    curl -s localhost:8000/api/admin/audit | head -c 200
    # → {"audit":[{"id":1,"name":"new-checkout","actor":"you","before":null,"after":{...}

`http://localhost:3000` lists the flags. Point an Unleash SDK at `http://localhost:8000/api`
with any `Authorization` value (client tokens are not checked yet — see Roadmap).

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/client/features` | none | Full snapshot, Unleash `version: 2`; `ETag`, `304` on match |
| GET | `/api/client/stream` | none | SSE: `ready`, then `update` on every write |
| POST | `/api/client/metrics` | none | Accepted (`202`), discarded |
| POST | `/api/client/register` | none | Accepted (`202`), discarded |
| GET | `/api/frontend?…` | none | Enabled toggles + variant for the context in the query |
| GET | `/api/admin/features` | none | All flags with `description` and `version` |
| PUT | `/api/admin/features/:name` | `Bearer $ADMIN_TOKEN` | Create or replace; `x-actor` goes to the audit |
| DELETE | `/api/admin/features/:name` | `Bearer $ADMIN_TOKEN` | `204`, or `404` |
| GET | `/api/admin/audit` | none | Last 100 audit rows, newest first |
| GET | `/api/health`, `/api/health/ready` | none | Probes |

## Compared with Unleash

Same, so their docs and SDKs apply:

- The client API payload and ETag semantics; SDK polling and `If-None-Match` work as documented.
- Strategy names and parameter keys (`rollout`, `stickiness`, `groupId`, `userIds`, `IPs`, `percentage`).
- Constraint operators and the `inverted`/`caseInsensitive` flags.
- Variant weighting and stickiness order (userId, sessionId, remoteAddress).
- The murmur3 normalisation, verified against the specification's numbers in the tests.

Better here:

- Typed end to end: `AppType` from `backend/src/app.ts` is the contract the frontend compiles against.
- The whole evaluator is 120 lines of TypeScript you can read, and the tests run without a
  database or a network in about a second.
- Every write is audited in the same transaction as the write; there is no path around it.
- The push stream is built in — Unleash needs Edge or the proxy for streaming.
- One codebase you own: a new strategy is a `case` and a test, not a plugin or a paid tier.
- Kubernetes manifests, probes, secrets rendering and CI that roll only what changed.

Not here yet:

- **Environments and projects.** One flat namespace; `prod` and `dev` are separate deployments
  of this service, not environments inside one.
- **Client and frontend API tokens.** The client routes are open; an `Authorization` header is
  accepted and ignored.
- **Segments** (reusable constraint sets), **strategy variants**, **dependent flags**, **release
  plans**.
- **Metrics and "last seen"** — the metrics endpoint discards its body.
- **The admin UI**: the page is a read-only table; writes are `curl`.
- **Change requests, RBAC, SSO, user management**, playground, impression data, Unleash Edge.
- **Multi-replica push**: `notify()` reaches the streams on the replica that took the write.

## Production

- Envs: `DATABASE_URL`, `ADMIN_TOKEN` (required for any write), `PORT`. Rendered from
  `backend/.env` into a Secret by `make k8s-secrets ENV=prod`; committed only as `backend/.env.age`.
- Scaling: stateless reads; add replicas freely. The SSE fan-out is per replica until it moves to
  `NOTIFY`/Redis.
- Probes: `/api/health` (liveness), `/api/health/ready` (readiness — does not yet check Postgres).
- Migrations: `backend/migrations/*.sql`, run by the `migrate` init container before each rollout.
- Deploy: `git push main` → dev; `make release` → prod. `k8s/README.md` explains the manifests.
- What pages you: `PUT` error rate (the admin token or Postgres), `/api/client/features` 5xx
  (SDKs keep their snapshot, but nothing new ships), replica restarts (streams drop and reconnect).

## Roadmap

1. `NOTIFY`-based fan-out so push works with replicas.
2. Client tokens: an `api_tokens` table and a check on `/api/client/*` and `/api/frontend`.
3. `/api/health/ready` that actually queries Postgres.
4. Persist metrics: `flag_metrics(name, hour, yes, no)` from `/api/client/metrics`.
5. Cache the client body and ETag in memory, invalidated on write.
6. Partition `flag_audit` by month.
7. Environments, once there is a second environment that cannot be a second deployment.
"##;

const FLAGS_AGENT_EVAL: &str = r##"---
name: evaluation-parity
description: Run on any change to backend/src/evaluate.ts, the client API body, or the admin schema. Checks that this service still evaluates exactly as an Unleash SDK would, so a flag means the same thing in every language.
tools: Read, Grep, Glob, Bash
---

You are checking that a flag evaluates to the same answer here and in an Unleash SDK holding
the same snapshot. Report each divergence as `path:line — what differs — a context that shows
it — the fix`.

Check:
1. `murmur3` in `evaluate.ts` is still x86 32-bit, seed 0, and `normalized` is
   `hash("group:id") % n + 1`. The vectors in `murmur3 agrees with the reference` still pass;
   if they were edited, that is the finding.
2. `flexibleRollout` reads `rollout`, `stickiness`, `groupId` with Unleash's defaults:
   group falls back to the flag name, stickiness `default` means userId then sessionId, and
   `random` is genuinely random.
3. Strategy and operator names match Unleash's spelling exactly (`userWithId`, `remoteAddress`,
   `STR_STARTS_WITH`, `NUM_GTE`, `DATE_AFTER`, `REGEX`). A new name that Unleash does not have
   will pass here and fail in every SDK.
4. A missing context field fails the constraint before `inverted` is applied; `caseInsensitive`
   lowercases both sides; `NUM_*` and `DATE_*` use `value`, list operators use `values`.
5. `isEnabled`: disabled flag → false before anything else; no strategies → true; otherwise
   `some(strategy)`, and a strategy's constraints are `every`.
6. `variant`: only when enabled; weights sum to the modulus; seed order is userId, sessionId,
   remoteAddress; a zero-weight variant can never be chosen.
7. The client body in `app.ts` is `{version: 2, features: [{name, type, enabled, description,
   strategies, variants}]}` with `variants` never null. An SDK's JSON parser is strict.
8. The ETag is derived from the serialised body and nothing else; a change that touches the body
   without changing the ETag means SDKs never see it.
9. `/api/frontend` uses the same `isEnabled` and `variant` as the SDK path — there is no second
   evaluator.
10. `flagBody` in `app.ts` accepts every field the client body serves; a field that can be stored
    but not written, or written but not served, is a finding.
11. Admin writes still go through `store.upsert`/`store.remove`, so the audit row exists, and
    still call `notify()` after the store returns.

End with one line: `evaluation-parity: N findings`, and if 0, which of the above you ran.
"##;
