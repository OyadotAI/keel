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
        ("CLAUDE.md", f(DAG_CLAUDE)),
        ("AGENTS.md", f(DAG_AGENTS)),
        ("README.md", f(DAG_README)),
        (".claude/agents/graph-semantics.md", DAG_REVIEWER.into()),
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

const DAG_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a DAG runtime for agent work, modelled on Airflow and Dagster: a DAG is a set of
tasks with explicit upstream edges, a scheduler starts every task whose upstreams succeeded (up to
a pool size), each task retries on its own policy, a failure kills only the subtree below it, and
a task whose inputs have not changed is served from a content-hash cache instead of being run
again. The example DAG fetches a page, summarises it in three parallel model calls, merges, and
has a model judge the result, recording latency and cost per node. "Done" here means: the run
row and every task instance in Postgres describe exactly what happened, `make check` is green
without a database, and a reviewer can read `GET /api/runs/:id` and explain the cost.

## Architecture

| File | Owns |
|---|---|
| `backend/src/dag.ts` | The engine: `Dag`/`Task` types, `validate`, `hashInputs`, `runDag` (scheduler, retries, `upstream_failed`, cache), the example `summariseDag`, `claudeModel` |
| `backend/src/app.ts` | The HTTP surface: list DAGs, trigger a run, read runs. Persists what the engine reports; contains no scheduling logic |
| `backend/src/store.ts` | `Store` interface with `pgStore` (Postgres) and `memoryStore` (tests). `cache` is part of the store |
| `backend/src/app.test.ts` | Five tests; the engine with an injected sleep, the app with an injected model and fetch |
| `backend/migrations/0002_dag.sql` | `dag_runs`, `task_instances`, `dag_cache` |
| `backend/src/server.ts` | Node server, SIGTERM drain (from the stack) |
| `backend/src/db.ts` | The one `postgres` pool (from the stack) |
| `backend/src/migrate.ts`, `seed.ts` | Plain SQL migrations; the seed writes the stack's `notes` table and nothing of this service's |
| `frontend/app/page.tsx` | Airflow's graph view: tasks laid out by depth, coloured by the state of the selected run |

### Request path: `POST /api/dags/:id/runs`

1. Look the DAG up in the in-process registry (`dags` in `app.ts`); 404 if absent.
2. Parse `{ input }` with zod; `input` must be an object (default `{}`).
3. `store.createRun` inserts the `dag_runs` row (`state: running`) and one `queued`
   `task_instances` row per task, in one transaction.
4. `runDag` validates the DAG (duplicate ids, unknown upstreams, cycles throw), then ticks: every
   task whose upstreams are all `success` starts, up to `concurrency` (default 4).
5. Each task computes its inputs — roots get `{ input }`, others get `{ [upstreamId]: output }` —
   hashes `(task id, inputs)`, and asks the cache. A hit is `success` with `cached: true` and
   `latency_ms: 0`. A miss runs `task.run`, retrying on the task's `retry` policy with
   `backoff_ms × factor^(attempt-1)` sleeps.
6. Every state change calls `onChange`, which the app wires to `store.putTask`, so the row is
   written at `running`, at `success`/`failed`, and at `upstream_failed`.
7. A `failed` task marks every queued descendant `upstream_failed`, recursively. Siblings run on.
8. When nothing is queued or running, `runDag` resolves; the app totals `cost_usd`, sets the run
   `success` only if every task is `success`, and returns `201` with the tasks inline.

The run executes inside the request. There is no worker in this pack.

### Data model

| Table | Column | Why it matters |
|---|---|---|
| `dag_runs` | `state` | `running` → `success` or `failed`; `failed` if any task failed |
| | `input` | The trigger body; with `dag_id`, enough to replay the run |
| | `cost_usd` | Sum of the tasks' cost; `numeric(12,6)` so cents of a cent survive |
| `task_instances` | `(run_id, task_id)` | Primary key; one row per node per run, inserted `queued` before anything runs |
| | `state` | `queued`, `running`, `success`, `failed`, `upstream_failed` |
| | `attempts` | How many times `run` was called; `0` for cached and `upstream_failed` |
| | `cached`, `input_hash` | Whether the output came from `dag_cache`, and the key that decided it |
| | `output`, `error` | The task's return value as jsonb, or the last error message |
| | `latency_ms`, `cost_usd` | Wall time across attempts; cost reported through `ctx.cost` |
| `dag_cache` | `input_hash` | Primary key: sha256 of task id + stable JSON of the inputs. Written `on conflict do nothing` |

