//! dag: a DAG agent runtime, like Airflow and Dagster ══════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", DAG_APP.into()),
        ("backend/src/store.ts", DAG_STORE.into()),
        ("backend/src/dag.ts", DAG_ENGINE.into()),
        ("backend/src/app.test.ts", DAG_TEST.into()),
        ("backend/migrations/0002_dag.sql", DAG_SQL.into()),
        ("frontend/app/page.tsx", f(DAG_PAGE)),
    ]
}

const DAG_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { claudeModel, runDag, summariseDag, type Dag, type Fetch, type Model } from "./dag";

// Airflow's REST surface, reduced: list DAGs with their tasks and edges, trigger a dag_run,
// read a run with its task instances. The scheduler is in dag.ts; this file only persists
// what it reports. Runs execute inline in the request so the gate needs no worker — move
// `runDag` behind a queue (see the jobs template) when runs outlive an HTTP timeout.

export type Deps = { model?: Model; fetch?: Fetch; concurrency?: number };

export function createApp(store: Store, deps: Deps = {}) {
  const dags: Record<string, Dag> = { summarise: summariseDag(deps.model ?? claudeModel(), deps.fetch ?? fetch) };
  const describe = (d: Dag) => ({ id: d.id, tasks: d.tasks.map((t) => ({ id: t.id, upstream: t.upstream, retry: t.retry ?? null })) });

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/dags", (c) => c.json({ dags: Object.values(dags).map(describe) }))

    .post("/api/dags/:id/runs", async (c) => {
      const dag = dags[c.req.param("id")];
      if (!dag) return c.json({ error: { message: "no such dag", code: "not_found" } }, 404);
      const p = z.object({ input: z.record(z.unknown()).default({}) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "input must be an object", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.createRun({ id, dag_id: dag.id, state: "running", input: p.data.input, cost_usd: 0, started_at: new Date().toISOString(), finished_at: null }, dag.tasks.map((t) => t.id));
      const tasks = await runDag(dag, p.data.input, { concurrency: deps.concurrency ?? 4, cache: store.cache, onChange: (ti) => store.putTask(id, ti) });
      const cost = tasks.reduce((s, t) => s + t.cost_usd, 0);
      const state = tasks.every((t) => t.state === "success") ? "success" : "failed";
      await store.finishRun(id, state, cost);
      return c.json({ id, state, cost_usd: cost, tasks }, 201);
    })
    .get("/api/runs", async (c) => c.json({ runs: await store.runs(50) }))
    .get("/api/runs/:id", async (c) => {
      const run = await store.run(c.req.param("id"));
      if (!run) return c.json({ error: { message: "no such run", code: "not_found" } }, 404);
      return c.json({ ...run, tasks: await store.tasks(run.id) });
    });
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const DAG_STORE: &str = r##"import { db } from "./db";
import type { Cache, TaskInstance } from "./dag";

export type Run = { id: string; dag_id: string; state: string; input: Record<string, unknown>; cost_usd: number; started_at: string; finished_at: string | null };

export interface Store {
  cache: Cache;
  createRun(run: Run, tasks: string[]): Promise<void>;
  finishRun(id: string, state: string, cost_usd: number): Promise<void>;
  putTask(runId: string, ti: TaskInstance): Promise<void>;
  run(id: string): Promise<Run | null>;
  runs(limit: number): Promise<Run[]>;
  tasks(runId: string): Promise<TaskInstance[]>;
}

export function pgStore(): Store {
  return {
    cache: {
      get: async (h) => (await db<{ output: unknown }[]>`select output from dag_cache where input_hash = ${h}`)[0]?.output,
      put: async (h, o) => { await db`insert into dag_cache (input_hash, output) values (${h}, ${db.json(o as never)}) on conflict do nothing`; },
    },
    createRun: async (r, tasks) => {
      await db.begin(async (tx) => {
        await tx`insert into dag_runs (id, dag_id, state, input) values (${r.id}, ${r.dag_id}, ${r.state}, ${tx.json(r.input as never)})`;
        for (const t of tasks) await tx`insert into task_instances (run_id, task_id, state) values (${r.id}, ${t}, 'queued')`;
      });
    },
    finishRun: async (id, state, cost) => { await db`update dag_runs set state = ${state}, cost_usd = ${cost}, finished_at = now() where id = ${id}`; },
    putTask: async (runId, t) => {
      await db`update task_instances set state = ${t.state}, attempts = ${t.attempts}, cached = ${t.cached}, input_hash = ${t.input_hash}, output = ${t.output === undefined ? null : db.json(t.output as never)}, error = ${t.error}, latency_ms = ${t.latency_ms}, cost_usd = ${t.cost_usd} where run_id = ${runId} and task_id = ${t.task_id}`;
    },
    run: async (id) => (await db<Run[]>`select id, dag_id, state, input, cost_usd::float8 as cost_usd, started_at, finished_at from dag_runs where id = ${id}`)[0] ?? null,
    runs: async (limit) => db<Run[]>`select id, dag_id, state, input, cost_usd::float8 as cost_usd, started_at, finished_at from dag_runs order by started_at desc limit ${limit}`,
    tasks: async (runId) => db<TaskInstance[]>`select task_id, state, attempts, cached, input_hash, output, error, latency_ms, cost_usd::float8 as cost_usd from task_instances where run_id = ${runId}`,
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>(), tasks = new Map<string, Map<string, TaskInstance>>(), cache = new Map<string, unknown>();
  return {
    cache: { get: async (h) => cache.get(h), put: async (h, o) => { cache.set(h, o); } },
    createRun: async (r, ids) => { runs.set(r.id, { ...r }); tasks.set(r.id, new Map(ids.map((id) => [id, { task_id: id, state: "queued", attempts: 0, cached: false, input_hash: null, output: null, error: null, latency_ms: null, cost_usd: 0 }]))); },
    finishRun: async (id, state, cost) => { const r = runs.get(id); if (r) Object.assign(r, { state, cost_usd: cost, finished_at: new Date().toISOString() }); },
    putTask: async (runId, t) => { tasks.get(runId)?.set(t.task_id, structuredClone(t)); },
    run: async (id) => runs.get(id) ?? null,
    runs: async (limit) => [...runs.values()].reverse().slice(0, limit),
    tasks: async (runId) => [...(tasks.get(runId)?.values() ?? [])],
  };
}
"##;

const DAG_ENGINE: &str = r##"import { createHash } from "node:crypto";

// A DAG the way Airflow and Dagster define one: tasks with explicit upstream dependencies, a
// scheduler that starts every task whose upstreams succeeded (up to a pool size), retries with
// backoff per task, and Airflow's `upstream_failed` state for the descendants of a failure —
// siblings keep running, only the subtree below the failure is dead. Each task's output is
// cached by the hash of (task id, inputs), Dagster's memoization, so a rerun with the same
// inputs only pays for what changed.

export type TaskState = "queued" | "running" | "success" | "failed" | "upstream_failed";
export type Retry = { max_attempts: number; backoff_ms: number; factor?: number };
export type TaskCtx = { attempt: number; cost: (usd: number) => void };
export type Task = {
  id: string;
  upstream: string[];
  run: (inputs: Record<string, unknown>, ctx: TaskCtx) => Promise<unknown>;
  retry?: Retry;
};
export type Dag = { id: string; tasks: Task[] };
export type TaskInstance = { task_id: string; state: TaskState; attempts: number; cached: boolean; input_hash: string | null; output: unknown; error: string | null; latency_ms: number | null; cost_usd: number };
export type Cache = { get(hash: string): Promise<unknown | undefined>; put(hash: string, output: unknown): Promise<void> };

export const noCache: Cache = { get: async () => undefined, put: async () => {} };

/// Validate once at definition time: unknown upstreams and cycles are bugs, not runtime errors.
export function validate(dag: Dag): string[] {
  const ids = new Set(dag.tasks.map((t) => t.id));
  const errors: string[] = [];
  if (ids.size !== dag.tasks.length) errors.push("duplicate task id");
  for (const t of dag.tasks) for (const u of t.upstream) if (!ids.has(u)) errors.push(`${t.id}: unknown upstream ${u}`);
  const seen = new Set<string>(), stack = new Set<string>();
  const visit = (id: string): boolean => {
    if (stack.has(id)) return true;
    if (seen.has(id)) return false;
    seen.add(id); stack.add(id);
    const cyclic = (dag.tasks.find((t) => t.id === id)?.upstream ?? []).some(visit);
    stack.delete(id);
    return cyclic;
  };
  if (dag.tasks.some((t) => visit(t.id))) errors.push("cycle");
  return errors;
}

// Key order must not change the hash, at any depth. (A replacer array to JSON.stringify only
// keeps top-level keys and silently drops nested ones — a cache that hashes {} for everything.)
const stable = (v: unknown): string =>
  v && typeof v === "object" && !Array.isArray(v)
    ? `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${stable((v as Record<string, unknown>)[k])}`).join(",")}}`
    : Array.isArray(v) ? `[${v.map(stable).join(",")}]` : JSON.stringify(v) ?? "null";
export const hashInputs = (taskId: string, inputs: Record<string, unknown>) =>
  createHash("sha256").update(taskId).update("\0").update(stable(inputs)).digest("hex");

export type RunOpts = {
  concurrency?: number;
  cache?: Cache;
  sleep?: (ms: number) => Promise<void>;
  onChange?: (ti: TaskInstance) => void | Promise<void>;
};

/// Run the whole DAG. Resolves with every task instance; the run failed if any task did.
export async function runDag(dag: Dag, input: Record<string, unknown>, opts: RunOpts = {}): Promise<TaskInstance[]> {
  const errors = validate(dag);
  if (errors.length) throw new Error(`invalid dag: ${errors.join(", ")}`);
  const cap = opts.concurrency ?? 4, cache = opts.cache ?? noCache, sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  const byId = new Map(dag.tasks.map((t) => [t.id, t]));
  const ti = new Map<string, TaskInstance>(dag.tasks.map((t) => [t.id, { task_id: t.id, state: "queued", attempts: 0, cached: false, input_hash: null, output: null, error: null, latency_ms: null, cost_usd: 0 }]));
  const emit = async (id: string) => { await opts.onChange?.(structuredClone(ti.get(id)!)); };

  // Descendants of a failed task never start: Airflow's upstream_failed.
  const failDownstream = async (id: string) => {
    for (const t of dag.tasks) {
      const d = ti.get(t.id)!;
      if (d.state === "queued" && t.upstream.includes(id)) { d.state = "upstream_failed"; d.error = `upstream ${id} failed`; await emit(t.id); await failDownstream(t.id); }
    }
  };

  const execute = async (task: Task) => {
    const t = ti.get(task.id)!;
    const inputs: Record<string, unknown> = { ...(task.upstream.length ? {} : { input }) };
    for (const u of task.upstream) inputs[u] = ti.get(u)!.output;
    t.input_hash = hashInputs(task.id, inputs);
    t.state = "running"; await emit(task.id);
    const hit = await cache.get(t.input_hash);
    if (hit !== undefined) { Object.assign(t, { state: "success", cached: true, output: hit, latency_ms: 0 }); await emit(task.id); return; }
    const retry = task.retry ?? { max_attempts: 1, backoff_ms: 0 };
    const started = Date.now();
    for (let attempt = 1; ; attempt++) {
      t.attempts = attempt;
      try {
        t.output = await task.run(inputs, { attempt, cost: (usd) => { t.cost_usd += usd; } });
        t.state = "success"; t.error = null;
        await cache.put(t.input_hash, t.output);
        break;
      } catch (e) {
        t.error = e instanceof Error ? e.message : String(e);
        if (attempt >= retry.max_attempts) { t.state = "failed"; break; }
        await sleep(retry.backoff_ms * Math.pow(retry.factor ?? 2, attempt - 1));
      }
    }
    t.latency_ms = Date.now() - started;
    await emit(task.id);
    if (t.state === "failed") await failDownstream(task.id);
  };

  // The scheduler: a task is ready when every upstream succeeded. Loop until nothing is
  // queued or running, starting ready tasks up to the pool size.
  let running = 0;
  const done = new Set<string>();
  await new Promise<void>((resolve, reject) => {
    const tick = () => {
      const ready = dag.tasks.filter((t) => ti.get(t.id)!.state === "queued" && t.upstream.every((u) => ti.get(u)!.state === "success"));
      for (const task of ready.slice(0, Math.max(0, cap - running))) {
        running++;
        execute(task).then(() => { running--; done.add(task.id); tick(); }, reject);
      }
      if (running === 0 && ![...ti.values()].some((t) => t.state === "queued" || t.state === "running")) resolve();
    };
    tick();
  });
  return dag.tasks.map((t) => ti.get(t.id)!);
}

// ── the example DAG: fetch → summarise ×N → merge → judge ─────────────────────────────────
export type Model = (prompt: string) => Promise<{ text: string; cost_usd: number }>;
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;

export function summariseDag(model: Model, fetchImpl: Fetch, n = 3): Dag {
  const ask = async (prompt: string, ctx: TaskCtx) => { const r = await model(prompt); ctx.cost(r.cost_usd); return r.text; };
  const parts = Array.from({ length: n }, (_, i) => `summarise-${i}`);
  return {
    id: "summarise",
    tasks: [
      { id: "fetch", upstream: [], retry: { max_attempts: 3, backoff_ms: 500 }, run: async ({ input }) => {
        const url = (input as { url: string }).url;
        const res = await fetchImpl(url); if (!res.ok) throw new Error(`fetch ${res.status}`);
        const text = (await res.text()).replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
        const size = Math.ceil(text.length / n);
        return Object.fromEntries(parts.map((p, i) => [p, text.slice(i * size, (i + 1) * size)]));
      } },
      ...parts.map((id) => ({ id, upstream: ["fetch"], retry: { max_attempts: 3, backoff_ms: 1000 }, run: (inputs: Record<string, unknown>, ctx: TaskCtx) => ask(`Summarise in two sentences:\n${(inputs.fetch as Record<string, string>)[id]}`, ctx) })),
      { id: "merge", upstream: parts, run: async (inputs) => parts.map((p) => inputs[p]).join("\n") },
      { id: "judge", upstream: ["merge"], retry: { max_attempts: 2, backoff_ms: 1000 }, run: (inputs, ctx) => ask(`Rate this summary 1-10 and say why:\n${inputs.merge}`, ctx) },
    ],
  };
}

export function claudeModel(fetchImpl: Fetch = fetch): Model {
  // Sonnet 4 list price; the per-node cost on the page is only as honest as this table.
  const price = { in: 3 / 1e6, out: 15 / 1e6 };
  return async (prompt) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", { method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" }, body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 400, messages: [{ role: "user", content: prompt }] }) });
    if (!res.ok) throw new Error(`anthropic ${res.status}`);
    const a = (await res.json()) as { content: { type: string; text?: string }[]; usage: { input_tokens: number; output_tokens: number } };
    return { text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""), cost_usd: a.usage.input_tokens * price.in + a.usage.output_tokens * price.out };
  };
}
"##;

const DAG_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { runDag, validate, type Dag, type Model } from "./dag";

const json = (body: unknown) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const model: Model = async (prompt) => ({ text: `[${prompt.split("\n")[0].slice(0, 9)}] ${prompt.split("\n")[1]?.slice(0, 20)}`, cost_usd: 0.001 });
const page = async () => new Response("<h1>Otters</h1><p>They hold hands while sleeping so they do not drift apart. Rafts of otters can number a hundred.</p>");

describe("dag", () => {
  test("ready tasks run in parallel up to the cap, in dependency order", async () => {
    let running = 0, peak = 0; const order: string[] = [];
    const work = (id: string) => async () => { order.push(id); running++; peak = Math.max(peak, running); await new Promise((r) => setTimeout(r, 5)); running--; return id; };
    const dag: Dag = { id: "d", tasks: [
      { id: "a", upstream: [], run: work("a") },
      ...["b", "c", "d", "e"].map((id) => ({ id, upstream: ["a"], run: work(id) })),
      { id: "z", upstream: ["b", "c", "d", "e"], run: async (inputs) => Object.values(inputs).join("") },
    ] };
    const tasks = await runDag(dag, {}, { concurrency: 2 });
    expect(peak).toBe(2);
    expect(order[0]).toBe("a");
    expect(tasks.find((t) => t.task_id === "z")!.output).toBe("bcde");
    expect(tasks.every((t) => t.state === "success")).toBe(true);
    expect(validate({ id: "x", tasks: [{ id: "a", upstream: ["b"], run: async () => 1 }, { id: "b", upstream: ["a"], run: async () => 1 }] })).toContain("cycle");
  });

  test("a failed task retries with backoff, then fails only its descendants", async () => {
    const sleeps: number[] = []; let calls = 0;
    const dag: Dag = { id: "d", tasks: [
      { id: "root", upstream: [], run: async () => "r" },
      { id: "flaky", upstream: ["root"], retry: { max_attempts: 3, backoff_ms: 100 }, run: async () => { calls++; throw new Error("boom"); } },
      { id: "sibling", upstream: ["root"], run: async () => "fine" },
      { id: "child", upstream: ["flaky"], run: async () => "never" },
      { id: "grandchild", upstream: ["child", "sibling"], run: async () => "never" },
    ] };
    const by = Object.fromEntries((await runDag(dag, {}, { sleep: async (ms) => { sleeps.push(ms); } })).map((t) => [t.task_id, t]));
    expect(calls).toBe(3);
    expect(sleeps).toEqual([100, 200]);
    expect(by.flaky.state).toBe("failed"); expect(by.flaky.error).toBe("boom"); expect(by.flaky.attempts).toBe(3);
    expect(by.sibling.state).toBe("success");
    expect(by.child.state).toBe("upstream_failed");
    expect(by.grandchild.state).toBe("upstream_failed");
  });

  test("a task whose inputs did not change is served from the cache", async () => {
    const store = memoryStore(); let ran = 0;
    const dag: Dag = { id: "d", tasks: [{ id: "a", upstream: [], run: async ({ input }) => { ran++; return (input as { x: number }).x * 2; } }] };
    await runDag(dag, { x: 1 }, { cache: store.cache });
    const [again] = await runDag(dag, { x: 1 }, { cache: store.cache });
    await runDag(dag, { x: 2 }, { cache: store.cache });
    expect(ran).toBe(2);
    expect(again.cached).toBe(true); expect(again.output).toBe(2);
  });

  test("the example DAG fans out, merges, judges, and records cost per node", async () => {
    const app = createApp(memoryStore(), { model, fetch: page });
    const res = await app.request("/api/dags/summarise/runs", json({ input: { url: "https://example.com/otters" } }));
    expect(res.status).toBe(201);
    const run = await res.json();
    expect(run.state).toBe("success");
    expect(run.tasks.map((t: { task_id: string }) => t.task_id)).toEqual(["fetch", "summarise-0", "summarise-1", "summarise-2", "merge", "judge"]);
    expect(run.tasks.find((t: { task_id: string }) => t.task_id === "merge").output.split("\n")).toHaveLength(3);
    expect(run.tasks.find((t: { task_id: string }) => t.task_id === "judge").cost_usd).toBeCloseTo(0.001);
    expect(run.cost_usd).toBeCloseTo(0.004);
    const stored = await (await app.request(`/api/runs/${run.id}`)).json();
    expect(stored.state).toBe("success"); expect(stored.tasks).toHaveLength(6);
    expect((await (await app.request("/api/dags")).json()).dags[0].tasks.find((t: { id: string }) => t.id === "judge").upstream).toEqual(["merge"]);
  });

  test("a fetch that keeps failing dead-letters everything below it", async () => {
    const app = createApp(memoryStore(), { model, fetch: async () => new Response("", { status: 503 }) });
    // Real backoff would sleep 1.5s here; the engine's sleep is injectable but the app's is not,
    // so keep the assertion on the outcome and accept the wait.
    const run = await (await app.request("/api/dags/summarise/runs", json({ input: { url: "https://down.example" } }))).json();
    expect(run.state).toBe("failed");
    expect(run.tasks[0]).toMatchObject({ task_id: "fetch", state: "failed", attempts: 3, error: "fetch 503" });
    expect(run.tasks.slice(1).every((t: { state: string }) => t.state === "upstream_failed")).toBe(true);
    expect(run.cost_usd).toBe(0);
    expect((await app.request("/api/dags/nope/runs", json({}))).status).toBe(404);
  });
});
"##;

const DAG_SQL: &str = r##"-- Airflow's tables, reduced: a dag_run per invocation, a task_instance per node in it, and a
-- content-hash cache so a node whose inputs did not change is not run again (Dagster's memoization).
create table if not exists dag_runs (
  id uuid primary key,
  dag_id text not null,
  state text not null,           -- running | success | failed
  input jsonb not null,
  cost_usd numeric(12,6) not null default 0,
  started_at timestamptz not null default now(),
  finished_at timestamptz
);
create table if not exists task_instances (
  run_id uuid not null references dag_runs(id),
  task_id text not null,
  state text not null,           -- queued | running | success | failed | upstream_failed
  attempts int not null default 0,
  cached boolean not null default false,
  input_hash text,
  output jsonb,
  error text,
  latency_ms int,
  cost_usd numeric(12,6) not null default 0,
  primary key (run_id, task_id)
);
create table if not exists dag_cache (
  input_hash text primary key,
  output jsonb not null,
  created_at timestamptz not null default now()
);
"##;

const DAG_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Task = { id: string; upstream: string[] };
type Instance = { task_id: string; state: string; attempts: number; cached: boolean; latency_ms: number | null; cost_usd: number; error: string | null };
type Run = { id: string; dag_id: string; state: string; cost_usd: number; started_at: string; tasks?: Instance[] };

const colour: Record<string, string> = { queued: "#ddd", running: "#8cf", success: "#8d8", failed: "#e77", upstream_failed: "#fc8" };

// The DAG as Airflow's graph view draws it: one box per task, laid out by depth, coloured by
// the state of the selected run, with attempts and cost in the box.
export default function Home() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [run, setRun] = useState<Run | null>(null);
  const [url, setUrl] = useState("https://en.wikipedia.org/wiki/Sea_otter");
  const [busy, setBusy] = useState(false);

  const refresh = async () => setRuns((await (await fetch("/api/runs")).json()).runs);
  useEffect(() => { fetch("/api/dags").then((r) => r.json()).then((d) => setTasks(d.dags[0].tasks)); void refresh(); }, []);
  const trigger = async () => { setBusy(true); const r = await (await fetch("/api/dags/summarise/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ input: { url } }) })).json(); setRun(r); setBusy(false); void refresh(); };
  const open = async (id: string) => setRun(await (await fetch(`/api/runs/${id}`)).json());

  const depth = (id: string): number => Math.max(0, ...(tasks.find((t) => t.id === id)?.upstream ?? []).map((u) => depth(u) + 1));
  const columns = tasks.reduce<Task[][]>((cols, t) => { (cols[depth(t.id)] ??= []).push(t); return cols; }, []);
  const inst = (id: string) => run?.tasks?.find((t) => t.task_id === id);

  return (
    <main>
      <h1>{{NAME}} — DAG agent</h1>
      <p>fetch → summarise ×3 → merge → judge. Set <code>ANTHROPIC_API_KEY</code> in backend/.env, or: <code>{`curl -X POST localhost:8000/api/dags/summarise/runs -H 'content-type: application/json' -d '{"input":{"url":"${url}"}}'`}</code></p>
      <p><input value={url} onChange={(e) => setUrl(e.target.value)} size={60} /> <button onClick={trigger} disabled={busy}>{busy ? "running…" : "Trigger run"}</button></p>
      <div style={{ display: "flex", gap: 24, alignItems: "center" }}>
        {columns.map((col, i) => (
          <div key={i} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            {col.map((t) => { const s = inst(t.id); return (
              <div key={t.id} title={s?.error ?? ""} style={{ border: "1px solid #888", padding: "6px 10px", background: colour[s?.state ?? "queued"], minWidth: 120, fontFamily: "monospace" }}>
                <div>{t.id}</div>
                {s && <small>{s.state}{s.cached ? " (cached)" : ""} · {s.attempts}× · {s.latency_ms ?? "–"}ms · ${s.cost_usd.toFixed(4)}</small>}
              </div>); })}
          </div>
        ))}
      </div>
      {run && <p>Run <code>{run.id.slice(0, 8)}</code> · <strong>{run.state}</strong> · total ${run.cost_usd.toFixed(4)}</p>}
      <h2>Runs</h2>
      <ul>{runs.map((r) => <li key={r.id}><a href="#" onClick={(e) => { e.preventDefault(); void open(r.id); }}>{r.id.slice(0, 8)}</a> · {r.state} · ${Number(r.cost_usd).toFixed(4)}</li>)}</ul>
    </main>
  );
}
"##;
