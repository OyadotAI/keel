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
        ("CLAUDE.md", f(PIPELINE_CLAUDE)),
        ("AGENTS.md", f(PIPELINE_AGENTS)),
        ("README.md", f(PIPELINE_README)),
        (
            ".claude/agents/pipeline-correctness.md",
            PIPELINE_REVIEWER.into(),
        ),
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

const PIPELINE_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is an event pipeline: raw events in over HTTP, pure transforms over them, a daily mart
out. It is modelled on the ingest half of Airbyte and the modelling half of dbt, cut down to what
one application needs — no connectors, no DAG, one incremental job with a watermark and a backfill
by date range. "Done" here means: a batch sent twice is stored once; a wrong transform is fixed by
changing the function and backfilling, never by editing rows; and `GET /api/metrics` answers from
`daily_metrics`, never from the raw table.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes: `/api/ingest`, `/api/metrics`, `/api/pipeline{,/run,/backfill}`. Validation with zod, the `range` rule (from ≤ to, ≤ 366 days). Exports `AppType`. |
| `backend/src/transforms.ts` | The pure half: `rawEvent` schema, `normalize` (staging), `aggregate` (the mart), `dayRange`. No I/O, no clock — the clock is passed in. |
| `backend/src/worker.ts` | The scheduled half: `transform` (watermark run), `backfill` (date range), the `bun src/worker.ts` loop and CLI. `JOB = "daily_metrics"`. |
| `backend/src/store.ts` | `Store` interface; `pgStore` (partitions, batches, upserts) and `memoryStore` (what the tests run on). |
| `backend/src/seed.ts` | Seven days of deterministic sample events, batch id `seed-<day>`, then one transform. |
| `backend/src/db.ts` | The `postgres` pool from `DATABASE_URL`. |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` in name order, once each. |
| `backend/migrations/0002_pipeline.sql` | `events` (partitioned by day), `ingest_batches`, `daily_metrics`, `watermarks`. |
| `backend/src/app.test.ts` | The five tests, all on `memoryStore()` with a fixed clock. |
| `frontend/app/page.tsx` | The mart for the last seven days as a table with bars. Reads `/api/metrics` only. |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client. |

The request path for an event, from `POST /api/ingest` to a number on the page:

1. `ingestBody` parses the batch: 1–1000 events, optional `batch_id` (a missing one becomes a fresh UUID, so the call is *not* idempotent — say so to clients).
2. `normalize(e, now)` stages each event: name lower-cased and `[\s-]+` → `_`, `occurred_at` defaulted to now and re-serialised as UTC, `day` derived once from it.
3. `store.ingest(batch_id, rows)` claims the batch id (`insert … on conflict do nothing returning id`); zero rows claimed means the batch was seen, and the reply is `{accepted: 0, duplicate: true}`.
4. For each distinct day in the batch, `create table if not exists events_<day> partition of events …` — the day passed the `DAY` regex, so the identifier is safe to splice. Rows are inserted 500 at a time. All of this is one transaction.
5. Later, `transform(store)` reads `after(watermark, 10000)` in pages, collects the days those events touch, and advances a local `mark` to the last id read.
6. `aggregate(forDays(days))` recomputes those days *whole* — count, sum of `value`, distinct `user_id` — and `upsertDaily` deletes the day's rows then inserts the new ones, in one transaction.
7. Only then `setWatermark(JOB, mark)`. A crash between 6 and 7 re-runs the same days next time; a crash between 5 and 6 costs nothing.
8. `GET /api/metrics?from&to&name` reads `daily_metrics`; the page renders it with a bar scaled to the largest count.

Data model:

| Table | Why the columns that matter |
|---|---|
| `events` | `primary key (day, id)` because a partitioned table's key must include the partition column; `id bigserial` is the watermark's cursor and `events_id` indexes it across partitions. `props jsonb` is stored, not aggregated. `occurred_at` is the truth; `day` is derived from it at staging so every downstream reader agrees. |
| `ingest_batches` | `id text primary key` is the idempotency key. `events int` is the count the client sent, for a retry to compare against. |
| `daily_metrics` | `primary key (day, name)`; `computed_at` says when the day was last rebuilt. Always written whole per day, never incremented. |
| `watermarks` | One row per job (`job text primary key`), `value` = highest event id folded in. Late events land above it whatever day they belong to. |

## Invariants

1. **Raw rows are never updated or deleted by code.** A wrong number is fixed by changing `transforms.ts` and running a backfill. The `Store` interface has no `updateEvent`; the only delete in `store.ts` is on `daily_metrics`. Guard: `app.test.ts` "backfill recomputes a range from raw events and leaves the watermark alone" — the raw events are re-read, not the mart.
2. **Every event that enters the store went through `normalize`.** Otherwise "Page View" and "page_view" are two metrics. `ingest` in `app.ts` maps `p.data.events.map((e) => normalize(e, now))` and nothing else calls `store.ingest` except `seed.ts`, which also normalises. Guard: "transforms are pure: names normalise, days derive from occurred_at, aggregates are stable".
3. **The batch id is claimed in the same transaction as the rows.** A claim committed before the rows would answer a retry with `duplicate: true` while the rows were still absent. `pgStore.ingest` is one `db.begin`. Guard: "a batch is accepted once, however often it is sent; a bad batch is refused" (memory store; the transactional shape in `pgStore` is by inspection — see the reviewer checklist).
4. **A day is recomputed whole, never incremented.** `upsertDaily(rows, days)` deletes every row for the day first, so an event name that vanished from a day (a purge, a bad batch removed by hand) does not linger. Guard: "the watermark job folds in new events and recomputes only the days they touch" — the late event for the 9th yields `count: 2`, not `1 + 1` applied twice.
5. **The watermark moves after the aggregates are written, never before.** `transform` calls `upsertDaily` and then `setWatermark`. Reversing the two lines turns a crash into a permanent gap. Guard: the same test asserts `watermark(JOB) === 2` after the run and `{processed: 0, days: []}` on the next.
6. **Backfill ignores and does not touch the watermark.** It is the same `aggregate` over `forDays(dayRange(from, to))`. Guard: "backfill … leaves the watermark alone" asserts `watermark(JOB) === 0` afterwards.
7. **`transforms.ts` imports nothing but `zod`.** No `db`, no `Date.now()`, no `process.env`. That is what lets the tests pin a day boundary (`23:30+02:00` is the 9th, not the 10th) without a database. Guard: the purity test; and a reviewer should refuse an import of `./db` or `./store` there.
8. **A day identifier is validated before it is spliced into DDL.** `pgStore.ingest` re-checks `DAY.test(day)` even though `normalize` produced it. Guard: by inspection — there is no test that exercises `pgStore`; the reviewer checklist has it.
9. **Ranges are bounded: at most 366 days, from ≤ to, at most 1000 events per batch, 10 000 events per transform page.** `range.refine` in `app.ts`, `ingestBody`, `BATCH` in `worker.ts`. Guard: the backfill test asserts 400 for `from > to` and for a six-year range on `/api/metrics`.
10. **The page reads the mart, never the raw table.** `frontend/app/page.tsx` calls `/api/metrics` only; there is no route that returns raw events. Guard: none automated — a route `GET /api/events` would need a limit, an index and a reason.
11. **Seed is deterministic and idempotent.** A seeded LCG and an injected clock, batch id `seed-<day>`. Guard: "seed is deterministic under a fake clock and idempotent".

## Extending it

**Add a metric (say, p95 of `value`).** `transforms.ts`: extend `DailyMetric` and `aggregate`. `0003_<name>.sql`: `alter table daily_metrics add column p95 double precision not null default 0` — additive, so the running version keeps working during the rollout. `store.ts`: add the column to `upsertDaily`'s column list and to `metrics`' select, in both stores. `page.tsx`: a column. Test: extend the expected rows in the purity test. Then `POST /api/pipeline/backfill` for the history you want the new number on — old days are not recomputed by themselves.

**Add a dimension (group by `props.country` as well as `name`).** Same files; the mart's key becomes `(day, name, country)`, which is a new primary key — do it as a new table (`daily_metrics_v2`), point `metrics` at it, backfill, then drop the old one in a later migration. Never rename in place.

**Add a second job (an hourly mart).** `worker.ts`: a second `JOB` constant and a `transformHourly` that shares `after`/`setWatermark` under its own job name — `watermarks` is keyed by job for exactly this. Add `upsertHourly`/`hourly` to `Store`, both implementations. A new table in `0003_`. Test: the hourly run advances only its own watermark.

**Add a source (a webhook from a payment provider).** Do not add a second ingest path. Add a route that verifies the provider's signature, maps its payload to `RawEvent[]`, and calls the same `store.ingest` with the provider's own event id as `batch_id` — their retries then dedupe for free. Test: the same payload twice yields `duplicate: true`.

**Add a filter to `/api/metrics` (by `user_id`).** The mart has no `user_id` column and must not grow one per user — that is the raw table's shape. Either add a mart keyed by user (a new table, see dimension above) or answer from `events` with a bounded range and say so in the route's comment. Test: the range bound still applies.

**Move the transform to a CronJob.** Keep `worker.ts` as is; in `k8s/base/` add a `CronJob` whose container runs `curl -X POST http://backend:8000/api/pipeline/run` every minute. The backend image already serves that route; the loop in `worker.ts` is for a laptop. See Operating it.

