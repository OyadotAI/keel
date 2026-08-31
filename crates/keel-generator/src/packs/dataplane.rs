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
        ("CLAUDE.md", f(DP_CLAUDE)),
        ("AGENTS.md", f(DP_AGENTS)),
        ("README.md", f(DP_README)),
        (".claude/agents/plane-separation.md", DP_REVIEWER.into()),
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

const DP_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a control-plane/data-plane pair, modelled on Envoy's xDS state-of-the-world
protocol. The control plane stores desired routes and a single `generation` counter that every
write advances; a data plane holds the last snapshot it pulled, answers every request from it,
and keeps it fresh by polling (`GET /api/control/snapshot?since=N`, `304` when unchanged) and by
push hints (`POST /api/notify`). The property the whole design exists for: **kill the control
plane and the data plane keeps answering**, from an older snapshot, and says how old. Both halves
live in one process by default and split into two deployments with one env var. "Done" means: no
code on the request path awaits the control plane, and `dataplane_snapshot_stale` tells the truth.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | Both surfaces: `/api/control/*` (desired state, snapshots, in-process notify) and the data plane (`/api/resolve`, `/api/notify`, `/api/status`, `/metrics`). Picks the `Pull` from `CONTROL_PLANE_URL` |
| `backend/src/dataplane.ts` | `DataPlane` (snapshot, single-flight `sync`, `notify`, poll loop, stale gauge, Prometheus text), `Lru` (bounded, single-flight), `httpPull` |
| `backend/src/store.ts` | The control plane's `Store`: `snapshot`, `put`, `remove`; `pgStore` bumps `generations` in the same transaction as the row; `memoryStore` for tests |
| `backend/src/app.test.ts` | Four tests: generations and `304`; push and pull between two apps; the control plane dying; the LRU |
| `backend/migrations/0002_dataplane.sql` | `routes`, and `generations` (one row, `check (id = 1)`) |
| `backend/src/server.ts`, `db.ts`, `migrate.ts`, `seed.ts` | From the stack: server with SIGTERM drain, the pool, SQL migrations, a seed for the stack's `notes` table (not this service's) |
| `frontend/app/page.tsx` | Two columns: desired routes and generation on the left, the data plane's snapshot generation, age and stale flag on the right; polls every 2s |

### Request path: `GET /api/resolve?path=`

1. `plane.resolve(path)` → `Lru.getOrCompute(path, …)`. A hit is returned after a move to the
   end of the map; concurrent misses on one key share one computation.
2. On a miss, longest-prefix match over `snapshot.routes` (the store already sorted them longest
   first) and record `snapshot.generation` in the answer. `computes` increments.
3. `404` with the generation when nothing matched; otherwise `{ path, route, generation, stale }`.
4. Nothing above awaits `pull`. The snapshot object is swapped atomically by `sync`.

### Control path: `PUT /api/control/routes/:name`

1. Validate with zod: `prefix` starts with `/` (≤ 200), `upstream` is a URL, `timeout_ms` 1–60000
   (default 5000), `headers` a string map.
2. `store.put` — in Postgres: `update generations set generation = generation + 1 … returning`,
   then upsert the route with that generation, one transaction.
3. Every registered notify function is called with the new generation. In one process, that is
   `plane.notify`, which pulls if the generation is newer than the snapshot's.
4. `{ name, generation }`.

### Sync path (`sync`, every `intervalMs` = 5s, and on notify)

1. If a sync is in flight, join it (single-flight).
2. `pull(snapshot.generation)` — in-process: the store's snapshot if newer, else null; across
   processes: `httpPull` to `CONTROL_PLANE_URL`, `304` → null, non-2xx → throw.
3. Success (including `304`): `lastPullAt = now`, `lastError = null`; if newer, replace the
   snapshot and clear the LRU.
4. Failure: `lastError` set; snapshot untouched; the loop continues. `stale` becomes true once
   `now - lastPullAt > staleAfterMs` (30s), or if nothing was ever pulled.

### Data model

| Table | Column | Why it matters |
|---|---|---|
| `routes` | `name` | Primary key; `PUT` upserts by it |
| | `prefix`, `upstream`, `timeout_ms`, `headers` | The route. Only `prefix` and `upstream` are used by `resolve`; the rest is carried for the proxy that is not written yet |
| | `generation` | The generation that last wrote this row (for audit; the snapshot uses the global one) |
| `generations` | `generation` | A single row (`check (id = 1)`), advanced under row lock by every write. Envoy's `version_info` |