## Invariants

1. **A DAG is validated before any task runs.** Duplicate ids, unknown upstreams and cycles throw
   from `runDag` before the first `run` — a malformed graph is a bug at definition time, not a
   run that hangs. Guarded by `app.test.ts` "ready tasks run in parallel up to the cap, in
   dependency order" (the `validate` assertion).
2. **A failure kills only its subtree.** Descendants of a `failed` task become `upstream_failed`,
   recursively; siblings finish and their outputs are kept. Guarded by "a failed task retries
   with backoff, then fails only its descendants".
3. **Retry is per task and its sleep is injectable.** `retry.max_attempts` bounds calls;
   `backoff_ms × factor^(attempt-1)` is the sleep sequence. Never add a retry loop inside a
   task's `run`; the engine already owns it and the attempts count would lie. Same test asserts
   `calls === 3` and `sleeps === [100, 200]`.
4. **The cache key is the stable hash of `(task id, inputs)` at every depth.** `stable()` sorts
   object keys recursively; a `JSON.stringify` replacer array would silently hash `{}` for every
   nested input. Guarded by "a task whose inputs did not change is served from the cache".
5. **The pool cap is honoured.** No more than `concurrency` tasks are in flight; the first test
   asserts `peak === 2` with `concurrency: 2`.
6. **Cost is only what tasks report.** `ctx.cost(usd)` is the sole way a task contributes to
   `cost_usd`; the run total is the sum. Guarded by "the example DAG fans out, merges, judges,
   and records cost per node" (`judge` ≈ 0.001, run ≈ 0.004).
7. **Task inputs are the upstream outputs keyed by upstream id; roots get `{ input }`.** A task
   never reads another task's output any other way. Guarded by the `merge` assertion in the same
   test (three lines, one per `summarise-*`).
8. **The run is `success` iff every task is `success`.** A run with an `upstream_failed` task is
   `failed`, and a run that never called a model costs `0`. Guarded by "a fetch that keeps
   failing dead-letters everything below it".
9. **Every state transition reaches the store.** `onChange` receives a clone at `running`, at the
   terminal state, and at `upstream_failed`; `app.ts` writes each one. The page relies on it.
10. **`app.ts` never schedules.** It looks the DAG up, persists, and returns. Anything about
    order, retries or caching belongs in `dag.ts` and is tested there with `memoryStore`.
11. **Both stores implement the same `Store`.** Tests run on `memoryStore`; `pgStore` is the same
    contract over SQL. A method added to one is added to the other in the same change.

## Extending it

**Add a DAG.** Write a `Dag` factory in `backend/src/dag.ts` (or a new file beside it) with
tasks, upstreams and per-task `retry`; register it in the `dags` record in `createApp`. Test it
through `runDag` with `memoryStore().cache` and an injected `sleep`, asserting order, states and
cache hits. No migration: DAG definitions are code, not rows.

**Add a task to the example DAG.** Add it to the `tasks` array in `summariseDag` with the right
`upstream`; if it is a fan-out, extend `parts`. Update the `task_id` order assertion in "the
example DAG fans out…". No migration.

**Change a retry policy.** Edit the task's `retry`. If the sequence changes, the `sleeps`
assertion in "a failed task retries…" changes with it — that is the review.

**Add a model provider.** Implement `Model = (prompt) => Promise<{ text, cost_usd }>` beside
`claudeModel`, with its own price table, and select it from an env var in `createApp`'s default.
The DAG tests already inject `model`; add one test for the provider's response parsing with a
fake `fetch`.

**Move runs off the request.** When a run can outlive an HTTP timeout: have the route insert the
run and enqueue its id, run `runDag` from a worker (`bun run worker` is already a script in
`package.json`), and have `GET /api/runs/:id` be the way to watch it. `dag_runs.state = running`
already exists for exactly this. Add a `worker.ts`, keep `onChange → putTask`, and add a
`claim` method to `Store` with `for update skip locked` (the controlplane pack shows the shape).

**Add a per-DAG pool.** `runDag` takes `concurrency`; pass it per DAG from the registry rather
than from `deps.concurrency`. Test with `peak`.

