//! dataplane: a control-plane/data-plane pair, like Envoy xDS ══════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", DP_APP.into()),
        ("backend/src/store.ts", DP_STORE.into()),
        ("backend/src/dataplane.ts", DP_ENGINE.into()),
        ("backend/src/app.test.ts", DP_TEST.into()),
        ("backend/migrations/0002_dataplane.sql", DP_SQL.into()),
        ("frontend/app/page.tsx", f(DP_PAGE)),
    ]
}

const DP_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { DataPlane, httpPull, type Pull } from "./dataplane";

// Envoy's split in one process: `/api/control/*` is the control plane (writes desired state,
// serves snapshots by generation), everything else is the data plane (serves from its local
// snapshot). Run them as two deployments by setting CONTROL_PLANE_URL on the data-plane pods —
// the data plane then pulls over HTTP and the control-plane routes on it go unused. Either
// way, no request handler below the control section ever awaits the control plane.

const routeSchema = z.object({ prefix: z.string().startsWith("/").max(200), upstream: z.string().url(), timeout_ms: z.number().int().min(1).max(60_000).default(5000), headers: z.record(z.string()).default({}) });

export function createApp(store: Store, opts: { pull?: Pull; intervalMs?: number; staleAfterMs?: number; now?: () => number } = {}) {
  const notify = new Set<(generation: number) => void>();
  const pull: Pull = opts.pull ?? (process.env.CONTROL_PLANE_URL ? httpPull(process.env.CONTROL_PLANE_URL) : async (since) => { const s = await store.snapshot(); return s.generation > since ? s : null; });
  const plane = new DataPlane({ pull, intervalMs: opts.intervalMs, staleAfterMs: opts.staleAfterMs, now: opts.now });
  if (!opts.pull) notify.add((g) => void plane.notify(g));   // same process: push is a function call

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── control plane ─────────────────────────────────────────────────────────────────
    .get("/api/control/snapshot", async (c) => {
      const since = Number(c.req.query("since") ?? -1);
      const s = await store.snapshot();
      return s.generation > since ? c.json(s) : c.body(null, 304);
    })
    .get("/api/control/routes", async (c) => c.json(await store.snapshot()))
    .put("/api/control/routes/:name", async (c) => {
      const p = routeSchema.safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: p.error.issues[0]?.message ?? "invalid route", code: "invalid" } }, 400);
      const generation = await store.put({ name: c.req.param("name"), ...p.data });
      for (const fn of notify) fn(generation);
      return c.json({ name: c.req.param("name"), generation });
    })
    .delete("/api/control/routes/:name", async (c) => {
      const generation = await store.remove(c.req.param("name"));
      if (generation === null) return c.json({ error: { message: "no such route", code: "not_found" } }, 404);
      for (const fn of notify) fn(generation);
      return c.json({ generation });
    })

    // ── data plane ────────────────────────────────────────────────────────────────────
    // What Envoy would do with the matched route is proxy the request; here the answer is the
    // decision itself, so the hot path is visible and testable.
    .get("/api/resolve", async (c) => {
      const path = c.req.query("path") ?? "/";
      const r = await plane.resolve(path);
      if (!r.route) return c.json({ error: { message: "no route", code: "not_found" }, generation: r.generation }, 404);
      return c.json({ path, route: r.route, generation: r.generation, stale: plane.stale });
    })
    // Push from a control plane in another process: "generation N exists, come and get it".
    .post("/api/notify", async (c) => {
      const p = z.object({ generation: z.number().int().nonnegative() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "generation required", code: "invalid" } }, 400);
      void plane.notify(p.data.generation);
      return c.json({ ok: true, generation: plane.snapshot.generation });
    })
    .get("/api/status", (c) => c.json({ generation: plane.snapshot.generation, routes: plane.snapshot.routes.length, stale: plane.stale, age_ms: plane.staleMs, last_error: plane.lastError }))
    .get("/metrics", (c) => c.text(plane.metrics()));

  return Object.assign(app, { plane });
}

const app = createApp(pgStore());
app.plane.start();
export type AppType = typeof app;
export default app;
"##;

const DP_STORE: &str = r##"import { db } from "./db";

export type Route = { name: string; prefix: string; upstream: string; timeout_ms: number; headers: Record<string, string> };
export type Snapshot = { generation: number; routes: Route[] };

