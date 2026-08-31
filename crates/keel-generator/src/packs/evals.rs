//! evals: versioned datasets, a runner, graders and a leaderboard, like promptfoo / Inspect ═══

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", EVALS_APP.into()),
        ("backend/src/store.ts", EVALS_STORE.into()),
        ("backend/src/runner.ts", EVALS_RUNNER.into()),
        ("backend/src/worker.ts", EVALS_WORKER.into()),
        ("backend/src/app.test.ts", EVALS_TEST.into()),
        ("backend/migrations/0002_evals.sql", EVALS_SQL.into()),
        ("frontend/app/page.tsx", f(EVALS_PAGE)),
        ("CLAUDE.md", f(EVALS_CLAUDE_MD)),
        ("AGENTS.md", f(EVALS_AGENTS_MD)),
        ("README.md", f(EVALS_README_MD)),
        (".claude/agents/grader-integrity.md", EVALS_REVIEWER.into()),
    ]
}

const EVALS_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { tick, claude, summarize, type Model } from "./runner";

// PUT /api/datasets/:name writes a new immutable version; POST /api/runs pins one and queues a
// run; the worker (bun run worker) executes it. With no worker — the first five minutes — the
// API drains the queue itself (INLINE_WORKER=false in the cluster). /api/leaderboard compares
// runs of one dataset; /api/runs/:id is the per-case drill-down.

const caseSchema = z.object({ id: z.string().min(1), vars: z.record(z.string()), assert: z.array(z.object({ type: z.enum(["equals", "contains", "llm-rubric"]), value: z.string() })).min(1) });
const inlineWorker = process.env.INLINE_WORKER !== "false";

export function createApp(store: Store, model: Model = claude(), opts: { inline?: boolean; judge?: Model } = {}) {
  const inline = opts.inline ?? inlineWorker; const judge = opts.judge ?? model;
  const drain = async () => { try { while (await tick(store, model, judge)); } catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); } };
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .get("/api/datasets", async (c) => c.json({ datasets: await store.listDatasets() }))
    .put("/api/datasets/:name", async (c) => {
      const p = z.object({ cases: z.array(caseSchema).min(1).max(5000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "cases required: [{id, vars, assert:[{type, value}]}]", code: "invalid" } }, 400);
      if (new Set(p.data.cases.map((k) => k.id)).size !== p.data.cases.length) return c.json({ error: { message: "case ids must be unique", code: "invalid" } }, 400);
      const d = await store.putDataset(c.req.param("name"), p.data.cases);
      return c.json({ name: d.name, version: d.version, cases: d.cases.length }, 201);
    })
    .get("/api/datasets/:name", async (c) => { const d = await store.getDataset(c.req.param("name"), c.req.query("version") ? Number(c.req.query("version")) : undefined); return d ? c.json(d) : c.json({ error: { message: "no such dataset", code: "not_found" } }, 404); })

    .post("/api/runs", async (c) => {
      const p = z.object({ dataset: z.string().min(1), version: z.number().int().positive().optional(), label: z.string().max(80).optional(), prompt: z.string().min(1).max(20_000), model: z.string().min(1).default(process.env.MODEL ?? "claude-opus-5") }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "dataset and prompt required", code: "invalid" } }, 400);
      const d = await store.getDataset(p.data.dataset, p.data.version);
      if (!d) return c.json({ error: { message: "no such dataset", code: "not_found" } }, 404);
      const id = crypto.randomUUID();
      await store.createRun({ id, dataset: d.name, version: d.version, label: p.data.label ?? `${p.data.model} · ${p.data.prompt.slice(0, 40)}`, prompt: p.data.prompt, model: p.data.model, state: "queued", score: null, pass_rate: null, cost: 0, cases: d.cases.length, created_at: new Date().toISOString() });
      if (inline) setTimeout(drain, 0);
      return c.json({ id, dataset: d.name, version: d.version, cases: d.cases.length }, 201);
    })
    .post("/api/tick", async (c) => c.json({ ran: await tick(store, model, judge) }))
    .get("/api/runs", async (c) => c.json({ runs: await store.listRuns(c.req.query("dataset")) }))
    .get("/api/runs/:id", async (c) => {
      const run = await store.getRun(c.req.param("id"));
      if (!run) return c.json({ error: { message: "no such run", code: "not_found" } }, 404);
      const results = await store.getResults(run.id);
      return c.json({ ...run, ...summarize(results), results });
    })
    .get("/api/leaderboard", async (c) => {
      const runs = (await store.listRuns(c.req.query("dataset"))).filter((r) => r.state === "done");
      const rows = [];
      for (const r of runs) { const s = summarize(await store.getResults(r.id)); rows.push({ id: r.id, label: r.label, dataset: r.dataset, version: r.version, model: r.model, cases: r.cases, ...s, created_at: r.created_at }); }
      return c.json({ runs: rows.sort((a, b) => b.score - a.score || a.cost - b.cost) });
    });
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const EVALS_STORE: &str = r##"import { db } from "./db";

export type Assert = { type: "equals" | "contains" | "llm-rubric"; value: string };
export type Case = { id: string; vars: Record<string, string>; assert: Assert[] };
export type Dataset = { name: string; version: number; cases: Case[]; created_at: string };
export type Run = { id: string; dataset: string; version: number; label: string; prompt: string; model: string; state: "queued" | "running" | "done" | "failed"; score: number | null; pass_rate: number | null; cost: number; cases: number; created_at: string };
export type Grade = { type: string; score: number; pass: boolean; reason?: string };
export type Result = { run_id: string; case_id: string; output: string | null; score: number; pass: boolean; grades: Grade[]; cost: number; latency_ms: number; input_tokens: number; output_tokens: number; attempts: number; error: string | null };

export interface Store {
  putDataset(name: string, cases: Case[]): Promise<Dataset>;
  getDataset(name: string, version?: number): Promise<Dataset | null>;
  listDatasets(): Promise<{ name: string; version: number; cases: number; created_at: string }[]>;
  createRun(r: Run): Promise<void>;
  claimRun(): Promise<Run | null>;
  updateRun(id: string, p: Partial<Run>): Promise<void>;
  getRun(id: string): Promise<Run | null>;
  listRuns(dataset?: string): Promise<Run[]>;
  putResult(r: Result): Promise<void>;
  getResults(runId: string): Promise<Result[]>;
}

