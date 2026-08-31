//! controlplane: a control plane, like Kubernetes controllers and Crossplane ═══════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", CP_APP.into()),
        ("backend/src/store.ts", CP_STORE.into()),
        ("backend/src/providers.ts", CP_PROVIDERS.into()),
        ("backend/src/reconciler.ts", CP_RECONCILER.into()),
        ("backend/src/worker.ts", CP_WORKER.into()),
        ("backend/src/app.test.ts", CP_TEST.into()),
        ("backend/migrations/0002_controlplane.sql", CP_SQL.into()),
        ("frontend/app/page.tsx", f(CP_PAGE)),
        ("CLAUDE.md", f(CP_CLAUDE)),
        ("AGENTS.md", f(CP_AGENTS)),
        ("README.md", f(CP_README)),
        (".claude/agents/reconcile-semantics.md", CP_REVIEWER.into()),
    ]
}

const CP_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { recordsProvider, type Provider } from "./providers";
import { reconcileOnce } from "./reconciler";

// The Kubernetes API server's surface for one resource type per provider: PUT a spec, GET it
// back with generation, resourceVersion and status, DELETE to mark it for finalization. Every
// write enqueues a reconcile; the worker (or POST /api/reconcile, for a demo with no worker)
// drives the world toward the spec. Status is read-only here — only the reconciler writes it.

export function createApp(store: Store, providerList: Provider[] = [recordsProvider()]) {
  const providers = new Map(providerList.map((p) => [p.kind, p]));
  const notFound = (c: { json: (b: unknown, s: 404) => Response }) => c.json({ error: { message: "no such resource", code: "not_found" } }, 404);

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/kinds", (c) => c.json({ kinds: [...providers.keys()] }))
    .get("/api/resources", async (c) => c.json({ resources: await store.list() }))
    .get("/api/resources/:kind/:name", async (c) => {
      const r = await store.get(c.req.param("kind"), c.req.param("name"));
      return r ? c.json(r) : notFound(c);
    })
    .get("/api/resources/:kind/:name/events", async (c) => c.json({ events: await store.events(c.req.param("kind"), c.req.param("name"), 50) }))

    // kubectl apply: create or update the spec. `resourceVersion`, when sent, must match.
    .put("/api/resources/:kind/:name", async (c) => {
      const kind = c.req.param("kind"), name = c.req.param("name");
      const provider = providers.get(kind);
      if (!provider) return c.json({ error: { message: `unknown kind ${kind}`, code: "invalid" } }, 400);
      const p = z.object({ spec: z.record(z.unknown()), resourceVersion: z.number().int().optional() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "spec required", code: "invalid" } }, 400);
      if (!/^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/.test(name) || name.length > 63) return c.json({ error: { message: "name must be a DNS label", code: "invalid" } }, 400);
      const err = provider.schema(p.data.spec);
      if (err) return c.json({ error: { message: err, code: "invalid" } }, 400);
      const before = await store.get(kind, name);
      if (before?.deletion_timestamp) return c.json({ error: { message: "resource is being deleted", code: "conflict" } }, 409);
      const r = await store.upsert(kind, name, p.data.spec, p.data.resourceVersion);
      if (!r) return c.json({ error: { message: `resourceVersion is stale`, code: "conflict" } }, 409);
      if (!before || r.generation !== before.generation) await store.enqueue(kind, name);
      return c.json(r, before ? 200 : 201);
    })
    .delete("/api/resources/:kind/:name", async (c) => {
      const r = await store.markDeleted(c.req.param("kind"), c.req.param("name"));
      if (!r) return notFound(c);
      await store.enqueue(r.kind, r.name);
      return c.json(r, 202);
    })
    // One reconcile pass, for demos and tests. In production `bun run worker` does this forever.
    .post("/api/reconcile", async (c) => c.json({ reconciled: await reconcileOnce(store, providers) }));
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const CP_STORE: &str = r##"import { db } from "./db";

export type Condition = { type: string; status: "True" | "False" | "Unknown"; reason: string; message: string; lastTransitionTime: string };
export type Status = { observedGeneration: number; conditions: Condition[] };
export type Resource = { kind: string; name: string; generation: number; resource_version: number; spec: Record<string, unknown>; status: Status; deletion_timestamp: string | null };
export type Event = { id: number; kind: string; name: string; type: "Normal" | "Warning"; reason: string; message: string; created_at: string };
export type QueueItem = { kind: string; name: string; attempts: number };

export interface Store {
  get(kind: string, name: string): Promise<Resource | null>;
  list(): Promise<Resource[]>;
  /// Insert or update the spec. Bumps generation only when the spec changed; refuses with
  /// null when `expectVersion` is given and does not match the row's.
  upsert(kind: string, name: string, spec: Record<string, unknown>, expectVersion?: number): Promise<Resource | null>;
  markDeleted(kind: string, name: string): Promise<Resource | null>;
  remove(kind: string, name: string): Promise<void>;
  setStatus(kind: string, name: string, status: Status): Promise<void>;
  enqueue(kind: string, name: string, runAt?: Date, attempts?: number): Promise<void>;
  /// Claim every item that is due. In Postgres this is SKIP LOCKED so several workers share it.
  claim(now: Date, limit: number): Promise<QueueItem[]>;
  dequeue(kind: string, name: string): Promise<void>;
  event(kind: string, name: string, type: Event["type"], reason: string, message: string): Promise<void>;
  events(kind: string, name: string, limit: number): Promise<Event[]>;
}

const initialStatus: Status = { observedGeneration: 0, conditions: [] };
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