**Prune the cache.** `dag_cache.created_at` exists for it; add a `delete … where created_at <
now() - interval` to a cron or to the worker loop. Migration only if you add an index on
`created_at`.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres for runs, task instances and the cache |
| `ANTHROPIC_API_KEY` | for real runs | `claudeModel` sends it as `x-api-key`; empty means every model call fails and the run dead-letters |
| `MODEL` | no | Model id; default `claude-sonnet-4-20250514`. The price table in `claudeModel` is Sonnet 4's; change both together |
| `PORT` | no | Default `8000` |

**Scaling knobs.** `deps.concurrency` (default 4) is the in-flight task cap per run, per process.
Replicas multiply it, and each running task holds a model connection: keep replicas × 4 under
your Anthropic rate limit. `db.ts` pools 10 connections per replica.

**Per-replica today.** Nothing — the registry is code, and every write goes to Postgres. The
cache is shared through `dag_cache`. There is no in-memory state to move.

**Failure modes.**

| What fails | What the user sees |
|---|---|
| The page fetch 5xx | `fetch` retries 3× (0.5s, 1s), then the run returns `201` with `state: failed`, `fetch.error: "fetch 503"`, every other task `upstream_failed`, cost `0` |
| One summarise call fails 3× | That node `failed`; the other two summaries `success`; `merge` and `judge` `upstream_failed`; the cost of the successful calls is recorded |
| Postgres down | `createRun` throws; Hono returns `500`; nothing ran, nothing was charged |
| Process killed mid-run | The run stays `running` with some tasks `running` forever. There is no resumption — see Ceilings |
| Missing `ANTHROPIC_API_KEY` | `anthropic 401` on every model node after 3 attempts each; run `failed` |

**What to watch.** `dag_runs` where `state = 'running' and started_at < now() - interval '10
min'` (a run that was killed); the sum of `cost_usd` per day; `task_instances` where `cached`
over total (cache hit rate); `attempts > 1` counts per `task_id` (a flaky upstream). Logs are the
stack's JSON lines; the engine itself logs nothing — the rows are the log.

## Ceilings

- **Runs execute inside the request.** A run longer than the ingress timeout is cut off with the
  row left `running`. Upgrade: the worker recipe above.
- **A killed run is not resumed.** Task rows say where it stopped; nothing picks it up. Upgrade:
  a worker that claims runs left `running`, re-runs `runDag`, and lets the cache skip what
  already succeeded (that is what `dag_cache` is for).
- **The app's sleep is real.** `runDag` takes an injectable `sleep`, but `createApp` does not
  pass one, so the dead-letter test waits ~1.5s. Upgrade: add `sleep` to `Deps`.
- **The cache is keyed by task id, not DAG id.** Two DAGs with a task of the same id and the same
  inputs share an entry. Upgrade: include `dag.id` in `hashInputs`, and bump nothing — old
  entries simply stop matching.
- **The cache is never pruned.** `created_at` is there; nothing reads it.
- **No schedule.** Airflow's cron trigger does not exist; runs start from `POST` only.
- **`GET /api/runs` is the last 50, not paginated.**
- **One DAG, defined in code.** No registration API, no DAG files, no versioning of definitions.
- **The price table is hard-coded** for Sonnet 4; a different `MODEL` reports the wrong cost.
- **`/api/health/ready` does not check the database.** It returns `db: "ok"` unconditionally.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const DAG_AGENTS: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run and test it.

## Run

    make demo         # postgres + redis, migrate, seed, backend on :8000, frontend on :3000
    make check        # the gate: typecheck both halves, bun test the backend — no database needed
    make backend      # API only, with reload
    make migrate      # apply backend/migrations to DATABASE_URL

Real model calls need `ANTHROPIC_API_KEY` in `backend/.env`. Without it, every model node fails
with `anthropic 401` after three attempts and the run is `failed` — the pipeline shape still
shows, the summaries do not.

There is no worker in this pack: a run executes inside `POST /api/dags/:id/runs`.

## Routes

List DAGs with tasks, edges and retry policy:

    curl localhost:8000/api/dags
    # {"dags":[{"id":"summarise","tasks":[{"id":"fetch","upstream":[],"retry":{"max_attempts":3,"backoff_ms":500}},
    #   {"id":"summarise-0","upstream":["fetch"],"retry":{"max_attempts":3,"backoff_ms":1000}}, … ,
    #   {"id":"merge","upstream":["summarise-0","summarise-1","summarise-2"],"retry":null},
    #   {"id":"judge","upstream":["merge"],"retry":{"max_attempts":2,"backoff_ms":1000}}]}]}