export function pgStore(): Store {
  return {
    putDataset: async (name, cases) => (await db<Dataset[]>`insert into datasets (name, version, cases) values (${name}, (select coalesce(max(version), 0) + 1 from datasets where name = ${name}), ${db.json(cases as never)}) returning *`)[0],
    getDataset: async (name, version) => (await db<Dataset[]>`select * from datasets where name = ${name} and (${version ?? null}::int is null or version = ${version ?? 0}) order by version desc limit 1`)[0] ?? null,
    listDatasets: async () => db`select name, version, jsonb_array_length(cases)::int as cases, created_at from datasets order by name, version desc`,
    createRun: async (r) => { await db`insert into eval_runs (id, dataset, version, label, prompt, model, cases) values (${r.id}, ${r.dataset}, ${r.version}, ${r.label}, ${r.prompt}, ${r.model}, ${r.cases})`; },
    claimRun: async () => (await db<Run[]>`update eval_runs set state = 'running', updated_at = now() where id = (select id from eval_runs where state = 'queued' order by created_at limit 1 for update skip locked) returning *`)[0] ?? null,
    updateRun: async (id, p) => { await db`update eval_runs set state = coalesce(${p.state ?? null}, state), score = coalesce(${p.score ?? null}, score), pass_rate = coalesce(${p.pass_rate ?? null}, pass_rate), cost = coalesce(${p.cost ?? null}, cost), updated_at = now() where id = ${id}`; },
    getRun: async (id) => (await db<Run[]>`select * from eval_runs where id = ${id}`)[0] ?? null,
    listRuns: async (dataset) => db<Run[]>`select * from eval_runs where ${dataset ?? null}::text is null or dataset = ${dataset ?? ""} order by created_at desc limit 100`,
    putResult: async (r) => { await db`insert into eval_results ${db(r as never, "run_id", "case_id", "output", "score", "pass", "cost", "latency_ms", "input_tokens", "output_tokens", "attempts", "error")} on conflict (run_id, case_id) do update set output = excluded.output, score = excluded.score, pass = excluded.pass, cost = excluded.cost, latency_ms = excluded.latency_ms, input_tokens = excluded.input_tokens, output_tokens = excluded.output_tokens, attempts = excluded.attempts, error = excluded.error`; await db`update eval_results set grades = ${db.json(r.grades as never)} where run_id = ${r.run_id} and case_id = ${r.case_id}`; },
    getResults: async (runId) => db<Result[]>`select * from eval_results where run_id = ${runId} order by case_id`,
  };
}

export function memoryStore(): Store {
  const datasets: Dataset[] = []; const runs = new Map<string, Run>(); const results = new Map<string, Result>();
  return {
    putDataset: async (name, cases) => { const version = Math.max(0, ...datasets.filter((d) => d.name === name).map((d) => d.version)) + 1; const d = { name, version, cases, created_at: new Date().toISOString() }; datasets.push(d); return d; },
    getDataset: async (name, version) => datasets.filter((d) => d.name === name && (version === undefined || d.version === version)).sort((a, b) => b.version - a.version)[0] ?? null,
    listDatasets: async () => datasets.map((d) => ({ name: d.name, version: d.version, cases: d.cases.length, created_at: d.created_at })),
    createRun: async (r) => { runs.set(r.id, { ...r }); },
    claimRun: async () => { const r = [...runs.values()].find((r) => r.state === "queued"); if (!r) return null; r.state = "running"; return { ...r }; },
    updateRun: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p); },
    getRun: async (id) => runs.get(id) ?? null,
    listRuns: async (dataset) => [...runs.values()].filter((r) => !dataset || r.dataset === dataset).reverse(),
    putResult: async (r) => { results.set(`${r.run_id}:${r.case_id}`, { ...r }); },
    getResults: async (runId) => [...results.values()].filter((r) => r.run_id === runId).sort((a, b) => a.case_id.localeCompare(b.case_id)),
  };
}
"##;

const EVALS_RUNNER: &str = r##"// promptfoo's shape: a prompt with {{vars}}, a provider (model id), tests with `vars` and
// `assert` rows of type equals / contains / llm-rubric; each result carries pass, score,
// latencyMs, cost and tokenUsage. The runner walks a pinned dataset version with a concurrency
// cap and retries, grades every case, and writes a row per case as it finishes — so a run that
// dies halfway shows what it got to. The model is a function; tests script it.
import type { Assert, Case, Grade, Result, Run, Store } from "./store";

export type Completion = { text: string; input_tokens: number; output_tokens: number };
export type Model = (req: { model: string; system?: string; prompt: string }) => Promise<Completion>;

/// $ per million tokens, input / output. Override with PRICES='{"model":[in,out]}'.
export const PRICES: Record<string, [number, number]> = { "claude-opus-5": [5, 25], "claude-sonnet-5": [2, 10], "claude-haiku-4-5": [1, 5], ...JSON.parse(process.env.PRICES ?? "{}") };
export const cost = (model: string, c: { input_tokens: number; output_tokens: number }) => { const [i, o] = PRICES[model] ?? [0, 0]; return (c.input_tokens * i + c.output_tokens * o) / 1e6; };

export const render = (template: string, vars: Record<string, string>) => template.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k: string) => vars[k] ?? "");
const norm = (s: string) => s.trim().toLowerCase().replace(/\s+/g, " ");

/// Graders. The two exact ones are pure; the rubric one is a cold-context judge: it sees the
/// rubric and the output, never the prompt or the model under test, so it grades the answer
/// rather than the question.
export async function grade(a: Assert, output: string, judge: Model, judgeModel: string): Promise<Grade> {
  if (a.type === "equals") { const pass = norm(output) === norm(a.value); return { type: a.type, score: pass ? 1 : 0, pass }; }
  if (a.type === "contains") { const pass = norm(output).includes(norm(a.value)); return { type: a.type, score: pass ? 1 : 0, pass }; }
  const r = await judge({ model: judgeModel, system: `You grade an output against a rubric. Reply with JSON only: {"score": 0..1, "reason": "one sentence"}.`, prompt: `Rubric: ${a.value}\n\nOutput:\n${output}` });
  try { const v = JSON.parse(r.text.slice(r.text.indexOf("{"), r.text.lastIndexOf("}") + 1)) as { score: number; reason?: string }; const score = Math.max(0, Math.min(1, Number(v.score) || 0)); return { type: a.type, score, pass: score >= 0.5, reason: v.reason }; }
  catch { return { type: a.type, score: 0, pass: false, reason: "judge gave no score" }; }
}

export type RunOpts = { concurrency?: number; maxRetries?: number; judgeModel?: string; sleep?: (ms: number) => Promise<void>; onResult?: (r: Result) => Promise<void> | void };

export async function runCase(run: Run, kase: Case, model: Model, judge: Model, opts: RunOpts): Promise<Result> {
  const maxRetries = opts.maxRetries ?? 2; const sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  const base: Result = { run_id: run.id, case_id: kase.id, output: null, score: 0, pass: false, grades: [], cost: 0, latency_ms: 0, input_tokens: 0, output_tokens: 0, attempts: 0, error: null };
  for (let attempt = 1; ; attempt++) {
    const t0 = Date.now();
    try {
      const c = await model({ model: run.model, prompt: render(run.prompt, kase.vars) });
      const latency_ms = Date.now() - t0;
      const grades: Grade[] = []; let judgeCost = 0;
      for (const a of kase.assert) grades.push(await grade(a, c.text, async (req) => { const r = await judge(req); judgeCost += cost(req.model, r); return r; }, opts.judgeModel ?? run.model));
      const score = grades.length ? grades.reduce((s, g) => s + g.score, 0) / grades.length : 1;
      return { ...base, output: c.text, score, pass: grades.every((g) => g.pass), grades, cost: cost(run.model, c) + judgeCost, latency_ms, input_tokens: c.input_tokens, output_tokens: c.output_tokens, attempts: attempt };
    } catch (e) {
      const error = e instanceof Error ? e.message : String(e);
      if (attempt > maxRetries) return { ...base, attempts: attempt, latency_ms: Date.now() - t0, error };
      await sleep(250 * 2 ** (attempt - 1) * (0.5 + Math.random()));   // jittered backoff, like promptfoo's provider retry
    }
  }
}