export function pgStore(): Store {
  const cols = db`kind, name, generation::int as generation, resource_version::int as resource_version, spec, status, deletion_timestamp`;
  return {
    get: async (k, n) => (await db<Resource[]>`select ${cols} from resources where kind = ${k} and name = ${n}`)[0] ?? null,
    list: async () => db<Resource[]>`select ${cols} from resources order by kind, name`,
    upsert: async (k, n, spec, expect) =>
      db.begin(async (tx) => {
        const [cur] = await tx<Resource[]>`select ${cols} from resources where kind = ${k} and name = ${n} for update`;
        if (expect !== undefined && cur && cur.resource_version !== expect) return null;
        if (!cur) {
          if (expect !== undefined) return null;
          return (await tx<Resource[]>`insert into resources (kind, name, spec) values (${k}, ${n}, ${tx.json(spec as never)}) returning ${cols}`)[0];
        }
        if (same(cur.spec, spec)) return cur;
        return (await tx<Resource[]>`update resources set spec = ${tx.json(spec as never)}, generation = generation + 1, resource_version = resource_version + 1, updated_at = now() where kind = ${k} and name = ${n} returning ${cols}`)[0];
      }),
    markDeleted: async (k, n) => (await db<Resource[]>`update resources set deletion_timestamp = coalesce(deletion_timestamp, now()), resource_version = resource_version + 1 where kind = ${k} and name = ${n} returning ${cols}`)[0] ?? null,
    remove: async (k, n) => { await db`delete from resources where kind = ${k} and name = ${n}`; },
    setStatus: async (k, n, status) => { await db`update resources set status = ${db.json(status as never)}, resource_version = resource_version + 1, updated_at = now() where kind = ${k} and name = ${n}`; },
    enqueue: async (k, n, runAt = new Date(), attempts = 0) => {
      await db`insert into reconcile_queue (kind, name, run_at, attempts) values (${k}, ${n}, ${runAt}, ${attempts})
        on conflict (kind, name) do update set run_at = least(reconcile_queue.run_at, excluded.run_at), attempts = excluded.attempts`;
    },
    claim: async (now, limit) =>
      db<QueueItem[]>`delete from reconcile_queue where (kind, name) in (select kind, name from reconcile_queue where run_at <= ${now} order by run_at limit ${limit} for update skip locked) returning kind, name, attempts`,
    dequeue: async () => {},
    event: async (k, n, type, reason, message) => { await db`insert into events (kind, name, type, reason, message) values (${k}, ${n}, ${type}, ${reason}, ${message})`; },
    events: async (k, n, limit) => db<Event[]>`select * from events where kind = ${k} and name = ${n} order by id desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const rows = new Map<string, Resource>(), queue = new Map<string, { run_at: number; attempts: number }>(), events: Event[] = [];
  const key = (k: string, n: string) => `${k}/${n}`;
  return {
    get: async (k, n) => structuredClone(rows.get(key(k, n)) ?? null),
    list: async () => [...rows.values()].map((r) => structuredClone(r)),
    upsert: async (k, n, spec, expect) => {
      const cur = rows.get(key(k, n));
      if (expect !== undefined && (!cur || cur.resource_version !== expect)) return null;
      if (!cur) { const r: Resource = { kind: k, name: n, generation: 1, resource_version: 1, spec, status: structuredClone(initialStatus), deletion_timestamp: null }; rows.set(key(k, n), r); return structuredClone(r); }
      if (!same(cur.spec, spec)) { cur.spec = spec; cur.generation++; cur.resource_version++; }
      return structuredClone(cur);
    },
    markDeleted: async (k, n) => { const r = rows.get(key(k, n)); if (!r) return null; r.deletion_timestamp ??= new Date().toISOString(); r.resource_version++; return structuredClone(r); },
    remove: async (k, n) => { rows.delete(key(k, n)); },
    setStatus: async (k, n, status) => { const r = rows.get(key(k, n)); if (r) { r.status = structuredClone(status); r.resource_version++; } },
    enqueue: async (k, n, runAt = new Date(), attempts = 0) => { const cur = queue.get(key(k, n)); const run_at = runAt.getTime(); queue.set(key(k, n), { run_at: cur ? Math.min(cur.run_at, run_at) : run_at, attempts }); },
    claim: async (now, limit) => {
      const due = [...queue.entries()].filter(([, q]) => q.run_at <= now.getTime()).sort((a, b) => a[1].run_at - b[1].run_at).slice(0, limit);
      for (const [k] of due) queue.delete(k);
      return due.map(([k, q]) => ({ kind: k.split("/")[0], name: k.slice(k.indexOf("/") + 1), attempts: q.attempts }));
    },
    dequeue: async (k, n) => { queue.delete(key(k, n)); },
    event: async (k, n, type, reason, message) => { events.push({ id: events.length + 1, kind: k, name: n, type, reason, message, created_at: new Date().toISOString() }); },
    events: async (k, n, limit) => events.filter((e) => e.kind === k && e.name === n).reverse().slice(0, limit),
  };
}
"##;

const CP_PROVIDERS: &str = r##"import { db } from "./db";

// Crossplane's provider contract, reduced to what a reconciler needs: observe what exists,
// create or update it to match the spec, delete it. A provider knows one external system and
// nothing about generations, conditions or the queue — those are the reconciler's.
export type Provider = {
  kind: string;
  schema: (spec: Record<string, unknown>) => string | null;   // validation error, or null
  observe(name: string): Promise<Record<string, unknown> | null>;
  apply(name: string, spec: Record<string, unknown>, observed: Record<string, unknown> | null): Promise<void>;
  delete(name: string): Promise<void>;
};

/// The real one: a `Record` resource is a row in the records table. Swap the table for a DNS
/// API, a bucket, a VM — the reconciler does not change.
export function recordsProvider(): Provider {
  return {
    kind: "Record",
    schema: (spec) => (typeof spec.value === "string" ? null : "spec.value must be a string"),
    observe: async (name) => (await db<{ value: string }[]>`select value from records where name = ${name}`)[0] ?? null,
    apply: async (name, spec, observed) => {
      if (observed?.value === spec.value) return;   // already right: a no-op reconcile
      await db`insert into records (name, value) values (${name}, ${spec.value as string}) on conflict (name) do update set value = excluded.value, updated_at = now()`;
    },
    delete: async (name) => { await db`delete from records where name = ${name}`; },
  };
}

/// The fake for tests: same contract over a Map, with a switch that makes the next N calls fail
/// so the retry path can be exercised without an unreliable system.
export function fakeProvider(kind = "Record") {
  const rows = new Map<string, Record<string, unknown>>();
  const calls: string[] = [];
  let failures = 0;
  const maybeFail = (op: string) => { calls.push(op); if (failures > 0) { failures--; throw new Error(`${op}: provider unavailable`); } };
  const provider: Provider = {
    kind,
    schema: (spec) => (typeof spec.value === "string" ? null : "spec.value must be a string"),
    observe: async (name) => { maybeFail(`observe ${name}`); return rows.get(name) ?? null; },
    apply: async (name, spec) => { maybeFail(`apply ${name}`); rows.set(name, { value: spec.value }); },
    delete: async (name) => { maybeFail(`delete ${name}`); rows.delete(name); },
  };
  return { provider, rows, calls, failNext: (n: number) => { failures = n; } };
}
"##;

const CP_RECONCILER: &str = r##"import type { Condition, Resource, Status, Store } from "./store";
import type { Provider } from "./providers";

// controller-runtime's loop: claim due items, read the resource, observe the world through its
// provider, apply the diff, write status. Success dequeues; an error writes a Warning event
// and requeues with exponential backoff (5s doubling to 5m, controller-runtime's default
// shape). A deleting resource is finalized — the external thing goes first, then the row.

export const backoffMs = (attempts: number) => Math.min(5_000 * 2 ** attempts, 300_000);

const setCondition = (status: Status, c: Omit<Condition, "lastTransitionTime">, now: Date): Status => {
  const prev = status.conditions.find((x) => x.type === c.type);
  const lastTransitionTime = prev && prev.status === c.status ? prev.lastTransitionTime : now.toISOString();
  return { ...status, conditions: [...status.conditions.filter((x) => x.type !== c.type), { ...c, lastTransitionTime }] };
};

export async function reconcileOne(store: Store, providers: Map<string, Provider>, res: Resource, now = new Date()): Promise<void> {
  const provider = providers.get(res.kind);
  if (!provider) throw new Error(`no provider for kind ${res.kind}`);
  if (res.deletion_timestamp) {
    await provider.delete(res.name);
    await store.event(res.kind, res.name, "Normal", "Deleted", "external resource removed");
    await store.remove(res.kind, res.name);
    return;
  }
  const observed = await provider.observe(res.name);
  const changed = !observed || Object.entries(res.spec).some(([k, v]) => JSON.stringify(observed[k]) !== JSON.stringify(v));
  if (changed) {
    await provider.apply(res.name, res.spec, observed);
    await store.event(res.kind, res.name, "Normal", observed ? "Updated" : "Created", `generation ${res.generation} applied`);
  }
  await store.setStatus(res.kind, res.name, setCondition({ ...res.status, observedGeneration: res.generation }, { type: "Ready", status: "True", reason: changed ? "Applied" : "InSync", message: `generation ${res.generation} ${changed ? "applied" : "already in sync"}` }, now));
}

/// One pass over everything that is due. Returns how many items were handled.
export async function reconcileOnce(store: Store, providers: Map<string, Provider>, now = new Date(), limit = 20): Promise<number> {
  const items = await store.claim(now, limit);
  for (const item of items) {
    const res = await store.get(item.kind, item.name);
    if (!res) continue;   // deleted under us: nothing to reconcile
    try {
      await reconcileOne(store, providers, res, now);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      const delay = backoffMs(item.attempts);
      await store.event(res.kind, res.name, "Warning", "ReconcileError", `${message} (retry in ${delay / 1000}s)`);
      await store.setStatus(res.kind, res.name, setCondition(res.status, { type: "Ready", status: "False", reason: "ReconcileError", message }, now));
      await store.enqueue(res.kind, res.name, new Date(now.getTime() + delay), item.attempts + 1);
    }
  }
  return items.length;
}
"##;

const CP_WORKER: &str = r##"import { pgStore } from "./store";
import { recordsProvider } from "./providers";
import { reconcileOnce } from "./reconciler";

// `bun run worker`: the controller process. Several can run; SKIP LOCKED shares the queue.
const store = pgStore();
const providers = new Map([[recordsProvider().kind, recordsProvider()]]);
let stop = false;
process.on("SIGTERM", () => { stop = true; });
while (!stop) {
  const n = await reconcileOnce(store, providers).catch((e) => { console.error(JSON.stringify({ level: "error", msg: "reconcile pass failed", error: String(e) })); return 0; });
  if (n === 0) await new Promise((r) => setTimeout(r, 1000));
}
"##;

const CP_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { fakeProvider } from "./providers";
import { backoffMs, reconcileOnce } from "./reconciler";

const put = (spec: unknown, resourceVersion?: number) => ({ method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ spec, resourceVersion }) });
const setup = () => { const store = memoryStore(); const fake = fakeProvider(); return { store, fake, app: createApp(store, [fake.provider]) }; };

describe("control plane", () => {
  test("generation counts spec changes; a stale resourceVersion is a conflict", async () => {
    const { app } = setup();
    const created = await app.request("/api/resources/Record/a", put({ value: "1" }));
    expect(created.status).toBe(201);
    expect(await created.json()).toMatchObject({ generation: 1, resource_version: 1, status: { observedGeneration: 0 } });
    // Same spec: no new generation. Different spec: generation 2.
    expect(await (await app.request("/api/resources/Record/a", put({ value: "1" }))).json()).toMatchObject({ generation: 1 });
    const v2 = await (await app.request("/api/resources/Record/a", put({ value: "2" }))).json();
    expect(v2).toMatchObject({ generation: 2, resource_version: 2 });
    // Someone else's write in between: the echoed version no longer matches.
    expect((await app.request("/api/resources/Record/a", put({ value: "3" }, 1))).status).toBe(409);
    expect((await app.request("/api/resources/Record/a", put({ value: "3" }, 2))).status).toBe(200);
    expect((await app.request("/api/resources/Widget/a", put({ value: "3" }))).status).toBe(400);
    expect((await app.request("/api/resources/Record/Bad_Name", put({ value: "3" }))).status).toBe(400);
    expect((await app.request("/api/resources/Record/a", put({ value: 3 }))).status).toBe(400);
  });

  test("reconcile applies the spec through the provider and reports Ready with observedGeneration", async () => {
    const { app, fake } = setup();
    await app.request("/api/resources/Record/a", put({ value: "hello" }));
    expect((await (await app.request("/api/reconcile", { method: "POST" })).json()).reconciled).toBe(1);
    expect(fake.rows.get("a")).toEqual({ value: "hello" });
    const r = await (await app.request("/api/resources/Record/a")).json();
    expect(r.status.observedGeneration).toBe(1);
    expect(r.status.conditions[0]).toMatchObject({ type: "Ready", status: "True", reason: "Applied" });
    const events = (await (await app.request("/api/resources/Record/a/events")).json()).events;
    expect(events.map((e: { reason: string }) => e.reason)).toEqual(["Created"]);
    // Nothing changed: a second pass finds nothing queued, and a forced pass is a no-op apply.
    expect((await (await app.request("/api/reconcile", { method: "POST" })).json()).reconciled).toBe(0);
    await app.request("/api/resources/Record/a", put({ value: "hello" }));
    expect(fake.calls.filter((c) => c.startsWith("apply"))).toHaveLength(1);
  });

  test("a provider failure sets Ready=False, records a Warning and requeues with backoff", async () => {
    const { app, store, fake } = setup();
    await app.request("/api/resources/Record/a", put({ value: "x" }));
    fake.failNext(2);
    const t0 = new Date();
    const providers = new Map([["Record", fake.provider]]);
    expect(await reconcileOnce(store, providers, t0)).toBe(1);
    let r = await store.get("Record", "a");
    expect(r!.status.conditions[0]).toMatchObject({ type: "Ready", status: "False", reason: "ReconcileError" });
    expect((await store.events("Record", "a", 10))[0]).toMatchObject({ type: "Warning", reason: "ReconcileError" });
    // Not due yet: backoff holds it. Due after 5s: fails again, backoff doubles to 10s.
    expect(await reconcileOnce(store, providers, t0)).toBe(0);
    expect(await reconcileOnce(store, providers, new Date(t0.getTime() + backoffMs(0)))).toBe(1);
    expect(await reconcileOnce(store, providers, new Date(t0.getTime() + backoffMs(0) + 6_000))).toBe(0);
    expect(await reconcileOnce(store, providers, new Date(t0.getTime() + backoffMs(0) + backoffMs(1)))).toBe(1);
    r = await store.get("Record", "a");
    expect(r!.status.conditions[0]).toMatchObject({ status: "True" });
    expect(fake.rows.get("a")).toEqual({ value: "x" });
    expect(backoffMs(10)).toBe(300_000);
  });

  test("delete finalizes: the external thing goes first, then the row", async () => {
    const { app, fake } = setup();
    await app.request("/api/resources/Record/a", put({ value: "x" }));
    await app.request("/api/reconcile", { method: "POST" });
    const del = await app.request("/api/resources/Record/a", { method: "DELETE" });
    expect(del.status).toBe(202);
    expect((await del.json()).deletion_timestamp).not.toBeNull();
    // A write to a deleting resource is refused; the row still exists until the reconciler runs.
    expect((await app.request("/api/resources/Record/a", put({ value: "y" }))).status).toBe(409);
    expect((await app.request("/api/resources/Record/a")).status).toBe(200);
    await app.request("/api/reconcile", { method: "POST" });
    expect(fake.rows.has("a")).toBe(false);
    expect((await app.request("/api/resources/Record/a")).status).toBe(404);
    expect(fake.calls.at(-1)).toBe("delete a");
    expect((await app.request("/api/resources/Record/zzz", { method: "DELETE" })).status).toBe(404);
  });
});
"##;

const CP_SQL: &str = r##"-- Kubernetes' object model in three tables. `generation` counts spec changes; `resource_version`
-- counts every write and is what a client must echo back to update (optimistic concurrency —
-- a stale version is a 409, never a silent overwrite). Status is written only by the
-- reconciler, and `observed_generation` in it says which spec the status describes.
create table if not exists resources (
  kind text not null,
  name text not null,
  generation bigint not null default 1,
  resource_version bigint not null default 1,
  spec jsonb not null,
  status jsonb not null default '{"observedGeneration":0,"conditions":[]}',
  deletion_timestamp timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  primary key (kind, name)
);
-- The work queue: one row per resource that needs a reconcile, with the backoff clock.
create table if not exists reconcile_queue (
  kind text not null,
  name text not null,
  run_at timestamptz not null default now(),
  attempts int not null default 0,
  primary key (kind, name)
);
create table if not exists events (
  id bigserial primary key,
  kind text not null,
  name text not null,
  type text not null,            -- Normal | Warning
  reason text not null,
  message text not null,
  created_at timestamptz not null default now()
);
create index if not exists events_resource on events (kind, name, id desc);
-- What the built-in Record provider manages: the "external system" is this table.
create table if not exists records (
  name text primary key,
  value text not null,
  updated_at timestamptz not null default now()
);
"##;

const CP_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Condition = { type: string; status: string; reason: string; message: string };
type Resource = { kind: string; name: string; generation: number; resource_version: number; spec: Record<string, unknown>; status: { observedGeneration: number; conditions: Condition[] }; deletion_timestamp: string | null };
type Event = { id: number; type: string; reason: string; message: string; created_at: string };

// kubectl get + describe, on one page: every resource with its generation against the
// generation the reconciler last observed, its Ready condition, and the events of the one
// you select. Polls, because the reconciler is a separate process.
export default function Home() {
  const [resources, setResources] = useState<Resource[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [events, setEvents] = useState<Event[]>([]);
  const [name, setName] = useState("greeting");
  const [value, setValue] = useState("hello");

  const refresh = async () => {
    setResources((await (await fetch("/api/resources")).json()).resources);
    if (selected) setEvents((await (await fetch(`/api/resources/${selected}/events`)).json()).events);
  };
  useEffect(() => { void refresh(); const t = setInterval(refresh, 2000); return () => clearInterval(t); }, [selected]);

  const apply = async () => { await fetch(`/api/resources/Record/${name}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ spec: { value } }) }); void refresh(); };
  const remove = async (r: Resource) => { await fetch(`/api/resources/${r.kind}/${r.name}`, { method: "DELETE" }); void refresh(); };
  const reconcile = async () => { await fetch("/api/reconcile", { method: "POST" }); void refresh(); };
  const ready = (r: Resource) => r.status.conditions.find((c) => c.type === "Ready");

  return (
    <main>
      <h1>{{NAME}} — control plane</h1>
      <p>Apply a spec; the reconciler (<code>bun run worker</code> in backend/) makes the records table match it. Or: <code>{`curl -X PUT localhost:8000/api/resources/Record/greeting -H 'content-type: application/json' -d '{"spec":{"value":"hello"}}'`}</code></p>
      <p>
        Record <input value={name} onChange={(e) => setName(e.target.value)} /> value <input value={value} onChange={(e) => setValue(e.target.value)} />
        <button onClick={apply}>Apply</button> <button onClick={reconcile}>Reconcile once</button>
      </p>
      <table>
        <thead><tr><th>kind</th><th>name</th><th>spec</th><th>generation</th><th>observed</th><th>ready</th><th>reason</th><th></th></tr></thead>
        <tbody>
          {resources.map((r) => { const c = ready(r); const key = `${r.kind}/${r.name}`; return (
            <tr key={key} style={{ background: r.deletion_timestamp ? "#eee" : c?.status === "True" && r.status.observedGeneration === r.generation ? "#dfd" : c?.status === "False" ? "#fdd" : "#ffd" }}>
              <td>{r.kind}</td><td><a href="#" onClick={(e) => { e.preventDefault(); setSelected(key); }}>{r.name}</a></td>
              <td><code>{JSON.stringify(r.spec)}</code></td><td>{r.generation}</td><td>{r.status.observedGeneration}</td>
              <td>{r.deletion_timestamp ? "deleting" : c?.status ?? "Unknown"}</td><td title={c?.message}>{c?.reason ?? "Pending"}</td>
              <td><button onClick={() => remove(r)}>delete</button></td>
            </tr>); })}
        </tbody>
      </table>
      {resources.length === 0 && <p>No resources yet.</p>}
      {selected && (<>
        <h2>Events · {selected}</h2>
        <ul>{events.map((e) => <li key={e.id}><code>{e.type}</code> {e.reason}: {e.message} <small>{e.created_at}</small></li>)}</ul>
      </>)}
    </main>
  );
}
"##;