**Purge a day (a bad batch, a GDPR request).** `drop table events_2026_03_09` — one child table, no delete over history — then `POST /api/pipeline/backfill {"from":"2026-03-09","to":"2026-03-09"}` so the mart forgets it too. Add this as a `bun src/worker.ts purge <day>` subcommand if it happens twice; it does not exist today.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres. Default `postgres://app:app@localhost:5432/app` for `make dev`. |
| `PORT` | no | API port, default 8000. |
| `TRANSFORM_INTERVAL_MS` | no | How often the `bun src/worker.ts` loop runs `transform`; default 60000. |
| `API_URL` | frontend | Where the Next.js server reaches the API; default `http://127.0.0.1:8000`. |
| `REDIS_URL`, `SENTRY_DSN`, `POSTHOG_*` | no | From the stack; unused by this pack's code today. |

Scaling and replicas:

- The API is stateless; the HPA in `k8s/base/backend.yaml` runs 2–5 of it. Ingest under concurrency is safe: the batch claim is a row insert with `on conflict`, and partition creation is `if not exists`.
- **The transform must run once at a time.** Two runners both read above the watermark, both recompute the same days, and the last writer wins — harmless for the mart but wasted work, and two `setWatermark` calls can regress the mark. Today nothing enforces this: the `worker.ts` loop is one process on one machine, and the backend image (`bun build src/server.ts`) does not even contain it. In the cluster, drive `POST /api/pipeline/run` from one `CronJob` (`concurrencyPolicy: Forbid`) — that is the single runner.
- Per-replica today: nothing. Every piece of state — batches, events, marts, watermarks — is in Postgres. Nothing moves to Redis when you add replicas.
- Pool: `db.ts` uses `max: 10`; five replicas plus a CronJob is 60 connections against Postgres's default `max_connections = 100`.