// The control plane's store. Every write bumps the single generation inside the same
// transaction as the row, so a data plane never sees a generation whose rows are not there yet.
export interface Store {
  snapshot(): Promise<Snapshot>;
  put(route: Route): Promise<number>;
  remove(name: string): Promise<number | null>;
}

export function pgStore(): Store {
  const routes = () => db<Route[]>`select name, prefix, upstream, timeout_ms, headers from routes order by length(prefix) desc, name`;
  return {
    snapshot: async () => {
      const [g] = await db<{ generation: number }[]>`select generation::int as generation from generations where id = 1`;
      return { generation: g.generation, routes: await routes() };
    },
    put: (r) => db.begin(async (tx) => {
      const [g] = await tx<{ generation: number }[]>`update generations set generation = generation + 1 where id = 1 returning generation::int as generation`;
      await tx`insert into routes (name, prefix, upstream, timeout_ms, headers, generation) values (${r.name}, ${r.prefix}, ${r.upstream}, ${r.timeout_ms}, ${tx.json(r.headers)}, ${g.generation})
        on conflict (name) do update set prefix = excluded.prefix, upstream = excluded.upstream, timeout_ms = excluded.timeout_ms, headers = excluded.headers, generation = excluded.generation, updated_at = now()`;
      return g.generation;
    }),
    remove: (name) => db.begin(async (tx) => {
      const gone = await tx`delete from routes where name = ${name} returning name`;
      if (gone.length === 0) return null;
      const [g] = await tx<{ generation: number }[]>`update generations set generation = generation + 1 where id = 1 returning generation::int as generation`;
      return g.generation;
    }),
  };
}

export function memoryStore(): Store {
  const routes = new Map<string, Route>(); let generation = 0;
  return {
    snapshot: async () => ({ generation, routes: [...routes.values()].sort((a, b) => b.prefix.length - a.prefix.length || a.name.localeCompare(b.name)) }),
    put: async (r) => { routes.set(r.name, structuredClone(r)); return ++generation; },
    remove: async (name) => (routes.delete(name) ? ++generation : null),
  };
}
"##;

const DP_ENGINE: &str = r##"import type { Route, Snapshot } from "./store";

// The data plane's local state: the last snapshot it pulled, an LRU over route lookups, and the
// loop that keeps the snapshot fresh. Nothing in here is awaited on the request path except the
// LRU itself. If the control plane is gone, `snapshot` is simply older — the gauge says by how
// much, and requests keep being answered from it. That is the whole point of the split.

export type Pull = (sinceGeneration: number) => Promise<Snapshot | null>;   // null = unchanged
export type Resolved = { route: Route | null; generation: number };

/// An LRU with single-flight: concurrent misses on one key share one in-flight computation
/// rather than each computing it (the thundering-herd on a cache flush).
export class Lru<V> {
  private map = new Map<string, V>();
  private inflight = new Map<string, Promise<V>>();
  constructor(private max: number) {}
  get size() { return this.map.size; }
  clear() { this.map.clear(); }
  async getOrCompute(key: string, compute: () => Promise<V>): Promise<V> {
    if (this.map.has(key)) { const v = this.map.get(key)!; this.map.delete(key); this.map.set(key, v); return v; }
    let p = this.inflight.get(key);
    if (!p) {
      p = compute().then((v) => {
        this.map.set(key, v);
        if (this.map.size > this.max) this.map.delete(this.map.keys().next().value!);
        return v;
      }).finally(() => this.inflight.delete(key));
      this.inflight.set(key, p);
    }
    return p;
  }
}

export type DataPlaneOpts = { pull: Pull; intervalMs?: number; staleAfterMs?: number; lruSize?: number; now?: () => number };

export class DataPlane {
  snapshot: Snapshot = { generation: 0, routes: [] };
  lastPullAt = 0;              // when we last heard from the control plane (pull or 304)
  lastError: string | null = null;
  computes = 0;                // how many lookups were actually computed (tests, and a metric)
  private lru: Lru<Resolved>;
  private syncing: Promise<void> | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private now: () => number;

  constructor(private opts: DataPlaneOpts) {
    this.lru = new Lru(opts.lruSize ?? 10_000);
    this.now = opts.now ?? Date.now;
  }