Trigger a run (blocks until it finishes):

    curl -X POST localhost:8000/api/dags/summarise/runs \
      -H 'content-type: application/json' \
      -d '{"input":{"url":"https://en.wikipedia.org/wiki/Sea_otter"}}'
    # 201 {"id":"…","state":"success","cost_usd":0.0041,
    #   "tasks":[{"task_id":"fetch","state":"success","attempts":1,"cached":false,"input_hash":"…","output":{…},"error":null,"latency_ms":312,"cost_usd":0},
    #            {"task_id":"summarise-0","state":"success","attempts":1,"cached":false,…,"cost_usd":0.0012}, …,
    #            {"task_id":"judge","state":"success",…}]}

Run it again with the same URL: every node comes back `"cached":true`, `"latency_ms":0`, cost `0`.

A URL that fails:

    curl -X POST localhost:8000/api/dags/summarise/runs -H 'content-type: application/json' -d '{"input":{"url":"https://httpstat.us/503"}}'
    # 201 {"state":"failed","cost_usd":0,"tasks":[{"task_id":"fetch","state":"failed","attempts":3,"error":"fetch 503",…},
    #   {"task_id":"summarise-0","state":"upstream_failed","error":"upstream fetch failed",…}, …]}

Bad input and unknown DAG:

    curl -X POST localhost:8000/api/dags/summarise/runs -H 'content-type: application/json' -d '{"input":"x"}'
    # 400 {"error":{"message":"input must be an object","code":"invalid"}}
    curl -X POST localhost:8000/api/dags/nope/runs
    # 404 {"error":{"message":"no such dag","code":"not_found"}}

Read runs:

    curl localhost:8000/api/runs          # {"runs":[{"id","dag_id","state","input","cost_usd","started_at","finished_at"}, …]} newest first, 50
    curl localhost:8000/api/runs/<id>     # the run plus "tasks":[…] as stored; 404 if unknown

## How the tests are built

`backend/src/app.test.ts` never opens a socket or a database.

- **The engine** is tested directly: `runDag(dag, input, { concurrency, sleep, cache })`. `sleep`
  is injected so a backoff of 100ms and 200ms is asserted as `[100, 200]` without waiting.
- **The app** is built with `createApp(memoryStore(), { model, fetch })`. `model` is a function
  returning a fixed text and `cost_usd: 0.001`; `fetch` returns a `Response` with an HTML body.
  `app.request()` drives Hono without a server.
- **The cache** is `memoryStore().cache`, the same `Cache` interface `pgStore` implements.

What each test pins: order and the concurrency cap and cycle detection; retries, backoff and the
`upstream_failed` subtree; cache hit on identical inputs and miss on changed ones; the example
DAG's shape, per-node cost and persistence; the dead-letter path with a `503` fetch, and 404.

## Adding a test

Build a small `Dag` inline with `run` functions that record into local arrays (`order`, `calls`,
`sleeps`); call `runDag` with `sleep: async (ms) => { sleeps.push(ms) }`; assert on the returned
`TaskInstance[]` by `task_id`. For anything that goes through HTTP, use `createApp(memoryStore(),
{ model, fetch })` and `app.request`. Do not add a real timer wait: if the code under test sleeps,
inject the sleep.
"##;

const DAG_README: &str = r##"# {{NAME}}

A DAG runtime for agent pipelines, in one TypeScript codebase you own: tasks with explicit
dependencies, parallel execution up to a pool size, per-task retries, subtree failure, and a
content-hash cache so re-running a pipeline only pays for what changed.

## What you get

- Tasks are functions; a DAG is tasks plus edges. Validated for cycles and unknown edges before
  anything runs.
- A scheduler that starts every ready task, up to a concurrency cap (default 4).
- Per-task retry with exponential backoff; a task that gives up fails only its descendants
  (`upstream_failed`), and siblings finish.
- Dagster-style memoisation: a task's output is cached by the hash of its id and inputs, in
  Postgres, across runs.
- Cost and latency per node, and a run total, from what the model actually returned.
- Routes: `GET /api/dags`, `POST /api/dags/:id/runs`, `GET /api/runs`, `GET /api/runs/:id`.
- An example DAG: fetch a page → three parallel summaries → merge → judge, with Claude.
- A graph view (`frontend/app/page.tsx`) coloured by task state, with attempts and cost per box.
- Tests that need no database, no network and no waiting; Postgres migrations; Docker and
  kustomize manifests for a cluster.