/// The pool: at most `concurrency` cases in flight, results written as each finishes.
export async function runEval(run: Run, cases: Case[], model: Model, judge: Model, opts: RunOpts = {}): Promise<Result[]> {
  const results: Result[] = []; let next = 0;
  const worker = async () => { while (next < cases.length) { const r = await runCase(run, cases[next++], model, judge, opts); results.push(r); await opts.onResult?.(r); } };
  await Promise.all(Array.from({ length: Math.max(1, Math.min(opts.concurrency ?? 4, cases.length)) }, worker));
  return results;
}

export const summarize = (rs: Result[]) => ({
  score: rs.length ? rs.reduce((s, r) => s + r.score, 0) / rs.length : 0,
  pass_rate: rs.length ? rs.filter((r) => r.pass).length / rs.length : 0,
  cost: rs.reduce((s, r) => s + r.cost, 0),
  p50_ms: rs.length ? [...rs].sort((a, b) => a.latency_ms - b.latency_ms)[Math.floor(rs.length / 2)].latency_ms : 0,
});

/// One queue step: claim a queued run, execute it, settle it. The worker loops this.
export async function tick(store: Store, model: Model, judge: Model = model, opts: RunOpts = {}): Promise<boolean> {
  const run = await store.claimRun(); if (!run) return false;
  const ds = await store.getDataset(run.dataset, run.version);
  if (!ds) { await store.updateRun(run.id, { state: "failed" }); return true; }
  const rs = await runEval(run, ds.cases, model, judge, { concurrency: Number(process.env.EVAL_CONCURRENCY ?? 4), ...opts, onResult: (r) => store.putResult(r) });
  await store.updateRun(run.id, { state: "done", ...summarize(rs) });
  return true;
}

/// Claude via the Messages API. ANTHROPIC_API_KEY from the environment, never the repository.
export function claude(fetchImpl: typeof fetch = fetch): Model {
  return async ({ model, system, prompt }) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model, max_tokens: 4096, ...(system ? { system } : {}), messages: [{ role: "user", content: prompt }] }),
    });
    if (!res.ok) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const a = (await res.json()) as { content: { type: string; text?: string }[]; usage: { input_tokens: number; output_tokens: number } };
    return { text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""), input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens };
  };
}
"##;

const EVALS_WORKER: &str = r##"import { pgStore } from "./store";
import { tick, claude } from "./runner";
import { db } from "./db";

// The eval worker: claim a queued run, execute it with the concurrency cap, settle, repeat.
const store = pgStore(); const model = claude();
let stop = false;
process.on("SIGTERM", () => { stop = true; });
while (!stop) {
  try { if (!(await tick(store, model))) await new Promise((r) => setTimeout(r, 1000)); }
  catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); await new Promise((r) => setTimeout(r, 1000)); }
}
await db.end();
"##;

const EVALS_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore, type Case, type Run } from "./store";
import { grade, runEval, cost, type Model } from "./runner";