Failure modes and what the user sees:

| Failure | Effect |
|---|---|
| Postgres down | `/api/ingest` and `/api/metrics` return 500; readiness still says `db: "ok"` (it does not check — fix that before relying on it). Clients should retry with the same `batch_id`. |
| Transform not running | Ingest succeeds, `/api/pipeline` shows a stale `watermark`, the page shows yesterday's numbers. Nothing pages. Alert on `now - max(events.occurred_at)` vs `watermarks.updated_at`. |
| Crash mid-transform | The next run redoes the same days. Idempotent by design (invariant 5). |
| A client retries without `batch_id` | Duplicate rows, double counts. The API cannot tell. Require the id in your client. |
| Late event (a phone syncing a day later) | Lands above the watermark, its day is recomputed on the next run. Correct, but the number for that day changes after people have seen it. |
| Out-of-order commit | A slow ingest transaction with a lower id than one already folded in commits after the run read past it. Those rows are below the watermark and never aggregated until a backfill. See Ceilings. |

What to watch: the `transform` log line (`{"level":"info","msg":"transform","processed":N,"days":[…]}`) — its absence is the alert; `GET /api/pipeline` for the watermark; row count of `ingest_batches` per hour; `pg_stat_user_tables` on `events_<day>` for partition bloat after a purge.

