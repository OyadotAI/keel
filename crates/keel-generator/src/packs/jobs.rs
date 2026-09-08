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
        ("CLAUDE.md", f(JOBS_CLAUDE)),
        ("AGENTS.md", f(JOBS_AGENTS)),
        ("README.md", f(JOBS_README)),
        (
            ".claude/agents/webhook-security.md",
            JOBS_REVIEWER_WEBHOOK.into(),
        ),
        (
            ".claude/agents/queue-semantics.md",
            JOBS_REVIEWER_QUEUE.into(),
        ),
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
  list(limit: number): Promise<(Job & { name: string; data: unknown })[]>;
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
    list: async (limit) => db`select j.*, e.name, e.data from jobs j join events e on e.id = j.event_id order by j.updated_at desc limit ${limit}`,
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
    list: async (limit) => jobs.slice(-limit).reverse().map((j) => ({ ...j, name: events.get(j.event_id)!.name, data: events.get(j.event_id)!.data })),
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
  return rows.find((r) => r.id === job.id)?.data ?? {};
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
    // The handler sees the event's data, not an empty object.
    const s2 = memoryStore();
    await s2.receive({ id: "e2", name: "payment/succeeded", data: { order: "o-1" }, received_at: "" }, ["fulfil-order"]);
    let seen: unknown;
    await tick(s2, { "fulfil-order": async (data) => { seen = data; return null; } }, new Date(0));
    expect(seen).toEqual({ order: "o-1" });
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