// A model that answers from a table, so a "prompt change" is a measurable change.
const table = (answers: Record<string, string>): Model => async ({ prompt }) => { await new Promise((r) => setTimeout(r, 2)); const q = Object.keys(answers).find((k) => prompt.includes(k)); return { text: q ? answers[q] : "I don't know", input_tokens: 100, output_tokens: 10 }; };
const CASES: Case[] = [
  { id: "c1", vars: { q: "capital of France" }, assert: [{ type: "equals", value: "Paris" }] },
  { id: "c2", vars: { q: "2+2" }, assert: [{ type: "contains", value: "4" }] },
  { id: "c3", vars: { q: "why is the sky blue" }, assert: [{ type: "llm-rubric", value: "mentions Rayleigh scattering" }] },
];
const run = (p: Partial<Run> = {}): Run => ({ id: "r1", dataset: "d", version: 1, label: "l", prompt: "Answer briefly: {{q}}", model: "claude-opus-5", state: "running", score: null, pass_rate: null, cost: 0, cases: 3, created_at: "", ...p });
const json = (body: unknown, method = "POST") => ({ method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

describe("evals", () => {
  test("a dataset is versioned: a PUT writes the next version and a run pins the one it saw", async () => {
    const app = createApp(memoryStore(), table({}), { inline: false });
    expect((await (await app.request("/api/datasets/qa", json({ cases: CASES.slice(0, 1) }, "PUT"))).json()).version).toBe(1);
    expect((await (await app.request("/api/datasets/qa", json({ cases: CASES }, "PUT"))).json()).version).toBe(2);
    expect((await (await app.request("/api/datasets/qa?version=1")).json()).cases.length).toBe(1);
    const r = await (await app.request("/api/runs", json({ dataset: "qa", version: 1, prompt: "{{q}}" }))).json();
    expect(r.version).toBe(1); expect(r.cases).toBe(1);
    expect((await app.request("/api/datasets/bad", json({ cases: [{ id: "x", vars: {}, assert: [] }] }, "PUT"))).status).toBe(400);
  });

  test("graders: exact and contains are pure; the rubric judge sees the rubric and the output only", async () => {
    const never: Model = async () => { throw new Error("should not be called"); };
    expect((await grade({ type: "equals", value: "paris" }, "  Paris ", never, "j")).pass).toBe(true);
    expect((await grade({ type: "contains", value: "4" }, "The answer is 4.", never, "j")).score).toBe(1);
    let seen = "";
    const judge: Model = async ({ prompt, system }) => { seen = system + prompt; return { text: 'Sure: {"score": 0.8, "reason": "close"}', input_tokens: 1, output_tokens: 1 }; };
    const g = await grade({ type: "llm-rubric", value: "mentions scattering" }, "Rayleigh scattering", judge, "claude-haiku-4-5");
    expect(g.score).toBe(0.8); expect(g.pass).toBe(true); expect(g.reason).toBe("close");
    expect(seen).toContain("mentions scattering"); expect(seen).toContain("Rayleigh scattering"); expect(seen).not.toContain("Answer briefly");
  });

  test("the runner caps concurrency and retries a failing call with backoff", async () => {
    let inflight = 0, peak = 0, calls = 0; const sleeps: number[] = [];
    const flaky: Model = async (req) => { calls++; inflight++; peak = Math.max(peak, inflight); await new Promise((r) => setTimeout(r, 3)); inflight--; if (req.prompt.includes("2+2") && calls < 4) throw new Error("529 overloaded"); return { text: "Paris 4", input_tokens: 1, output_tokens: 1 }; };
    const rs = await runEval(run(), CASES.slice(0, 2).concat(CASES.slice(0, 2).map((c) => ({ ...c, id: c.id + "b" }))), flaky, flaky, { concurrency: 2, maxRetries: 3, sleep: async (ms) => { sleeps.push(ms); } });
    expect(peak).toBe(2);
    expect(rs.find((r) => r.case_id === "c2")!.attempts).toBeGreaterThan(1);
    expect(sleeps.length).toBeGreaterThan(0);
    const dead = await runEval(run(), CASES.slice(1, 2), async () => { throw new Error("down"); }, flaky, { maxRetries: 1, sleep: async () => {} });
    expect(dead[0].error).toBe("down"); expect(dead[0].attempts).toBe(2); expect(dead[0].pass).toBe(false);
  });

  test("every case records score, cost and latency; cost follows the price table", async () => {
    const rs = await runEval(run(), CASES.slice(0, 2), table({ France: "Paris", "2+2": "four" }), table({}), { concurrency: 1 });
    expect(rs.map((r) => r.pass)).toEqual([true, false]);
    expect(rs[0].latency_ms).toBeGreaterThanOrEqual(1);
    expect(rs[0].cost).toBeCloseTo(cost("claude-opus-5", { input_tokens: 100, output_tokens: 10 }), 9);
    expect(cost("claude-opus-5", { input_tokens: 1e6, output_tokens: 1e6 })).toBe(30);
  });

  test("the leaderboard ranks runs of a dataset by score and the drill-down shows each case", async () => {
    const good = table({ France: "Paris", "2+2": "4", sky: '{"score":1,"reason":"yes"} Rayleigh' }), bad = table({ France: "Lyon" });
    const app = createApp(memoryStore(), good, { inline: true, judge: async () => ({ text: '{"score": 1}', input_tokens: 1, output_tokens: 1 }) });
    await app.request("/api/datasets/qa", json({ cases: CASES }, "PUT"));
    const a = await (await app.request("/api/runs", json({ dataset: "qa", prompt: "Q: {{q}}", label: "good" }))).json();
    const wait = async (id: string) => { for (let i = 0; i < 100 && (await (await app.request(`/api/runs/${id}`)).json()).state !== "done"; i++) await new Promise((r) => setTimeout(r, 5)); };
    await wait(a.id);
    const app2 = createApp(memoryStore(), bad, { inline: true, judge: async () => ({ text: '{"score": 0}', input_tokens: 1, output_tokens: 1 }) });
    await app2.request("/api/datasets/qa", json({ cases: CASES }, "PUT"));
    const b = await (await app2.request("/api/runs", json({ dataset: "qa", prompt: "Q: {{q}}", label: "bad" }))).json();
    await wait(b.id);
    const lb = await (await app.request("/api/leaderboard?dataset=qa")).json();
    expect(lb.runs.length).toBe(1); expect(lb.runs[0].label).toBe("good"); expect(lb.runs[0].score).toBe(1); expect(lb.runs[0].pass_rate).toBe(1);
    const drill = await (await app2.request(`/api/runs/${b.id}`)).json();
    expect(drill.results.length).toBe(3); expect(drill.results[0].output).toBe("Lyon"); expect(drill.results[0].grades[0].pass).toBe(false); expect(drill.score).toBe(0);
  });
});
"##;

const EVALS_SQL: &str = r##"-- A dataset version is immutable: PUT with the same name writes the next version, and a run
-- pins (name, version) so a leaderboard row never changes meaning after the fact.
create table if not exists datasets (
  name text not null,
  version int not null,
  cases jsonb not null,
  created_at timestamptz not null default now(),
  primary key (name, version)
);
create table if not exists eval_runs (
  id uuid primary key,
  dataset text not null,
  version int not null,
  label text not null,
  prompt text not null,
  model text not null,
  state text not null default 'queued',    -- queued | running | done | failed
  score real,                              -- mean case score, 0..1
  pass_rate real,
  cost real not null default 0,
  cases int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists eval_runs_ready on eval_runs (state, created_at);
create table if not exists eval_results (
  run_id uuid not null references eval_runs(id),
  case_id text not null,
  output text,
  score real not null default 0,
  pass boolean not null default false,
  grades jsonb not null default '[]',
  cost real not null default 0,
  latency_ms int not null default 0,
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  attempts int not null default 1,
  error text,
  primary key (run_id, case_id)
);
"##;

const EVALS_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Row = { id: string; label: string; dataset: string; version: number; model: string; cases: number; score: number; pass_rate: number; cost: number; p50_ms: number };
type Result = { case_id: string; output: string | null; score: number; pass: boolean; grades: { type: string; score: number; reason?: string }[]; cost: number; latency_ms: number; attempts: number; error: string | null };
type Run = Row & { state: string; prompt: string; results: Result[] };

const SAMPLE = { cases: [
  { id: "capital", vars: { q: "What is the capital of France?" }, assert: [{ type: "equals", value: "Paris" }] },
  { id: "sum", vars: { q: "What is 12 * 12?" }, assert: [{ type: "contains", value: "144" }] },
  { id: "sky", vars: { q: "Why is the sky blue?" }, assert: [{ type: "llm-rubric", value: "Explains Rayleigh scattering in one or two sentences." }] },
] };

// The leaderboard: one row per finished run of a dataset, best score first, with its cost and
// median latency; click a row for every case, its output and what each grader said.
export default function Home() {
  const [datasets, setDatasets] = useState<{ name: string; version: number; cases: number }[]>([]);
  const [dataset, setDataset] = useState("qa");
  const [rows, setRows] = useState<Row[]>([]);
  const [run, setRun] = useState<Run | null>(null);
  const [prompt, setPrompt] = useState("Answer in as few words as possible: {{q}}");
  const [model, setModel] = useState("claude-opus-5");
  const [busy, setBusy] = useState(false);

  const load = () => {
    fetch("/api/datasets").then((r) => r.json()).then((d) => setDatasets(d.datasets)).catch(() => {});
    fetch(`/api/leaderboard?dataset=${encodeURIComponent(dataset)}`).then((r) => r.json()).then((d) => setRows(d.runs)).catch(() => {});
  };
  useEffect(load, [dataset]);

  async function seed() { await fetch("/api/datasets/qa", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(SAMPLE) }); setDataset("qa"); load(); }
  async function start() {
    setBusy(true);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ dataset, prompt, model }) });
    const d = await res.json();
    if (!res.ok) { alert(d.error.message); setBusy(false); return; }
    for (;;) { const r: Run = await (await fetch(`/api/runs/${d.id}`)).json(); setRun(r); if (r.state === "done" || r.state === "failed") break; await new Promise((x) => setTimeout(x, 1000)); }
    setBusy(false); load();
  }

  return (
    <main>
      <h1>{{NAME}} — evals</h1>
      <p>Versioned datasets, a runner with a concurrency cap and retries, exact / contains / rubric graders. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>
      <pre>{`curl -X PUT localhost:8000/api/datasets/qa -H 'content-type: application/json' -d '{"cases":[{"id":"c1","vars":{"q":"2+2"},"assert":[{"type":"equals","value":"4"}]}]}'\ncurl localhost:8000/api/runs -H 'content-type: application/json' -d '{"dataset":"qa","prompt":"Answer: {{q}}","model":"claude-opus-5"}'`}</pre>
      <p>Dataset: <select value={dataset} onChange={(e) => setDataset(e.target.value)}>{[...new Set([dataset, ...datasets.map((d) => d.name)])].map((n) => <option key={n}>{n}</option>)}</select> {datasets.length === 0 && <button onClick={seed}>Create sample dataset</button>}</p>
      <p>Prompt: <input value={prompt} onChange={(e) => setPrompt(e.target.value)} style={{ width: "50%" }} /> Model: <input value={model} onChange={(e) => setModel(e.target.value)} /> <button onClick={start} disabled={busy}>{busy ? "Running…" : "Run"}</button></p>
      <table border={1} cellPadding={4} style={{ borderCollapse: "collapse" }}>
        <thead><tr><th>#</th><th>run</th><th>model</th><th>v</th><th>score</th><th>pass</th><th>cost</th><th>p50</th></tr></thead>
        <tbody>{rows.map((r, i) => (
          <tr key={r.id} style={{ cursor: "pointer" }} onClick={() => fetch(`/api/runs/${r.id}`).then((x) => x.json()).then(setRun)}>
            <td>{i + 1}</td><td>{r.label}</td><td>{r.model}</td><td>{r.version}</td><td>{(r.score * 100).toFixed(0)}%</td><td>{(r.pass_rate * 100).toFixed(0)}%</td><td>${r.cost.toFixed(4)}</td><td>{r.p50_ms} ms</td>
          </tr>))}</tbody>
      </table>
      {run && (
        <section>
          <h2>{run.label} <small>{run.state}</small></h2>
          <p><code>{run.prompt}</code></p>
          <table border={1} cellPadding={4} style={{ borderCollapse: "collapse", width: "100%" }}>
            <thead><tr><th>case</th><th>output</th><th>grades</th><th>score</th><th>cost</th><th>ms</th></tr></thead>
            <tbody>{run.results.map((r) => (
              <tr key={r.case_id} style={{ background: r.error ? "#fdd" : r.pass ? "#dfd" : "#ffd" }}>
                <td>{r.case_id}{r.attempts > 1 ? ` ×${r.attempts}` : ""}</td><td style={{ whiteSpace: "pre-wrap", maxWidth: 400 }}>{r.error ? `Error: ${r.error}` : r.output}</td>
                <td>{r.grades.map((g, i) => <div key={i}>{g.type}: {g.score}{g.reason ? ` — ${g.reason}` : ""}</div>)}</td><td>{r.score.toFixed(2)}</td><td>${r.cost.toFixed(5)}</td><td>{r.latency_ms}</td>
              </tr>))}</tbody>
          </table>
        </section>
      )}
    </main>
  );
}
"##;

