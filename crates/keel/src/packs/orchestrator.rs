//! orchestrator: planner, dependency queue, workers and a judge, like CrewAI / AutoGen ════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", ORCH_APP.into()),
        ("backend/src/store.ts", ORCH_STORE.into()),
        ("backend/src/engine.ts", ORCH_ENGINE.into()),
        ("backend/src/worker.ts", ORCH_WORKER.into()),
        ("backend/src/app.test.ts", ORCH_TEST.into()),
        ("backend/migrations/0002_orchestrator.sql", ORCH_SQL.into()),
        ("frontend/app/page.tsx", f(ORCH_PAGE)),
    ]
}

const ORCH_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { plan, tick, claude, TOOLS, type Model } from "./engine";

// POST /api/runs plans a goal into a task graph and stores it; the worker (bun run worker)
// drains the queue. With no worker running — the first five minutes — the API drains it itself
// in the background (INLINE_WORKER=false turns that off for the cluster, where the Deployment
// does it). GET /api/runs/:id is the live graph the page polls.

const inlineWorker = process.env.INLINE_WORKER !== "false";

export function createApp(store: Store, model: Model = claude(), opts: { inline?: boolean } = {}) {
  const inline = opts.inline ?? inlineWorker;
  const drain = async () => { try { while (await tick(store, model)); } catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); } };
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description })) }))

    .post("/api/runs", async (c) => {
      const p = z.object({ goal: z.string().min(1).max(4000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "goal required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.createRun({ id, goal: p.data.goal, state: "planning", input_tokens: 0, output_tokens: 0, created_at: new Date().toISOString() });
      try {
        const g = await plan(p.data.goal, model);
        await store.addTasks(g.tasks.map((t) => ({ id: crypto.randomUUID(), run_id: id, ...t, budget_tokens: Number(process.env.TASK_BUDGET_TOKENS ?? 20_000), state: "queued", attempts: 0, output: null, verdict: null, input_tokens: 0, output_tokens: 0 })));
        await store.updateRun(id, { state: "running", input_tokens: g.input_tokens, output_tokens: g.output_tokens });
      } catch (e) {
        await store.updateRun(id, { state: "failed" });
        return c.json({ error: { message: e instanceof Error ? e.message : String(e), code: "plan_failed" } }, 502);
      }
      if (inline) setTimeout(drain, 0);
      return c.json({ id, tasks: (await store.getTasks(id)).length }, 201);
    })
    .post("/api/tick", async (c) => c.json({ ran: await tick(store, model) }))
    .get("/api/runs", async (c) => c.json({ runs: await store.listRuns(50) }))
    .get("/api/runs/:id", async (c) => {
      const run = await store.getRun(c.req.param("id"));
      if (!run) return c.json({ error: { message: "no such run", code: "not_found" } }, 404);
      return c.json({ ...run, tasks: await store.getTasks(run.id) });
    });
  return app;
}
const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const ORCH_STORE: &str = r##"import { db } from "./db";

export type Run = { id: string; goal: string; state: "planning" | "running" | "done" | "failed"; input_tokens: number; output_tokens: number; created_at: string };
export type Task = { id: string; run_id: string; key: string; description: string; expected_output: string; depends_on: string[]; tools: string[]; budget_tokens: number; state: "queued" | "running" | "done" | "failed" | "dead"; attempts: number; output: string | null; verdict: { pass: boolean; reason: string } | null; input_tokens: number; output_tokens: number };

export interface Store {
  createRun(r: Run): Promise<void>;
  updateRun(id: string, p: Partial<Run>): Promise<void>;
  getRun(id: string): Promise<Run | null>;
  listRuns(limit: number): Promise<Run[]>;
  addTasks(ts: Task[]): Promise<void>;
  getTasks(runId: string): Promise<Task[]>;
  /// The queue: one queued task whose dependencies are all done, claimed for this worker.
  /// Dependency release is the query itself — nothing has to notify anything when a task ends.
  claimReady(): Promise<Task | null>;
  updateTask(id: string, p: Partial<Task>): Promise<void>;
}

export function pgStore(): Store {
  return {
    createRun: async (r) => { await db`insert into runs (id, goal, state) values (${r.id}, ${r.goal}, ${r.state})`; },
    updateRun: async (id, p) => { await db`update runs set state = coalesce(${p.state ?? null}, state), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens), output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`; },
    getRun: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    listRuns: async (limit) => db<Run[]>`select * from runs order by created_at desc limit ${limit}`,
    addTasks: async (ts) => db.begin(async (tx) => { for (const t of ts) await tx`insert into tasks (id, run_id, key, description, expected_output, depends_on, tools, budget_tokens) values (${t.id}, ${t.run_id}, ${t.key}, ${t.description}, ${t.expected_output}, ${t.depends_on}, ${t.tools}, ${t.budget_tokens})`; }),
    getTasks: async (runId) => db<Task[]>`select * from tasks where run_id = ${runId} order by key`,
    claimReady: async () => (await db<Task[]>`update tasks set state = 'running', attempts = attempts + 1, updated_at = now() where id = (
        select t.id from tasks t where t.state = 'queued'
          and not exists (select 1 from tasks d where d.run_id = t.run_id and d.key = any(t.depends_on) and d.state <> 'done')
        order by t.updated_at limit 1 for update skip locked) returning *`)[0] ?? null,
    updateTask: async (id, p) => { await db`update tasks set state = coalesce(${p.state ?? null}, state), description = coalesce(${p.description ?? null}, description), output = coalesce(${p.output ?? null}, output),
      verdict = coalesce(${p.verdict ? db.json(p.verdict) : null}, verdict), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens), output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`; },
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>(); const tasks: Task[] = [];
  return {
    createRun: async (r) => { runs.set(r.id, { ...r }); },
    updateRun: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p); },
    getRun: async (id) => runs.get(id) ?? null,
    listRuns: async (limit) => [...runs.values()].reverse().slice(0, limit),
    addTasks: async (ts) => { tasks.push(...ts.map((t) => ({ ...t }))); },
    getTasks: async (runId) => tasks.filter((t) => t.run_id === runId).map((t) => ({ ...t })),
    claimReady: async () => {
      const t = tasks.find((t) => t.state === "queued" && t.depends_on.every((k) => tasks.find((d) => d.run_id === t.run_id && d.key === k)?.state === "done"));
      if (!t) return null; t.state = "running"; t.attempts++; return { ...t };
    },
    updateTask: async (id, p) => { const t = tasks.find((t) => t.id === id); if (t) Object.assign(t, p); },
  };
}
"##;

const ORCH_ENGINE: &str = r##"// CrewAI's shape: a Task has a description, an expected_output, context (the tasks it depends
// on) and the tools it may use; a crew runs them in dependency order. Here the planner writes
// that list from a goal, the store is the queue, a worker runs whatever is released, and a judge
// with a cold context — only the task and its output, none of the worker's transcript — decides
// whether it passed. A failed node is re-planned (the planner rewrites it with the verdict) and
// requeued; its dependants wait. Every step is a row, so a run survives a worker restart.
import type { Store, Task } from "./store";

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number; stop: "tool_use" | "end_turn" | "max_tokens" };
export type Message = { role: "user" | "assistant"; content: unknown };
export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<string> };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;