## Five minutes

    make demo

Then, in another terminal (set `ANTHROPIC_API_KEY` in `backend/.env` first for real summaries):

    curl localhost:8000/api/dags
    # {"dags":[{"id":"summarise","tasks":[{"id":"fetch","upstream":[],…},{"id":"summarise-0","upstream":["fetch"],…},…]}]}

    curl -X POST localhost:8000/api/dags/summarise/runs -H 'content-type: application/json' \
      -d '{"input":{"url":"https://en.wikipedia.org/wiki/Sea_otter"}}'
    # {"id":"…","state":"success","cost_usd":0.004…,"tasks":[{"task_id":"fetch","state":"success","latency_ms":300,…},…]}

    # same input again: every node is served from the cache
    curl -X POST localhost:8000/api/dags/summarise/runs -H 'content-type: application/json' \
      -d '{"input":{"url":"https://en.wikipedia.org/wiki/Sea_otter"}}' | grep -o '"cached":true' | wc -l
    # 6

    curl localhost:8000/api/runs
    # {"runs":[{"id":"…","dag_id":"summarise","state":"success","cost_usd":0,…},{…,"cost_usd":0.004…}]}

Open http://localhost:3000 to see the graph and click a run.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not currently check the database) |
| GET | `/api/dags` | none | Every DAG with its tasks, upstream edges and retry policy |
| POST | `/api/dags/:id/runs` | none | Trigger a run with `{ input }`; blocks; `201` with the run and its tasks |
| GET | `/api/runs` | none | Last 50 runs, newest first |
| GET | `/api/runs/:id` | none | One run with its task instances |

There is no authentication on any route. Put it behind the stack's ingress rules or add auth
before exposing it.

## Compared with Airflow and Dagster

**Same shapes, so their docs transfer**

- Tasks with `upstream` edges; a DAG run with one task instance per task (Airflow's `dag_run`
  and `task_instance`).