const EVALS_CLAUDE_MD: &str = r##"# {{NAME}} — evals

This service runs prompts against versioned datasets and ranks the runs. It is modelled on
promptfoo: a prompt with `{{vars}}`, a provider (a model id), tests with `vars` and `assert`
rows of type `equals` / `contains` / `llm-rubric`, and a result per case carrying pass, score,
latency, cost and tokens. What promptfoo keeps in a YAML file and a local cache, this keeps in
Postgres: a dataset `PUT` is a new immutable version, a run pins one, and a leaderboard compares
every finished run of a dataset. "Done" means: a dataset is versioned, `POST /api/runs` queues
a run, `tick` executes it with a concurrency cap and retries, every case is a row the moment it
finishes, and `bun test` proves the graders and the runner against a table-driven model.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes; dataset validation (`caseSchema`); run creation pinning `(dataset, version)`; the inline drain; `/api/leaderboard` |
| `backend/src/runner.ts` | `render` (`{{var}}` substitution), `grade` (the three graders), `runCase` (one case with retries and backoff), `runEval` (the concurrency pool), `summarize`, `tick`, `PRICES`/`cost`, `claude()` |
| `backend/src/store.ts` | `Store`: datasets, runs, results; `pgStore` and `memoryStore` |
| `backend/src/worker.ts` | The worker process: `tick` in a loop, 1s idle backoff, SIGTERM stops it |
| `backend/src/app.test.ts` | Five tests against `memoryStore` and a table-driven `Model` |
| `backend/src/server.ts`, `db.ts` | Listener and pool (from the stack) |
| `backend/migrations/0002_evals.sql` | `datasets`, `eval_runs`, `eval_results` |
| `frontend/app/page.tsx` | The leaderboard and the per-case drill-down; a "create sample dataset" button |

### The request path

1. `PUT /api/datasets/:name {cases}` — 1 to 5,000 cases, each `{id, vars, assert[≥1]}` with `assert.type` in the three kinds; duplicate ids are `400`. The store inserts `(name, max(version)+1)`. Response `201 {name, version, cases}`.
2. `POST /api/runs {dataset, version?, label?, prompt, model?}` — the dataset (latest version unless pinned) is loaded; `404` if absent. An `eval_runs` row is inserted `queued` with `cases = d.cases.length` and `model` defaulting to `MODEL`. Response `201 {id, dataset, version, cases}`.
3. If `INLINE_WORKER` is not `false`, the API schedules `drain()`; in the cluster the worker does it.
4. `tick`: `claimRun` takes the oldest `queued` run with `for update skip locked` and marks it `running`; the pinned dataset version is loaded (missing → run `failed`).
5. `runEval` starts `min(EVAL_CONCURRENCY, cases)` workers over one shared cursor; each calls `runCase`.
6. `runCase`: `render(run.prompt, kase.vars)` → `model({model: run.model, prompt})`; a throw is retried up to `maxRetries` (2) times with `250ms · 2^n · jitter`; after that the result is written with `error` and `pass: false`. On success every assert is graded; the case score is the mean of grade scores, `pass` is every grade passing, cost is the model's plus the judge's.
7. `onResult` writes the row (`putResult`, an upsert on `(run_id, case_id)`) as each case finishes, so a run that dies shows what it got to.
8. The run is settled `done` with `summarize` — mean score, pass rate, total cost, p50 latency. `GET /api/runs/:id` recomputes the summary from the rows and returns them; `GET /api/leaderboard?dataset=` does that for every `done` run and sorts by score, then cost.

### Data model

| Table | Column that matters | Why |
|---|---|---|
| `datasets` | `primary key (name, version)` | A version is immutable; `PUT` inserts the next one; nothing updates a row |
| `datasets` | `cases jsonb` | The whole dataset in one column; read once per run |
| `eval_runs` | `dataset`, `version` | The pin — a leaderboard row never changes meaning after a dataset edit |
| `eval_runs` | `prompt`, `model`, `label` | What was tested; `label` defaults to `model · prompt[:40]` |
| `eval_runs` | `state` | `queued → running → done \| failed`; the index `(state, created_at)` serves the claim |
| `eval_runs` | `score`, `pass_rate`, `cost` | Written at settle; the drill-down recomputes from results anyway |
| `eval_results` | `primary key (run_id, case_id)` | One row per case per run; the upsert makes a re-run of a case idempotent |
| `eval_results` | `grades jsonb` | Every assert's `{type, score, pass, reason?}` — what the drill-down shows |
| `eval_results` | `attempts`, `error`, `latency_ms` | Retries and the last error are visible per case |

## Invariants