const CP_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a control plane, modelled on the Kubernetes API server plus a controller-runtime
reconciler, with Crossplane's provider contract for the external system. A client PUTs a desired
`spec` for a named resource of a kind; the API stores it with a `generation` (spec changes) and a
`resource_version` (every write, for optimistic concurrency) and enqueues a reconcile; a worker
claims the queue, observes the external system through the kind's provider, applies the
difference, and writes `status` with `observedGeneration` and a `Ready` condition. Errors become
`Warning` events and a requeue with exponential backoff. Deletion is two-phase: the row is marked,
the external thing is removed, then the row. The built-in provider manages rows in a `records`
table; the reconciler does not know that. "Done" means: `generation === status.observedGeneration`
and `Ready=True` for every resource, and the external system matches every spec.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The API server: `PUT`/`GET`/`DELETE` a resource, list, events, kinds, and `POST /api/reconcile` for one pass without a worker. Validates names and specs, decides when to enqueue. Never writes status |
| `backend/src/store.ts` | `Store` interface, `pgStore` (resources, queue with `SKIP LOCKED`, events) and `memoryStore` (tests). `upsert` bumps `generation` only when the spec changed |
| `backend/src/providers.ts` | `Provider` contract (`schema`, `observe`, `apply`, `delete`), `recordsProvider` (the real one over the `records` table), `fakeProvider` (tests, with `failNext(n)`) |
| `backend/src/reconciler.ts` | `reconcileOne` (finalize or observe/apply/status) and `reconcileOnce` (claim due items, run each, requeue on error with `backoffMs`) |
| `backend/src/worker.ts` | `bun run worker`: `reconcileOnce` in a loop, 1s idle sleep, stops on SIGTERM |
| `backend/src/app.test.ts` | Four tests: generation/version semantics, apply and status, failure/backoff, finalization |
| `backend/migrations/0002_controlplane.sql` | `resources`, `reconcile_queue`, `events`, `records` |
| `backend/src/server.ts`, `db.ts`, `migrate.ts`, `seed.ts` | From the stack: server with SIGTERM drain, the pool, SQL migrations, a seed for the stack's `notes` table (not this service's) |
| `frontend/app/page.tsx` | `kubectl get` and `describe` on one page: generation vs observed, Ready, events of the selected resource; polls every 2s |