export const TOOLS: ToolDef[] = [
  { name: "calculator", description: "Evaluate an arithmetic expression (+ - * / ( ) and numbers).", input_schema: { type: "object", properties: { expression: { type: "string" } }, required: ["expression"] },
    run: async ({ expression }) => { const e = String(expression); if (!/^[\d\s+\-*/().]+$/.test(e)) throw new Error("only arithmetic"); return String(Function(`"use strict"; return (${e})`)()); } },
  { name: "web_fetch", description: "Fetch a public http(s) URL and return its text (tags stripped, truncated).", input_schema: { type: "object", properties: { url: { type: "string" } }, required: ["url"] },
    run: async ({ url }) => { const u = new URL(String(url)); if (!/^https?:$/.test(u.protocol)) throw new Error("http(s) only"); const r = await fetch(u, { signal: AbortSignal.timeout(10_000) }); return (await r.text()).replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>/gi, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").slice(0, 8000); } },
];

export type PlannedTask = { key: string; description: string; expected_output: string; depends_on: string[]; tools: string[] };
const PLANNER = `You are a planner. Split the goal into 2-6 tasks as JSON: {"tasks":[{"key":"t1","description":"...","expected_output":"what a correct result contains","depends_on":["keys"],"tools":["calculator"|"web_fetch"]}]}. Keys are unique; depends_on names earlier keys only; tools are only the ones the task needs. Reply with JSON only.`;

export async function plan(goal: string, model: Model): Promise<{ tasks: PlannedTask[]; input_tokens: number; output_tokens: number }> {
  const r = await model(PLANNER, [{ role: "user", content: goal }], []);
  const parsed = JSON.parse(extractJson(r.text)) as { tasks: PlannedTask[] };
  const keys = new Set(parsed.tasks.map((t) => t.key));
  for (const t of parsed.tasks) {
    t.depends_on = (t.depends_on ?? []).filter((k) => keys.has(k) && k !== t.key);
    t.tools = (t.tools ?? []).filter((n) => TOOLS.some((x) => x.name === n));
  }
  if (!parsed.tasks.length || hasCycle(parsed.tasks)) throw new Error("planner produced no usable graph");
  return { tasks: parsed.tasks, input_tokens: r.input_tokens, output_tokens: r.output_tokens };
}

/// Re-plan one failed node: same key and dependencies, a rewritten description that carries what
/// the judge saw. The rest of the graph is untouched, so completed work is never redone.
export async function replan(task: Task, model: Model): Promise<{ description: string; input_tokens: number; output_tokens: number }> {
  const r = await model("Rewrite this task's description so a worker gets it right this time. Keep the intent. Reply with the new description only, plain text.",
    [{ role: "user", content: `Task: ${task.description}\nExpected: ${task.expected_output}\nAttempt produced: ${task.output ?? ""}\nJudge said: ${task.verdict?.reason ?? ""}` }], []);
  return { description: r.text.trim() || task.description, input_tokens: r.input_tokens, output_tokens: r.output_tokens };
}

/// A worker turn: the ReAct loop over the task's scoped tools, until the model answers in text or
/// the token budget is spent. Dependency outputs are the context, exactly as CrewAI passes them.
export async function work(task: Task, context: { key: string; output: string }[], model: Model, tools = TOOLS): Promise<{ output: string | null; input_tokens: number; output_tokens: number; error?: string }> {
  const scoped = tools.filter((t) => task.tools.includes(t.name));
  const messages: Message[] = [{ role: "user", content: `${task.description}\n\nExpected output: ${task.expected_output}${context.length ? "\n\nContext from earlier tasks:\n" + context.map((c) => `[${c.key}]\n${c.output}`).join("\n\n") : ""}` }];
  let input_tokens = 0, output_tokens = 0;
  for (let n = 0; n < 12; n++) {
    if (input_tokens + output_tokens > task.budget_tokens) return { output: null, input_tokens, output_tokens, error: `budget of ${task.budget_tokens} tokens exceeded` };
    const r = await model("You complete one task using only the tools given. When done, reply with the result as plain text and no tool call.", messages, scoped);
    input_tokens += r.input_tokens; output_tokens += r.output_tokens;
    if (!r.calls.length) return { output: r.text, input_tokens, output_tokens };
    const blocks: unknown[] = r.text ? [{ type: "text", text: r.text }] : [];
    for (const c of r.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    messages.push({ role: "assistant", content: blocks });
    const results = [];
    for (const c of r.calls) {
      const tool = scoped.find((t) => t.name === c.name);
      try { if (!tool) throw new Error(`tool ${c.name} is not available to this task`); results.push({ type: "tool_result", tool_use_id: c.id, content: (await tool.run(c.input)).slice(0, 8000) }); }
      catch (e) { results.push({ type: "tool_result", tool_use_id: c.id, content: `Error: ${e instanceof Error ? e.message : String(e)}`, is_error: true }); }
    }
    messages.push({ role: "user", content: results });
  }
  return { output: null, input_tokens, output_tokens, error: "too many steps" };
}

/// Cold context: the judge sees the task, the acceptance criteria and the output. Not the
/// worker's reasoning, not the tool calls — so it cannot be talked into a pass.
export async function judge(task: Pick<Task, "description" | "expected_output">, output: string, model: Model): Promise<{ pass: boolean; reason: string; input_tokens: number; output_tokens: number }> {
  const r = await model(`You are a strict reviewer. Given a task, its acceptance criteria and an output, reply with JSON only: {"pass": true|false, "reason": "one sentence"}.`,
    [{ role: "user", content: `Task: ${task.description}\nAcceptance criteria: ${task.expected_output}\nOutput:\n${output}` }], []);
  try { const v = JSON.parse(extractJson(r.text)) as { pass: boolean; reason: string }; return { pass: !!v.pass, reason: String(v.reason ?? ""), input_tokens: r.input_tokens, output_tokens: r.output_tokens }; }
  catch { return { pass: false, reason: "judge gave no verdict", input_tokens: r.input_tokens, output_tokens: r.output_tokens }; }
}

export const MAX_ATTEMPTS = Number(process.env.MAX_TASK_ATTEMPTS ?? 3);

/// One queue step: claim a released task, work it, judge it, and settle it. Returns false when
/// nothing was ready. The worker loops this; tests call it directly.
export async function tick(store: Store, model: Model, tools = TOOLS): Promise<boolean> {
  const task = await store.claimReady(); if (!task) return false;
  const siblings = await store.getTasks(task.run_id);
  const context = siblings.filter((s) => task.depends_on.includes(s.key)).map((s) => ({ key: s.key, output: s.output ?? "" }));
  let tok = { input_tokens: 0, output_tokens: 0 };
  const add = (r: { input_tokens: number; output_tokens: number }) => { tok = { input_tokens: tok.input_tokens + r.input_tokens, output_tokens: tok.output_tokens + r.output_tokens }; };
  const w = await work(task, context, model, tools); add(w);
  let verdict = w.output === null ? { pass: false, reason: w.error ?? "no output" } : await judge(task, w.output, model).then((v) => { add(v); return v; });
  const failed = { ...task, output: w.output, verdict };
  if (verdict.pass) await store.updateTask(task.id, { state: "done", output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });
  else if (task.attempts < MAX_ATTEMPTS) {
    const p = await replan(failed, model); add(p);
    await store.updateTask(task.id, { state: "queued", description: p.description, output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });
  } else await store.updateTask(task.id, { state: "dead", output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });

  // Settle the run from its tasks; its tokens are the planner's plus every tick's.
  const all = await store.getTasks(task.run_id); const run = await store.getRun(task.run_id);
  const state = all.some((t) => t.state === "dead") ? "failed" : all.every((t) => t.state === "done") ? "done" : "running";
  await store.updateRun(task.run_id, { state, input_tokens: (run?.input_tokens ?? 0) + tok.input_tokens, output_tokens: (run?.output_tokens ?? 0) + tok.output_tokens });
  return true;
}

function extractJson(s: string): string { const i = s.indexOf("{"), j = s.lastIndexOf("}"); return i >= 0 && j > i ? s.slice(i, j + 1) : s; }
function hasCycle(ts: PlannedTask[]): boolean {
  const seen = new Map<string, number>(); const by = new Map(ts.map((t) => [t.key, t]));
  const visit = (k: string): boolean => { const s = seen.get(k); if (s === 1) return true; if (s === 2) return false; seen.set(k, 1); for (const d of by.get(k)?.depends_on ?? []) if (visit(d)) return true; seen.set(k, 2); return false; };
  return ts.some((t) => visit(t.key));
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Opus.
export function claude(fetchImpl: typeof fetch = fetch): Model {
  return async (system, messages, tools) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-opus-5", max_tokens: 4096, system, messages, tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }),
    });
    if (!res.ok) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const a = (await res.json()) as { content: { type: string; text?: string; id?: string; name?: string; input?: Record<string, unknown> }[]; stop_reason: string; usage: { input_tokens: number; output_tokens: number } };
    return {
      text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""),
      calls: a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, name: b.name!, input: b.input ?? {} })),
      input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens,
      stop: a.stop_reason === "tool_use" ? "tool_use" : a.stop_reason === "max_tokens" ? "max_tokens" : "end_turn",
    };
  };
}
"##;