## Ceilings

- **One transform runner, unenforced** (`worker.ts`: `ponytail: one replica runs this loop`). Upgrade: a `CronJob` with `concurrencyPolicy: Forbid`, or a lease row (`select … for update skip locked`) in `watermarks`.
- **Out-of-order commits can be skipped by the watermark.** `bigserial` is assigned at insert, the watermark is the highest id read, and a transaction with a lower id can commit later. Upgrade: re-read a margin (`after(mark - 10_000)`), or mark rows with `received_at` and lag the watermark by a minute; either is a few lines in `transform`.
- **A day is recomputed from every one of its events in memory.** `forDays` loads them all; a day with millions of events will not fit. Upgrade: push `aggregate` into SQL (`insert into daily_metrics select day, name, count(*), sum(value), count(distinct user_id) …`) for `pgStore` and keep the TS version for the memory store and the tests.
- **Backfill is one transaction per range** up to 366 days. Upgrade: loop days in chunks of seven inside `backfill`.
- **Partitions are created by the ingest path**, under the batch's transaction, one DDL per new day. Fine at a few days per batch; a backfill of years of history through `/api/ingest` will take a lock per day. Upgrade: pre-create partitions for the coming month from the CronJob.
- **The mart is count, sum, distinct users.** No percentiles, no funnels, no sessionisation. Each is a recipe above.
- **No connectors.** Sources push to `/api/ingest`; nothing polls Stripe or a database for you. Upgrade: a per-source route (recipe above) or Airbyte in front of it writing to the same endpoint.
- **Readiness does not check Postgres.** `/api/health/ready` returns a literal. Upgrade: `await db\`select 1\`` with a two-second timeout.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const PIPELINE_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the architecture. This is how to run and test it.

## Run

    make demo         # postgres + redis, migrate, seed 7 days, API on :8000, app on :3000
    make dev          # just postgres + redis
    make backend      # API with reload on :8000
    make frontend     # Next.js on :3000, /api proxied to :8000
    make check        # the gate: typecheck both halves, run backend tests

The transform, from `backend/`:

    bun run worker                              # transform every TRANSFORM_INTERVAL_MS (60s)
    bun src/worker.ts backfill 2026-03-01 2026-03-31
    curl -X POST localhost:8000/api/pipeline/run   # one run, over HTTP

`bun run seed` is idempotent per day (batch id `seed-YYYY-MM-DD`) and runs one transform afterwards.

## Routes, with bodies

Ingest a batch (idempotent by `batch_id`):

    curl -s -X POST localhost:8000/api/ingest -H 'content-type: application/json' -d '{
      "batch_id": "orders-2026-03-10-17",
      "events": [
        {"name": "Page View", "user_id": "u1", "occurred_at": "2026-03-10T17:02:11Z"},
        {"name": "purchase", "user_id": "u1", "value": 19.9, "props": {"sku": "A-1"}}
      ]}'
    # 202 {"batch_id":"orders-2026-03-10-17","accepted":2,"duplicate":false}
    # sent again: 202 {"batch_id":"orders-2026-03-10-17","accepted":0,"duplicate":true}
    # {"events":[]} or a bad occurred_at: 400 {"error":{"message":"invalid batch","code":"invalid","issues":{…}}}