  /// Request path: local snapshot + LRU. Never touches `pull`.
  resolve(path: string): Promise<Resolved> {
    return this.lru.getOrCompute(path, async () => {
      this.computes++;
      // Longest-prefix match; routes arrive sorted longest first.
      const route = this.snapshot.routes.find((r) => path.startsWith(r.prefix)) ?? null;
      return { route, generation: this.snapshot.generation };
    });
  }

  /// One pull, single-flight: a push notification arriving mid-pull joins it instead of starting
  /// another. Errors are recorded, not thrown — the loop keeps going.
  sync(): Promise<void> {
    if (this.syncing) return this.syncing;
    this.syncing = (async () => {
      try {
        const next = await this.opts.pull(this.snapshot.generation);
        this.lastPullAt = this.now(); this.lastError = null;
        if (next && next.generation > this.snapshot.generation) { this.snapshot = next; this.lru.clear(); }
      } catch (e) {
        this.lastError = e instanceof Error ? e.message : String(e);
      } finally { this.syncing = null; }
    })();
    return this.syncing;
  }

  /// Push notify: the control plane says a newer generation exists. Pull now if it is newer
  /// than ours — a notify is a hint, the pull is the truth.
  notify(generation: number) { return generation > this.snapshot.generation ? this.sync() : Promise.resolve(); }

  start() { if (!this.timer) { void this.sync(); this.timer = setInterval(() => void this.sync(), this.opts.intervalMs ?? 5_000); this.timer.unref?.(); } return this; }
  stop() { if (this.timer) clearInterval(this.timer); this.timer = null; }

  get staleMs() { return this.lastPullAt ? this.now() - this.lastPullAt : -1; }
  get stale() { return this.lastPullAt === 0 || this.staleMs > (this.opts.staleAfterMs ?? 30_000); }

  /// Prometheus text format; `snapshot_stale` is the alert, the others explain it.
  metrics() {
    return [
      "# TYPE dataplane_snapshot_generation gauge", `dataplane_snapshot_generation ${this.snapshot.generation}`,
      "# TYPE dataplane_snapshot_age_seconds gauge", `dataplane_snapshot_age_seconds ${this.lastPullAt ? (this.staleMs / 1000).toFixed(3) : "NaN"}`,
      "# TYPE dataplane_snapshot_stale gauge", `dataplane_snapshot_stale ${this.stale ? 1 : 0}`,
      "# TYPE dataplane_lru_entries gauge", `dataplane_lru_entries ${this.lru.size}`,
      "# TYPE dataplane_route_computes_total counter", `dataplane_route_computes_total ${this.computes}`,
      "",
    ].join("\n");
  }
}

/// Pull over HTTP from a control plane: `GET /api/control/snapshot?since=N`, 304 when unchanged.
export function httpPull(base: string, fetchImpl: (url: string, init?: RequestInit) => Promise<Response> = fetch): Pull {
  return async (since) => {
    const res = await fetchImpl(`${base}/api/control/snapshot?since=${since}`);
    if (res.status === 304) return null;
    if (!res.ok) throw new Error(`control plane ${res.status}`);
    return (await res.json()) as Snapshot;
  };
}
"##;

const DP_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { Lru, type Pull } from "./dataplane";