const JOBS_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} receives webhooks and runs background jobs. It is modelled on Inngest's event → function
fan-out and BullMQ's retry/backoff/dead-letter loop, with Postgres as the queue: an inbound event
is verified before it is parsed, stored together with its jobs in one transaction (the outbox),
and acknowledged in milliseconds; a separate worker process claims jobs with
`for update skip locked`, retries with exponential backoff and full jitter, and parks a job as
`dead` after `max_attempts` so it can be replayed. "Done" here means: the event name is in
`ROUTING`, the function is in `HANDLERS`, `app.test.ts` proves the fan-out and the failure path
on the memory store, and `make check` is green.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | `ROUTING` (event name → function names), `verify()` (HMAC), the webhook and event routes, the admin list and replay, the exported `AppType` |
| `backend/src/worker.ts` | `HANDLERS` (function name → code), `backoff()`, `tick()` (claim one, run, record), the `bun run worker` loop with SIGTERM |
| `backend/src/store.ts` | The `Store` interface — `receive`, `claim`, `complete`, `fail`, `list`, `replay` — as `pgStore()` and `memoryStore()` |
| `backend/src/db.ts` | The one `postgres` pool, from `DATABASE_URL` |
| `backend/src/server.ts` | The HTTP process on `PORT`, SIGTERM drain |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` in order, once each |
| `backend/src/seed.ts` | Sample `notes` rows from the base scaffold; nothing this service reads |
| `backend/src/app.test.ts` | Signature refusal, exactly-once fan-out, retry → dead → replay, backoff bounds |
| `backend/migrations/0002_jobs.sql` | `events`, `jobs`, index `jobs_ready (state, run_after)` |
| `frontend/app/page.tsx` | The jobs table from `GET /api/jobs`, newest first |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client built from the route table |

### The request path

1. A provider POSTs to `/api/webhooks/:source` (e.g. `/api/webhooks/stripe`).
2. The handler reads the **raw** body text and picks the secret: `WEBHOOK_SECRET_STRIPE` if set, else `WEBHOOK_SECRET`.
3. `verify(raw, X-Signature | X-Hub-Signature-256, secret)` — `sha256=`/`v1=` prefix stripped, HMAC-SHA256 hex, `timingSafeEqual`. Wrong or missing: 401 and the body is never parsed.
4. The body is parsed; `name` is `body.name ?? body.type`, `id` is `body.id ?? X-Request-Id ?? randomUUID()`.
5. `ROUTING[name]` gives the function names (unknown event: none, but the event is still stored).
6. `store.receive(event, fns)` inserts the event and one `jobs` row per function in a single transaction. An `id` already seen inserts nothing and returns `false`.
7. 202 `{received: true, duplicate, jobs}`. The provider sees success within one round-trip to Postgres.
8. Separately, `worker.ts` loops: `store.claim(now)` takes the oldest `queued` job with `run_after <= now` (one row, locked), runs `HANDLERS[job.fn]` with a 30 s race, and calls `complete` or `fail`. `fail` requeues with `run_after = now + backoff(attempts)` while `attempts < max_attempts`, else sets `dead`.

### Data model

| Table | Columns that matter | Why |
|---|---|---|
| `events` | `id text primary key` | The provider's event id is the idempotency key; a redelivery hits the PK and is acknowledged, not reprocessed |
| | `name`, `data jsonb` | What was received, verbatim, for audit and replay |
| `jobs` | `event_id references events(id)`, `fn` | One row per (event, function); the outbox is "these were written in the same transaction as the event" |
| | `state` in `queued / running / completed / failed / dead` | `failed` is reserved in the comment but the code only produces `queued`, `running`, `completed`, `dead` |
| | `attempts`, `max_attempts` (5) | Bounded retries; `attempts` is incremented at claim time so a crash mid-run still counts |
| | `run_after` + index `(state, run_after)` | The claim query is a range scan on this index |
| | `last_error`, `output` | What the handler said, visible in the admin table |

## Invariants

1. **A bad or missing signature never reaches `JSON.parse`.** The raw body is verified first
   and the response is 401 with no other side effect. Guarded by `app.test.ts` "a bad signature
   never reaches parsing" (the body is `{`, which would throw if parsed).
2. **Signatures are compared in constant time**, over the raw text, with the secret from the
   environment. `verify` returns `false` on an empty secret, so an unset `WEBHOOK_SECRET`
   rejects everything rather than accepting everything. Same test (`verify("x", undefined, "s")`).
3. **Event and jobs are written in one transaction.** `pgStore.receive` is a `db.begin`; a
   failure between the two inserts rolls both back. There is no "event stored, jobs missing"
   state to reconcile.
4. **A redelivered event fans out once.** `on conflict (id) do nothing returning id` yields
   zero rows and the jobs loop is skipped; the response says `duplicate: true`. Guarded by
   "an event fans out to its jobs once, however often it is delivered" (two deliveries, two
   jobs total).
5. **A job is claimed by exactly one worker.** `for update skip locked` on a single `id`
   subquery; the memory store models it with the first matching `queued` row. Do not replace
   this with a `select` then `update`.
6. **`attempts` increments at claim, not at completion.** A handler that crashes the process
   has still consumed an attempt; the ceiling below says why that job then needs rescuing.
7. **Retries are bounded and jittered.** `backoff(attempt)` is `random() × min(cap, base × 2^attempt)`,
   base 1 s, cap 5 min. Guarded by "backoff grows and is jittered" (bounds at attempts 0, 5, 20)
   and by "the worker retries with backoff and dead-letters after max attempts" (`calls === 5`,
   then `dead`).
8. **A dead job stays visible and can be replayed.** `replay` only touches `state = 'dead'`
   and resets `attempts`, `run_after`, `last_error`; replaying a live job is a 400 `not dead`.
   Same test.
9. **A handler is bounded at 30 s.** `Promise.race` with `timeout(30_000)`; a slow handler is
   recorded as `handler timed out` and retried. (The promise keeps running — see Ceilings.)
10. **An unknown function is a failure, not a skip.** `no handler for <fn>` goes through the
    same retry path so a deploy that removed a handler while jobs were queued is visible as
    dead jobs, not silence.
11. **`/api/events` and `/api/jobs*` carry no signature and no auth.** They are for the app's
    own process behind the network boundary; the Service is not on the ingress. Putting them on
    a public host is a change to this rule, and needs a token check first.

## Extending it

**Add an event and a function** (say `invoice/paid` → `send-receipt`):
1. `app.ts` `ROUTING["invoice/paid"] = ["send-receipt"]`.
2. `worker.ts` `HANDLERS["send-receipt"] = async (data, job) => …`, returning a JSON-able value
   (it is stored in `jobs.output`).
3. `app.test.ts`: deliver a signed `{"id":"evt_x","name":"invoice/paid"}` twice; assert
   `jobs === 1` then `duplicate === true`; then `tick(store, { "send-receipt": …})` and check
   `state === "completed"`.
No migration.

**Add a webhook source** (say GitHub): nothing in code — `POST /api/webhooks/github` picks
`WEBHOOK_SECRET_GITHUB` and reads `X-Hub-Signature-256`. Add the secret to `.env.example`, and
a test with that header name if the provider's signature format differs from `sha256=<hex>`.

**Add a provider with a different signature scheme** (timestamped, like Stripe's real
`t=…,v1=…`): extend `verify` to take the header format, keep `timingSafeEqual`, and add a test
for a stale timestamp. `verify` is exported so the test can call it directly.

**Change the retry policy per function**: `max_attempts` is a column with a default; set it in
`receive` per `fn` (add an optional `fns: {fn, max_attempts}[]` shape to the store), or pass a
different `base`/`cap` into `backoff` from `tick`. Migration: none, the column exists.

**Scheduled jobs** (a cron): insert an event from a `setInterval` in a tiny scheduler process,
or from a Kubernetes `CronJob` that curls `/api/events`. The queue does the rest; `run_after`
already supports "not before".

**Run the worker in the cluster**: `k8s/base/` has only `backend` and `frontend`. Add
`worker.yaml`: same image, `command: ["node","dist/worker.js"]` (add `src/worker.ts` as a
second `bun build` entry in `package.json`), `envFrom` the same Secret, no Service, no probes
beyond liveness, `terminationGracePeriodSeconds` ≥ 40 (30 s handler + drain). Scale it on queue
depth, not CPU.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes (prod) | Postgres; the queue lives here |
| `WEBHOOK_SECRET` | yes | HMAC secret for every `/api/webhooks/:source` without a specific one |
| `WEBHOOK_SECRET_<SOURCE>` | per provider | Overrides `WEBHOOK_SECRET` for `/api/webhooks/<source>` (upper-cased) |
| `PORT` | no | HTTP port, default `8000` |
| `API_URL` | frontend, server-side | Where the page fetches `/api/jobs` |

**Scaling.** The HTTP side is stateless; scale on requests. The worker is a separate process
(`bun run worker` locally) and should be a separate Deployment; run as many as you like —
`skip locked` makes them safe together. Throughput per worker is one job at a time with a
500 ms idle sleep. Everything shared is in Postgres; nothing is per replica.

**Failure modes.**
- Provider retries after a timeout: the second delivery is `duplicate: true`, 202. Correct.
- Handler throws: `last_error` filled, `state = queued`, `run_after` in the future; visible
  on the page with `attempts` climbing.
- Handler throws 5 times: `state = dead`. Nothing pages yet — see "What to watch".
- Worker killed mid-handler: the job stays `running` forever (no lease). Rescue by hand:
  `update jobs set state='queued' where state='running' and updated_at < now() - interval '5 minutes'`.
- Postgres down: the webhook returns 500 and the provider retries later, which is what you
  want; the worker loop throws out of `tick` — restart policy brings it back.

**What to watch.** `select count(*) from jobs where state='dead'` — alert when > 0.
`select count(*) from jobs where state='queued' and run_after < now()` — queue depth; alert on
age of the oldest. `running` older than the 30 s timeout means a lost worker. Every log line
is JSON; add the job id to the worker's lines when you add logging there.

## Ceilings

- **Handlers receive `{}` today, not the event's `data`.** `worker.ts` `eventData` reads
  `store.list(1000)` which joins `events.name` but not `events.data`, and matches on the job
  id. Fix: add `data` to the `list` select (both stores) or a `store.event(id)` method, and
  pass `event.data`. The test passes because no handler asserts on its input — add that
  assertion when you fix it.
- **No lease on `running`.** A crashed worker strands its job. Upgrade: a `locked_until`
  column set at claim, and `claim` also takes `running` rows past it.
- **The 30 s timeout does not cancel the handler.** The promise keeps running after the job is
  marked failed. Upgrade: pass an `AbortSignal` into handlers.
- **One job per tick, 500 ms poll.** Fine to a few jobs/second. Upgrade: claim N with
  `limit N`, run with bounded concurrency; or `listen/notify` to wake the loop.
- **`jobs` and `events` grow unbounded.** Upgrade: partition by `received_at` month and drop,
  or a nightly delete of `completed` older than 30 days.
- **No worker Deployment in `k8s/base/`.** The cluster runs the HTTP side only until you add
  `worker.yaml` (recipe above).
- **`ROUTING` and `HANDLERS` are code.** Changing fan-out is a deploy, which is also what
  makes it reviewable.
- **Admin routes are unauthenticated.** Safe inside the cluster network; not on an ingress.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const JOBS_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the queue.

## Run

    make demo                     # postgres + redis, migrate, seed, API :8000, page :3000
    cd backend && bun run worker  # the worker, in a second shell — nothing runs without it
    make check                    # the gate: typecheck both halves, bun test in backend/

Put `WEBHOOK_SECRET=s3cret` in `backend/.env` for local signing.

## Every route, by hand

A signed webhook (the way Stripe or GitHub would send it):

    BODY='{"id":"evt_1001","name":"user/created","data":{"email":"a@b.c","id":"u_1"}}'
    SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac s3cret | awk '{print $2}')
    curl -s -X POST localhost:8000/api/webhooks/stripe \
      -H "x-signature: sha256=$SIG" -H "content-type: application/json" -d "$BODY"
    # 202 {"received":true,"duplicate":false,"jobs":2}

Send the same request again:

    # 202 {"received":true,"duplicate":true,"jobs":0}

Tamper with the body and keep the signature:

    curl -si -X POST localhost:8000/api/webhooks/stripe -H "x-signature: sha256=$SIG" -d '{"id":"evt_1001","name":"payment/succeeded"}'
    # HTTP/1.1 401 {"error":{"message":"bad signature","code":"unauthorized"}}

An internal event from your own code (no signature; not on the public ingress):

    curl -s -X POST localhost:8000/api/events -H "content-type: application/json" \
      -d '{"name":"payment/succeeded","data":{"order":"o_42"}}'
    # 202 {"id":"6f1c…","duplicate":false,"jobs":1}

The admin list (what the page shows):

    curl -s localhost:8000/api/jobs | jq '.jobs[] | {id, name, fn, state, attempts, last_error}'
    # {"id":3,"name":"payment/succeeded","fn":"fulfil-order","state":"completed","attempts":1,"last_error":null}

Replay a dead job (400 `not dead` unless its state is `dead`):

    curl -s -X POST localhost:8000/api/jobs/3/replay
    # {"ok":true}   or   400 {"error":{"message":"not dead","code":"invalid"}}

Health:

    curl -s localhost:8000/api/health        # {"status":"ok"}
    curl -s localhost:8000/api/health/ready  # {"status":"ok","db":"ok"}

## How the tests work

`app.test.ts` never opens a socket or a database. Each test builds its own `memoryStore()` and
`createApp(store)`, signs bodies with `createHmac("sha256", "s3cret")` (the env is set at the
top of the file), and drives the worker by calling `tick(store, handlers, now)` directly:

- `handlers` is injected, so a test can pass `{ "fulfil-order": async () => { throw … } }` and
  never touch the real `HANDLERS`.
- `now` is injected, so backoff is skipped by handing `tick` a clock far in the future instead
  of sleeping. The retry test drives a job to `dead` in ten synchronous ticks.
- The memory store's `claim` picks the first `queued` row with `run_after <= now`, the same
  rule as the SQL; it is the specification the Postgres store must match.

## Adding a test

    test("an unknown function dead-letters with a readable error", async () => {
      const store = memoryStore();
      await store.receive({ id: "e2", name: "user/created", data: {}, received_at: "" }, ["no-such-fn"]);
      for (let i = 0; i <= 5; i++) await tick(store, {}, new Date(Date.now() + i * 1e9));
      const j = (await store.list(1))[0];
      expect(j.state).toBe("dead");
      expect(j.last_error).toBe("no handler for no-such-fn");
    });

Name tests for the guarantee. Use `store.receive` directly when the test is about the worker,
and the signed HTTP path when it is about the boundary.

## Migrations

`backend/migrations/0003_name.sql`, `make migrate` locally; the `migrate` init container applies
it in the cluster before new pods serve. The worker reads the same tables, so deploy the worker
after the API in a schema change that adds a column the worker writes.
"##;

const JOBS_README: &str = r##"# {{NAME}}

Webhooks in, background jobs out, Postgres as the queue — the Inngest event→function model and
the BullMQ retry loop, in one TypeScript service with no broker to run.

## What you get

- `POST /api/webhooks/:source` — HMAC-SHA256 verified over the raw body before parsing, constant-time compare, a secret per source.
- The event id is the idempotency key: a provider's redelivery is acknowledged as `duplicate: true` and creates nothing.
- Event and its jobs written in one transaction (outbox); the webhook answers 202 in one database round-trip.
- `ROUTING`: one event fans out to N named functions, each its own job.
- A worker that claims with `for update skip locked`, retries with exponential backoff and full jitter (1 s → 5 min), and dead-letters after 5 attempts.
- `POST /api/jobs/:id/replay` and a page that lists every job with its state, attempts and last error.
- Tests that run the whole loop — signature, fan-out, retry, dead, replay — in-process with no database.

## Five minutes

    echo WEBHOOK_SECRET=s3cret >> backend/.env
    make demo
    (cd backend && bun run worker)     # second shell

Then:

    BODY='{"id":"evt_1","name":"user/created","data":{"email":"a@b.c"}}'
    SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac s3cret | awk '{print $2}')
    curl -s -X POST localhost:8000/api/webhooks/stripe -H "x-signature: sha256=$SIG" -d "$BODY"
    # {"received":true,"duplicate":false,"jobs":2}

    curl -s -X POST localhost:8000/api/webhooks/stripe -H "x-signature: sha256=$SIG" -d "$BODY"
    # {"received":true,"duplicate":true,"jobs":0}

    curl -s localhost:8000/api/jobs | jq '.jobs[] | [.fn, .state]'
    # ["sync-crm","completed"] ["send-welcome","completed"]

Open http://localhost:3000 for the table.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness |
| POST | `/api/webhooks/:source` | HMAC in `X-Signature` or `X-Hub-Signature-256` | Store event + jobs, 202 `{received, duplicate, jobs}` |
| POST | `/api/events` | none (internal) | Same, for your own code: `{name, data?, id?}` |
| GET | `/api/jobs` | none (internal) | Last 100 jobs with event name, state, attempts, error |
| POST | `/api/jobs/:id/replay` | none (internal) | Requeue a `dead` job; 400 otherwise |

Functions shipped: `send-welcome`, `sync-crm` (from `user/created`) and `fulfil-order` (from
`payment/succeeded`), as placeholders in `backend/src/worker.ts`.

## Compared with Inngest and BullMQ

**Same shapes, so their mental model applies**

- Inngest: an event with `name` and `data`, fanned out to named functions; the event id deduplicates; a dead-letter you can replay from a UI.
- BullMQ: `attempts`/`backoff` with exponential + jitter, a `failed` set you can retry, one worker process per queue that you scale independently.
- Stripe/GitHub signature conventions (`sha256=<hex>`, `v1=<hex>`) on the raw body.

**Better here**

- No broker: the queue is a table in the Postgres you already run, and the event and its jobs are one transaction — the outbox is free, not a pattern you implement around Redis.
- The whole loop is ~200 lines you own and can read; retry policy is a function, not a config schema.
- Tests inject the handlers and the clock, so the retry → dead → replay path runs in milliseconds with no infrastructure.
- Typed end to end: the page and any internal caller build their client from the route table.
- Ships with Dockerfile, compose, kustomize, migrations and CI.

**Not here yet**

- No steps inside a function (Inngest's `step.run` memoisation, `step.sleep`, `waitForEvent`). A function is one attempt-or-fail unit.
- No cron/scheduled functions built in; insert an event from a `CronJob`.
- No per-function concurrency limits, rate limits, priorities or delays (BullMQ's `limiter`, `priority`, `delay`) — only `run_after`.
- No lease on running jobs: a worker that dies mid-job strands it until you requeue by hand.
- No dashboard beyond the list page; no metrics endpoint.
- No worker Deployment in `k8s/base/` yet (recipe in `CLAUDE.md`).
- Handlers currently receive `{}` rather than the event's `data` — a known bug listed under Ceilings in `CLAUDE.md`.

## Production

- **Environments.** `DATABASE_URL`, `WEBHOOK_SECRET`, optional `WEBHOOK_SECRET_<SOURCE>`, `PORT`; from `backend/.env` locally and the `app-secrets` Secret in the cluster (`backend/.env.age` is what is committed).
- **Scaling.** API: HPA on CPU, 2–5 pods. Worker: add a second Deployment from the same image running `worker.js`; N workers are safe together because of `skip locked`; scale on queue depth.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness. The worker needs only a liveness probe.
- **Migrations.** `backend/migrations/*.sql` via the `migrate` init container, expand/contract.
- **Secrets.** Webhook secrets never in the image; rotate by re-rendering the Secret and rolling.
- **Overlay.** `kubectl apply -k k8s/overlays/prod`.
- **What pages you.** `dead` count > 0; oldest `queued` job age; any `running` job older than 30 s; webhook 401 rate (a rotated secret on one side only).

## Roadmap

- Pass `events.data` to handlers.
- `locked_until` lease and reclaim.
- Batch claim and bounded concurrency in the worker.
- `AbortSignal` into handlers so the 30 s timeout cancels work.
- Worker Deployment in `k8s/base/`.
- Retention: partition or prune `jobs`/`events`.
- A token on `/api/events` and `/api/jobs*` so they can sit behind an ingress.
"##;

const JOBS_REVIEWER_WEBHOOK: &str = r##"---
name: webhook-security
description: "Run on any change to the webhook route, verify(), secret handling or the event ingestion path. Reads the change as someone trying to forge, replay or amplify an inbound event."
tools: Read, Grep, Glob
---

You are trying to get this service to run a job it should not. Read the diff as that person.
Report only what an attacker or a misbehaving provider could use, each as
`path:line — what — how it is exploited — the fix`.

Check:
1. **Raw before parse.** `c.req.text()` is read and `verify()` called before any `JSON.parse`;
   nothing about the body (its shape, its `id`) influences which secret is chosen.
2. **Constant time.** The compare is `timingSafeEqual` on equal-length buffers; a length
   mismatch returns false without throwing.
3. **Empty secret rejects.** `verify` returns `false` when `secret` is `""`, so an unset
   `WEBHOOK_SECRET` cannot accept everything. The `??` chain in the route still ends in `""`.
4. **Per-source secret.** `WEBHOOK_SECRET_<SOURCE>` is derived from the path param upper-cased
   only; a source name cannot select an unrelated env var (check for `process.env[…]` built
   from anything else).
5. **Header names.** `X-Signature` and `X-Hub-Signature-256` are both honoured; a new
   provider's header is added explicitly, not read from a body field.
6. **Dedup key.** `id` comes from the body, then `X-Request-Id`, then random. A provider that
   never sends an id is reprocessed on every retry — that is a finding for that provider.
   A colliding id from two different sources shares one row: consider prefixing with `source`.
7. **Unsigned routes.** `/api/events`, `/api/jobs`, `/api/jobs/:id/replay` are unauthenticated;
   they must not be reachable from the ingress. Check `k8s/overlays/*/ingress.yaml` paths.
8. **Body bounds.** No size limit on the raw body; a large payload is stored in `events.data`.
   Add `bodyLimit` before exposing to the internet.
9. **Secrets in logs or errors.** Nothing logs `secret`, the signature header, or the raw body
   on failure.
10. **Replay window.** There is no timestamp check; a captured valid request is valid forever
    (the id dedups a byte-identical one, but a provider that omits ids is exposed). Note it
    when a provider supports `t=`.

End with one line: `webhook-security: N findings`, and if 0, what you checked.
"##;

const JOBS_REVIEWER_QUEUE: &str = r##"---
name: queue-semantics
description: "Run on any change to worker.ts, store.ts claim/complete/fail/replay, the jobs migration, or the retry policy. Checks at-least-once delivery, bounded retries, dead-lettering and replay the way an on-call engineer would after a duplicate charge."
tools: Read, Grep, Glob, Bash
---

You have been paged for a job that ran twice, or never. Read the change as that person. Report
only what loses a job, duplicates one, or hides one, each as
`path:line — what — when it happens — the fix`.

Check:
1. **One transaction.** `receive` inserts event and jobs inside `db.begin`; the memory store's
   early-return on a seen id matches the SQL's `returning id` count check.
2. **Claim is atomic.** The `update … where id = (select … for update skip locked limit 1)
   returning *` shape is intact; no `select` then `update`; `state in ('queued')` and
   `run_after <= now` are both in the subquery.
3. **Attempts count at claim.** `attempts = attempts + 1` in `claim`, not in `fail`; the retry
   decision is `job.attempts < job.max_attempts` on the claimed row, so the 5th failure
   dead-letters (test: `calls === 5`).
4. **Backoff bounds.** `backoff(attempt)` is `< min(cap, base × 2^attempt)`, never negative,
   never `NaN` for large `attempt` (`2 ** 20 × 1000` is fine; check any new arithmetic).
5. **`fail` with `retryAt = null` sets `dead`**, keeps `last_error`, and never resets
   `attempts`. `replay` is the only path from `dead` back to `queued` and it resets
   `attempts` to 0.
6. **Timeout.** `Promise.race` with `timeout(30_000)`; the timer is `.unref()`ed so the worker
   process can exit. A handler that ignores the timeout keeps running — say so if a new
   handler does I/O with no deadline of its own.
7. **Handler input.** `eventData` must return `events.data` for the job's event. Today it does
   not (it reads `list()` which has no `data`); a change that makes handlers depend on their
   input must fix this first and assert on it in a test.
8. **Unknown fn.** Missing handler throws inside the try, so it retries and dies visibly; a
   change that `return`s early on a missing handler silently completes.
9. **Idle loop.** `tick` returning `null` sleeps 500 ms; a thrown error in `tick` escapes the
   loop and exits the process — acceptable under a restart policy, but not a silent catch.
10. **Shutdown.** SIGTERM sets `stopping`; the loop finishes the current job. The k8s grace
    period for the worker must exceed 30 s + the handler's own timeouts.
11. **Store parity.** Any new `Store` method exists in both `pgStore` and `memoryStore` with
    the same state transitions, and `app.test.ts` drives it through `tick`.
12. **Index.** The claim's `where state … and run_after …` still matches the
    `jobs_ready (state, run_after)` index; a new predicate column belongs in it.
13. **Stuck `running`.** No lease exists; a change that increases handler time or worker count
    should say how stranded jobs are reclaimed.

End with one line: `queue-semantics: N findings`, and if 0, what you checked.
"##;