1. **A dataset version never changes.** `putDataset` only inserts; there is no update path in either store. Guarded by `app.test.ts` "a dataset is versioned: a PUT writes the next version and a run pins the one it saw".
2. **A run pins a version at creation.** `createRun` stores `version` from the dataset it loaded, and `tick` reloads that exact version. Same test (`r.version === 1` after version 2 exists).
3. **Case ids are unique within a version.** Checked in the handler before insert; the results table's primary key depends on it. Same test's `400` on empty asserts; the duplicate check is the `Set` size comparison in `app.ts`.
4. **Exact graders are pure.** `equals` and `contains` normalise whitespace and case and never call the judge. Guarded by "graders: exact and contains are pure; the rubric judge sees the rubric and the output only" (the judge is `never`, which throws if called).
5. **The rubric judge is cold.** It receives the rubric and the output — not the prompt, not the model under test. Same test (`seen` lacks `"Answer briefly"`).
6. **A rubric score is clamped to `[0, 1]` and passes at `≥ 0.5`;** an unparseable reply is `score 0, pass false, reason "judge gave no score"`. `grade` in `runner.ts`; the parse test covers the happy path — add one for prose if you touch it.
7. **Concurrency is capped.** `runEval` starts at most `concurrency` workers. Guarded by "the runner caps concurrency and retries a failing call with backoff" (`peak === 2`).
8. **Retries are bounded and backed off.** `attempt > maxRetries` ends with `error`; each retry sleeps through the injected `sleep`. Same test (`attempts === 2` for `maxRetries: 1`, sleeps recorded).
9. **Every result carries cost and latency from the price table and the clock.** `cost(model, usage)` uses `PRICES`; unknown models cost 0. Guarded by "every case records score, cost and latency; cost follows the price table".
10. **The leaderboard only ranks `done` runs and recomputes from results.** `app.ts` filters `state === "done"` and calls `summarize(getResults)`. Guarded by "the leaderboard ranks runs of a dataset by score and the drill-down shows each case".
11. **Results are written as they finish, not at the end.** `tick` passes `onResult: store.putResult`. Not separately tested — `tick` is the guard; a test that throws from the third case and asserts two rows exist is the one to add.
12. **The model is a function of `{model, system?, prompt}`.** `claude()` is the default; tests use `table()`. Every test depends on it.

## Extending it

**Add an assert type** (say `regex`). Add it to the `z.enum` in `app.ts`'s `caseSchema` and to `Assert.type` in `store.ts`; add a branch in `grade` before the rubric fallthrough; pure graders never call `judge`. Test: one `grade(...)` call with `never` as the judge, as the exact graders are tested. No migration — asserts live inside `cases jsonb`.

**Use a different judge model.** `createApp(store, model, {judge})` takes a second `Model`; `RunOpts.judgeModel` names the model id the judge is called with (default: the run's own model). To make it a request field, add `judge_model` to the `POST /api/runs` schema, a column on `eval_runs` (migration), and pass it through `tick`'s `opts`. Test: assert the judge fake saw the id.

**Add a price.** `PRICES` in `runner.ts`, or `PRICES='{"model":[in,out]}'` in the environment without a code change. Test: `cost(...)` for the new id.

**Add a per-case timeout.** `claude()` has none. Pass `signal: AbortSignal.timeout(ms)` in the fetch and let the throw be retried like any other. Test: a model that never resolves, `maxRetries: 0`, assert `error` contains `abort`.

**Compare several prompts at once.** A run is one prompt × one model. Post several runs with the same dataset and labels; the leaderboard is the comparison. If you want one request to fan out, add a `POST /api/experiments` that creates N runs — no runner change.

**Resume a run.** Results are per case and upserted, so a resumed run can skip cases that already have a row without `error`. Filter `ds.cases` by existing results in `tick` before `runEval`. Test: two rows present, model called once for a three-case dataset.

**Add a route.** Into the chain in `app.ts`, zod on the body, `{error: {message, code}}` on failure — never a detached `app.get`.

## Operating it

| Env var | Required | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | yes | Model under test and judge |
| `MODEL` | no | Default model id for runs, `claude-opus-5` |
| `EVAL_CONCURRENCY` | no | Cases in flight per run, default 4 |
| `PRICES` | no | JSON `{"model":[in_per_M, out_per_M]}` merged over the built-in table |
| `INLINE_WORKER` | no | Anything but `false`: the API drains the queue itself |
| `DATABASE_URL` | yes | Datasets, runs, results, and the queue |
| `PORT` | no | Default 8000 |

**Processes.** API plus worker (`bun run worker`). One worker executes one run at a time with `EVAL_CONCURRENCY` cases in flight; more workers run more runs in parallel. `k8s/base` has the API manifest; add a worker Deployment (no Service, no probes) and set `INLINE_WORKER=false` on the API.

**Per-replica today:** nothing — the claim, the results and the summary are rows. Provider rate limits are per key, not per replica: `EVAL_CONCURRENCY × workers` is your concurrent call count against Anthropic.

**Failure modes.** Provider 429/529 on a case → retried twice with backoff, then the case has `error` and the run continues. Worker dies mid-run → the run stays `running` (no lease); its finished cases are in `eval_results`. Dataset version deleted by hand → run `failed`. Judge returns prose → that grade is 0 with a reason; the case fails. Unknown model id → the provider's 4xx becomes the case error after retries; cost is 0.

**What to watch.** `error` per run (a provider incident shows as a run with every case errored), `attempts > 1` rate, run duration versus `cases / EVAL_CONCURRENCY × p50`, runs `running` longer than that, judge cost as a share of run cost.

## Ceilings

- **No lease on a running run.** `claimRun` only takes `queued`; a crash leaves `running` forever. Upgrade: reclaim `running` with an old `updated_at`, and resume from existing results.
- **No per-call timeout.** `claude()` waits as long as the socket does; a hung call holds one of the concurrency slots. Upgrade: `AbortSignal.timeout` in the fetch.
- **The judge defaults to the model under test.** With no `judgeModel`, the rubric is graded by the same model id that produced the output. Upgrade: a fixed judge id in the environment.
- **The leaderboard is O(runs × cases) per request.** It loads every result of every done run. Upgrade: read `score`/`pass_rate`/`cost` from `eval_runs` (already written at settle) and compute p50 there too.
- **A dataset is one JSON column.** 5,000 cases is the API cap; larger datasets belong in a `cases` table.
- **One prompt, one model per run.** No matrix; N runs are N requests.
- **No auth, no owner.** Anyone who reaches the API can write datasets and start runs that cost money. Put a gateway in front.
- **`listRuns` is `limit 100`, no cursor.**

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const EVALS_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the service.

## Run

    make demo                     # infra, migrations, seed, API :8000, page :3000 — the API executes runs itself
    make check                    # typecheck both halves, bun test the backend
    cd backend && bun run worker  # a worker; set INLINE_WORKER=false on the API when one runs
    make migrate

`ANTHROPIC_API_KEY` in `backend/.env`. Without it a run finishes `done` with every case
`error: "anthropic 401: …"` after three attempts.

## Every route, with curl

    J='-H content-type:application/json'

    curl -s localhost:8000/api/health
    # {"status":"ok"}

    curl -s -X PUT $J localhost:8000/api/datasets/qa -d '{"cases":[
      {"id":"capital","vars":{"q":"What is the capital of France?"},"assert":[{"type":"equals","value":"Paris"}]},
      {"id":"sum","vars":{"q":"What is 12 * 12?"},"assert":[{"type":"contains","value":"144"}]},
      {"id":"sky","vars":{"q":"Why is the sky blue?"},"assert":[{"type":"llm-rubric","value":"Explains Rayleigh scattering in one or two sentences."}]}]}'
    # 201 {"name":"qa","version":1,"cases":3}
    # PUT again → {"name":"qa","version":2,"cases":3}; version 1 is unchanged
    # 400 {"error":{"message":"cases required: [{id, vars, assert:[{type, value}]}]","code":"invalid"}}
    # 400 {"error":{"message":"case ids must be unique","code":"invalid"}}

    curl -s localhost:8000/api/datasets
    # {"datasets":[{"name":"qa","version":2,"cases":3,"created_at":"…"},{"name":"qa","version":1,…}]}

    curl -s 'localhost:8000/api/datasets/qa?version=1'
    # {"name":"qa","version":1,"cases":[…],"created_at":"…"}   — omit version for the latest; 404 not_found otherwise

    curl -s $J localhost:8000/api/runs -d '{"dataset":"qa","prompt":"Answer in as few words as possible: {{q}}","model":"claude-opus-5","label":"terse"}'
    # 201 {"id":"0191…","dataset":"qa","version":2,"cases":3}
    # add "version":1 to pin an older one; 404 {"error":{"message":"no such dataset","code":"not_found"}}

    curl -s localhost:8000/api/runs/0191…
    # {"id":"0191…","dataset":"qa","version":2,"label":"terse","prompt":"…","model":"claude-opus-5","state":"done",
    #  "score":0.93,"pass_rate":1,"cost":0.0041,"p50_ms":812,"cases":3,
    #  "results":[
    #   {"case_id":"capital","output":"Paris","score":1,"pass":true,"grades":[{"type":"equals","score":1,"pass":true}],"cost":0.0009,"latency_ms":640,"input_tokens":31,"output_tokens":2,"attempts":1,"error":null},
    #   {"case_id":"sky","output":"Sunlight scatters off air molecules…","score":0.8,"pass":true,"grades":[{"type":"llm-rubric","score":0.8,"pass":true,"reason":"Names Rayleigh scattering briefly."}],…},
    #   {"case_id":"sum","output":"144","score":1,"pass":true,…}]}
    # While running, the results list grows as cases finish.

    curl -s 'localhost:8000/api/runs?dataset=qa'
    # {"runs":[{"id":…,"label":"terse","state":"done","score":0.93,…}]}

    curl -s 'localhost:8000/api/leaderboard?dataset=qa'
    # {"runs":[{"id":…,"label":"terse","model":"claude-opus-5","version":2,"cases":3,"score":0.93,"pass_rate":1,"cost":0.0041,"p50_ms":812,"created_at":"…"}]}
    # best score first, cheaper first on a tie; only done runs

    curl -s -X POST localhost:8000/api/tick
    # {"ran":true}   — executed one queued run in this request; {"ran":false} when the queue is empty