const put = (body: unknown) => ({ method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const route = (upstream = "http://api:8000") => ({ prefix: "/api", upstream });

describe("data plane", () => {
  test("the control plane writes generations; a snapshot is served only when newer", async () => {
    const app = createApp(memoryStore(), { pull: async () => null });
    expect((await app.request("/api/control/snapshot")).status).toBe(200);
    expect((await app.request("/api/control/snapshot?since=0")).status).toBe(304);
    expect((await (await app.request("/api/control/routes/api", put(route()))).json()).generation).toBe(1);
    expect((await (await app.request("/api/control/routes/web", put({ prefix: "/", upstream: "http://web:3000" }))).json()).generation).toBe(2);
    const s = await (await app.request("/api/control/snapshot?since=1")).json();
    expect(s.generation).toBe(2);
    expect(s.routes.map((r: { name: string }) => r.name)).toEqual(["api", "web"]);   // longest prefix first
    expect((await app.request("/api/control/snapshot?since=2")).status).toBe(304);
    expect((await app.request("/api/control/routes/bad", put({ prefix: "nope", upstream: "x" }))).status).toBe(400);
    expect((await app.request("/api/control/routes/none", { method: "DELETE" })).status).toBe(404);
  });

  test("the data plane serves from its snapshot and picks up pushes and pulls", async () => {
    const cp = createApp(memoryStore(), { pull: async () => null });
    // A separate data plane whose only link to the control plane is the pull function.
    const pulls: number[] = [];
    const pull: Pull = async (since) => { pulls.push(since); const r = await cp.request(`/api/control/snapshot?since=${since}`); return r.status === 304 ? null : r.json(); };
    const dp = createApp(memoryStore(), { pull });
    await dp.plane.sync();
    expect((await dp.request("/api/resolve?path=/api/x")).status).toBe(404);
    await cp.request("/api/control/routes/api", put(route()));
    // Still the old snapshot: the request path does not ask the control plane.
    expect((await dp.request("/api/resolve?path=/api/x")).status).toBe(404);
    expect(pulls).toEqual([0]);
    // Push notify triggers a pull; a stale notify (older generation) does not.
    await dp.request("/api/notify", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ generation: 1 }) });
    await dp.plane.sync();
    const r = await (await dp.request("/api/resolve?path=/api/x")).json();
    expect(r).toMatchObject({ route: { name: "api", upstream: "http://api:8000" }, generation: 1, stale: false });
    // The notify's pull and the explicit sync were one pull (single-flight); an old notify is none.
    await dp.plane.notify(0);
    expect(pulls).toEqual([0, 0]);
  });

  test("killing the control plane: the data plane keeps serving and the stale gauge says so", async () => {
    let clock = 1_000_000, alive = true;
    const cp = createApp(memoryStore(), { pull: async () => null });
    await cp.request("/api/control/routes/api", put(route()));
    const pull: Pull = async (since) => { if (!alive) throw new Error("ECONNREFUSED"); const r = await cp.request(`/api/control/snapshot?since=${since}`); return r.status === 304 ? null : r.json(); };
    const dp = createApp(memoryStore(), { pull, staleAfterMs: 30_000, now: () => clock });
    await dp.plane.sync();
    expect((await dp.request("/api/resolve?path=/api/users")).status).toBe(200);

    alive = false;   // the control plane is gone
    for (let i = 0; i < 20; i++) { clock += 5_000; await dp.plane.sync(); }
    // Every request still answers, from the last good snapshot.
    for (let i = 0; i < 50; i++) expect((await dp.request(`/api/resolve?path=/api/u/${i}`)).status).toBe(200);
    const status = await (await dp.request("/api/status")).json();
    expect(status).toMatchObject({ generation: 1, stale: true, last_error: "ECONNREFUSED", age_ms: 100_000 });
    const metrics = await (await dp.request("/metrics")).text();
    expect(metrics).toContain("dataplane_snapshot_stale 1");
    expect(metrics).toContain("dataplane_snapshot_age_seconds 100.000");
    expect((await (await dp.request("/api/resolve?path=/api/x")).json()).stale).toBe(true);

    alive = true;   // it comes back: the next pull is a 304, and the gauge clears
    await dp.plane.sync();
    expect(await (await dp.request("/metrics")).text()).toContain("dataplane_snapshot_stale 0");
  });

  test("the LRU is single-flight and bounded, and a new snapshot flushes it", async () => {
    let computes = 0;
    const lru = new Lru<number>(2);
    const slow = () => new Promise<number>((r) => setTimeout(() => r(++computes), 5));
    const all = await Promise.all([lru.getOrCompute("a", slow), lru.getOrCompute("a", slow), lru.getOrCompute("a", slow)]);
    expect(all).toEqual([1, 1, 1]);
    await lru.getOrCompute("b", slow); await lru.getOrCompute("a", slow); await lru.getOrCompute("c", slow);   // touches a, evicts b
    expect(lru.size).toBe(2);
    expect(await lru.getOrCompute("a", slow)).toBe(1);
    expect(await lru.getOrCompute("b", slow)).toBe(4);

    const app = createApp(memoryStore());
    await app.request("/api/control/routes/api", put(route()));
    await app.plane.sync();
    await Promise.all(Array.from({ length: 10 }, () => app.request("/api/resolve?path=/api/x")));
    expect(app.plane.computes).toBe(1);
    await app.request("/api/control/routes/api", put(route("http://api-v2:8000")));
    await app.plane.sync();
    expect((await (await app.request("/api/resolve?path=/api/x")).json()).route.upstream).toBe("http://api-v2:8000");
    expect(app.plane.computes).toBe(2);
  });
});
"##;

