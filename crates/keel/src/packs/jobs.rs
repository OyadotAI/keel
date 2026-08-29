//! jobs: webhooks and background jobs, like Inngest / BullMQ ═══════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", JOBS_APP.into()),
        ("backend/src/store.ts", JOBS_STORE.into()),
        ("backend/src/worker.ts", JOBS_WORKER.into()),
        ("backend/src/app.test.ts", JOBS_TEST.into()),
        ("backend/migrations/0002_jobs.sql", JOBS_SQL.into()),
        ("frontend/app/page.tsx", f(JOBS_PAGE)),
    ]
}

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
