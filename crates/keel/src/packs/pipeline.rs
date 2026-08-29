//! pipeline: data pipeline, like Airflow / dbt ══════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", PIPELINE_APP.into()),
        ("backend/src/store.ts", PIPELINE_STORE.into()),
        ("backend/src/transforms.ts", PIPELINE_TRANSFORMS.into()),
        ("backend/src/worker.ts", PIPELINE_WORKER.into()),
        ("backend/src/seed.ts", PIPELINE_SEED.into()),
        ("backend/src/app.test.ts", PIPELINE_TEST.into()),
        ("backend/migrations/0002_pipeline.sql", PIPELINE_SQL.into()),
        ("frontend/app/page.tsx", f(PIPELINE_PAGE)),
    ]
}

const PIPELINE_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { rawEvent, normalize, dayRange } from "./transforms";
import { transform, backfill, JOB } from "./worker";

// Airflow's and dbt's surface, cut down to what an app needs: raw events in (POST /api/ingest,
// batched, idempotent by batch id), pure models over them (transforms.ts), a scheduled
// incremental run with a watermark and a backfill by date range (worker.ts), and the mart
// queryable (GET /api/metrics). Raw rows are never rewritten — a wrong transform is fixed by
// changing the function and backfilling.

const ingestBody = z.object({ batch_id: z.string().min(1).max(200).optional(), events: z.array(rawEvent).min(1).max(1000) });
const DAY = z.string().regex(/^\d{4}-\d{2}-\d{2}$/);
const range = z.object({ from: DAY, to: DAY, name: z.string().optional() }).refine((r) => r.from <= r.to && dayRange(r.from, r.to).length <= 366, "from must not be after to, and at most a year");

export function createApp(store: Store, clock: () => Date = () => new Date()) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // Append-only. The batch id is the idempotency key; omit it and every call is a new batch.
    .post("/api/ingest", async (c) => {
      const p = ingestBody.safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: "invalid batch", code: "invalid", issues: p.error.flatten() } }, 400);
      const now = clock();
      const batch_id = p.data.batch_id ?? crypto.randomUUID();
      const fresh = await store.ingest(batch_id, p.data.events.map((e) => normalize(e, now)));
      return c.json({ batch_id, accepted: fresh ? p.data.events.length : 0, duplicate: !fresh }, 202);
    })

    // The mart. Defaults to the last seven days.
    .get("/api/metrics", async (c) => {
      const today = clock().toISOString().slice(0, 10);
      const week = new Date(clock()); week.setUTCDate(week.getUTCDate() - 6);
      const p = range.safeParse({ from: c.req.query("from") ?? week.toISOString().slice(0, 10), to: c.req.query("to") ?? today, name: c.req.query("name") });
      if (!p.success) return c.json({ error: { message: p.error.issues[0].message, code: "invalid" } }, 400);
      return c.json({ from: p.data.from, to: p.data.to, metrics: await store.metrics(p.data.from, p.data.to, p.data.name) });
    })

    // Operate the pipeline over HTTP too: where it is, run the incremental job now, backfill a range.
    .get("/api/pipeline", async (c) => c.json({ job: JOB, watermark: await store.watermark(JOB) }))
    .post("/api/pipeline/run", async (c) => c.json(await transform(store)))
    .post("/api/pipeline/backfill", async (c) => {
      const p = range.safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues[0].message, code: "invalid" } }, 400);
      return c.json(await backfill(store, p.data.from, p.data.to));
    });
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const PIPELINE_STORE: &str = r##"import { db } from "./db";
import type { DailyMetric, Event } from "./transforms";

export interface Store {
  /// Append a batch. Returns false when the batch id was already accepted — the client's retry.
  ingest(batchId: string, events: Omit<Event, "id">[]): Promise<boolean>;
  /// Events with id above the watermark, oldest first, bounded.
  after(id: number, limit: number): Promise<Event[]>;
  /// Every event for a set of days — the transform recomputes whole days.
  forDays(days: string[]): Promise<Event[]>;
  upsertDaily(rows: DailyMetric[], days: string[]): Promise<void>;
  watermark(job: string): Promise<number>;
  setWatermark(job: string, value: number): Promise<void>;
  metrics(from: string, to: string, name?: string): Promise<DailyMetric[]>;
}