## How the tests work

`backend/src/app.test.ts` uses no database and no provider:

- **`table({...})`** is a `Model` that answers by substring match on the rendered prompt and
  waits 2 ms so latency is measurable. A "prompt change" in a test is a different answer table.
- **`memoryStore()`** implements versioning (`max(version) + 1`), the claim, and the upsert on
  `(run_id, case_id)` in arrays and Maps.
- **`grade` is tested directly** with `never` as the judge for the pure graders, and a
  recording judge for the rubric.
- **`runEval` is tested directly** with injected `concurrency`, `maxRetries` and `sleep`, so
  backoff is asserted by the recorded delays, not by waiting.
- **The HTTP tests use `{inline: true}`** and a stubbed `judge`, then poll `GET /api/runs/:id`
  until `done`.

Run one: `cd backend && bun test -t "leaderboard"`.

## Adding a test

```ts
test("a result row exists for each finished case before the run settles", async () => {
  const store = memoryStore();
  const ds = await store.putDataset("d", CASES.slice(0, 2));
  await store.createRun(run({ id: "r2", dataset: "d", version: ds.version, state: "queued", cases: 2 }));
  let n = 0;
  const model: Model = async (req) => { if (++n === 2) throw new Error("boom"); return { text: "Paris", input_tokens: 1, output_tokens: 1 }; };
  await tick(store, model, model, { concurrency: 1, maxRetries: 0 });
  const rs = await store.getResults("r2");
  expect(rs.length).toBe(2); expect(rs.find((r) => r.case_id === "c2")?.error).toBe("boom");
});
```

Import `tick` from `./runner` for that one. A test that needs a real key is a script you run
by hand, not a `bun test`.
"##;

const EVALS_README_MD: &str = r##"# {{NAME}}

Versioned datasets, a runner with retries and a concurrency cap, three graders, and a
leaderboard — promptfoo's eval loop as a service with a database behind it.

## What you get

- `PUT /api/datasets/:name` writes an immutable version; a run pins the version it saw, so a
  score never changes meaning after the dataset is edited.
- `POST /api/runs` queues a prompt × model against a dataset; a worker (or the API itself, until
  you have one) executes it with `EVAL_CONCURRENCY` cases in flight, retrying provider failures
  with jittered backoff.
- Graders: `equals` and `contains` (whitespace- and case-normalised, pure), and `llm-rubric`
  (a judge that sees the rubric and the output only, scoring 0–1, passing at 0.5).
- A row per case the moment it finishes: output, every grade with its reason, cost from a price
  table, latency, tokens, attempts, error.
- `GET /api/leaderboard?dataset=` ranks finished runs by score then cost, with pass rate and
  p50 latency; `GET /api/runs/:id` is the per-case drill-down.
- A Next.js page for both; Hono API; kustomize overlays; five tests with no database and no key.

## Five minutes

    cp backend/.env.example backend/.env    # add ANTHROPIC_API_KEY=…
    make demo

    curl -s -X PUT localhost:8000/api/datasets/qa -H content-type:application/json -d '{"cases":[
      {"id":"capital","vars":{"q":"What is the capital of France?"},"assert":[{"type":"equals","value":"Paris"}]},
      {"id":"sum","vars":{"q":"What is 12 * 12?"},"assert":[{"type":"contains","value":"144"}]}]}'
    # {"name":"qa","version":1,"cases":2}

    curl -s localhost:8000/api/runs -H content-type:application/json \
      -d '{"dataset":"qa","prompt":"Answer in one word: {{q}}","label":"one-word"}'
    # {"id":"0191…","dataset":"qa","version":1,"cases":2}

    curl -s localhost:8000/api/runs -H content-type:application/json \
      -d '{"dataset":"qa","prompt":"{{q}} Explain your reasoning.","label":"verbose"}'

    sleep 15; curl -s 'localhost:8000/api/leaderboard?dataset=qa'
    # {"runs":[{"label":"one-word","score":1,"pass_rate":1,"cost":0.0012,"p50_ms":700,…},{"label":"verbose","score":0.5,…}]}
    # "equals" fails the verbose prompt on "capital": the output is a paragraph, not "Paris".

