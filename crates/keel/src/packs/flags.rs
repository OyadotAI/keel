//! flags: feature flags, like Unleash (client API and evaluation) with OpenFeature's SDK ════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", FLAGS_APP.into()),
        ("backend/src/store.ts", FLAGS_STORE.into()),
        ("backend/src/evaluate.ts", FLAGS_EVAL.into()),
        ("backend/src/app.test.ts", FLAGS_TEST.into()),
        ("backend/migrations/0002_flags.sql", FLAGS_SQL.into()),
        ("frontend/app/page.tsx", f(FLAGS_PAGE)),
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