Where the pipeline is, and run it:

    curl -s localhost:8000/api/pipeline
    # {"job":"daily_metrics","watermark":412}
    curl -s -X POST localhost:8000/api/pipeline/run
    # {"processed":2,"days":["2026-03-10"]}

Backfill a range (recomputes from raw events, watermark untouched):

    curl -s -X POST localhost:8000/api/pipeline/backfill -H 'content-type: application/json' \
      -d '{"from":"2026-03-01","to":"2026-03-10"}'
    # {"days":["2026-03-01",…,"2026-03-10"],"rows":27}
    # from > to, or more than 366 days: 400 {"error":{"message":"from must not be after to, and at most a year","code":"invalid"}}

Read the mart (defaults to the last seven days; `name` filters):

    curl -s 'localhost:8000/api/metrics?from=2026-03-09&to=2026-03-10&name=purchase'
    # {"from":"2026-03-09","to":"2026-03-10","metrics":[
    #   {"day":"2026-03-09","name":"purchase","count":11,"sum":402.13,"users":9},
    #   {"day":"2026-03-10","name":"purchase","count":1,"sum":19.9,"users":1}]}

Health: `GET /api/health` → `{"status":"ok"}`; `GET /api/health/ready` → `{"status":"ok","db":"ok"}` (a literal today).

## How the tests are built

`backend/src/app.test.ts` runs on `memoryStore()` — no Postgres, no Docker — with the clock fixed
at `NOW = 2026-03-10T12:00Z` through `createApp(store, () => NOW)` and `seed(store, NOW)`. The
pure functions (`normalize`, `aggregate`, `dayRange`) are tested directly; the routes through
`app.request`; the job through `transform(store)` and `POST /api/pipeline/run`. Because the memory
store implements the same `Store` interface, a route test is also a contract test for `pgStore`'s
signatures — but not for its SQL. `pgStore` is exercised by `make demo`, not by `bun test`.

To add a test: a `test(...)` in the `describe("pipeline")` block, its own `memoryStore()` (state
is per test), the fixed clock, and an assertion on the *whole* response with `toEqual` rather
than on one field — the shapes are small enough and a missing field is a real regression. For a
new transform, test the function on hand-built `Event[]` before testing the route.
"##;

const PIPELINE_README: &str = r##"# {{NAME}}

An event pipeline you own: batched, idempotent ingest over HTTP; pure TypeScript transforms; a
daily mart with an incremental watermark job and a backfill by date range; Postgres partitioned by
day. Modelled on Airbyte's ingest and dbt's models, without the connectors and the DAG.

## What you get

- `POST /api/ingest` — up to 1000 events per batch, deduplicated by `batch_id`, so a client can retry safely. Raw rows are append-only.
- `transforms.ts` — staging (`normalize`) and the mart (`aggregate`) as pure functions with no I/O, tested to the row.
- A watermark job (`transform`) that folds in new events and recomputes only the days they touch, whole; and a `backfill` that recomputes any range from raw events.
- `GET /api/metrics` — per day and event name: count, sum, distinct users.
- Postgres `events` partitioned by day, partitions created on first sight; a purge is `drop table`.
- Tests that run without a database (`memoryStore`), a typed client in the frontend built from the API's route types, a seven-day seed, Docker Compose locally and kustomize manifests for a cluster.

## Five minutes

    make demo