## Invariants

1. **The request path never awaits the control plane.** `resolve` touches the local snapshot
   and the LRU only; `pull` is called from `sync` alone. Guarded by "the data plane serves from
   its snapshot and picks up pushes and pulls" — a `PUT` on the control plane is invisible to
   `/api/resolve` until a sync, and `pulls` records exactly when the control plane was asked.
2. **The generation and the row move in one transaction.** A data plane can never see a
   generation whose routes are not there. `pgStore.put`/`remove` are `db.begin`; a change that
   splits them reintroduces the race. Guarded by review — the memory store cannot show it.
3. **A snapshot is served only when newer.** `?since=N` returns `304` for `generation <= N`;
   the data plane replaces its snapshot only when `next.generation > snapshot.generation`.
   Guarded by "the control plane writes generations; a snapshot is served only when newer".
4. **A new snapshot clears the LRU.** A resolve cached under generation 1 must not answer under
   generation 2. Guarded by "the LRU is single-flight and bounded, and a new snapshot flushes it"
   (`computes` goes 1 → 2 after a route change).
5. **A notify is a hint; the pull is the truth.** `notify(g)` pulls only if `g >
   snapshot.generation` and never installs the notified generation itself. An old or forged
   notify does nothing. Guarded by `pulls === [0, 0]` after `notify(0)` in the push/pull test.
6. **`sync` is single-flight.** A notify during a poll joins the poll. Same assertion.
7. **Errors are recorded, not thrown.** A failed pull sets `lastError`, keeps the snapshot, and
   the loop continues. Guarded by "killing the control plane: the data plane keeps serving and
   the stale gauge says so" — 50 resolves succeed with the control plane down.
8. **`stale` is a function of `lastPullAt` and `now`, and `now` is injectable.** A `304` counts
   as hearing from the control plane. The gauge clears on the first successful pull. Same test,
   including `age_ms: 100_000` and `dataplane_snapshot_age_seconds 100.000`.
9. **Routes are ordered longest-prefix-first by the store**, not by the resolver. Both stores
   sort; `resolve` takes the first match. Guarded by `["api", "web"]` in the first test.
10. **The LRU is bounded and single-flight.** Size ≤ `lruSize`; three concurrent misses on one
    key compute once. Guarded by the LRU test.
11. **Route input is validated at the boundary** — prefix shape, URL, timeout range, header map;
    `400` before the store. Guarded by the `prefix: "nope"` assertion.
12. **Both stores implement the same `Store`.** Tests use `memoryStore`; a method added to one
    is added to the other in the same change.

## Extending it

**Add a field to a route.** `routeSchema` in `app.ts`, the `Route` type and both stores in
`store.ts`, a column in a new migration (`0003_…sql`, `add column … default`), and the page. If
`resolve` uses it, extend the LRU test with a route change that only touches that field, since
it must flush the cache.

**Add a match kind (host, header, method).** Extend `Route` and the schema; change the predicate
in `DataPlane.resolve` and the LRU key (`path` alone is no longer the key — include what the
match reads, or the cache answers wrong). Add a test with two routes that differ only on the new
dimension.

**Proxy for real.** Replace the body of `/api/resolve` (or add a catch-all route) that forwards
to `route.upstream` with `timeout_ms` and `headers`, streaming the response. Keep `resolve`
as the decision and test the decision; test the forward with an injected `fetch`.

**Split into two deployments.** Nothing in code: set `CONTROL_PLANE_URL` on the data-plane
pods; `httpPull` takes over and the `/api/control/*` routes on those pods go unused. Add a second
Deployment to `k8s/base` (the stack ships one backend), and have the control plane `POST
/api/notify` to each data plane's Service if you want sub-5s propagation — today nothing sends
that call across processes.

**Push to many data planes.** Keep a list of data-plane base URLs (env or a table), and in the
`PUT`/`DELETE` handlers `fetch(url + "/api/notify", …)` with a short timeout, fire-and-forget.
Failures are fine: polling catches up within `intervalMs`.

**Add a metric.** Append to `metrics()` in `dataplane.ts` with a `# TYPE` line, and assert the
line in the "killing the control plane" test.