const DAY = /^\d{4}-\d{2}-\d{2}$/;

export function pgStore(): Store {
  return {
    ingest: (batchId, events) => db.begin(async (tx) => {
      const claimed = await tx`insert into ingest_batches (id, events) values (${batchId}, ${events.length}) on conflict (id) do nothing returning id`;
      if (claimed.count === 0) return false;
      // One partition per day, made on first sight. The day is validated (DAY) so the identifier
      // is safe to splice; DDL cannot take a parameter.
      for (const day of new Set(events.map((e) => e.day))) {
        if (!DAY.test(day)) throw new Error(`bad day ${day}`);
        const next = new Date(day + "T00:00:00Z"); next.setUTCDate(next.getUTCDate() + 1);
        await tx.unsafe(`create table if not exists events_${day.replaceAll("-", "_")} partition of events for values from ('${day}') to ('${next.toISOString().slice(0, 10)}')`);
      }
      for (let i = 0; i < events.length; i += 500) {
        const rows = events.slice(i, i + 500).map((e) => ({ day: e.day, name: e.name, user_id: e.user_id, value: e.value, props: JSON.stringify(e.props), occurred_at: e.occurred_at }));
        await tx`insert into events ${tx(rows, "day", "name", "user_id", "value", "props", "occurred_at")}`;
      }
      return true;
    }),
    after: (id, limit) => db<Event[]>`select id::int, day::text, name, user_id, value, props, occurred_at::text from events where id > ${id} order by id limit ${limit}`,
    forDays: (days) => (days.length ? db<Event[]>`select id::int, day::text, name, user_id, value, props, occurred_at::text from events where day = any(${days}::date[]) order by id` : Promise.resolve([])),
    upsertDaily: (rows, days) => db.begin(async (tx) => {
      // Names that vanished from a day (after a purge, say) must not linger with stale numbers.
      if (days.length) await tx`delete from daily_metrics where day = any(${days}::date[])`;
      if (rows.length) await tx`insert into daily_metrics ${tx(rows, "day", "name", "count", "sum", "users")}`;
    }),
    watermark: async (job) => Number((await db<{ value: string }[]>`select value from watermarks where job = ${job}`)[0]?.value ?? 0),
    setWatermark: async (job, value) => { await db`insert into watermarks (job, value) values (${job}, ${value}) on conflict (job) do update set value = excluded.value, updated_at = now()`; },
    metrics: async (from, to, name) => db<DailyMetric[]>`select day::text, name, count::int, sum, users::int from daily_metrics
      where day between ${from} and ${to} ${name ? db`and name = ${name}` : db``} order by day, name`,
  };
}

export function memoryStore(): Store {
  const batches = new Set<string>(); const events: Event[] = []; const daily = new Map<string, DailyMetric>(); const marks = new Map<string, number>();
  return {
    ingest: async (batchId, rows) => { if (batches.has(batchId)) return false; batches.add(batchId); for (const r of rows) events.push({ ...r, id: events.length + 1 }); return true; },
    after: async (id, limit) => events.filter((e) => e.id > id).slice(0, limit),
    forDays: async (days) => events.filter((e) => days.includes(e.day)),
    upsertDaily: async (rows, days) => { for (const k of [...daily.keys()]) if (days.includes(k.slice(0, 10))) daily.delete(k); for (const r of rows) daily.set(`${r.day} ${r.name}`, r); },
    watermark: async (job) => marks.get(job) ?? 0,
    setWatermark: async (job, v) => { marks.set(job, v); },
    metrics: async (from, to, name) => [...daily.values()].filter((m) => m.day >= from && m.day <= to && (!name || m.name === name)).sort((a, b) => a.day.localeCompare(b.day) || a.name.localeCompare(b.name)),
  };
}
"##;