Then, in another shell:

    curl -s -X POST localhost:8000/api/ingest -H 'content-type: application/json' \
      -d '{"batch_id":"first","events":[{"name":"signup","user_id":"u1"},{"name":"purchase","user_id":"u1","value":19.9}]}'
    # {"batch_id":"first","accepted":2,"duplicate":false}

    curl -s -X POST localhost:8000/api/ingest -H 'content-type: application/json' \
      -d '{"batch_id":"first","events":[{"name":"signup","user_id":"u1"}]}'
    # {"batch_id":"first","accepted":0,"duplicate":true}      ← the retry stored nothing

    curl -s -X POST localhost:8000/api/pipeline/run
    # {"processed":2,"days":["2026-03-10"]}

    curl -s 'localhost:8000/api/metrics?name=purchase'
    # {"from":"2026-03-04","to":"2026-03-10","metrics":[…,{"day":"2026-03-10","name":"purchase","count":…,"sum":…,"users":…}]}

Open http://localhost:3000 for the same numbers as a table.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/ingest` | none | Append a batch of 1–1000 events; `batch_id` is the idempotency key. 202. |
| GET | `/api/metrics?from&to&name` | none | The mart for a day range (≤ 366 days; default last 7). |
| GET | `/api/pipeline` | none | The job name and its watermark. |
| POST | `/api/pipeline/run` | none | Run the incremental transform now. |
| POST | `/api/pipeline/backfill` | none | `{from,to}`: recompute a range from raw events. |
| GET | `/api/health`, `/api/health/ready` | none | Liveness; readiness (does not check the database yet). |

There is no authentication on any route. Put the API behind your ingress's auth or add a bearer check in `app.ts` before exposing it — the `auth` pack is the shape to copy.

## Compared with Airbyte and dbt

Same shapes, so their vocabulary and habits carry over:

