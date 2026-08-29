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