const PIPELINE_TRANSFORMS: &str = r##"import { z } from "zod";

// The pure half of the pipeline, dbt-style: models are functions of their inputs and nothing
// else. No clock, no database, no I/O — which is why they are the part with the most tests.

export const rawEvent = z.object({
  name: z.string().min(1).max(100),
  user_id: z.string().max(200).optional(),
  value: z.number().finite().default(0),
  occurred_at: z.string().datetime({ offset: true }).optional(),
  props: z.record(z.unknown()).default({}),
});
export type RawEvent = z.infer<typeof rawEvent>;

export type Event = { id: number; day: string; name: string; user_id: string | null; value: number; props: Record<string, unknown>; occurred_at: string };
export type DailyMetric = { day: string; name: string; count: number; sum: number; users: number };

export const dayOf = (iso: string) => iso.slice(0, 10);

// Staging: one shape for every source. Names are normalised so "Page View" and "page_view"
// aggregate together; the day is derived once, here, and never recomputed downstream.
export function normalize(raw: RawEvent, now: Date): Omit<Event, "id"> {
  const occurred_at = raw.occurred_at ? new Date(raw.occurred_at).toISOString() : now.toISOString();
  return {
    day: dayOf(occurred_at),
    name: raw.name.trim().toLowerCase().replace(/[\s-]+/g, "_"),
    user_id: raw.user_id ?? null,
    value: raw.value,
    props: raw.props,
    occurred_at,
  };
}

// The mart: per day and name — count, sum of value, distinct users. Sorted so output is stable.
export function aggregate(events: Event[]): DailyMetric[] {
  const groups = new Map<string, { m: DailyMetric; users: Set<string> }>();
  for (const e of events) {
    const key = `${e.day} ${e.name}`;
    let g = groups.get(key);
    if (!g) { g = { m: { day: e.day, name: e.name, count: 0, sum: 0, users: 0 }, users: new Set() }; groups.set(key, g); }
    g.m.count++;
    g.m.sum += e.value;
    if (e.user_id) g.users.add(e.user_id);
  }
  return [...groups.values()]
    .map(({ m, users }) => ({ ...m, users: users.size }))
    .sort((a, b) => a.day.localeCompare(b.day) || a.name.localeCompare(b.name));
}

// Every day in [from, to], inclusive, as YYYY-MM-DD. Used by backfill and by the range check.
export function dayRange(from: string, to: string): string[] {
  const out: string[] = [];
  for (let d = new Date(from + "T00:00:00Z"); d.toISOString().slice(0, 10) <= to; d.setUTCDate(d.getUTCDate() + 1)) out.push(d.toISOString().slice(0, 10));
  return out;
}
"##;

const PIPELINE_WORKER: &str = r##"import { pgStore, type Store } from "./store";
import { aggregate, dayRange } from "./transforms";

// The scheduled half. `transform` is Airflow's incremental run: read everything above the
// watermark, recompute the days those events touch, advance the watermark — all idempotent, so
// a crash between steps costs a repeat, never a gap. `backfill` is the same computation pointed
// at a date range, ignoring the watermark.

export const JOB = "daily_metrics";
const BATCH = 10_000;

export async function transform(store: Store): Promise<{ processed: number; days: string[] }> {
  let mark = await store.watermark(JOB);
  const touched = new Set<string>(); let processed = 0;
  for (;;) {
    const fresh = await store.after(mark, BATCH);
    if (fresh.length === 0) break;
    for (const e of fresh) touched.add(e.day);
    processed += fresh.length;
    mark = fresh[fresh.length - 1].id;
    if (fresh.length < BATCH) break;
  }
  const days = [...touched].sort();
  if (days.length) await store.upsertDaily(aggregate(await store.forDays(days)), days);
  // The watermark moves only after the aggregates are written: a crash before this line re-runs
  // the same days, which is harmless; a watermark ahead of the data would lose them for good.
  await store.setWatermark(JOB, mark);
  return { processed, days };
}