**Change the stale threshold or poll interval.** `staleAfterMs` and `intervalMs` are
constructor options; wire them to env in `createApp`'s default and document in `.env.example`.
Keep `staleAfterMs` well above `intervalMs` or every missed poll pages.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | control plane | Routes and the generation counter |
| `CONTROL_PLANE_URL` | data plane only | When set, this process pulls snapshots over HTTP from that base URL and its own `/api/control/*` is unused |
| `PORT` | no | Default `8000` |

**Modes.** Unset `CONTROL_PLANE_URL`: one process, control and data plane together, notify is a
function call. Set it: this process is a data plane; it still needs `DATABASE_URL` only because
`db.ts` builds a pool at import (it will not be queried).

**Scaling knobs.** Data planes scale horizontally with no coordination — each pulls its own
snapshot. `lruSize` (10 000 paths), `intervalMs` (5s), `staleAfterMs` (30s) are constructor
options. The control plane is a single writer through one Postgres row lock; that is the
throughput ceiling for writes, which is fine — route changes are rare by nature.

**Per-replica today.** The snapshot and the LRU. Both are meant to be per-replica; that is the
design, not a gap. Two data planes may be a generation apart for up to `intervalMs`.

**Failure modes.**

| What fails | What the user sees |
|---|---|
| Control plane down | `/api/resolve` keeps answering from the last snapshot; `/api/status` shows `last_error: "ECONNREFUSED"`, `age_ms` climbing, `stale: true` after 30s; `dataplane_snapshot_stale 1` |
| Control plane back | Next poll is a `200` or `304`; `stale` clears; new routes land |
| Data plane restarted while control plane is down | Empty snapshot, `stale: true` from the start (`lastPullAt === 0`), every resolve `404` — see Ceilings |
| A route with a bad prefix | `400` at the control plane; nothing changes; no generation consumed |
| Postgres down | Control-plane routes `500`; data planes unaffected until their next pull, which records the error |

**What to watch.** `dataplane_snapshot_stale` (the alert), `dataplane_snapshot_age_seconds`
(how far behind), `dataplane_snapshot_generation` across replicas (skew), `dataplane_route_computes_total`
against request rate (LRU effectiveness), `dataplane_lru_entries` against `lruSize`.

## Ceilings

- **A restarted data plane starts empty.** No snapshot on disk; until the first successful pull
  it answers `404`. Upgrade: write the last snapshot to a file on change and load it at start.
- **Push across processes is a route with no caller.** `/api/notify` exists; the control plane
  does not know its data planes. Propagation is the 5s poll. Upgrade: the push recipe above.
- **Nothing proxies.** `/api/resolve` returns the decision; `timeout_ms` and `headers` are
  stored and unused. Upgrade: the proxy recipe.
- **State-of-the-world only.** Every change ships the full route table. Fine to thousands of
  routes; beyond that, delta snapshots keyed by name and generation.
- **The LRU key is the path.** Adding host or header matching without changing the key serves
  wrong answers from cache.
- **`/api/control/*` has no auth.** Anyone who can reach the control plane can reroute traffic.
- **No ACK/NACK.** The control plane cannot tell which data planes have which generation;
  `/api/status` per replica is the only view.
- **`db.ts` opens a pool even in data-plane mode.**
- **`/api/health/ready` returns `db: "ok"` without asking the database**, and does not consider
  `stale` — a data plane with no snapshot reports ready.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const DP_AGENTS: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run and test it.

## Run

    make demo         # postgres + redis, migrate, seed, backend on :8000 (both planes), page on :3000
    make check        # the gate: typecheck both halves, bun test the backend — no database needed
    make backend      # API only, with reload

Two-process mode: start a second backend with `CONTROL_PLANE_URL=http://localhost:8000 PORT=8001
bun run dev` in `backend/`; it pulls from the first and serves `/api/resolve` on :8001.

## Routes

Control plane — write routes:

    curl -X PUT localhost:8000/api/control/routes/api -H 'content-type: application/json' \
      -d '{"prefix":"/api","upstream":"http://api:8000","timeout_ms":3000,"headers":{"x-tenant":"a"}}'
    # {"name":"api","generation":1}
    curl -X PUT localhost:8000/api/control/routes/web -H 'content-type: application/json' \
      -d '{"prefix":"/","upstream":"http://web:3000"}'
    # {"name":"web","generation":2}
    curl -X PUT localhost:8000/api/control/routes/bad -H 'content-type: application/json' -d '{"prefix":"nope","upstream":"x"}'
    # 400 {"error":{"message":"Invalid input: must start with \"/\"","code":"invalid"}}
    curl -X DELETE localhost:8000/api/control/routes/web
    # {"generation":3}
    curl -X DELETE localhost:8000/api/control/routes/none
    # 404 {"error":{"message":"no such route","code":"not_found"}}