- States `queued`, `running`, `success`, `failed`, `upstream_failed` — Airflow's names.
- Per-task `retries`/`retry_delay` with exponential backoff, and a pool-style concurrency cap.
- Memoised task outputs keyed by input hash (Dagster's memoization).
- A graph view coloured by state.

**Better here**

- Typed end to end: a task's inputs are the upstream outputs, and the frontend's client is built
  from the API's route type — a field the API stops returning stops the frontend compiling.
- Tests run without a scheduler, a database or the network, in well under a second, with the
  backoff sleep injected rather than waited for.
- Cost per node, from the model's own usage numbers, next to latency — Airflow has no notion of
  what a task cost.
- Three tables and ~200 lines of engine you can read in one sitting; no metadata database
  schema with dozens of tables, no executor abstraction, no DAG parsing.
- Kubernetes manifests, probes, migrations and encrypted secrets are already here; it deploys
  with the same `git push` as the rest of the stack.

**Not here yet**

- No scheduler: runs start from `POST`, not from a cron or a sensor.
- Runs execute inside the HTTP request; there is no worker and a killed run is not resumed.
- No backfills, no catch-up, no run-level parameters beyond `input`.
- No task-level timeouts, no SLAs, no per-task pools or priority weights — one concurrency cap.
- No dynamic task mapping at run time; fan-out is fixed at definition time.
- No DAG versioning, no UI for editing DAGs; definitions are TypeScript.
- No XCom-style side channel; a task sees only its upstream outputs.
- No assets, partitions, sensors or software-defined lineage (Dagster).
- No auth, no RBAC, no audit log on the API.
- Cache is never pruned, and is keyed by task id rather than DAG id.

## Production

- **Environments.** `DATABASE_URL`, `ANTHROPIC_API_KEY`, optional `MODEL`. `backend/.env` is
  ignored; `backend/.env.age` is committed and decrypted by CI.
- **Scaling.** Each replica runs up to 4 tasks per run in flight; a model call holds a
  connection for its duration. Size replicas to your model rate limit, and keep replicas × 10 (the
  pool) under Postgres `max_connections`.
- **Probes.** The stack's startup/readiness/liveness probes hit `/api/health*`. Readiness does
  not yet check Postgres.
- **Migrations.** `backend/migrations/0002_dag.sql`; applied by `make migrate` locally and the
  migrate init container before a rollout.
- **Secrets.** The API key never appears in a row or a log; the engine records only text and
  cost.
- **Kubernetes.** `k8s/base` + `k8s/overlays/{dev,prod}`; a single backend Deployment. Because
  runs are request-bound, set the ingress timeout above your longest expected run.
- **What pages you.** Runs left `running` past your longest expected duration; daily
  `cost_usd` above budget; `attempts > 1` climbing on a task (an upstream is degrading).

## Roadmap

In order of how soon a real workload hits them:

1. Runs behind a worker, with `GET /api/runs/:id` as the way to watch; resumption of runs left
   `running`, letting the cache skip finished nodes.
2. An injectable sleep in `createApp` so the dead-letter test stops waiting 1.5s.
3. `dag.id` in the cache key, and a pruning job on `dag_cache.created_at`.
4. A cron trigger and keyset pagination on `/api/runs`.
5. Per-task timeouts and a per-DAG pool size.
"##;

const DAG_REVIEWER: &str = r##"---
name: graph-semantics
description: Run on any change to backend/src/dag.ts, a DAG definition, the store's cache, or the run route. Reads the change as someone who has debugged a scheduler that ran a task twice, hung on a cycle, or served a stale cached output, and reports what will do that here.
tools: Read, Grep, Glob, Bash
---

You review DAG scheduling and caching semantics. The engine is `backend/src/dag.ts`; the tests
that pin it are in `backend/src/app.test.ts`. Report only what breaks, each as
`path:line — what — the input that triggers it — the fix`.

Check:
1. **Validation runs first.** `runDag` calls `validate` before touching any task. A new field on
   `Task` that could make the graph invalid (a self-edge, an empty id) is caught there, not at
   runtime. Failure: a run that never resolves.
2. **Readiness means every upstream is `success`.** Not "not queued", not "finished". A task
   whose upstream is `upstream_failed` or `failed` must never start. Failure: a task runs on
   `undefined` inputs.
3. **The loop terminates.** `tick` resolves only when nothing is `queued` or `running`. Any new
   state must be either terminal or one the scheduler advances; a task left in a new
   intermediate state hangs the request. Failure: `POST /api/dags/:id/runs` never returns.
4. **`failDownstream` reaches every descendant**, not only children, and marks only `queued`
   tasks — a running sibling is not touched. Failure: a grandchild runs after its parent died.
5. **Concurrency is a cap, not a batch.** `ready.slice(0, cap - running)` starts up to the free
   slots each tick; a change that waits for a whole level to finish before starting the next
   serialises the DAG. Failure: `peak` in the first test drops or wall time climbs.
6. **Retries live in the engine only.** A `run` function with its own retry loop double-counts
   attempts and sleeps. Grep DAG definitions for `for (` and `catch` around the model call.
7. **Backoff is `backoff_ms × factor^(attempt-1)`** and the sleep is the injected one. A
   `setTimeout` written directly into the engine makes the retry test take real time.
8. **The cache key is `hashInputs(task.id, inputs)` with `stable()`** — recursive key sorting,
   arrays in order, `undefined` as `null`. Any change to `stable` needs the cache test extended
   with a nested object whose key order differs between the two runs.
9. **A cache hit is recorded honestly:** `cached: true`, `attempts: 0`, `latency_ms: 0`, cost `0`,
   and `cache.put` is not called again. A hit that increments attempts lies to the page.
10. **Cache writes happen after success only.** A `put` in the catch path caches a failure
    forever. `dag_cache` uses `on conflict do nothing`, so the first output wins; a task whose
    output must change with the same inputs must not be cached — say so in the review.
11. **`onChange` gets a clone**, and is awaited, so the store writes cannot reorder. A change that
    fires it without awaiting can persist `success` before `running`.
12. **`app.ts` still only persists.** Anything that decides order, retry or caching in the route
    is a scheduler outside the tests.
13. **Cost only through `ctx.cost`.** A task that computes cost into its output, or the app that
    reads cost from anywhere but `TaskInstance.cost_usd`, breaks the per-node and run totals.
14. **Inputs shape.** Roots receive `{ input }`; non-roots receive `{ [upstreamId]: output }` and
    nothing else. A task reading `input` when it has upstreams gets `undefined`.

End with one line: `graph-semantics: N findings`, and if 0, which of the above you checked.
"##;