Open <http://localhost:3000>: the leaderboard, click a row for every case and what each grader
said. There is a button to create this sample dataset.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not yet probe the database) |
| GET | `/api/datasets` | none | Every version of every dataset with case counts |
| PUT | `/api/datasets/:name` | none | `{cases: [{id, vars, assert: [{type, value}]}]}` → next version |
| GET | `/api/datasets/:name?version=` | none | One version (latest by default) with its cases |
| POST | `/api/runs` | none | `{dataset, version?, label?, prompt, model?}` → `201 {id, version, cases}` |
| GET | `/api/runs?dataset=` | none | Latest 100 runs |
| GET | `/api/runs/:id` | none | The run, its summary, and every case's result |
| GET | `/api/leaderboard?dataset=` | none | Finished runs ranked by score, then cost |
| POST | `/api/tick` | none | Execute one queued run in this request |

No authentication: anyone who reaches the API can start runs that cost money. Put a gateway
in front before exposing it beyond the cluster.

## Compared with promptfoo

**Same shape, so their docs describe this**

- A prompt template with `{{var}}` substitution; a provider named by model id.
- Tests as `vars` + `assert`, with `equals`, `contains` and `llm-rubric` meaning what they mean
  there; a rubric grade is a 0–1 score with a reason, passing at 0.5.
- Per-result `pass`, `score`, latency, cost and token usage; provider calls retried with backoff.
- Results viewed as a table of runs and a drill-down of cases.

**Better here**

- Datasets are versioned server-side and runs pin a version — a leaderboard row is reproducible
  by construction, not by remembering which YAML you ran.
- A durable queue: runs are rows, results land per case as they finish, and a second worker
  doubles throughput with no configuration.
- One leaderboard across runs of a dataset, over time, with cost beside score.
- Typed end to end (`AppType` → the frontend client) and tested without a provider: graders,
  the concurrency cap and the backoff are each asserted in under a second.
- Kubernetes manifests, encrypted env secrets, one deploy command — the same stack as the rest
  of your services.

**Not here yet**

- The rest of promptfoo's assertion library: `javascript`/`python` functions, `regex`,
  `is-json`, `similar` (embeddings), `latency`/`cost` thresholds, `moderation`,
  `factuality`, `model-graded-closedqa`, weighted asserts and a pass threshold per test.
- Providers other than Claude; a judge model separate from the model under test is possible
  (`opts.judge`, `judgeModel`) but not a request field.
- A prompt × provider matrix in one run; here that is N runs and the leaderboard.
- Transforms, scenarios, test generation, dataset import from CSV/HF, and the CLI /
  `promptfooconfig.yaml` workflow.
- Red-teaming, the GitHub Action, result sharing, caching of identical calls.
- Auth and per-user datasets.

## Production

**Processes.** The API Deployment, plus a worker Deployment running `bun src/worker.ts` (or
`dist/worker.js`); then `INLINE_WORKER=false` on the API. `k8s/base` ships the API manifest
only — add the worker with no Service and no probes.

**Environments and secrets.** `k8s/overlays/{dev,prod}`; `ANTHROPIC_API_KEY`, `DATABASE_URL`
and optionally `PRICES` from `backend/.env.age` via `make k8s-secrets ENV=…`. Dev and prod
never share a database.

**Scaling.** `EVAL_CONCURRENCY` cases per run per worker; worker replicas for runs in parallel.
The product of the two is your concurrent call count against the provider's rate limit.

**Probes.** `/api/health` and `/api/health/ready` on the API; the worker has none — watch for
runs `running` longer than `cases ÷ EVAL_CONCURRENCY × p50`.

**Migrations.** `0002_evals.sql`; new ones through the migrate init container.

**What pages you.** A run with every case in `error` (provider incident or a bad key), runs
stuck in `running` (a worker died; there is no lease yet), judge cost climbing past model cost,
a leaderboard request slower than a second (it loads every result of every run).

## Roadmap

- A lease on `running` runs and resume from existing results.
- A per-call timeout in `claude()`.
- A judge model id per run, defaulting to a fixed judge rather than the model under test.
- Leaderboard from the settled columns instead of reloading every result.
- More assert types, starting with `regex` and `is-json`.
- A `cases` table for datasets past 5,000 rows.
- Auth and ownership.
"##;

const EVALS_REVIEWER: &str = r##"---
name: grader-integrity
description: Run on any change to backend/src/runner.ts, the dataset or run schemas in app.ts, the store's versioning or result upsert, or the leaderboard. Checks that a score still means what it meant yesterday — graders, pinning, retries, cost — and reports only what would make a leaderboard row wrong or unreproducible.
tools: Read, Grep, Glob, Bash
---

You review what a number on the leaderboard is made of. A grader that leaks the prompt, a run
that reads the latest dataset instead of its pin, or a cost from the wrong table makes every
comparison a lie without failing any test.

Report each as `path:line — what — the run it corrupts — the fix`.

Check:
1. **Immutability.** No code path updates or deletes a `datasets` row; `putDataset` inserts
   `max(version) + 1` in the same statement (Postgres) so two concurrent PUTs cannot share a
   version number. `primary key (name, version)` still exists.
2. **Pinning.** `createRun` stores the `version` of the dataset it loaded; `tick` calls
   `getDataset(run.dataset, run.version)`, never `getDataset(run.dataset)`.
3. **Case ids.** The handler rejects duplicates before insert; `eval_results`' primary key is
   `(run_id, case_id)`. A duplicate id would silently overwrite a result.
4. **Pure graders.** `equals` and `contains` return before any `judge` call; both use `norm`
   (trim, lower, collapse whitespace) on both sides.
5. **Cold judge.** The `llm-rubric` branch passes `a.value` and `output` only; nothing from
   `run.prompt`, `kase.vars` or `run.model` reaches the judge's prompt or system.
6. **Rubric scoring.** `score` is clamped to `[0, 1]`, `pass` is `score >= 0.5`, a parse
   failure is `{score: 0, pass: false, reason: "judge gave no score"}`, and a non-numeric score
   is 0, not `NaN` (`Number(v.score) || 0`).
7. **Case score.** Mean of grade scores; `pass` is `every(g => g.pass)`; a case with no asserts
   cannot exist (the schema says `min(1)`).
8. **Retries.** Only a thrown model call is retried; a grading failure is not a retry. Attempts
   are counted from 1, the final result carries `attempts` and the last `error`, and the sleep
   is the injected one so tests do not wait.
9. **Concurrency.** `runEval` starts `min(concurrency, cases)` workers over one shared index
   and never starts a case twice.
10. **Cost.** `cost(run.model, completion) + judgeCost`, with judge cost priced at the judge's
    model id, not the run's; an unknown model prices at `[0, 0]`, not a throw.
11. **Latency.** `latency_ms` measures the successful model call only, not the grading.
12. **Write-as-you-go.** `tick` passes `onResult: store.putResult` and `putResult` is an
    upsert; the run is settled only after `runEval` resolves.
13. **Leaderboard.** Filters `state === "done"`, recomputes from `getResults`, sorts by score
    descending then cost ascending. A `failed` or `running` run never appears.
14. **Render.** `render` substitutes only `{{name}}` with word characters and leaves unknown
    names as empty strings — a template typo becomes a visible empty slot, not a literal
    `{{q}}` in the prompt.
15. **Schema bounds.** `cases` 1–5,000, `prompt` ≤ 20,000, `label` ≤ 80; a new field has a
    bound.

End with one line: `grader-integrity: N findings`, and if 0, which items you checked.
"##;