Control plane — snapshots:

    curl localhost:8000/api/control/routes
    # {"generation":3,"routes":[{"name":"api","prefix":"/api","upstream":"http://api:8000","timeout_ms":3000,"headers":{"x-tenant":"a"}}]}
    curl -i 'localhost:8000/api/control/snapshot?since=3'     # HTTP/1.1 304
    curl 'localhost:8000/api/control/snapshot?since=2'        # 200, the snapshot above

Data plane:

    curl 'localhost:8000/api/resolve?path=/api/users'
    # {"path":"/api/users","route":{"name":"api",…},"generation":3,"stale":false}
    curl 'localhost:8000/api/resolve?path=/nothing'
    # 404 {"error":{"message":"no route","code":"not_found"},"generation":3}
    curl -X POST localhost:8000/api/notify -H 'content-type: application/json' -d '{"generation":3}'
    # {"ok":true,"generation":3}
    curl localhost:8000/api/status
    # {"generation":3,"routes":1,"stale":false,"age_ms":1200,"last_error":null}
    curl localhost:8000/metrics
    # dataplane_snapshot_generation 3
    # dataplane_snapshot_age_seconds 1.200
    # dataplane_snapshot_stale 0
    # dataplane_lru_entries 2
    # dataplane_route_computes_total 2

Prove the property: stop the :8000 process while a :8001 data plane is running; `/api/resolve`
on :8001 keeps answering, `/api/status` shows `last_error` and, after 30s, `stale: true`.

## How the tests are built

`backend/src/app.test.ts` never opens a socket or a database.

- `createApp(memoryStore(), { pull })` builds one app; `pull` is the only link a data plane has
  to a control plane. A control plane under test gets `pull: async () => null`.
- Two apps in one test: `cp` (control plane) and `dp` (data plane) where `dp`'s `pull` calls
  `cp.request("/api/control/snapshot?since=…")`. `pulls` records every `since` so single-flight
  and "notify is a hint" are asserted as an array.
- Time is injected: `now: () => clock` and `staleAfterMs`; the "control plane dies" test
  advances `clock` by 5s twenty times with `alive = false` and asserts `age_ms: 100_000`.
- `app.plane` is exposed for `sync()`, `notify()` and `computes`; the poll timer is only started
  by `app.plane.start()` in the module's default export, never in tests.

What each test pins: generation increments and `304`; the request path not touching the
control plane, push, single-flight, stale notify; serving through an outage and the gauges;
the LRU's single-flight, bound, eviction order and flush on a new snapshot.

## Adding a test

Use two `createApp` instances with a `pull` closure between them; drive writes through
`cp.request`, sync with `await dp.plane.sync()`, and assert through `dp.request`. Never call
`start()` in a test — the timer would keep the process alive. For time, pass `now` and step a
number. For the LRU, instantiate `new Lru<T>(max)` directly.
"##;

const DP_README: &str = r##"# {{NAME}}

A control plane and a data plane, modelled on Envoy's xDS: desired routes live in Postgres with a
generation counter; data planes hold a snapshot, answer every request from it, and survive the
control plane being gone — visibly.

## What you get

- A control-plane API to write routes; every write advances one generation atomically with the
  row.
- Snapshot pulls by generation (`?since=N`, `304` when unchanged) and push hints
  (`POST /api/notify`), single-flight, with a poll loop as the backstop.
- A data plane whose request path never awaits the control plane: longest-prefix resolve over
  a local snapshot through a bounded, single-flight LRU.
- A stale gauge that tells the truth: `dataplane_snapshot_stale`, `_age_seconds`,
  `_generation` in Prometheus text at `/metrics`, and the same in JSON at `/api/status`.
- One process by default; two deployments by setting `CONTROL_PLANE_URL`.
- Routes: `PUT/DELETE /api/control/routes/:name`, `GET /api/control/routes`,
  `GET /api/control/snapshot`, `GET /api/resolve`, `POST /api/notify`, `GET /api/status`,
  `GET /metrics`.