- Raw → staging → mart, with the mart rebuilt from raw rather than patched (dbt's `table` materialisation, per day).
- An incremental model with a watermark, and a full-refresh (`backfill`) for when the model changes.
- Idempotent loads keyed by batch, so at-least-once delivery from a source is safe.
- Events partitioned by time; retention and purge by dropping partitions.

Better here, for a team that owns one application:

- The transforms are TypeScript functions in the same repository as the API and the page, typed end to end (`AppType` → `hc` client) and tested in milliseconds with no warehouse.
- One codebase, one deploy: no Airbyte server, no dbt Cloud, no scheduler to run.
- The watermark job is idempotent by construction and documented line by line; a crash costs a repeat, not a gap.
- k8s manifests with rolling updates, probes, an HPA and a migrate init container come with it.

Not here yet — Airbyte and dbt have these and this does not:

- **Connectors.** Nothing pulls from Stripe, Postgres, S3 or a SaaS API; sources push to `/api/ingest`.
- **A DAG.** One job, one mart. Dependencies between models are function calls, not a graph with lineage.
- **A scheduler.** The `bun src/worker.ts` loop is for a laptop; in production you run a `CronJob` that hits `/api/pipeline/run`, and the manifest is yours to add.
- **Schema evolution for sources.** `props` is stored as `jsonb`; nothing infers or tracks a schema.
- **Tests as data assertions** (dbt's `not_null`, `unique`, `accepted_values`) — here they are unit tests on functions.
- **Docs and lineage UI, incremental strategies (merge/delete+insert), snapshots, seeds from CSV, macros.**
- **Multi-tenancy, authentication, rate limits.**

## Production

- **Environments.** `backend/.env` (never committed) → `backend/.env.age` (committed) → `k8s/secrets.yaml` via `make k8s-secrets ENV=dev|prod`. Keys with `_DEV` win in the dev overlay.
- **Scaling.** The API is stateless; the HPA runs 2–5 replicas. The transform must run once at a time: one `CronJob` (`concurrencyPolicy: Forbid`) calling `POST /api/pipeline/run` every minute. Do not run `bun run worker` in more than one place.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness — the latter is a literal today; make it `select 1` before an outage teaches you.
- **Migrations.** SQL files in `backend/migrations/`, applied by the `migrate` init container before each rollout. Additive only during a rollout; new mart shapes are new tables, then backfill, then drop.
- **Secrets.** `DATABASE_URL` only, today.
- **Overlays.** `kubectl apply -k k8s/overlays/dev` / `prod`; `git push main` deploys dev, `make release` tags prod.
- **What pages.** The absence of the `transform` log line for ten minutes; `watermarks.updated_at` older than `max(events.occurred_at)` by more than your SLO; 5xx on `/api/ingest`. Do not page on CPU.

## Roadmap

In order of how soon each will matter, with what it takes:

1. A lease or a `CronJob` so two transforms cannot run at once.
2. A margin below the watermark (or a `received_at` lag) so a slow ingest transaction is not skipped.
3. `aggregate` in SQL for `pgStore`, so a day of millions of events does not pass through memory.
4. Readiness that checks Postgres.
5. Authentication on `/api/ingest` and `/api/pipeline/*`.
6. Pre-created partitions and a `purge <day>` subcommand.
"##;

const PIPELINE_REVIEWER: &str = r##"---
name: pipeline-correctness
description: Run on any change to transforms.ts, worker.ts, store.ts, a migration, or the ingest route. Reads the change as the person who will be asked why last Tuesday's number changed, and reports what makes the mart wrong, double-counted, or silently stale.
tools: Read, Grep, Glob, Bash
---

You review a data pipeline change. The failure you are looking for is never a crash; it is a
number that is wrong and nobody notices for a week.

Report each finding as `path:line — what is wrong — the input that shows it — the fix`.

Check:
1. Purity: `backend/src/transforms.ts` imports only `zod`. No `./db`, `./store`, `Date.now()`,
   `process.env`. A transform that reads the clock gives different answers on a backfill.
2. Staging is the only door: every path into `store.ingest` maps through `normalize`. Grep for
   `store.ingest(` and `.ingest(` — `app.ts` and `seed.ts` are the only callers, and both
   normalise. A raw `name` that skips it becomes a second metric.
3. Idempotency of ingest: in `pgStore.ingest`, the batch claim and the row inserts are inside
   one `db.begin`. A claim outside the transaction answers a retry as duplicate while the rows
   are missing. The memory store must return `false` on a repeated id, not throw.
4. Watermark order in `worker.ts`: `upsertDaily` then `setWatermark`, never the reverse, and the
   mark is the last id *read*, not `mark + processed`. A mark ahead of the data is a permanent gap.
5. Whole-day recompute: `upsertDaily` deletes the day's rows before inserting. A change that
   increments `count` or `sum` in place double-counts on every retry.
6. Days come from `occurred_at` once, in `normalize`. Grep for `.slice(0, 10)` and `dayOf(`
   outside `transforms.ts`; a second derivation with a different timezone splits a day.
7. DDL splice safety: any identifier built from data (`events_${day}`) is validated against the
   `DAY` regex on the same line or the one before. There is no test for `pgStore`; you are it.
8. Bounds: `ingestBody` ≤ 1000 events, `range` ≤ 366 days and `from <= to`, `BATCH` in
   `worker.ts` finite, `metrics` limited by the range. A route that returns raw events needs a
   limit and an index or it does not ship.
9. Migrations are additive during a rollout: a new column has a default; a new mart shape is a
   new table (backfill, switch, drop later), never an `alter … drop` or a rename. `0002` is not
   edited after it has shipped.
10. `aggregate` output is sorted (`day`, then `name`) so `toEqual` in tests and diffs in a
    backfill are stable. A change that returns `Map` order breaks both.
11. Backfill does not call `setWatermark`. If it did, a backfill of old history would rewind the
    job and re-process everything since.
12. Determinism of `seed.ts`: the LCG and the injected `now` stay; a `Math.random()` or a
    `new Date()` makes two developers' marts disagree and the seed test flaky.
13. The `transform` log line still prints `processed` and `days`; it is the only signal that
    the job ran.

End with one line: `pipeline-correctness: N findings`, and if 0, what you checked.
"##;