const DP_SQL: &str = r##"-- The control plane's desired state: routes, and one generation counter that every write
-- advances. A data plane asks "anything after generation N?" and gets the whole snapshot or a
-- 304 — Envoy's xDS state-of-the-world, keyed by version_info.
create table if not exists routes (
  name text primary key,
  prefix text not null,
  upstream text not null,
  timeout_ms int not null default 5000,
  headers jsonb not null default '{}',
  generation bigint not null,
  updated_at timestamptz not null default now()
);
create table if not exists generations (
  id int primary key default 1 check (id = 1),
  generation bigint not null default 0
);
insert into generations (id, generation) values (1, 0) on conflict do nothing;
"##;

const DP_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Route = { name: string; prefix: string; upstream: string; timeout_ms: number };
type Status = { generation: number; routes: number; stale: boolean; age_ms: number; last_error: string | null };

// Two columns: what the control plane wants (routes, generation) and what the data plane is
// actually serving (its snapshot generation, how old it is, whether that counts as stale).
// Stop the control plane and the right column keeps answering — that is the property.
export default function Home() {
  const [desired, setDesired] = useState<{ generation: number; routes: Route[] }>({ generation: 0, routes: [] });
  const [status, setStatus] = useState<Status | null>(null);
  const [name, setName] = useState("api"); const [prefix, setPrefix] = useState("/api"); const [upstream, setUpstream] = useState("http://api:8000");
  const [path, setPath] = useState("/api/users"); const [answer, setAnswer] = useState("");

  const refresh = async () => {
    setStatus(await (await fetch("/api/status")).json());
    setDesired(await (await fetch("/api/control/routes")).json().catch(() => desired));
  };
  useEffect(() => { void refresh(); const t = setInterval(refresh, 2000); return () => clearInterval(t); }, []);
  const save = async () => { await fetch(`/api/control/routes/${name}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ prefix, upstream }) }); void refresh(); };
  const remove = async (n: string) => { await fetch(`/api/control/routes/${n}`, { method: "DELETE" }); void refresh(); };
  const resolve = async () => { const r = await (await fetch(`/api/resolve?path=${encodeURIComponent(path)}`)).json(); setAnswer(JSON.stringify(r)); };

  return (
    <main>
      <h1>{{NAME}} — data plane</h1>
      <p>Desired state goes in on the left; the data plane serves from its own snapshot on the right. <code>{`curl -X PUT localhost:8000/api/control/routes/api -H 'content-type: application/json' -d '{"prefix":"/api","upstream":"http://api:8000"}'`}</code> then <code>curl 'localhost:8000/api/resolve?path=/api/users'</code>.</p>
      <div style={{ display: "flex", gap: 40 }}>
        <section>
          <h2>Control plane · generation {desired.generation}</h2>
          <p><input value={name} onChange={(e) => setName(e.target.value)} size={8} /> <input value={prefix} onChange={(e) => setPrefix(e.target.value)} size={12} /> <input value={upstream} onChange={(e) => setUpstream(e.target.value)} size={24} /> <button onClick={save}>Save route</button></p>
          <table><thead><tr><th>name</th><th>prefix</th><th>upstream</th><th></th></tr></thead>
            <tbody>{desired.routes.map((r) => <tr key={r.name}><td>{r.name}</td><td><code>{r.prefix}</code></td><td>{r.upstream}</td><td><button onClick={() => remove(r.name)}>delete</button></td></tr>)}</tbody></table>
        </section>
        <section style={{ background: status?.stale ? "#fdd" : "#dfd", padding: 12 }}>
          <h2>Data plane · snapshot {status?.generation ?? "–"}</h2>
          <p>{status ? `${status.routes} routes · snapshot age ${(status.age_ms / 1000).toFixed(0)}s · ${status.stale ? "STALE" : "fresh"}${status.last_error ? ` · last error: ${status.last_error}` : ""}` : "…"}</p>
          <p><input value={path} onChange={(e) => setPath(e.target.value)} /> <button onClick={resolve}>Resolve</button></p>
          {answer && <pre>{answer}</pre>}
          <p><a href="/metrics">/metrics</a></p>
        </section>
      </div>
    </main>
  );
}
"##;