- A page showing desired state beside served state, red when the snapshot is stale.
- Tests that run two planes in-process with an injected clock; migrations; Docker and kustomize
  manifests.

## Five minutes

    make demo

Then:

    curl -X PUT localhost:8000/api/control/routes/api -H 'content-type: application/json' \
      -d '{"prefix":"/api","upstream":"http://api:8000"}'
    # {"name":"api","generation":1}

    curl 'localhost:8000/api/resolve?path=/api/users'
    # {"path":"/api/users","route":{"name":"api","prefix":"/api","upstream":"http://api:8000","timeout_ms":5000,"headers":{}},"generation":1,"stale":false}

    curl localhost:8000/metrics
    # dataplane_snapshot_generation 1
    # dataplane_snapshot_stale 0
    # …

    # a second process that is only a data plane
    (cd backend && CONTROL_PLANE_URL=http://localhost:8000 PORT=8001 bun run dev) &
    curl 'localhost:8001/api/resolve?path=/api/users'      # same answer, pulled over HTTP

Now stop the :8000 process. `:8001/api/resolve` keeps answering; `:8001/api/status` shows the
error and, after 30 seconds, `"stale":true`. Start :8000 again and it clears.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not check the database or the snapshot) |
| GET | `/api/control/routes` | none | The full snapshot: generation and routes |
| GET | `/api/control/snapshot?since=N` | none | The snapshot if newer than N, else `304` |
| PUT | `/api/control/routes/:name` | none | Create or replace a route; `{ name, generation }`; `400` on a bad body |
| DELETE | `/api/control/routes/:name` | none | Remove a route; `{ generation }`; `404` |
| GET | `/api/resolve?path=` | none | Longest-prefix match from the local snapshot; `404` with the generation when none |
| POST | `/api/notify` | none | Hint that generation N exists; pulls if newer |
| GET | `/api/status` | none | Snapshot generation, route count, stale, age, last error |
| GET | `/metrics` | none | Prometheus text |

No route is authenticated. The control-plane routes reroute traffic; put them behind ingress
auth before exposing them.

## Compared with Envoy xDS

**Same shapes, so their docs transfer**

- State-of-the-world snapshots keyed by a version (`generation` ≈ `version_info`); a request
  for a version the server already has yields nothing new (`304` ≈ no response).
- The split: control plane owns desired state, data plane serves from local state, and the two
  fail independently.
- Push-and-pull: a notify prompts a fetch; polling is the fallback.
- Longest-prefix route matching; the snapshot is atomically replaced, never mutated.
- Prometheus metrics for snapshot version and staleness.

**Better here**

- The whole protocol is ~150 lines of TypeScript you can read, not a gRPC streaming state
  machine with ADS ordering rules; the data plane is a class with a `sync()` you can call in a
  test.
- The outage is a test: "killing the control plane: the data plane keeps serving and the
  stale gauge says so" runs in milliseconds with an injected clock.
- Typed from the route schema to the page; the client is built from the API's route type.
- The generation is a Postgres row lock: no version drift between resource types, no
  ACK/NACK bookkeeping.
- One binary is both planes until you need two; splitting is an env var and a manifest.

**Not here yet**

- No proxying. `/api/resolve` returns the decision; nothing forwards bytes, so `timeout_ms`
  and `headers` are stored and unused.
- No delta/incremental xDS; every change ships the full table.
- No ACK/NACK, no per-data-plane view from the control plane, no node identity.
- No push across processes without adding a caller for `/api/notify`; propagation is the 5s
  poll.
- No snapshot persisted on the data plane; a restart during a control-plane outage serves
  nothing.
- No host, header, method or weighted matching; prefix only.
- No TLS/mTLS between planes, no auth on `/api/control/*`.
- No upstream health checking, retries, circuit breaking or load balancing — those are the
  proxy's job and there is no proxy.
- No multi-tenancy or multiple resource types (routes only).

## Production

- **Environments.** Control plane: `DATABASE_URL`. Data plane: `CONTROL_PLANE_URL` (plus
  `DATABASE_URL`, unused but required by `db.ts`). `backend/.env` is ignored; `backend/.env.age`
  is committed and decrypted by CI.
- **Scaling.** Data planes scale by count; no coordination. The control plane is a single
  writer by design. Snapshot size × data planes × (1/5s) is the control plane's read load.