export async function backfill(store: Store, from: string, to: string): Promise<{ days: string[]; rows: number }> {
  const days = dayRange(from, to);
  const rows = aggregate(await store.forDays(days));
  await store.upsertDaily(rows, days);
  return { days, rows: rows.length };
}

if (import.meta.main) {
  const store = pgStore();
  const [cmd, from, to] = process.argv.slice(2);
  if (cmd === "backfill") {
    if (!from || !to) { console.error("usage: bun src/worker.ts backfill YYYY-MM-DD YYYY-MM-DD"); process.exit(2); }
    console.log(JSON.stringify({ level: "info", msg: "backfill", ...(await backfill(store, from, to)) }));
    process.exit(0);
  }
  // ponytail: one replica runs this loop; make it a k8s CronJob (or take a lease row) before adding a second.
  const every = Number(process.env.TRANSFORM_INTERVAL_MS ?? 60_000);
  let stopping = false;
  process.on("SIGTERM", () => { stopping = true; });
  while (!stopping) {
    const r = await transform(store);
    if (r.processed) console.log(JSON.stringify({ level: "info", msg: "transform", ...r }));
    await new Promise((r) => setTimeout(r, every));
  }
}
"##;

const PIPELINE_SEED: &str = r##"import { pgStore, type Store } from "./store";
import { normalize } from "./transforms";
import { transform } from "./worker";

// Seven days of sample traffic so the first screen has numbers on it. Deterministic: a seeded
// generator and an injected clock, so the tests and two developers get the same rows. The batch
// id is the day, so running it twice inserts nothing the second time.
export async function seed(store: Store, now: Date): Promise<boolean> {
  let s = 42; const rand = () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  const names = ["page_view", "signup", "purchase"];
  const events = [];
  for (let d = 6; d >= 0; d--) {
    const day = new Date(now); day.setUTCDate(day.getUTCDate() - d);
    const n = 40 + Math.floor(rand() * 40);
    for (let i = 0; i < n; i++) {
      const name = names[Math.floor(rand() * rand() * names.length)];
      const at = new Date(day); at.setUTCHours(Math.floor(rand() * 24), Math.floor(rand() * 60), 0, 0);
      events.push(normalize({ name, user_id: `u${Math.floor(rand() * 25)}`, value: name === "purchase" ? Math.round(rand() * 9000) / 100 : 0, occurred_at: at.toISOString(), props: {} }, now));
    }
  }
  const fresh = await store.ingest(`seed-${now.toISOString().slice(0, 10)}`, events);
  if (fresh) await transform(store);
  return fresh;
}

if (import.meta.main) {
  const { db } = await import("./db");
  const fresh = await seed(pgStore(), new Date());
  console.log(fresh ? "seeded 7 days of events and their aggregates" : "already seeded today");
  await db.end();
}
"##;

const PIPELINE_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { seed } from "./seed";
import { aggregate, normalize, dayRange } from "./transforms";
import { transform, JOB } from "./worker";