### Request path: `PUT /api/resources/:kind/:name`

1. Look the kind up in the provider map; unknown kind is `400`.
2. Parse `{ spec, resourceVersion? }` with zod; `spec` must be an object.
3. Name must be a DNS label (`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, ≤ 63); else `400`.
4. `provider.schema(spec)` validates the spec for that kind; a message is `400`.
5. Read the current row. A resource with `deletion_timestamp` refuses the write with `409`.
6. `store.upsert` — in Postgres, `select … for update` then insert, or update with
   `generation + 1, resource_version + 1` only if the spec differs; if `resourceVersion` was sent
   and does not match, returns null → `409`.
7. If the row is new or `generation` changed, `store.enqueue(kind, name)` (upsert into
   `reconcile_queue`, keeping the earlier `run_at`, resetting `attempts` to 0).
8. Return the resource: `201` created, `200` updated (an identical spec returns `200` with the
   same generation).

### Reconcile path: `reconcileOnce` (the worker, or `POST /api/reconcile`)

1. `store.claim(now, 20)` — `delete … where (kind, name) in (select … where run_at <= now …
   for update skip locked) returning`, so several workers share the queue without double work.
2. For each item, read the resource; if it vanished, skip.
3. If `deletion_timestamp` is set: `provider.delete`, event `Deleted`, `store.remove`. Done.
4. Else `provider.observe(name)`; `changed` if nothing observed or any spec key differs.
5. If changed, `provider.apply(name, spec, observed)` and event `Created`/`Updated`.
6. `store.setStatus` with `observedGeneration = generation` and `Ready=True`
   (`reason: Applied | InSync`). `lastTransitionTime` is kept if the status did not change.
7. On any throw: event `Warning/ReconcileError` with the delay, status `Ready=False`
   (`observedGeneration` unchanged), and `enqueue` at `now + backoffMs(attempts)` with
   `attempts + 1`. `backoffMs` is `min(5s × 2^attempts, 5m)`.

### Data model

| Table | Column | Why it matters |
|---|---|---|
| `resources` | `(kind, name)` | Primary key; one namespace, one name per kind |
| | `generation` | Counts spec changes only. The reconciler's target |
| | `resource_version` | Counts every write, including status. A client echoes it to update; stale is `409`, never a silent overwrite |
| | `spec` | Desired state, jsonb, validated by the provider's `schema` |
| | `status` | `{ observedGeneration, conditions[] }`. Written only by the reconciler |
| | `deletion_timestamp` | Set by `DELETE`; the row exists until the finalizer ran |
| `reconcile_queue` | `(kind, name)` | Primary key: one pending reconcile per resource, however many writes |
| | `run_at`, `attempts` | The backoff clock; `attempts` resets to 0 on a new spec |
| `events` | `type`, `reason`, `message` | `Normal`/`Warning`, `Created`/`Updated`/`Deleted`/`ReconcileError`; indexed by `(kind, name, id desc)` |
| `records` | `name`, `value` | The built-in provider's "external system" |

## Invariants

1. **`generation` moves only when the spec changes.** An identical `PUT` returns the same
   generation and enqueues nothing. Guarded by `app.test.ts` "generation counts spec changes; a
   stale resourceVersion is a conflict".
2. **A stale `resourceVersion` is `409`.** Optimistic concurrency, never last-write-wins; a
   `resourceVersion` on a resource that does not exist is also refused. Same test.
3. **Only the reconciler writes `status`.** No route calls `setStatus`. `observedGeneration`
   tells the reader which spec the status describes; the page colours a row green only when it
   equals `generation`. Guarded by "reconcile applies the spec through the provider and reports
   Ready with observedGeneration".
4. **Every generation change enqueues; the queue holds at most one item per resource.**
   `enqueue` is an upsert keeping the earliest `run_at`. Guarded by the `reconciled: 0` second
   pass and the single `apply` in the same test.
5. **Apply is a no-op when observed equals spec.** The reconciler diffs `spec` against
   `observe()` before calling `apply`; `recordsProvider.apply` checks again. Guarded by
   `fake.calls.filter(apply).length === 1`.
6. **A provider error is `Ready=False`, a `Warning` event, and a requeue at
   `backoffMs(attempts)`.** Never a dropped item, never a crash of the pass. Guarded by "a
   provider failure sets Ready=False, records a Warning and requeues with backoff", including
   the 5s → 10s doubling and the 5m cap.
7. **Delete finalizes external-first.** `DELETE` marks and returns `202`; the row stays readable;
   a write to it is `409`; the reconciler calls `provider.delete` and only then removes the row.
   Guarded by "delete finalizes: the external thing goes first, then the row".
8. **`claim` is `SKIP LOCKED` in Postgres.** Several workers share the queue by construction, so
   there is no leader election to get wrong. The memory store mimics it single-threaded.
9. **A provider knows one external system and nothing else.** No provider touches
   `generation`, conditions, events or the queue; `Provider` has four methods and that is the
   whole contract. The reconciler is tested with `fakeProvider` for that reason.
10. **Names are DNS labels; kinds are registered providers.** `400` for both, before any read.
11. **Both stores implement the same `Store`.** Tests run on `memoryStore`; a method added to
    one is added to the other in the same change.

## Extending it

**Add a kind (a provider).** Implement `Provider` in `backend/src/providers.ts` — `kind`,
`schema`, `observe`, `apply`, `delete` — against the external API, with a timeout on every call.
Register it in the `providerList` default in `createApp` and in `worker.ts`'s map. Test the
reconciler with `fakeProvider("YourKind")` for the state machine, and the provider itself with a
fake `fetch` for its mapping. Migration only if the external system is a table here.

**Add a field to a spec.** Change the provider's `schema` and `apply`; the reconciler's diff is
key-by-key over `spec`, so a new key is picked up as a change. If `observe` cannot return it,
every reconcile will call `apply` — make `observe` return it or exclude it from the diff
deliberately, in the provider. Add a test that a second pass is a no-op.

**Add a condition.** `setCondition` in `reconciler.ts` keeps `lastTransitionTime` per
`type`; add a second `type` (e.g. `Synced`) beside `Ready` and assert its transition time holds
across an unchanged status.

**Add periodic resync.** Kubernetes re-lists every 10 hours to catch drift. Add to `worker.ts`:
every N minutes, `enqueue` every resource from `store.list()`. No migration; `reconcile_queue`
already dedupes.

**Add a second worker.** Nothing to change: run two `bun run worker`; `SKIP LOCKED` shares the
claims. Add a worker Deployment to `k8s/base` — the stack ships none.

**Expose status writes to a client (a `/status` subresource).** Do not. The invariant is that
only the reconciler writes status; a route that does breaks the meaning of `observedGeneration`.

**Add a list filter.** `GET /api/resources?kind=` is a `where` in `store.list`; add the parameter
to both stores and a test.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Resources, queue, events, and the `records` table |
| `PORT` | no | API port, default `8000` |

Two processes: the API (`bun run dev` / the container's `start`) and the worker (`bun run
worker`). Without a worker, nothing reconciles; `POST /api/reconcile` runs one pass by hand.

**Scaling knobs.** `reconcileOnce`'s `limit` (20 per claim) and the worker's 1s idle sleep are in
code. Workers scale horizontally with no coordination. API replicas are stateless.

**Per-replica today.** Nothing. The provider map is code; every write is a row. The events table
grows without bound (see Ceilings).

**Failure modes.**

| What fails | What the user sees |
|---|---|
| Provider unreachable | `Ready=False / ReconcileError`, a `Warning` event with `(retry in 5s)`, then 10s, 20s … capped at 5m, forever. Spec is kept; `observedGeneration` stays behind |
| Worker not running | Every write sits in `reconcile_queue`; the page shows `Pending` and generation ahead of observed |
| Two clients write the same resource | The second with a stale `resourceVersion` gets `409`; without one, last write wins by design |
| Worker killed mid-apply | The item was already claimed (deleted from the queue). If the process died before `setStatus`, the resource is not requeued — see Ceilings |
| Postgres down | API `500`; the worker logs `reconcile pass failed` as JSON and retries after 1s |

**What to watch.** Resources where `generation <> (status->>'observedGeneration')::int` for
longer than your backoff tolerance; `reconcile_queue` where `attempts >= 6` (5m cap reached);
`events` where `type = 'Warning'` per hour; queue depth vs worker count. The worker's only log
line is the pass failure; per-resource history is in `events`.

## Ceilings

- **A worker crash between `claim` and `setStatus` loses the item.** `claim` deletes the row
  before the work is done. Upgrade: claim by setting a `locked_until` instead of deleting, and
  let a sweeper requeue expired locks.
- **No periodic resync.** Drift in the external system after a successful reconcile is not
  detected until the next spec change. Upgrade: the resync recipe above.
- **No worker Deployment in `k8s/`.** The stack ships a backend and a frontend; the worker
  needs its own manifest with `replicas` and no HTTP probes.
- **Events are never pruned.** Upgrade: partition by time, or delete rows older than N days from
  the worker loop.
- **No namespaces, labels, selectors or list filters.** `GET /api/resources` is everything.
- **No watch.** The page polls every 2s. Upgrade: `LISTEN/NOTIFY` on writes, SSE to the page.
- **No auth or RBAC on the API.**
- **Kinds are registered in code**, not via a CRD-like resource.
- **`/api/health/ready` returns `db: "ok"` without asking the database.**
- **The reconciler's diff is `JSON.stringify` per spec key** — key order inside nested objects
  matters. Upgrade: a stable stringify (the dag pack has one).

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const CP_AGENTS: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run and test it.

## Run

    make demo                    # postgres + redis, migrate, seed, API on :8000, page on :3000
    cd backend && bun run worker # the controller loop; without it nothing reconciles
    make check                   # the gate: typecheck both halves, bun test — no database needed
    make migrate                 # apply backend/migrations to DATABASE_URL

For a demo without a worker, `POST /api/reconcile` runs one pass.

## Routes

Apply a spec (create):

    curl -X PUT localhost:8000/api/resources/Record/greeting \
      -H 'content-type: application/json' -d '{"spec":{"value":"hello"}}'
    # 201 {"kind":"Record","name":"greeting","generation":1,"resource_version":1,"spec":{"value":"hello"},
    #      "status":{"observedGeneration":0,"conditions":[]},"deletion_timestamp":null}

Same spec again: `200`, same generation. Changed spec: `200`, `generation: 2`.
With optimistic concurrency:

    curl -X PUT localhost:8000/api/resources/Record/greeting -H 'content-type: application/json' \
      -d '{"spec":{"value":"bye"},"resourceVersion":1}'
    # 409 {"error":{"message":"resourceVersion is stale","code":"conflict"}}   (if someone wrote since)

Refusals:

    curl -X PUT localhost:8000/api/resources/Widget/a -H 'content-type: application/json' -d '{"spec":{}}'
    # 400 {"error":{"message":"unknown kind Widget","code":"invalid"}}
    curl -X PUT localhost:8000/api/resources/Record/Bad_Name -H 'content-type: application/json' -d '{"spec":{"value":"x"}}'
    # 400 {"error":{"message":"name must be a DNS label","code":"invalid"}}
    curl -X PUT localhost:8000/api/resources/Record/a -H 'content-type: application/json' -d '{"spec":{"value":3}}'
    # 400 {"error":{"message":"spec.value must be a string","code":"invalid"}}

Reconcile once, then read it back:

    curl -X POST localhost:8000/api/reconcile
    # {"reconciled":1}
    curl localhost:8000/api/resources/Record/greeting
    # {"…","generation":1,"resource_version":2,"status":{"observedGeneration":1,"conditions":[
    #   {"type":"Ready","status":"True","reason":"Applied","message":"generation 1 applied","lastTransitionTime":"…"}]},…}
    curl localhost:8000/api/resources/Record/greeting/events
    # {"events":[{"id":1,"kind":"Record","name":"greeting","type":"Normal","reason":"Created","message":"generation 1 applied","created_at":"…"}]}

List and kinds:

    curl localhost:8000/api/resources      # {"resources":[…]} ordered by kind, name
    curl localhost:8000/api/kinds          # {"kinds":["Record"]}

Delete (two-phase):

    curl -X DELETE localhost:8000/api/resources/Record/greeting
    # 202 {…,"deletion_timestamp":"…"}
    curl -X PUT localhost:8000/api/resources/Record/greeting -H 'content-type: application/json' -d '{"spec":{"value":"y"}}'
    # 409 {"error":{"message":"resource is being deleted","code":"conflict"}}
    curl -X POST localhost:8000/api/reconcile      # provider.delete, then the row
    curl localhost:8000/api/resources/Record/greeting
    # 404 {"error":{"message":"no such resource","code":"not_found"}}

Check the external system directly: `psql $DATABASE_URL -c 'select * from records'`.

## How the tests are built

`backend/src/app.test.ts` uses `memoryStore()` and `fakeProvider()`; no database, no network.

- `createApp(store, [fake.provider])` gives the API over the memory store with the fake as the
  `Record` kind. `app.request()` drives Hono without a server.
- `fakeProvider()` returns `{ provider, rows, calls, failNext(n) }`: `rows` is the "external
  system", `calls` records every `observe`/`apply`/`delete`, `failNext(n)` makes the next n
  calls throw.
- Time is a parameter: `reconcileOnce(store, providers, now)` takes the clock, so backoff is
  asserted by passing `t0 + backoffMs(0)` rather than sleeping.

What each test pins: generation and resource_version semantics plus the three `400`s; apply,
status, events and the no-op second pass; failure → `Ready=False`, `Warning`, 5s then 10s then
the 5m cap; finalization order and the `409` on a deleting resource.

## Adding a test

Start from `setup()`; write through `app.request` with `put(spec, resourceVersion?)`; drive the
reconciler either through `POST /api/reconcile` or directly with `reconcileOnce(store, providers,
now)` when you need to control the clock. Assert on `store.get(...)` for status, on
`store.events(...)` for what happened, and on `fake.rows` / `fake.calls` for what reached the
external system. A new provider gets its own fake or a fake `fetch`; never a real endpoint.
"##;

const CP_README: &str = r##"# {{NAME}}

A control plane in TypeScript: declare what should exist, and a reconciler makes an external
system match — with generations, optimistic concurrency, conditions, events, backoff and
two-phase deletion the way Kubernetes does them.

## What you get

- `PUT` a spec, get back `generation`, `resource_version` and `status`; an identical spec is a
  no-op, a stale `resourceVersion` is a `409`.
- A reconciler that observes, applies only the difference, and reports `Ready` with
  `observedGeneration` so you can tell "in sync" from "not yet".
- Errors as `Warning` events and exponential backoff (5s doubling to 5m); never a dropped item.
- Two-phase delete: the external thing goes first, then the row.
- A `Provider` contract of four methods for each kind; the reconciler is provider-agnostic.
- A work queue in Postgres with `SKIP LOCKED`, so workers scale by adding processes.
- Routes: `GET/PUT/DELETE /api/resources/:kind/:name`, `GET …/events`, `GET /api/resources`,
  `GET /api/kinds`, `POST /api/reconcile`.
- A page that is `kubectl get` and `kubectl describe` side by side.
- Tests without a database or a clock; migrations; Docker and kustomize manifests.

## Five minutes

    make demo
    cd backend && bun run worker      # second terminal

Then:

    curl -X PUT localhost:8000/api/resources/Record/greeting -H 'content-type: application/json' -d '{"spec":{"value":"hello"}}'
    # 201 {"kind":"Record","name":"greeting","generation":1,"resource_version":1,"status":{"observedGeneration":0,"conditions":[]},…}

    sleep 1; curl localhost:8000/api/resources/Record/greeting
    # {…,"generation":1,"status":{"observedGeneration":1,"conditions":[{"type":"Ready","status":"True","reason":"Applied",…}]},…}

    curl -X PUT localhost:8000/api/resources/Record/greeting -H 'content-type: application/json' -d '{"spec":{"value":"hi"}}'
    # 200 {…,"generation":2,…}        the worker applies it within a second

    curl localhost:8000/api/resources/Record/greeting/events
    # {"events":[{"type":"Normal","reason":"Updated","message":"generation 2 applied",…},{"reason":"Created",…}]}

Open http://localhost:3000 and watch the `observed` column catch up with `generation`.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not currently check the database) |
| GET | `/api/kinds` | none | Registered kinds |
| GET | `/api/resources` | none | Every resource, ordered by kind and name |
| GET | `/api/resources/:kind/:name` | none | One resource with spec and status |
| GET | `/api/resources/:kind/:name/events` | none | Last 50 events, newest first |
| PUT | `/api/resources/:kind/:name` | none | Create or update the spec; optional `resourceVersion`; `201`/`200`/`400`/`409` |
| DELETE | `/api/resources/:kind/:name` | none | Mark for deletion; `202`; finalized by the reconciler |
| POST | `/api/reconcile` | none | One reconcile pass (demo and tests); the worker does this in a loop |

No route is authenticated. Put the API behind your ingress auth before exposing it.

## Compared with Kubernetes controllers and Crossplane

**Same shapes, so their docs transfer**

- `spec` / `status` split; `metadata.generation` and `resourceVersion` with the same meanings;
  `409 Conflict` on a stale version.
- `status.observedGeneration` and `conditions[]` with `type`, `status`, `reason`, `message`,
  `lastTransitionTime` (kept when the status did not change).
- Events with `Normal`/`Warning` and a `reason`.
- `deletionTimestamp` and finalizer-style two-phase delete.
- controller-runtime's requeue-with-backoff on error, 5s → 5m.
- Crossplane's provider shape: observe, create/update, delete, one external system per kind.

**Better here**

- One codebase, four files, ~300 lines of control logic you can read end to end — no
  code generation, no informers, no CRD YAML, no `kubebuilder`.
- Reconcile tests that control the clock and the provider and run in milliseconds; the backoff
  schedule is asserted, not trusted.
- Typed from the provider's `schema` to the frontend's client; a spec field that stops being
  returned stops the page compiling.
- The work queue is a Postgres table: visible with `select * from reconcile_queue`, shared by
  `SKIP LOCKED`, no leader election.
- Runs anywhere Postgres does; the cluster is optional and the manifests are already written.

**Not here yet**

- No watch/stream API; clients poll.
- No namespaces, labels, annotations, selectors, or list filters.
- No periodic resync, so drift after a successful reconcile is only caught on the next spec
  change.
- No admission webhooks, defaulting, or schema evolution for specs; validation is a function
  per provider.
- No RBAC, no authentication, no audit log.
- No owner references, garbage collection, or cross-resource dependencies.
- No leader election (not needed with `SKIP LOCKED`), but also no per-kind concurrency limits.
- No worker Deployment manifest in `k8s/`; the stack ships API and frontend only.
- An in-flight item is lost if the worker dies between claim and status write.
- Events are unbounded; `resourceVersion` is per row, not a global watch cursor.
- One built-in provider, and it manages a table in the same database.

## Production

- **Environments.** `DATABASE_URL` only. `backend/.env` is ignored; `backend/.env.age` is
  committed and decrypted by CI.
- **Scaling.** API replicas are stateless. Workers scale by count; each claims up to 20 items a
  pass with `SKIP LOCKED`. Keep (API + worker replicas) × 10 under Postgres `max_connections`.
- **Probes.** The stack's `/api/health*` probes on the API. The worker has no HTTP surface; use
  a liveness command or the queue-age alert below.
- **Migrations.** `backend/migrations/0002_controlplane.sql`; `make migrate` locally, the
  migrate init container in the cluster.
- **Secrets.** A real provider's credentials go in `backend/.env` and reach the worker as env.
- **Kubernetes.** `k8s/base` + overlays for the API and page. Add a worker Deployment (no HPA,
  no HTTP probes, `bun run worker`) before relying on it.
- **What pages you.** Any resource with `generation <> observedGeneration` for longer than 10
  minutes; `reconcile_queue.attempts >= 6` (backoff at its 5m cap); `Warning` events per hour
  above baseline; oldest `run_at` in the queue older than a minute while workers are up.

## Roadmap

1. Lease-style claims (`locked_until`) so a worker crash requeues instead of losing the item.
2. Periodic resync from the worker loop.
3. A worker Deployment in `k8s/base`.
4. Event pruning by age.
5. `LISTEN/NOTIFY` → SSE so the page stops polling.
6. List filters by kind and a stable diff for nested specs.
"##;

const CP_REVIEWER: &str = r##"---
name: reconcile-semantics
description: Run on any change to backend/src/reconciler.ts, store.ts, providers.ts, worker.ts, or the resource routes. Reads the change as someone who has seen a controller overwrite a user's edit, loop forever on a no-op, or delete the row before the thing it managed, and reports what will do that here.
tools: Read, Grep, Glob, Bash
---

You review reconciliation semantics. Kubernetes has spent a decade finding these bugs; the
checklist is what it found. Report only what breaks, each as
`path:line — what — the sequence that triggers it — the fix`.

Check:
1. **`generation` bumps only on a spec change.** `upsert` compares the spec before updating;
   a change that bumps on every `PUT` makes every write a reconcile and hides real changes.
2. **`resource_version` bumps on every write, status included**, and a stale one is refused
   inside the same transaction that reads the row (`for update`). Check-then-write outside the
   transaction is a lost update.
3. **Status is written only by the reconciler.** Grep `setStatus` — it must appear in
   `reconciler.ts` only. A route that writes status decouples `observedGeneration` from truth.
4. **`observedGeneration` is set from the resource the reconciler read**, not re-read after
   apply. If the spec changed during the apply, the new generation is still queued and the
   status honestly describes the old one.
5. **Enqueue on every generation change and on delete**, and nowhere else; the queue is one row
   per resource, keeping the earlier `run_at`. A `PUT` that resets a backoff timer for an
   unchanged spec is a way to hammer a failing provider.
6. **A new spec resets `attempts`.** `enqueue` from the route passes attempts 0. Fine — but a
   change that carries the old attempts across a spec change delays a fix by up to 5m.
7. **Error handling in `reconcileOnce` catches everything from `reconcileOne`**, writes the
   `Warning`, sets `Ready=False`, and requeues with `backoffMs(item.attempts)`. A thrown
   `setStatus` or `event` inside the catch aborts the pass for every remaining claimed item —
   claimed and now gone from the queue.
8. **Finalization order.** For a deleting resource: `provider.delete` first, `store.remove`
   last, and a failed delete leaves the row so the retry can happen. A change that removes the
   row first orphans the external thing.
9. **Writes to a deleting resource are `409`.** Check the route reads the row before `upsert`.
10. **Apply is idempotent and diffed.** `changed` compares every spec key to `observe()`; a
    provider whose `observe` cannot return some key will apply every pass. Ask: is a second
    pass a no-op? The test `fake.calls.filter(apply).length === 1` must still hold.
11. **The provider knows nothing of the control plane.** No provider imports `store.ts` or
    `reconciler.ts`, touches `status`, or enqueues.
12. **`claim` is `for update skip locked` and bounded** (`limit`). A claim without `skip locked`
    serialises workers; without a limit, one pass can hold a transaction across hundreds of
    provider calls.
13. **Backoff is `min(5000 × 2^attempts, 300000)`**; `backoffMs(10) === 300_000` is asserted.
    A change to the constants changes the test, deliberately.
14. **The worker stops on SIGTERM after the current pass**, and logs a failed pass as one JSON
    line and continues. A crash loop on a bad provider is a change that removed the catch.
15. **Name and kind are validated before any store call.** DNS-label regex and provider map
    lookup happen first; an unknown kind must never reach `upsert`.

End with one line: `reconcile-semantics: N findings`, and if 0, which of the above you checked.
"##;