- **Probes.** The stack's `/api/health*`. Consider readiness = "has a snapshot"
  (`/api/status.generation > 0`) for data planes so a fresh pod does not take traffic empty.
- **Migrations.** `backend/migrations/0002_dataplane.sql`; `make migrate` locally, the migrate
  init container in the cluster.
- **Kubernetes.** `k8s/base` + overlays for one backend and the page. For two planes, add a
  second Deployment with `CONTROL_PLANE_URL` set to the first's Service.
- **What pages you.** `dataplane_snapshot_stale == 1` on any replica for more than a minute;
  `max(generation) - min(generation)` across data planes above 1 for longer than two poll
  intervals; `dataplane_route_computes_total` growing at request rate (the LRU is thrashing:
  raise `lruSize` or check the key).

## Roadmap

1. Persist the snapshot on the data plane so a restart during an outage still serves.
2. A registry of data planes and a push after every write.
3. The proxy: forward with `timeout_ms` and `headers`.
4. Readiness that requires a snapshot.
5. Auth on `/api/control/*`.
6. Host and header matching, with the LRU key extended to match.
"##;

const DP_REVIEWER: &str = r##"---
name: plane-separation
description: Run on any change to backend/src/dataplane.ts, the resolve or control routes, the store's generation handling, or the LRU. Reads the change as someone who has watched a data plane fall over because the control plane did, or serve a route that was deleted an hour ago, and reports what will do that here.
tools: Read, Grep, Glob, Bash
---

You review the separation between the control plane and the data plane. The property is: the
data plane answers from local state and only local state, and every way it can be behind is
measured. Report only what breaks, each as `path:line — what — the sequence that triggers it —
the fix`.

Check:
1. **Nothing on the request path awaits `pull`, the store, or the network.** Read
   `DataPlane.resolve` and the `/api/resolve` handler; the only `await` is the LRU. A change that
   "falls back to the control plane on a miss" makes the data plane fail with it.
2. **Generation and rows change in one transaction** in `pgStore.put` and `remove`. Two
   statements outside `db.begin` let a data plane read generation N with N−1's rows.
3. **The snapshot is replaced only when strictly newer**, and never mutated in place. A
   `push`/`splice` on `snapshot.routes` is visible mid-update to concurrent resolves.
4. **The LRU is cleared on every snapshot replacement**, in `sync`, before anything can resolve
   against the new one. A cache that outlives the snapshot serves deleted routes.
5. **The LRU key covers everything the match reads.** Today `path`; a matcher that reads host
   or a header without changing the key is wrong from cache.
6. **`notify` pulls only when the hinted generation is newer, and never installs it.** A notify
   that sets `snapshot.generation` skips the rows and makes the next poll a `304` forever.
7. **`sync` is single-flight** (`this.syncing`), and the `finally` clears it on error too. A
   missing `finally` wedges every future sync after one failure.
8. **Errors set `lastError` and keep the snapshot.** A throw out of `sync` is an unhandled
   rejection in the timer callback.
9. **A `304` updates `lastPullAt`.** Staleness is "have we heard from the control plane", not
   "did we get new data"; a change that only stamps on a `200` pages during every quiet hour.
10. **`stale` is derived from `lastPullAt` and the injected `now`**; `lastPullAt === 0` is
    stale. No wall-clock call outside `this.now`.
11. **Longest-prefix order comes from the store** (both implementations sort by prefix length
    desc, then name), and `resolve` takes the first match. A store that stops sorting makes
    `/` shadow `/api`.
12. **`start()` is called only in the module default export**, never in `createApp` or a test,
    and the timer is `unref`'d.
13. **Route validation is the zod schema and it runs before `store.put`.** `prefix` starts with
    `/`, `upstream` is a URL, `timeout_ms` is bounded. Unknown fields must not reach the row.
14. **Metrics keep their names.** `dataplane_snapshot_stale`, `_age_seconds`, `_generation`,
    `dataplane_lru_entries`, `dataplane_route_computes_total` are what the alerts read; a rename
    is a silent alert deletion.
15. **`CONTROL_PLANE_URL` set means the in-process notify is not registered** and `httpPull` is
    the `Pull`; a change that registers both makes a data plane read its own empty database.

End with one line: `plane-separation: N findings`, and if 0, which of the above you checked.
"##;