const ORCH_WORKER: &str = r##"import { pgStore } from "./store";
import { tick, claude } from "./engine";
import { db } from "./db";

// The worker Deployment: claim, work, judge, settle, repeat. Idle polls back off to a second.
// Run more replicas for more parallelism — SKIP LOCKED keeps them off each other's tasks.
const store = pgStore(); const model = claude();
let stop = false;
process.on("SIGTERM", () => { stop = true; });
while (!stop) {
  try { if (!(await tick(store, model))) await new Promise((r) => setTimeout(r, 1000)); }
  catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); await new Promise((r) => setTimeout(r, 1000)); }
}
await db.end();
"##;

const ORCH_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore, type Task } from "./store";
import { plan, tick, work, MAX_ATTEMPTS, type Model, type ModelReply } from "./engine";

// The model is a function: the planner, worker, judge and re-planner are told apart by their
// system prompts, so one fake can play all four. No provider is ever called.
const reply = (text: string, extra: Partial<ModelReply> = {}): ModelReply => ({ text, calls: [], input_tokens: 10, output_tokens: 5, stop: "end_turn", ...extra });
const GRAPH = { tasks: [{ key: "a", description: "compute 6*7", expected_output: "the number 42", depends_on: [], tools: ["calculator"] }, { key: "b", description: "state the answer", expected_output: "a sentence", depends_on: ["a"], tools: [] }] };
const roles = (opts: { pass?: (task: string, output: string) => boolean } = {}): Model => async (system, messages, tools) => {
  if (system.startsWith("You are a planner")) return reply(JSON.stringify(GRAPH));
  if (system.startsWith("You are a strict reviewer")) { const m = String(messages[0].content); const out = m.slice(m.indexOf("Output:\n") + 8); return reply(JSON.stringify({ pass: opts.pass ? opts.pass(m, out) : true, reason: "checked" })); }
  if (system.startsWith("Rewrite")) return reply("REPLANNED: " + String(messages[0].content).split("\n")[0]);
  // worker: use the calculator if it has one and has not yet, else answer with the context.
  const last = messages.at(-1)!;
  if (tools.some((t) => t.name === "calculator") && messages.length === 1) return reply("", { calls: [{ id: "c1", name: "calculator", input: { expression: "6*7" } }], stop: "tool_use" });
  if (Array.isArray(last.content)) return reply(`result ${(last.content[0] as { content: string }).content}`);
  return reply(`done (tools: ${tools.map((t) => t.name).join(",") || "none"}; ctx: ${String(last.content).includes("[a]") ? "a" : "-"})`);
};
const task = (p: Partial<Task>): Task => ({ id: crypto.randomUUID(), run_id: "r", key: "k", description: "d", expected_output: "e", depends_on: [], tools: [], budget_tokens: 20_000, state: "queued", attempts: 0, output: null, verdict: null, input_tokens: 0, output_tokens: 0, ...p });