const NOW = new Date("2026-03-10T12:00:00Z");
const post = (app: ReturnType<typeof createApp>, path: string, body: unknown) =>
  app.request(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

describe("pipeline", () => {
  test("transforms are pure: names normalise, days derive from occurred_at, aggregates are stable", () => {
    const a = normalize({ name: " Page View ", value: 0, props: {}, occurred_at: "2026-03-09T23:30:00+02:00" }, NOW);
    expect(a.name).toBe("page_view");
    expect(a.day).toBe("2026-03-09"); // 23:30+02:00 is 21:30Z, same day
    const b = normalize({ name: "page-view", user_id: "u1", value: 0, props: {} }, NOW);
    expect(b.day).toBe("2026-03-10");
    const rows = aggregate([
      { id: 1, ...a, user_id: "u1" }, { id: 2, ...a, user_id: "u1", value: 2 }, { id: 3, ...b }, { id: 4, ...b, name: "signup", value: 5 },
    ]);
    expect(rows).toEqual([
      { day: "2026-03-09", name: "page_view", count: 2, sum: 2, users: 1 },
      { day: "2026-03-10", name: "page_view", count: 1, sum: 0, users: 1 },
      { day: "2026-03-10", name: "signup", count: 1, sum: 5, users: 1 },
    ]);
    expect(dayRange("2026-02-27", "2026-03-02")).toEqual(["2026-02-27", "2026-02-28", "2026-03-01", "2026-03-02"]);
  });

  test("a batch is accepted once, however often it is sent; a bad batch is refused", async () => {
    const store = memoryStore(); const app = createApp(store, () => NOW);
    const body = { batch_id: "b1", events: [{ name: "signup", user_id: "u1" }, { name: "signup", user_id: "u2" }] };
    const first = await post(app, "/api/ingest", body);
    expect(first.status).toBe(202);
    expect(await first.json()).toEqual({ batch_id: "b1", accepted: 2, duplicate: false });
    const again = await post(app, "/api/ingest", body);
    expect((await again.json()).duplicate).toBe(true);
    expect((await store.after(0, 100)).length).toBe(2);
    expect((await post(app, "/api/ingest", { events: [] })).status).toBe(400);
    expect((await post(app, "/api/ingest", { events: [{ name: "x", occurred_at: "yesterday" }] })).status).toBe(400);
  });

  test("the watermark job folds in new events and recomputes only the days they touch", async () => {
    const store = memoryStore(); const app = createApp(store, () => NOW);
    await post(app, "/api/ingest", { events: [{ name: "signup", user_id: "u1", occurred_at: "2026-03-09T10:00:00Z" }, { name: "signup", user_id: "u2" }] });
    let r = await transform(store);
    expect(r).toEqual({ processed: 2, days: ["2026-03-09", "2026-03-10"] });
    expect(await store.watermark(JOB)).toBe(2);
    expect(await transform(store)).toEqual({ processed: 0, days: [] });
    // A late event for the 9th: only that day is recomputed, and whole — the aggregate is not incremented.
    await post(app, "/api/ingest", { events: [{ name: "signup", user_id: "u1", occurred_at: "2026-03-09T23:00:00Z" }] });
    r = await (await post(app, "/api/pipeline/run", {})).json();
    expect(r).toEqual({ processed: 1, days: ["2026-03-09"] });
    const res = await app.request("/api/metrics?from=2026-03-09&to=2026-03-10");
    expect(await res.json()).toEqual({ from: "2026-03-09", to: "2026-03-10", metrics: [
      { day: "2026-03-09", name: "signup", count: 2, sum: 0, users: 1 },
      { day: "2026-03-10", name: "signup", count: 1, sum: 0, users: 1 },
    ] });
  });

  test("backfill recomputes a range from raw events and leaves the watermark alone", async () => {
    const store = memoryStore(); const app = createApp(store, () => NOW);
    await post(app, "/api/ingest", { events: [{ name: "purchase", value: 10, occurred_at: "2026-03-01T10:00:00Z" }, { name: "purchase", value: 5, occurred_at: "2026-03-03T10:00:00Z" }] });
    const res = await post(app, "/api/pipeline/backfill", { from: "2026-03-01", to: "2026-03-03" });
    expect(await res.json()).toEqual({ days: ["2026-03-01", "2026-03-02", "2026-03-03"], rows: 2 });
    expect(await store.watermark(JOB)).toBe(0);
    const m = await (await app.request("/api/metrics?from=2026-03-01&to=2026-03-03&name=purchase")).json();
    expect(m.metrics.map((x: { sum: number }) => x.sum)).toEqual([10, 5]);
    expect((await post(app, "/api/pipeline/backfill", { from: "2026-03-03", to: "2026-03-01" })).status).toBe(400);
    expect((await app.request("/api/metrics?from=2020-01-01&to=2026-03-01")).status).toBe(400);
  });

  test("seed is deterministic under a fake clock and idempotent", async () => {
    const a = memoryStore(); const b = memoryStore();
    expect(await seed(a, NOW)).toBe(true);
    expect(await seed(b, NOW)).toBe(true);
    expect(await seed(a, NOW)).toBe(false);
    const ma = await a.metrics("2026-03-04", "2026-03-10"); const mb = await b.metrics("2026-03-04", "2026-03-10");
    expect(ma).toEqual(mb);
    expect(new Set(ma.map((m) => m.day)).size).toBe(7);
    expect(ma.some((m) => m.name === "purchase" && m.sum > 0)).toBe(true);
  });
});
"##;

const PIPELINE_SQL: &str = r##"-- Raw events, partitioned by day. A partition per day is what makes a backfill or a purge
-- cheap: one child table, not a delete over the whole history. Partitions are created by the
-- ingest path for each day it sees (create table if not exists ... partition of events).
create table if not exists events (
  id bigserial,
  day date not null,
  name text not null,
  user_id text,
  value double precision not null default 0,
  props jsonb not null default '{}',
  occurred_at timestamptz not null,
  primary key (day, id)
) partition by range (day);
create index if not exists events_id on events (id);

-- One row per accepted batch: the idempotency key for the ingest endpoint. A client that retries
-- after a timeout is answered from here, and its events are not stored twice.
create table if not exists ingest_batches (
  id text primary key,
  events int not null,
  received_at timestamptz not null default now()
);

-- What the transform produces: one row per day and event name, recomputed whole whenever the day
-- gets new events. Upserted, so re-running (or backfilling) a day is idempotent.
create table if not exists daily_metrics (
  day date not null,
  name text not null,
  count bigint not null,
  sum double precision not null,
  users bigint not null,
  computed_at timestamptz not null default now(),
  primary key (day, name)
);

-- The watermark: the highest event id the transform has folded in. Late events land above it
-- whatever day they belong to, so their day is recomputed on the next run.
create table if not exists watermarks (
  job text primary key,
  value bigint not null default 0,
  updated_at timestamptz not null default now()
);
"##;

const PIPELINE_PAGE: &str = r##"type Metric = { day: string; name: string; count: number; sum: number; users: number };

async function metrics(): Promise<{ from: string; to: string; metrics: Metric[] }> {
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/api/metrics`, { cache: "no-store" });
  return res.json();
}

// The mart, last seven days: one row per day and event name, with a bar so the shape is visible
// without a chart library. Numbers come from daily_metrics, never from the raw table.
export default async function Home() {
  const { from, to, metrics: rows } = await metrics();
  const max = Math.max(1, ...rows.map((m) => m.count));
  return (
    <main>
      <h1>{{NAME}} — pipeline</h1>
      <p>
        Ingest: <code>{`curl -X POST http://localhost:8000/api/ingest -H 'content-type: application/json' -d '{"batch_id":"b1","events":[{"name":"signup","user_id":"u1"},{"name":"purchase","user_id":"u1","value":19.9}]}'`}</code>
        {" "}then <code>curl -X POST http://localhost:8000/api/pipeline/run</code> (or <code>bun run worker</code> in backend/ to run it every minute).
        Backfill: <code>bun src/worker.ts backfill 2026-01-01 2026-01-31</code>.
      </p>
      <p>Daily metrics {from} to {to}</p>
      <table>
        <thead><tr><th>day</th><th>event</th><th>count</th><th>sum</th><th>users</th><th></th></tr></thead>
        <tbody>
          {rows.map((m) => (
            <tr key={m.day + m.name}>
              <td>{m.day}</td><td>{m.name}</td><td>{m.count}</td><td>{m.sum.toFixed(2)}</td><td>{m.users}</td>
              <td><span style={{ display: "inline-block", height: 10, width: Math.round((m.count / max) * 200), background: "#888" }} /></td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && <p>No aggregates yet. Run <code>bun run seed</code> in backend/.</p>}
    </main>
  );
}
"##;