describe("orchestrator", () => {
  test("the planner's graph is stored with dependencies and scoped tools; cycles are refused", async () => {
    const g = await plan("anything", roles());
    expect(g.tasks.map((t) => t.key)).toEqual(["a", "b"]); expect(g.tasks[1].depends_on).toEqual(["a"]);
    await expect(plan("x", async () => reply(JSON.stringify({ tasks: [{ key: "a", description: "", expected_output: "", depends_on: ["b"], tools: ["shell"] }, { key: "b", description: "", expected_output: "", depends_on: ["a"], tools: [] }] })))).rejects.toThrow("no usable graph");
  });

  test("the queue releases a task only when its dependencies are done", async () => {
    const store = memoryStore();
    await store.addTasks([task({ key: "b", depends_on: ["a"] }), task({ key: "a" })]);
    const first = await store.claimReady(); expect(first?.key).toBe("a");
    expect(await store.claimReady()).toBeNull();            // b is blocked, a is running
    await store.updateTask(first!.id, { state: "done", output: "42" });
    expect((await store.claimReady())?.key).toBe("b");
  });

  test("a worker sees only the task's tools and its dependencies' outputs, and the judge passes it", async () => {
    const store = memoryStore();
    await store.addTasks([task({ run_id: "r", key: "a", tools: ["calculator"] }), task({ run_id: "r", key: "b", depends_on: ["a"] })]);
    await store.createRun({ id: "r", goal: "g", state: "running", input_tokens: 0, output_tokens: 0, created_at: "" });
    expect(await tick(store, roles())).toBe(true); expect(await tick(store, roles())).toBe(true); expect(await tick(store, roles())).toBe(false);
    const ts = await store.getTasks("r");
    expect(ts[0].output).toBe("result 42"); expect(ts[0].verdict?.pass).toBe(true);
    expect(ts[1].output).toBe("done (tools: none; ctx: a)");
    const run = await store.getRun("r"); expect(run?.state).toBe("done"); expect(run!.input_tokens).toBeGreaterThan(0);
    // A call to a tool outside the scope is an error result, never an execution.
    const w = await work(task({ tools: [] }), [], async (_s, m) => m.length === 1 ? reply("", { calls: [{ id: "x", name: "calculator", input: { expression: "1" } }], stop: "tool_use" }) : reply("ok"));
    expect(w.output).toBe("ok");
  });

  test("a failed verdict re-plans the node and requeues it; after MAX_ATTEMPTS it is dead and the run fails", async () => {
    const store = memoryStore();
    await store.addTasks([task({ run_id: "r", key: "a" })]);
    await store.createRun({ id: "r", goal: "g", state: "running", input_tokens: 0, output_tokens: 0, created_at: "" });
    const model = roles({ pass: () => false });
    for (let i = 1; i <= MAX_ATTEMPTS; i++) {
      await tick(store, model);
      const [t] = await store.getTasks("r");
      expect(t.attempts).toBe(i);
      if (i < MAX_ATTEMPTS) { expect(t.state).toBe("queued"); expect(t.description.startsWith("REPLANNED:")).toBe(true); }
      else { expect(t.state).toBe("dead"); expect((await store.getRun("r"))?.state).toBe("failed"); }
    }
  });

  test("a task that exceeds its token budget fails with the reason", async () => {
    const greedy: Model = async () => reply("", { calls: [{ id: "c", name: "calculator", input: { expression: "1+1" } }], stop: "tool_use", input_tokens: 5000, output_tokens: 1000 });
    const w = await work(task({ tools: ["calculator"], budget_tokens: 10_000 }), [], greedy);
    expect(w.output).toBeNull(); expect(w.error).toContain("budget");
    expect(w.input_tokens).toBe(10_000);
  });

  test("POST /api/runs plans, drains inline, and the run viewer shows the finished graph", async () => {
    const app = createApp(memoryStore(), roles(), { inline: true });
    const res = await app.request("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ goal: "what is 6*7" }) });
    expect(res.status).toBe(201);
    const { id } = await res.json();
    for (let i = 0; i < 50 && (await (await app.request(`/api/runs/${id}`)).json()).state === "running"; i++) await new Promise((r) => setTimeout(r, 5));
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("done"); expect(run.tasks.length).toBe(2); expect(run.tasks.every((t: Task) => t.state === "done")).toBe(true);
  });
});
"##;

const ORCH_SQL: &str = r##"create table if not exists runs (
  id uuid primary key,
  goal text not null,
  state text not null default 'planning',  -- planning | running | done | failed
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create table if not exists tasks (
  id uuid primary key,
  run_id uuid not null references runs(id),
  key text not null,
  description text not null,
  expected_output text not null,
  depends_on text[] not null default '{}',
  tools text[] not null default '{}',
  budget_tokens int not null default 20000,
  state text not null default 'queued',    -- queued | running | done | failed | dead
  attempts int not null default 0,
  output text,
  verdict jsonb,                           -- { pass, reason } from the judge
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  updated_at timestamptz not null default now(),
  unique (run_id, key)
);
create index if not exists tasks_ready on tasks (state, run_id);
"##;

const ORCH_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Task = { id: string; key: string; description: string; expected_output: string; depends_on: string[]; tools: string[]; state: string; attempts: number; output: string | null; verdict: { pass: boolean; reason: string } | null; input_tokens: number; output_tokens: number };
type Run = { id: string; goal: string; state: string; input_tokens: number; output_tokens: number; created_at: string; tasks?: Task[] };

const COLOR: Record<string, string> = { queued: "#ddd", running: "#ffd", done: "#dfd", failed: "#fdd", dead: "#f99", planning: "#ffd" };

// The run viewer: the task graph as a table in dependency order, coloured by state, polled while
// the run is live. Each row is a node — what it was asked, what it produced, what the judge said.
export default function Home() {
  const [runs, setRuns] = useState<Run[]>([]);
  const [run, setRun] = useState<Run | null>(null);
  const [goal, setGoal] = useState("Find the population of Iceland and of Malta from Wikipedia, then compute the ratio.");
  const [busy, setBusy] = useState(false);

  const load = () => fetch("/api/runs").then((r) => r.json()).then((d) => setRuns(d.runs)).catch(() => {});
  useEffect(() => { load(); }, []);
  useEffect(() => {
    if (!run || (run.state !== "running" && run.state !== "planning")) return;
    const t = setInterval(() => fetch(`/api/runs/${run.id}`).then((r) => r.json()).then(setRun).catch(() => {}), 1000);
    return () => clearInterval(t);
  }, [run]);

  async function start() {
    setBusy(true);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ goal }) });
    const d = await res.json();
    if (res.ok) setRun(await (await fetch(`/api/runs/${d.id}`)).json()); else alert(d.error.message);
    setBusy(false); load();
  }

  return (
    <main>
      <h1>{{NAME}} — orchestrator</h1>
      <p>Planner → task graph → workers with scoped tools → judge. Set <code>ANTHROPIC_API_KEY</code> in backend/.env; <code>bun run worker</code> runs tasks in the cluster, the API drains them itself locally.</p>
      <pre>{`curl localhost:8000/api/runs -H 'content-type: application/json' -d '{"goal":"..."}'\ncurl localhost:8000/api/runs/<id>`}</pre>
      <textarea value={goal} onChange={(e) => setGoal(e.target.value)} rows={2} style={{ width: "100%" }} />
      <p><button onClick={start} disabled={busy}>{busy ? "Planning…" : "Run"}</button></p>
      {run && (
        <section>
          <h2>{run.goal} <small style={{ background: COLOR[run.state] }}>{run.state}</small> <small>{run.input_tokens + run.output_tokens} tokens</small></h2>
          <table border={1} cellPadding={4} style={{ borderCollapse: "collapse", width: "100%" }}>
            <thead><tr><th>key</th><th>after</th><th>tools</th><th>state</th><th>task</th><th>output</th><th>judge</th></tr></thead>
            <tbody>{run.tasks?.map((t) => (
              <tr key={t.id} style={{ background: COLOR[t.state] }}>
                <td>{t.key}</td><td>{t.depends_on.join(", ")}</td><td>{t.tools.join(", ")}</td><td>{t.state}{t.attempts > 1 ? ` ×${t.attempts}` : ""}</td>
                <td>{t.description}</td><td style={{ whiteSpace: "pre-wrap", maxWidth: 400 }}>{t.output}</td><td>{t.verdict ? `${t.verdict.pass ? "pass" : "fail"}: ${t.verdict.reason}` : ""}</td>
              </tr>))}</tbody>
          </table>
        </section>
      )}
      <h2>Runs</h2>
      <ul>{runs.map((r) => <li key={r.id}><a href="#" onClick={(e) => { e.preventDefault(); fetch(`/api/runs/${r.id}`).then((x) => x.json()).then(setRun); }}>{r.goal}</a> — {r.state}</li>)}</ul>
    </main>
  );
}
"##;
